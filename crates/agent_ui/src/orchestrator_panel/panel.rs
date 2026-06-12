//! The Orchestrator chat workspace panel (PARITY_SPEC §4.1): crumb header
//! over the virtualized transcript (greeting when empty), with the composer
//! deck beneath (Z3 Task 3). Holds the shared `Entity<BridgeStore>` and a
//! [`TranscriptWatch`] — the `/transcript` poll runs only while this panel
//! is alive (Lightness: panel presence gates the poll).

use std::time::Duration;

use gpui::{
    Action, AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable,
    SharedString, Subscription, Task, WeakEntity, Window, actions, list,
};
use ui::prelude::*;
use workspace::Workspace;
use workspace::dock::{DockPosition, Panel, PanelEvent};

use crate::bridge::{self, BridgeStore, TranscriptSnapshot, TranscriptWatch};
use crate::task_board::TaskBoardPanel;
use crate::task_board::motion::StateFade;

use super::composer::Composer;
use super::message::render_message;
use super::transcript::{TranscriptView, render_greeting, render_shimmer};

actions!(
    orchestrator_panel,
    [
        /// Toggles focus on the orchestrator chat panel.
        ToggleFocus,
        /// Sends the composer's message (Enter via the
        /// `OrchestratorComposer > Editor` keymap binding).
        Send
    ]
);

/// The worked-for rolling-tick cadence (RUST_PORT_NOTES §5: the 1s
/// worked-for tickers are the only client timers that survive natively).
const BUSY_TICK: Duration = Duration::from_secs(1);

pub struct OrchestratorPanel {
    focus_handle: FocusHandle,
    pub(super) store: Entity<BridgeStore>,
    workspace: WeakEntity<Workspace>,
    position: DockPosition,
    pub(super) transcript: TranscriptView,
    pub(super) composer: Composer,
    /// Orchestrator busy flag from the latest snapshot.
    pub(super) busy: bool,
    /// The 1s rolling-tick task — `Some` only while busy (store ticker
    /// pattern; no idle timers). Repaints so a live trailing reply's
    /// worked-for label (`worked_s + seen_at.elapsed()`) rolls.
    busy_ticker: Option<Task<()>>,
    /// Change gate for the store observer (the store also notifies at 1Hz
    /// for board elapsed this panel renders only via the busy tick).
    last_snapshot: Option<TranscriptSnapshot>,
    /// Tracked message hover (the meta-trio 150ms reveal — same idiom as
    /// the board's row hover).
    hovered_msg: Option<(usize, StateFade)>,
    unhovered_msg: Option<(usize, StateFade)>,
    _transcript_watch: TranscriptWatch,
    _store_subscription: Subscription,
}

impl OrchestratorPanel {
    pub fn new(
        workspace: WeakEntity<Workspace>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let store = bridge::global_store(cx);
        let watch = store.update(cx, |store, cx| store.watch_transcript(cx));
        let _store_subscription =
            cx.observe(&store, |this: &mut Self, _, cx| this.sync_from_store(cx));
        Self {
            focus_handle: cx.focus_handle(),
            store,
            workspace,
            position: DockPosition::Left,
            transcript: TranscriptView::new(),
            composer: Composer::new(window, cx),
            busy: false,
            busy_ticker: None,
            last_snapshot: None,
            hovered_msg: None,
            unhovered_msg: None,
            _transcript_watch: watch,
            _store_subscription,
        }
    }

    /// A store notify landed: re-sync only when the transcript actually
    /// changed (board/usage notifies ride the same entity).
    fn sync_from_store(&mut self, cx: &mut Context<Self>) {
        let Some(snapshot) = self.store.read(cx).transcript.clone() else {
            return;
        };
        if self.last_snapshot.as_ref() == Some(&snapshot) {
            return;
        }
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
        cx.notify();
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

    /// Step-row click → route to the task board and open that run's drawer
    /// (the Z2 toast's pattern — emits `TaskBoardEvent::OpenRun` inside).
    pub(super) fn open_run(
        &mut self,
        run_id: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.workspace
            .update(cx, |workspace, cx| {
                workspace.focus_panel::<TaskBoardPanel>(window, cx);
                if let Some(panel) = workspace.panel::<TaskBoardPanel>(cx) {
                    panel.update(cx, |panel, cx| panel.open_run(run_id.clone(), cx));
                }
            })
            .ok();
    }

    /// Tracked-hover flip for a message's meta trio (board row idiom).
    pub(super) fn set_message_hover(&mut self, ix: usize, hovered: bool, cx: &mut Context<Self>) {
        if hovered {
            if self.hovered_msg.as_ref().is_some_and(|(at, _)| *at == ix) {
                return;
            }
            if let Some((old, _)) = self.hovered_msg.take() {
                self.unhovered_msg = Some((old, StateFade::begun()));
            }
            self.hovered_msg = Some((ix, StateFade::begun()));
            cx.notify();
        } else if self.hovered_msg.as_ref().is_some_and(|(at, _)| *at == ix) {
            let (old, _) = self.hovered_msg.take().unwrap();
            self.unhovered_msg = Some((old, StateFade::begun()));
            cx.notify();
        }
    }

    /// (hovered, mid-crossfade) for a message index.
    fn hover_state(&self, ix: usize) -> (bool, bool) {
        if let Some((at, fade)) = &self.hovered_msg
            && *at == ix
        {
            return (true, fade.fresh());
        }
        if let Some((at, fade)) = &self.unhovered_msg
            && *at == ix
        {
            return (false, fade.fresh());
        }
        (false, false)
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
        let live_ix = self
            .busy
            .then(|| self.transcript.live_agent_ix())
            .flatten();

        list(
            self.transcript.list_state.clone(),
            cx.processor(move |this, ix: usize, window, cx| {
                let content: AnyElement = if let Some(view) = this.transcript.message(ix) {
                    let live = live_ix == Some(ix);
                    let hover = this.hover_state(ix);
                    render_message(view, ix, live, hover, window, cx)
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
        v_flex()
            .key_context("OrchestratorPanel")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(colors.panel_background)
            .children(self.render_crumb(cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .pt(px(20.))
                    .pb(px(8.))
                    .child(self.render_body(cx)),
            )
            .child(self.render_composer(window, cx))
    }
}

impl Focusable for OrchestratorPanel {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        // Focusing the panel focuses the composer (the agent-panel idiom) —
        // ToggleFocus drops the caret straight into the input.
        self.composer.editor.read(cx).focus_handle(cx)
    }
}

impl EventEmitter<PanelEvent> for OrchestratorPanel {}

impl Panel for OrchestratorPanel {
    fn persistent_name() -> &'static str {
        "OrchestratorPanel"
    }

    fn panel_key() -> &'static str {
        "OrchestratorPanel"
    }

    fn position(&self, _window: &Window, _cx: &App) -> DockPosition {
        self.position
    }

    fn position_is_valid(&self, position: DockPosition) -> bool {
        matches!(position, DockPosition::Left | DockPosition::Right)
    }

    fn set_position(&mut self, position: DockPosition, _: &mut Window, cx: &mut Context<Self>) {
        // Runtime-only, mirroring the task board (Z1).
        self.position = position;
        cx.notify();
    }

    fn default_size(&self, _window: &Window, _cx: &App) -> Pixels {
        px(520.)
    }

    fn icon(&self, _window: &Window, _cx: &App) -> Option<IconName> {
        Some(IconName::ZedAgent)
    }

    fn icon_tooltip(&self, _window: &Window, _cx: &App) -> Option<&'static str> {
        Some("Orchestrator")
    }

    fn toggle_action(&self) -> Box<dyn Action> {
        Box::new(ToggleFocus)
    }

    fn activation_priority(&self) -> u32 {
        4
    }
}
