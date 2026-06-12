//! The Task Board workspace panel (PARITY_SPEC §4.2 board anatomy):
//! header ("Task board" · run count · "N live" · Graph/Grid seg-toggle) over
//! the inbox list or the spawn-tree graph. Holds the shared
//! `Entity<BridgeStore>` and re-renders on its notifies — push-driven, no
//! panel-side timers (RUST_PORT_NOTES general principle 1).

use gpui::{
    Action, App, Context, Entity, EventEmitter, FocusHandle, Focusable, FontWeight, SharedString,
    Subscription, Window, actions,
};
use settings::Settings as _;
use theme_settings::ThemeSettings;
use ui::prelude::*;
use workspace::dock::{DockPosition, Panel, PanelEvent};

use crate::agent_accents::STATUS_RUNNING;
use crate::bridge::{self, BridgeStore};

use super::inbox;
use super::style::SURFACE_1;

actions!(
    task_board,
    [
        /// Toggles focus on the task board panel.
        ToggleFocus
    ]
);

/// Which board body is showing — `.seg-toggle` state. Defaults to Graph
/// (the web default).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoardView {
    Graph,
    Grid,
}

/// Row/node click event — consumers (and the panel itself) open the run
/// drawer from it.
#[derive(Debug, Clone)]
pub enum TaskBoardEvent {
    OpenRun(SharedString),
}

pub struct TaskBoardPanel {
    focus_handle: FocusHandle,
    store: Entity<BridgeStore>,
    view: BoardView,
    position: DockPosition,
    _store_subscription: Subscription,
}

impl TaskBoardPanel {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let store = bridge::global_store(cx);
        let _store_subscription = cx.observe(&store, |_, _, cx| cx.notify());
        Self {
            focus_handle: cx.focus_handle(),
            store,
            view: BoardView::Graph,
            position: DockPosition::Left,
            _store_subscription,
        }
    }

    /// Row/node click → emit `OpenRun` (the run drawer consumes it — Task 4).
    pub fn open_run(&mut self, run_id: SharedString, cx: &mut Context<Self>) {
        cx.emit(TaskBoardEvent::OpenRun(run_id));
    }

    fn set_view(&mut self, view: BoardView, cx: &mut Context<Self>) {
        if self.view != view {
            self.view = view;
            cx.notify();
        }
    }

    /// `.board-head`: title 18px/500 · mono run count (+ "N live" in the
    /// running color) · Graph/Grid seg-toggle (hairline, flat, no boxes).
    fn render_header(&self, total: usize, running: usize, cx: &mut Context<Self>) -> Div {
        let colors = cx.theme().colors();
        let mono = ThemeSettings::get_global(cx).buffer_font.family.clone();

        let run_count = (total > 0).then(|| {
            h_flex()
                .items_center()
                .gap(px(4.))
                .font_family(mono)
                .text_size(px(13.))
                .text_color(colors.text_placeholder)
                .child(SharedString::from(format!(
                    "{total} run{}",
                    if total > 1 { "s" } else { "" }
                )))
                .when(running > 0, |this| {
                    this.child("·").child(
                        div()
                            .text_color(STATUS_RUNNING)
                            .child(SharedString::from(format!("{running} live"))),
                    )
                })
        });

        h_flex()
            .flex_none()
            .items_center()
            .gap(px(14.))
            .px(px(28.))
            .pt(px(18.))
            .pb(px(14.))
            .border_b_1()
            .border_color(colors.border)
            .child(
                div()
                    .text_size(px(18.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(colors.text)
                    .child("Task board"),
            )
            .children(run_count)
            .child(div().flex_1())
            .child(self.render_seg_toggle(cx))
    }

    fn render_seg_toggle(&self, cx: &mut Context<Self>) -> Div {
        let colors = cx.theme().colors();
        let segment = |label: &'static str, view: BoardView, this: &Self| {
            let on = this.view == view;
            div()
                .id(ElementId::Name(format!("seg-{label}").into()))
                .px(px(12.))
                .py(px(6.))
                .text_size(px(12.))
                .font_weight(FontWeight::MEDIUM)
                .cursor_pointer()
                .when(on, |seg| seg.bg(SURFACE_1).text_color(colors.text))
                .when(!on, |seg| seg.text_color(colors.text_placeholder))
                .child(label)
        };
        h_flex()
            .flex_none()
            .border_1()
            .border_color(colors.border)
            .rounded(px(8.))
            .overflow_hidden()
            .child(
                segment("Graph", BoardView::Graph, self).on_click(cx.listener(
                    |this, _, _, cx| this.set_view(BoardView::Graph, cx),
                )),
            )
            .child(
                segment("Grid", BoardView::Grid, self).on_click(cx.listener(
                    |this, _, _, cx| this.set_view(BoardView::Grid, cx),
                )),
            )
    }
}

impl Render for TaskBoardPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let store = self.store.read(cx);
        let connected = store.connected;
        let board = store.board.clone();
        let total = board.len();
        let running = board.iter().filter(|r| r.status == "running").count();

        let body = match self.view {
            BoardView::Grid => {
                // Newest activity first (board.js reverses before render).
                let mut rows = board;
                rows.reverse();
                inbox::inbox_list(std::sync::Arc::new(rows), cx.weak_entity(), cx)
            }
            // Spawn-tree graph lands in the next task; until then the graph
            // view shows its empty state.
            BoardView::Graph => super::style::empty_state(
                IconName::ListTree,
                "No spawn tree yet",
                "When the orchestrator delegates, subagents grow under their conversation root here.",
                cx,
            )
            .into_any_element(),
        };

        let colors = cx.theme().colors();
        v_flex()
            .key_context("TaskBoardPanel")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(colors.panel_background)
            .child(self.render_header(total, running, cx))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    // Bridge offline → grey out the (kept) last snapshot.
                    .when(!connected, |this| this.opacity(0.5))
                    .child(body),
            )
    }
}

impl Focusable for TaskBoardPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<PanelEvent> for TaskBoardPanel {}
impl EventEmitter<TaskBoardEvent> for TaskBoardPanel {}

impl Panel for TaskBoardPanel {
    fn persistent_name() -> &'static str {
        "TaskBoardPanel"
    }

    fn panel_key() -> &'static str {
        "TaskBoardPanel"
    }

    fn position(&self, _window: &Window, _cx: &App) -> DockPosition {
        self.position
    }

    fn position_is_valid(&self, position: DockPosition) -> bool {
        matches!(position, DockPosition::Left | DockPosition::Right)
    }

    fn set_position(&mut self, position: DockPosition, _: &mut Window, cx: &mut Context<Self>) {
        // Runtime-only for Z1 — no settings persistence (the M2 switcher
        // opens panels in the dock they live in).
        self.position = position;
        cx.notify();
    }

    fn default_size(&self, _window: &Window, _cx: &App) -> Pixels {
        px(420.)
    }

    fn icon(&self, _window: &Window, _cx: &App) -> Option<IconName> {
        Some(IconName::ListTodo)
    }

    fn icon_tooltip(&self, _window: &Window, _cx: &App) -> Option<&'static str> {
        Some("Task Board")
    }

    fn toggle_action(&self) -> Box<dyn Action> {
        Box::new(ToggleFocus)
    }

    fn activation_priority(&self) -> u32 {
        5
    }
}
