//! The bridge store entity: the snapshot Z1+ panels render from, plus the 1s
//! elapsed ticker that keeps running runs' counters live between server
//! frames (serve.py's board digest ignores `elapsed_s` — clients tick
//! elapsed locally; RUST_PORT_NOTES §5's worked-for ticker).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use gpui::{App, AppContext as _, Entity, Task};

use super::client::{connection_loop, fetch_and_apply_transcript, transcript_poll_loop};
use super::protocol::{
    BridgeEvent, PoolRow, ProjectRow, RunRow, TranscriptSnapshot, UsageMeta, pools_from_object,
    usage_meta_from_object,
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
            projects: Vec::new(),
            board_received_at: Instant::now(),
            last_event_at: None,
            ticker: None,
            transcript_watchers: Arc::new(AtomicUsize::new(0)),
            transcript_task: None,
            transcript_refetch: None,
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

    pub(super) fn apply_event(&mut self, event: BridgeEvent, cx: &mut gpui::Context<Self>) {
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
            BridgeEvent::Unknown => {}
        }
        if changed {
            cx.notify();
        }
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

    pub(super) fn apply_projects(&mut self, projects: Vec<ProjectRow>, cx: &mut gpui::Context<Self>) {
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
