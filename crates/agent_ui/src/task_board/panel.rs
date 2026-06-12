//! The Task Board workspace panel (PARITY_SPEC §4.2 board anatomy):
//! header ("Task board" · run count · "N live" · Graph/Grid seg-toggle) over
//! the inbox list or the spawn-tree graph. Holds the shared
//! `Entity<BridgeStore>` and re-renders on its notifies — push-driven
//! (RUST_PORT_NOTES general principle 1); the only timer is the store's 1s
//! elapsed ticker while a run is live (the §5 worked-for ticker).

use gpui::{
    Action, Animation, AnimationExt as _, AnyElement, App, Context, Entity, EventEmitter,
    FocusHandle, Focusable, FontWeight, Hsla, SharedString, Subscription, Window, actions,
};
use settings::Settings as _;
use theme_settings::ThemeSettings;
use ui::prelude::*;
use workspace::dock::{DockPosition, Panel, PanelEvent};

use crate::agent_accents::STATUS_RUNNING;
use crate::bridge::{self, BridgeStore};

use super::motion::{EFFECTS, STATE_FADE, StateFade, mix};
use super::style::SURFACE_1;
use super::{inbox, run_detail};

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
/// drawer from it; root-node clicks select the conversation.
#[derive(Debug, Clone)]
pub enum TaskBoardEvent {
    OpenRun(SharedString),
    SelectConversation(SharedString),
}

pub struct TaskBoardPanel {
    focus_handle: FocusHandle,
    store: Entity<BridgeStore>,
    view: BoardView,
    position: DockPosition,
    /// The graph's `seenRuns` (run id → first graph render), driving
    /// fresh-node spawn + edge draw-in exactly once per run.
    graph_seen: std::collections::HashMap<SharedString, std::time::Instant>,
    /// The inbox twin: run id → first board appearance. Rows attach their
    /// `.rise` animation only inside the fresh window, so a row scrolling
    /// back into the uniform_list viewport never replays its entrance
    /// (element state drops on cull; this map persists outside it).
    inbox_seen: std::collections::HashMap<SharedString, std::time::Instant>,
    /// Graph scroll position — drives conv-tree viewport culling.
    graph_scroll: gpui::ScrollHandle,
    /// Cached static paint geometry (settled edges, root rings) shared into
    /// the graph's paint closures.
    graph_paint_cache: super::paint_cache::SharedPaintCache,
    /// The open run drawer, if any (right slide-over).
    drawer: Option<Entity<run_detail::RunDrawer>>,
    /// Seg-toggle 150ms state crossfade (web `.seg-toggle button` transition).
    view_fade: StateFade,
    /// Tracked inbox-row hover (replaces the instant `.hover()` style):
    /// drives the 150ms row-bg crossfade and the pill-elapsed reveal.
    hovered_row: Option<(SharedString, StateFade)>,
    /// The row that most recently lost hover — keeps its fade-out alive.
    unhovered_row: Option<(SharedString, StateFade)>,
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
            graph_seen: std::collections::HashMap::new(),
            inbox_seen: std::collections::HashMap::new(),
            graph_scroll: gpui::ScrollHandle::new(),
            graph_paint_cache: Default::default(),
            drawer: None,
            view_fade: StateFade::default(),
            hovered_row: None,
            unhovered_row: None,
            _store_subscription,
        }
    }

    /// Tracked-hover transition for an inbox row: each flip opens a 150ms
    /// crossfade window (web `.inbox-row { transition: background .15s }`).
    pub(super) fn set_row_hover(
        &mut self,
        run_id: SharedString,
        hovered: bool,
        cx: &mut Context<Self>,
    ) {
        if hovered {
            if self
                .hovered_row
                .as_ref()
                .is_some_and(|(id, _)| *id == run_id)
            {
                return;
            }
            if let Some((old, _)) = self.hovered_row.take() {
                self.unhovered_row = Some((old, StateFade::begun()));
            }
            self.hovered_row = Some((run_id, StateFade::begun()));
            cx.notify();
        } else if self
            .hovered_row
            .as_ref()
            .is_some_and(|(id, _)| *id == run_id)
        {
            let (old, _) = self.hovered_row.take().unwrap();
            self.unhovered_row = Some((old, StateFade::begun()));
            cx.notify();
        }
    }

    /// Row/node click → emit `OpenRun` + open the run drawer in place.
    pub fn open_run(&mut self, run_id: SharedString, cx: &mut Context<Self>) {
        let drawer = cx.new(|cx| run_detail::RunDrawer::new(run_id.clone(), cx));
        cx.subscribe(&drawer, |this, _, _: &run_detail::DismissDrawer, cx| {
            this.drawer = None;
            cx.notify();
        })
        .detach();
        self.drawer = Some(drawer);
        cx.emit(TaskBoardEvent::OpenRun(run_id));
        cx.notify();
    }

    /// Graph root click → emit (the web's `selectConv`; the native
    /// orchestrator panel consumes this when Z3 lands).
    pub fn select_conversation(&mut self, conv: SharedString, cx: &mut Context<Self>) {
        cx.emit(TaskBoardEvent::SelectConversation(conv));
    }

    fn set_view(&mut self, view: BoardView, cx: &mut Context<Self>) {
        if self.view != view {
            self.view = view;
            self.view_fade.bump();
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
        let border = colors.border;
        let on_text = colors.text;
        let off_text = colors.text_placeholder;
        let on_bg: Hsla = SURFACE_1.into();
        let off_bg = gpui::transparent_black();
        let fresh = self.view_fade.fresh();
        let generation = self.view_fade.generation() as u64;
        let current = self.view;

        let segment = |label: &'static str, view: BoardView, cx: &Context<Self>| -> AnyElement {
            let on = current == view;
            let base = div()
                .id(ElementId::Name(format!("seg-{label}").into()))
                .px(px(12.))
                .py(px(6.))
                .text_size(px(12.))
                .font_weight(FontWeight::MEDIUM)
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| this.set_view(view, cx)))
                .child(label);
            // 150ms effects-curve bg/color crossfade on the state flip
            // (board.css `.seg-toggle button { transition: … .15s }`);
            // settled segments render bare — no idle animation wrappers.
            if fresh {
                let (from_bg, to_bg) = if on { (off_bg, on_bg) } else { (on_bg, off_bg) };
                let (from_text, to_text) = if on {
                    (off_text, on_text)
                } else {
                    (on_text, off_text)
                };
                base.with_animation(
                    ElementId::NamedInteger(format!("seg-fade-{label}").into(), generation),
                    Animation::new(STATE_FADE).with_easing(EFFECTS.easing()),
                    move |seg, t| {
                        seg.bg(mix(from_bg, to_bg, t))
                            .text_color(mix(from_text, to_text, t))
                    },
                )
                .into_any_element()
            } else if on {
                base.bg(on_bg).text_color(on_text).into_any_element()
            } else {
                base.text_color(off_text).into_any_element()
            }
        };
        h_flex()
            .flex_none()
            .border_1()
            .border_color(border)
            .rounded(px(8.))
            .overflow_hidden()
            .child(segment("Graph", BoardView::Graph, cx))
            .child(segment("Grid", BoardView::Grid, cx))
    }
}

impl Render for TaskBoardPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let store = self.store.read(cx);
        let connected = store.connected;
        // Locally-ticked elapsed (the store's 1s ticker drives re-renders
        // while any run is live; the server frame re-bases the offset).
        let board = store.ticked_board();
        let total = board.len();
        let running = board.iter().filter(|r| r.status == "running").count();

        let body = match self.view {
            BoardView::Grid => {
                // Newest activity first (board.js reverses before render).
                let mut rows = board;
                rows.reverse();
                let hover = inbox::RowHoverState {
                    hovered: self
                        .hovered_row
                        .as_ref()
                        .map(|(id, fade)| (id.clone(), fade.fresh())),
                    unhovered: self
                        .unhovered_row
                        .as_ref()
                        .map(|(id, fade)| (id.clone(), fade.fresh())),
                };
                inbox::inbox_list(
                    std::sync::Arc::new(rows),
                    &mut self.inbox_seen,
                    hover,
                    cx.weak_entity(),
                    cx,
                )
            }
            BoardView::Graph => {
                let weak = cx.weak_entity();
                let scroll = self.graph_scroll.clone();
                let cache = self.graph_paint_cache.clone();
                super::graph::graph_view(board, &mut self.graph_seen, &scroll, &cache, weak, cx)
            }
        };

        let colors = cx.theme().colors();
        v_flex()
            .key_context("TaskBoardPanel")
            .track_focus(&self.focus_handle)
            .relative()
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
            .children(self.drawer.clone())
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
