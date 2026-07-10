//! The Orchestrator chat surface (PARITY_SPEC §4.1): crumb header over the
//! virtualized transcript (greeting when empty), with the composer deck
//! beneath (Z3 Task 3). Holds the shared `Entity<BridgeStore>` and — only
//! while the center tab shows it — a [`TranscriptWatch`]: the watch is
//! acquired on `ModeSurface::set_surface_active(true)` and dropped on
//! `(false)`, so the `/transcript` poll runs only while the surface is
//! actually visible (Lightness: *visibility* gates the poll — the entity
//! itself is eagerly built at workspace init and would otherwise hold the
//! poll open for the whole workspace lifetime; the web's `unmountChat`
//! clears its interval on route exit the same way).

use std::time::Duration;

use gpui::{
    AnyElement, App, Context, Entity, FocusHandle, Focusable, SharedString, Subscription, Task,
    WeakEntity, Window, actions, list,
};
use ui::prelude::*;
use workspace::Workspace;

use crate::bridge::{self, BridgeStore, TranscriptSnapshot, TranscriptWatch};
use crate::commands::CommandRegistry;
use crate::islands::TasksIsland;
use crate::task_board::motion::StateFades;

use super::composer::Composer;
use super::message::render_message;
use super::transcript::{TranscriptView, render_greeting, render_shimmer};
use super::typeahead_menu::TypeaheadMenu;

actions!(
    orchestrator_panel,
    [
        /// Toggles focus on the orchestrator chat panel.
        ToggleFocus,
        /// Sends the composer's message (Enter via the
        /// `OrchestratorComposer > Editor` keymap binding).
        Send,
        /// Move the `/` typeahead selection up (Up arrow while the menu is
        /// open; ignored otherwise so the caret moves normally).
        TypeaheadUp,
        /// Move the `/` typeahead selection down (Down arrow while open).
        TypeaheadDown,
        /// Accept the highlighted `/` command (Tab while the menu is open).
        TypeaheadAccept,
        /// Dismiss the `/` typeahead, dropping the stray slash token (Escape).
        TypeaheadDismiss
    ]
);

/// The worked-for rolling-tick cadence (RUST_PORT_NOTES §5: the 1s
/// worked-for tickers are the only client timers that survive natively).
const BUSY_TICK: Duration = Duration::from_secs(1);

pub struct OrchestratorPanel {
    focus_handle: FocusHandle,
    pub(super) store: Entity<BridgeStore>,
    pub(super) workspace: WeakEntity<Workspace>,
    pub(super) transcript: TranscriptView,
    pub(super) composer: Composer,
    /// The composer-anchored running-tasks island (§4.9) — `pub(crate)` so
    /// `islands::tasks_island`'s retract clock and listeners can reach it.
    pub(crate) tasks_island: TasksIsland,
    /// The toast stack, homed on the composer anchor (user ruling
    /// 2026-07-07: "all elements to give information live on it"; was the
    /// top-right corner stack). Usage itself is the deck's own strip —
    /// [`super::usage_strip`], user ruling 2026-07-08.
    pub(super) notif_stack: gpui::Entity<crate::islands::NotifStack>,
    /// Message feedback verdicts, keyed by the message ts bits (`true` =
    /// thumbs-up). Session-local render state; the durable record is the
    /// bridge's vault ledger (POST /feedback, dogfood 2026-07-08).
    pub(super) feedback: std::collections::HashMap<u64, bool>,
    /// A `/wave` submission is in flight — double-fires must not
    /// double-spawn real runs (2026-07-10 review find #7).
    pub(super) wave_in_flight: bool,
    /// The last `/wave` refusal (usage hint or the bridge's error verbatim),
    /// rendered above the composer deck until the next submission.
    pub(super) wave_note: Option<gpui::SharedString>,
    /// The `/` command registry (S0) — the source the typeahead + `/help`
    /// render from. Shared entity, built at panel construction.
    pub(super) registry: Entity<CommandRegistry>,
    /// The `/` typeahead menu state (S1): open flag + selection index. Live
    /// rows are recomputed each keystroke from the composer text + registry.
    pub(super) typeahead: TypeaheadMenu,
    /// Orchestrator busy flag from the latest snapshot.
    pub(super) busy: bool,
    /// The 1s rolling-tick task — `Some` only while busy (store ticker
    /// pattern; no idle timers). Repaints so a live trailing reply's
    /// worked-for label (`worked_s + seen_at.elapsed()`) rolls.
    busy_ticker: Option<Task<()>>,
    /// Change gate for the store observer (the store also notifies at 1Hz
    /// for board elapsed this panel renders only via the busy tick).
    last_snapshot: Option<TranscriptSnapshot>,
    /// Every tracked hover/focus crossfade in the panel (meta-trio reveals,
    /// summary pills, step rows, composer chrome, island chrome) — the §0
    /// "hovers are gentle fades" class, read per frame and pumped by ONE
    /// `request_animation_frame` in [`Render::render`].
    pub(crate) fades: StateFades,
    /// Held only while the center tab shows this panel
    /// (`ModeSurface::set_surface_active`) — the RAII guard whose presence
    /// keeps the `/transcript` poll alive.
    transcript_watch: Option<TranscriptWatch>,
    _store_subscription: Subscription,
}

impl OrchestratorPanel {
    pub fn new(
        workspace: WeakEntity<Workspace>,
        notif_stack: gpui::Entity<crate::islands::NotifStack>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let store = bridge::global_store(cx);
        let _store_subscription =
            cx.observe(&store, |this: &mut Self, _, cx| this.sync_from_store(cx));

        // The `/` command registry: static seed is live immediately, the
        // user-local skills/commands stream in from a background walk. The
        // registry resolves the workspace root LAZILY inside its discovery task
        // (dispatch law, RUST_PORT_NOTES §11: `OrchestratorPanel::new` runs
        // inside the Workspace's own `observe_new` update — a synchronous
        // `workspace.read(cx)` here re-enters the mid-update Workspace entity
        // and panics; the deferred read in the background task is safe).
        let fs = <dyn fs::Fs>::global(cx);
        let home = paths::home_dir().clone();
        let registry = cx.new(|cx| CommandRegistry::new(fs, home, workspace.clone(), cx));

        Self {
            focus_handle: cx.focus_handle(),
            store,
            workspace,
            transcript: TranscriptView::new(),
            composer: Composer::new(window, cx),
            tasks_island: TasksIsland::new(cx),
            notif_stack,
            feedback: std::collections::HashMap::new(),
            wave_in_flight: false,
            wave_note: None,
            registry,
            typeahead: TypeaheadMenu::new(),
            busy: false,
            busy_ticker: None,
            last_snapshot: None,
            fades: StateFades::new(),
            transcript_watch: None,
            _store_subscription,
        }
    }

    /// The center tab's visibility signal drives the poll lifetime: acquire
    /// the watch when the tab becomes the pane's visible item, drop it when
    /// another tab takes the pane or the tab closes. Acquiring on the 0→1
    /// watcher transition (re)starts the poll loop, whose first iteration
    /// fetches immediately — re-activation is also the fresh fetch.
    fn set_poll_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if active {
            if self.transcript_watch.is_none() {
                self.transcript_watch =
                    Some(self.store.update(cx, |store, cx| store.watch_transcript(cx)));
            }
        } else {
            self.transcript_watch = None;
        }
    }

    /// A store notify landed: feed the tasks island from the board and
    /// re-sync the transcript — each side change-gated, so usage notifies
    /// and idle ticks stay free (board/usage/transcript all ride the same
    /// entity). Derivation and change-comparison run under scoped read
    /// borrows; nothing is cloned on a no-change notify (the store also
    /// notifies at 1Hz for board elapsed).
    fn sync_from_store(&mut self, cx: &mut Context<Self>) {
        let island_rows =
            crate::islands::tasks_island::running_rows(&self.store.read(cx).board);
        let mut changed = self.tasks_island.sync(island_rows, cx);

        // Compare under the read borrow; clone the snapshot only on change.
        let snapshot = {
            let store = self.store.read(cx);
            match &store.transcript {
                Some(snapshot) if self.last_snapshot.as_ref() != Some(snapshot) => {
                    Some(snapshot.clone())
                }
                _ => None,
            }
        };
        if let Some(snapshot) = snapshot {
            self.transcript.sync(&snapshot, cx);
            if snapshot.busy != self.busy {
                self.busy = snapshot.busy;
                if self.busy {
                    self.arm_busy_ticker(cx);
                } else {
                    // Reply landed — server worked_s rules again.
                    self.busy_ticker = None;
                }
            }
            self.last_snapshot = Some(snapshot);
            changed = true;
        }
        if changed {
            cx.notify();
        }
    }

    /// 1s repaint driver for the rolling worked-for tick, alive only while
    /// busy (the BridgeStore elapsed-ticker pattern).
    fn arm_busy_ticker(&mut self, cx: &mut Context<Self>) {
        if self.busy_ticker.is_some() {
            return;
        }
        self.busy_ticker = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(BUSY_TICK).await;
                let live = this.update(cx, |this, cx| {
                    if this.busy {
                        cx.notify();
                        true
                    } else {
                        this.busy_ticker = None;
                        false
                    }
                });
                if !matches!(live, Ok(true)) {
                    return;
                }
            }
        }));
    }

    pub(super) fn toggle_worked(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.transcript.toggle_worked(ix);
        cx.notify();
    }

    /// A thumbs click (dogfood 2026-07-08): fill locally at once, and bank
    /// the verdict on the bridge's vault ledger in the background. A repeat
    /// click with the other thumb overrides (ledger keeps history; readers
    /// take the last row).
    pub(super) fn send_feedback(&mut self, ts: f64, up: bool, cx: &mut Context<Self>) {
        self.feedback.insert(ts.to_bits(), up);
        cx.notify();
        let conv = {
            let store = self.store.read(cx);
            store
                .transcript
                .as_ref()
                .map(|snapshot| snapshot.id.clone())
                .or_else(|| store.transcript_conv.clone())
                .unwrap_or_default()
        };
        let client = cx.http_client();
        cx.background_spawn(async move {
            let body = serde_json::json!({
                "conv": conv,
                "message_ts": ts,
                "verdict": if up { "up" } else { "down" },
            })
            .to_string();
            crate::bridge::post_json(
                client.as_ref(),
                &format!("{}/feedback", crate::bridge::BRIDGE_BASE_URL),
                body,
            )
            .await
            .ok();
        })
        .detach();
    }

    /// Step-row / island-row click → route to the task board (taskboard
    /// mode / center tab) and open that run's drawer (the Z2 toast's
    /// pattern — emits `TaskBoardEvent::OpenRun` inside). `pub(crate)`: the
    /// tasks island's rows route here too.
    pub(crate) fn open_run(
        &mut self,
        run_id: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Route to the board one cycle later. open_task_board_run reads the
        // ActivityBar (via switch_to_mode_id → the surfaces registry on the
        // bar), so the layout mutation is deferred out of this listener per the
        // interaction-dispatch law (RUST_PORT_NOTES 2026-07-04). The callers
        // (step-row / tasks-island-row clicks) run inside this panel's update,
        // not the bar's — no panic today, but the deferred form is the uniform
        // safe idiom and is one refactor from a cycle.
        let workspace = self.workspace.clone();
        window.defer(cx, move |window, cx| {
            workspace
                .update(cx, |workspace, cx| {
                    crate::mode_item::open_task_board_run(run_id, workspace, window, cx);
                })
                .ok();
        });
    }

    /// Flip a tracked interactive element's hover/focus crossfade (the §0
    /// gentle-fades class). `pub(crate)`: island chrome rides the same map.
    pub(crate) fn set_fade(
        &mut self,
        id: impl Into<gpui::ElementId>,
        engaged: bool,
        duration: Duration,
        cx: &mut Context<Self>,
    ) {
        if self.fades.set(id, engaged, duration) {
            cx.notify();
        }
    }

    /// `.chat-crumb`: `project / Conversation Title`, 13px, shown once the
    /// conversation has messages.
    fn render_crumb(&self, cx: &App) -> Option<AnyElement> {
        if self.transcript.is_empty() {
            return None;
        }
        let snapshot = self.last_snapshot.as_ref()?;
        let store = self.store.read(cx);
        let project = store
            .project_name_for_conv(&snapshot.id)
            .unwrap_or("workspace")
            .to_string();
        let colors = cx.theme().colors();
        Some(
            h_flex()
                .w_full()
                .justify_center()
                .pt(px(14.))
                .child(
                    h_flex()
                        .w_full()
                        .max_w(px(780.))
                        .px(px(32.))
                        .gap(px(4.))
                        .text_size(px(13.))
                        .overflow_hidden()
                        .child(
                            div()
                                .flex_none()
                                .text_color(colors.text_placeholder)
                                .child(SharedString::from(project)),
                        )
                        .child(div().flex_none().text_color(colors.text_placeholder).child("/"))
                        .child(
                            div()
                                .min_w_0()
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(colors.text_muted)
                                .truncate()
                                .child(SharedString::from(snapshot.title.clone())),
                        ),
                )
                .into_any_element(),
        )
    }

    fn render_body(&mut self, cx: &mut Context<Self>) -> AnyElement {
        if self.transcript.is_empty() && !self.busy {
            return render_greeting(cx);
        }
        let message_count = self.transcript.message_count();
        let live_ix = self.transcript.live_agent_ix(self.busy);

        list(
            self.transcript.list_state.clone(),
            cx.processor(move |this, ix: usize, window, cx| {
                let content: AnyElement = if let Some(view) = this.transcript.message(ix) {
                    let live = live_ix == Some(ix);
                    let verdict = this.feedback.get(&view.ts.to_bits()).copied();
                    render_message(view, ix, live, verdict, &this.fades, window, cx)
                } else if ix == message_count {
                    render_shimmer(cx)
                } else {
                    gpui::Empty.into_any_element()
                };
                // `.chat-col`: max-width 780, centered, 32px gutters.
                h_flex()
                    .w_full()
                    .justify_center()
                    .child(div().w_full().max_w(px(780.)).px(px(32.)).child(content))
                    .into_any_element()
            }),
        )
        .with_sizing_behavior(gpui::ListSizingBehavior::Auto)
        .flex_grow()
        .into_any_element()
    }
}

impl Render for OrchestratorPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors();
        let panel = v_flex()
            .key_context("OrchestratorPanel")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(colors.panel_background)
            .children(self.render_crumb(cx))
            .child(
                // `.flex_1().size_full()` is the thread_view list-host idiom
                // (thread_view.rs render): without the definite `size_full`
                // height the Auto-sized `list()` measures ZERO and the
                // transcript paints nothing (dogfood 2026-07-07 — greeting
                // rendered, 12 live messages didn't).
                v_flex()
                    .flex_1()
                    .size_full()
                    .min_h_0()
                    .pt(px(20.))
                    .pb(px(8.))
                    .child(self.render_body(cx)),
            )
            .child(self.render_composer(window, cx));
        // The single frame pump for every Instant-clocked motion value the
        // panel renders bare (the §8.7a stable-identity rule: animated
        // styles read `current()` per frame instead of riding identity-
        // churning animation wrappers). Settled frames schedule nothing
        // (§8 idle cost).
        if self.fades.any_animating()
            || (self.tasks_island.visible() && self.tasks_island.any_animating())
        {
            window.request_animation_frame();
        }
        panel
    }
}

impl Focusable for OrchestratorPanel {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        // Focusing the panel focuses the composer (the agent-panel idiom) —
        // ToggleFocus drops the caret straight into the input.
        self.composer.editor.read(cx).focus_handle(cx)
    }
}

impl crate::mode_item::ModeSurface for OrchestratorPanel {
    // Center-pane surface (Amendment 2026-07-04 (2)) — the dock `Panel`
    // era is retired.
    fn fallback_tab_title() -> SharedString {
        "Orchestrator".into()
    }

    fn fallback_tab_icon() -> IconName {
        IconName::Chat
    }

    fn set_surface_active(&mut self, active: bool, cx: &mut Context<Self>) {
        // The center item host invokes this on tab activation/deactivation
        // (`mode_item.rs`) — the native equivalent of the web's route
        // mount/unmount, gating the `/transcript` poll.
        self.set_poll_active(active, cx);
    }
}
