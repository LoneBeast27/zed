//! The bridge store entity: the snapshot Z1+ panels render from, plus the 1s
//! elapsed ticker that keeps running runs' counters live between server
//! frames (serve.py's board digest ignores `elapsed_s` — clients tick
//! elapsed locally; RUST_PORT_NOTES §5's worked-for ticker).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use gpui::{App, AppContext as _, Entity, Task};

use super::client::{
    connection_loop, fetch_and_apply_transcript, plan_poll_loop, transcript_poll_loop,
};
use super::protocol::{
    ApprovalRow, BridgeEvent, ChannelRow, ConversationRow, PlanSnapshot, PoolRow, ProjectRow,
    RunRow, TranscriptSnapshot, UsageMeta, pools_from_object, usage_meta_from_object,
};

/// Local elapsed-tick cadence while any run is `running`.
const ELAPSED_TICK: Duration = Duration::from_secs(1);

/// Which transport is feeding the store right now (Z4: the settings status
/// panel's bridge-connection section). Falls back to [`Transport::None`]
/// whenever the bridge goes offline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Transport {
    #[default]
    None,
    /// The `/sse` push stream.
    Sse,
    /// The `/board`+`/usage` polling fallback window.
    Polling,
}

/// The bridge snapshot Z1+ panels render from. `connected == false` means the
/// bridge is offline — the last snapshot is kept so panels can grey-out rather
/// than blank.
pub struct BridgeStore {
    pub board: Vec<RunRow>,
    pub usage: Vec<PoolRow>,
    /// `_source` / `_scraped` metadata riding the usage payload (Z2: the
    /// usage panel's staleness banner + the island's reset phrase).
    pub usage_meta: UsageMeta,
    pub connected: bool,
    /// The transport currently feeding the store (Z4 settings status) —
    /// [`Transport::None`] while offline.
    pub transport: Transport,
    /// The last `/transcript` snapshot (Z3) — `None` until the first fetch.
    /// Polled (not pushed): SSE carries only board/usage today; the bridge-
    /// side transcript event is deferred work.
    pub transcript: Option<TranscriptSnapshot>,
    /// The conversation the transcript poll follows. `None` = the bridge's
    /// default (latest); the poll canonicalizes it to the served id.
    pub transcript_conv: Option<String>,
    /// The last plan/wave snapshot (Symphony live check-off) — `None` until
    /// the first `plan` event / `/plan` poll. Fed by SSE when connected and by
    /// the watch-gated `/plan` poll in the polling-fallback window.
    pub plan: Option<PlanSnapshot>,
    /// The conversation id the current [`Self::plan`] belongs to — surfaced by
    /// the Symphony header when the plan is the bridge default (nothing is
    /// explicitly followed) so it labels WHICH conversation's plan it renders
    /// (Finding 2). Empty when the plan carries no conv (single-conv bridge).
    pub plan_conv: String,
    /// Boost-channel rows (T2, the constellation's sibling edges) — fed by
    /// the SSE `channels` event and the constellation's supplemental poll.
    pub channels: Vec<ChannelRow>,
    /// `GET /projects` rows — fetched lazily by the transcript poll when the
    /// crumb can't resolve the conversation's project name yet.
    projects: Vec<ProjectRow>,
    /// When the current board snapshot arrived. The server value re-bases the
    /// local tick offset on every board frame; running runs render
    /// `elapsed_s + (now − board_received_at)`.
    board_received_at: Instant,
    /// When the last bridge event (any type) was applied — `None` until the
    /// first frame. The settings status panel renders its age.
    last_event_at: Option<Instant>,
    /// The 1s ticker task — `Some` only while any run is `running`
    /// (Lightness Mandate: no idle timers).
    ticker: Option<Task<()>>,
    /// Live [`TranscriptWatch`] count. The poll task reads it each cycle and
    /// exits at zero — panel presence gates the poll, never a free timer.
    transcript_watchers: Arc<AtomicUsize>,
    /// The transcript poll task — replaced on every 0→1 watcher transition
    /// (dropping the old task cancels it, so two loops never overlap).
    transcript_task: Option<Task<()>>,
    /// In-flight one-shot refetch (post-send / conv switch).
    transcript_refetch: Option<Task<()>>,
    /// Live [`PlanWatch`] count — the `/plan` poll reads it each wake and
    /// exits at zero (Symphony panel presence gates the poll). SSE feeds the
    /// plan directly when connected; this poll is the polling-window fallback.
    plan_watchers: Arc<AtomicUsize>,
    /// The `/plan` poll task — replaced on every 0→1 watcher transition.
    plan_task: Option<Task<()>>,
    /// Pending runtime approvals (Phase-2 permissions §5.2) — fed by the SSE
    /// `permission` frame and the watch-gated `/approvals` poll fallback.
    /// REPLACE-on-frame: every frame carries the full pending set, so an
    /// empty frame retires the last row (a resolve needs no tombstone).
    pub pending_approvals: Vec<ApprovalRow>,
    /// Live [`super::approvals::ApprovalWatch`] count — the `/approvals` poll
    /// reads it each wake and exits at zero (a held toast / visible panel
    /// gates the poll, the [`TranscriptWatch`] law).
    pub(super) approval_watchers: Arc<AtomicUsize>,
    /// The `/approvals` poll task — replaced on every 0→1 watcher transition.
    pub(super) approval_task: Option<Task<()>>,
}

impl Default for BridgeStore {
    fn default() -> Self {
        Self {
            board: Vec::new(),
            usage: Vec::new(),
            usage_meta: UsageMeta::default(),
            connected: false,
            transport: Transport::None,
            transcript: None,
            transcript_conv: None,
            plan: None,
            plan_conv: String::new(),
            channels: Vec::new(),
            projects: Vec::new(),
            board_received_at: Instant::now(),
            last_event_at: None,
            ticker: None,
            transcript_watchers: Arc::new(AtomicUsize::new(0)),
            transcript_task: None,
            transcript_refetch: None,
            plan_watchers: Arc::new(AtomicUsize::new(0)),
            plan_task: None,
            pending_approvals: Vec::new(),
            approval_watchers: Arc::new(AtomicUsize::new(0)),
            approval_task: None,
        }
    }
}

/// RAII registration of a live transcript consumer (a chat panel). Dropping
/// it decrements the watcher count; the poll task notices on its next wake
/// and exits — cx-free teardown, so panel drops never need a context.
pub struct TranscriptWatch {
    watchers: Arc<AtomicUsize>,
}

impl Drop for TranscriptWatch {
    fn drop(&mut self) {
        self.watchers.fetch_sub(1, Ordering::SeqCst);
    }
}

/// RAII registration of a live plan consumer (the Symphony panel). Dropping
/// it pauses the `/plan` poll on the next wake — the [`TranscriptWatch`]
/// pattern, cx-free teardown.
pub struct PlanWatch {
    watchers: Arc<AtomicUsize>,
}

impl Drop for PlanWatch {
    fn drop(&mut self) {
        self.watchers.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Whether an incoming plan snapshot should be adopted, given the currently
/// followed conversation (`None` = nothing explicitly followed — a fresh app).
///
/// The follow-default fold (Finding 2, PARITY_SPEC §4.3 "symphony renders
/// orchestrator plans as a live score"):
/// - Nothing followed → adopt ANY plan. The bridge pushes the GLOBAL-newest
///   plan on the single SSE stream / returns it for an empty `/plan?conv=`, so
///   "adopt whatever arrives" IS defaulting to the most-recent conversation
///   with a plan — the panel shows it instead of an empty state.
/// - Following conv A → adopt only conv A's plan (scope by conv, no flicker to
///   another conversation's plan on the shared stream).
/// - An empty incoming `conv` (single-conv bridge / older bridge) is always
///   accepted — there is no conv to mismatch against.
pub(super) fn accepts_plan(followed: Option<&str>, incoming_conv: &str) -> bool {
    match followed {
        None => true,
        Some(conv) => incoming_conv.is_empty() || conv == incoming_conv,
    }
}

/// Adds the local tick offset to every `running` run's `elapsed_s` — the
/// re-base math of the 1s ticker (settled runs keep the server value).
fn tick_elapsed(board: &mut [RunRow], offset_s: f64) {
    for run in board {
        if run.status == "running" {
            run.elapsed_s += offset_s;
        }
    }
}

impl BridgeStore {
    /// The board with locally-ticked elapsed: the server pushes a frame only
    /// when the run/status digest changes, so between frames running runs
    /// tick `elapsed_s` forward from the snapshot's arrival time.
    pub fn ticked_board(&self) -> Vec<RunRow> {
        let mut board = self.board.clone();
        tick_elapsed(&mut board, self.board_received_at.elapsed().as_secs_f64());
        board
    }

    /// Age of the last applied bridge event — `None` until the first frame.
    pub fn last_event_age(&self) -> Option<Duration> {
        self.last_event_at.map(|at| at.elapsed())
    }

    pub(crate) fn apply_event(&mut self, event: BridgeEvent, cx: &mut gpui::Context<Self>) {
        let mut changed = !self.connected;
        self.connected = true;
        self.last_event_at = Some(Instant::now());
        match event {
            BridgeEvent::Board { board } => {
                // The server's elapsed_s is authoritative at receive time —
                // re-base the local tick even when the rows are unchanged.
                self.board_received_at = Instant::now();
                if self.board != board {
                    self.board = board;
                    changed = true;
                }
                self.update_ticker(cx);
            }
            BridgeEvent::Usage { fields } => {
                let usage = pools_from_object(&fields);
                if self.usage != usage {
                    self.usage = usage;
                    changed = true;
                }
                let meta = usage_meta_from_object(&fields);
                if self.usage_meta != meta {
                    self.usage_meta = meta;
                    changed = true;
                }
            }
            BridgeEvent::Plan { plan } => {
                // P5 + Finding 2: the SSE `plan` frame is GLOBAL-newest across
                // all conversations (bridge serve.py pushes latest_plan_view on
                // the single stream). [`accepts_plan`] decides whether to take
                // it: it matches the followed conv, OR nothing is explicitly
                // followed (a fresh app) — in which case the global-newest IS
                // the "most recent conversation with a plan" the panel should
                // default to, so we adopt it instead of showing "No plans yet"
                // while a live plan exists bridge-wide. A followed conv still
                // scopes the panel to that conv's plan (no flicker to conv B).
                if accepts_plan(self.transcript_conv.as_deref(), &plan.conv)
                    && (self.plan.as_ref() != Some(&plan) || self.plan_conv != plan.conv)
                {
                    self.plan_conv = plan.conv.clone();
                    self.plan = Some(plan);
                    changed = true;
                }
            }
            BridgeEvent::Channels { channels } => {
                if self.channels != channels {
                    self.channels = channels;
                    changed = true;
                }
            }
            BridgeEvent::Permission { pending } => {
                // Replace-on-frame (the frame carries the FULL pending set),
                // change-gated like every other arm.
                if self.pending_approvals != pending {
                    self.pending_approvals = pending;
                    changed = true;
                }
            }
            BridgeEvent::Unknown => {}
        }
        if changed {
            cx.notify();
        }
    }

    /// All conversation rows across projects (the constellation's root ctx
    /// source — the web's `state.projects.flatMap(p => p.conversations)`).
    pub fn conversations(&self) -> impl Iterator<Item = &ConversationRow> {
        self.projects
            .iter()
            .flat_map(|project| project.conversations.iter())
    }

    /// Registers a transcript consumer and (re)starts the poll task on the
    /// 0→1 transition. The returned guard keeps the poll alive; dropping the
    /// last one pauses it (the next wake exits the loop).
    pub fn watch_transcript(&mut self, cx: &mut gpui::Context<Self>) -> TranscriptWatch {
        let watchers = self.transcript_watchers.clone();
        if watchers.fetch_add(1, Ordering::SeqCst) == 0 {
            let http_client = cx.http_client();
            let loop_watchers = watchers.clone();
            // Replacing the slot drops (cancels) any old loop mid-sleep, so
            // a fast unwatch→watch can never run two loops at once.
            self.transcript_task = Some(cx.spawn(async move |this, cx| {
                transcript_poll_loop(http_client, loop_watchers, this, cx).await
            }));
        }
        TranscriptWatch { watchers }
    }

    /// Registers a plan consumer (the Symphony panel) and (re)starts the
    /// `/plan` poll on the 0→1 transition. SSE feeds [`Self::plan`] directly
    /// when connected; this poll covers the polling-fallback window so the
    /// panel checks off either way. The poll follows [`Self::transcript_conv`]
    /// (the active conversation) each cycle.
    pub fn watch_plan(&mut self, cx: &mut gpui::Context<Self>) -> PlanWatch {
        let watchers = self.plan_watchers.clone();
        if watchers.fetch_add(1, Ordering::SeqCst) == 0 {
            let http_client = cx.http_client();
            let loop_watchers = watchers.clone();
            self.plan_task = Some(cx.spawn(async move |this, cx| {
                plan_poll_loop(http_client, loop_watchers, this, cx).await
            }));
        }
        PlanWatch { watchers }
    }

    /// Apply a `/plan` poll or default-fetch result — change-gated like every
    /// other apply, and scoped by the same [`accepts_plan`] fold as the SSE
    /// path so the poll and the push never disagree on which conv's plan wins.
    pub(super) fn apply_plan(&mut self, plan: PlanSnapshot, cx: &mut gpui::Context<Self>) {
        if accepts_plan(self.transcript_conv.as_deref(), &plan.conv)
            && (self.plan.as_ref() != Some(&plan) || self.plan_conv != plan.conv)
        {
            self.plan_conv = plan.conv.clone();
            self.plan = Some(plan);
            cx.notify();
        }
    }

    /// Follow a different conversation (`None` = the bridge default) and
    /// refetch at once — the web's `selectConv` + immediate `loop()`.
    pub fn set_conversation(&mut self, conv: Option<String>, cx: &mut gpui::Context<Self>) {
        if self.transcript_conv != conv {
            self.transcript_conv = conv;
            self.refetch_transcript_soon(cx);
        }
    }

    /// One-shot out-of-cadence transcript fetch (post-send, conv switch) —
    /// racing the poll loop is harmless: both apply the same source and
    /// [`Self::apply_transcript`] is change-gated.
    pub fn refetch_transcript_soon(&mut self, cx: &mut gpui::Context<Self>) {
        let http_client = cx.http_client();
        self.transcript_refetch = Some(cx.spawn(async move |this, cx| {
            fetch_and_apply_transcript(&http_client, &this, cx).await.ok();
        }));
    }

    /// The crumb's project resolution: the project whose conversation list
    /// contains `conv` (web: `state.projects.find(p => p.conversations…)`).
    pub fn project_name_for_conv(&self, conv: &str) -> Option<&str> {
        if conv.is_empty() {
            return None;
        }
        self.projects
            .iter()
            .find(|project| {
                project
                    .conversations
                    .iter()
                    .any(|conversation| conversation.id == conv)
            })
            .map(|project| project.name.as_str())
    }

    pub(super) fn apply_transcript(
        &mut self,
        snapshot: TranscriptSnapshot,
        cx: &mut gpui::Context<Self>,
    ) {
        // Canonicalize the followed conversation to the served id (the web's
        // `state.conv = t.id`) so the next poll pins what the bridge chose.
        let mut changed = false;
        if !snapshot.id.is_empty() && self.transcript_conv.as_deref() != Some(&snapshot.id) {
            self.transcript_conv = Some(snapshot.id.clone());
            changed = true;
        }
        if self.transcript.as_ref() != Some(&snapshot) {
            self.transcript = Some(snapshot);
            changed = true;
        }
        if changed {
            cx.notify();
        }
    }

    pub(crate) fn apply_projects(&mut self, projects: Vec<ProjectRow>, cx: &mut gpui::Context<Self>) {
        if self.projects != projects {
            self.projects = projects;
            cx.notify();
        }
    }

    pub(super) fn set_connected(&mut self, connected: bool, cx: &mut gpui::Context<Self>) {
        let mut changed = false;
        if self.connected != connected {
            self.connected = connected;
            changed = true;
        }
        // Offline has no transport; reconnection sets the real one.
        if !connected && self.transport != Transport::None {
            self.transport = Transport::None;
            changed = true;
        }
        if changed {
            cx.notify();
        }
    }

    /// Which transport is feeding the store (set by the connection loop:
    /// `Sse` on stream attach, `Polling` on a successful fallback fetch).
    pub(super) fn set_transport(&mut self, transport: Transport, cx: &mut gpui::Context<Self>) {
        if self.transport != transport {
            self.transport = transport;
            cx.notify();
        }
    }

    /// Starts the 1s ticker when a run is live, drops it when none is. Every
    /// board mutation funnels through [`Self::apply_event`], so this is the
    /// single start/stop point; the task also self-clears as a backstop.
    fn update_ticker(&mut self, cx: &mut gpui::Context<Self>) {
        let any_running = self.board.iter().any(|run| run.status == "running");
        if !any_running {
            self.ticker = None;
        } else if self.ticker.is_none() {
            self.ticker = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(ELAPSED_TICK).await;
                    let live = this.update(cx, |store, cx| {
                        let live = store.board.iter().any(|run| run.status == "running");
                        if live {
                            cx.notify();
                        } else {
                            store.ticker = None;
                        }
                        live
                    });
                    if !matches!(live, Ok(true)) {
                        return;
                    }
                }
            }));
        }
    }
}

struct GlobalBridgeStore(Entity<BridgeStore>);

impl gpui::Global for GlobalBridgeStore {}

/// The shared store IF a consumer already built it — a read-only accessor
/// that never boots the connection loop. Late-bound consumers (the run
/// drawer's approval section) use it: by the time a drawer opens, a Z1
/// panel has long since built the store; in CPU tests no global means no
/// background loop to hang `run_until_parked`.
pub fn try_global_store(cx: &App) -> Option<Entity<BridgeStore>> {
    cx.try_global::<GlobalBridgeStore>()
        .map(|global| global.0.clone())
}

/// The app-wide shared store. Created lazily on first access — the bridge
/// connection task starts non-blocking when the first consumer (a Z1+ panel)
/// builds, per the Lightness Mandate's lazy-startup rule.
pub fn global_store(cx: &mut App) -> Entity<BridgeStore> {
    if let Some(global) = cx.try_global::<GlobalBridgeStore>() {
        return global.0.clone();
    }
    let store = init(cx);
    cx.set_global(GlobalBridgeStore(store.clone()));
    store
}

/// Creates the store entity and detaches the connection loop. The loop exits
/// on its own once the entity is dropped (weak-handle update failure).
pub fn init(cx: &mut App) -> Entity<BridgeStore> {
    let http_client = cx.http_client();
    cx.new(|cx| {
        cx.spawn(async move |this, cx| connection_loop(http_client, this, cx).await)
            .detach();
        BridgeStore::default()
    })
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
