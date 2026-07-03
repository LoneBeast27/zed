//! The Constellation panel — the subagent-lifecycle board as a RIGHT-DOCK
//! workspace `Panel` beside the orchestrator conversation (RUST_PORT_NOTES
//! §10, user-locked 2026-07-03). Holds the shared `Entity<BridgeStore>`
//! (SSE-push-fed) + the [`Sim`]; each frame: fold data → advance sim →
//! build elements → pump the next frame while anything breathes.
//!
//! Data flow: SSE pushes run/status + channel frames through the store; the
//! supplemental 2s poll (feed.rs) covers what the SSE digests don't carry
//! (token mass, ctx ramp, overlap) — visibility-gated, change-gated.
//! `ZED_CONSTELLATION_DEMO=1` swaps in the staged scenario instead.

use std::time::Instant;

use gpui::{
    Action, App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable,
    FontWeight, MouseButton, Pixels, Point, SharedString, Subscription, Task, Window, actions,
};
use ui::prelude::*;
use workspace::dock::{DockPosition, Panel, PanelEvent};

use crate::bridge::{self, BridgeStore, OverlapRow};
use crate::task_board::run_detail;
use crate::task_board::style::empty_state;

use super::demo;
use super::draw::conv_block;
use super::feed;
use super::sim::Sim;

actions!(
    constellation,
    [
        /// Toggles focus on the constellation panel.
        ToggleFocus
    ]
);

/// An in-flight node drag: pointer deltas re-aim the anchor 1:1.
struct DragState {
    run_id: SharedString,
    start_mouse: Point<Pixels>,
    start_anchor: (f32, f32),
    moved: bool,
}

pub struct ConstellationPanel {
    focus_handle: FocusHandle,
    store: Entity<BridgeStore>,
    sim: Sim,
    /// Staged-scenario mode (`ZED_CONSTELLATION_DEMO=1`) — no bridge I/O.
    demo: bool,
    demo_start: Instant,
    drag: Option<DragState>,
    /// A drag that moved suppresses the click it lands on.
    suppress_click: bool,
    hovered: Option<SharedString>,
    /// The run drawer (task_board's — same drawer, same anatomy).
    drawer: Option<Entity<run_detail::RunDrawer>>,
    /// Stamped every render — the feed poll's visibility gate.
    last_render_at: Instant,
    position: DockPosition,
    scroll: gpui::ScrollHandle,
    _feed: Option<Task<()>>,
    _store_subscription: Subscription,
}

impl ConstellationPanel {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let demo = demo::is_demo();
        let store = bridge::global_store(cx);
        let _store_subscription = cx.observe(&store, |_, _, cx| cx.notify());
        let feed = (!demo).then(|| {
            let http_client = cx.http_client();
            cx.spawn(async move |this, cx| feed::feed_loop(http_client, this, cx).await)
        });
        Self {
            focus_handle: cx.focus_handle(),
            store,
            sim: Sim::default(),
            demo,
            demo_start: Instant::now(),
            drag: None,
            suppress_click: false,
            hovered: None,
            drawer: None,
            last_render_at: Instant::now(),
            position: DockPosition::Right,
            scroll: gpui::ScrollHandle::new(),
            _feed: feed,
            _store_subscription,
        }
    }

    /// Whether the panel rendered within the feed's visibility window (a
    /// hidden dock panel isn't rendered → the poll idles).
    pub(super) fn rendered_recently(&self) -> bool {
        self.last_render_at.elapsed() < feed::VISIBLE_WINDOW
    }

    pub(super) fn store(&self) -> Entity<BridgeStore> {
        self.store.clone()
    }

    /// Latest overlap venn rows from the feed (arrange weights).
    pub(super) fn set_overlap(&mut self, rows: Vec<OverlapRow>) {
        self.sim.set_overlap(rows);
    }

    // ── interaction plumbing (draw.rs calls these through the weak entity) ──

    pub(super) fn begin_node_drag(
        &mut self,
        run_id: SharedString,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let Some(anchor) = self
            .sim
            .convs
            .iter()
            .flat_map(|conv| conv.nodes.iter())
            .find(|node| node.run_id == run_id.as_ref())
            .map(|node| (node.ax, node.ay))
        else {
            return;
        };
        self.sim.begin_drag(&run_id);
        self.drag = Some(DragState {
            run_id,
            start_mouse: position,
            start_anchor: anchor,
            moved: false,
        });
        cx.notify();
    }

    fn drag_moved(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(drag) = &mut self.drag else { return };
        let dx = (position.x - drag.start_mouse.x).as_f32();
        let dy = (position.y - drag.start_mouse.y).as_f32();
        if dx.abs() + dy.abs() > 4. {
            drag.moved = true;
        }
        let run_id = drag.run_id.clone();
        let (ax, ay) = (drag.start_anchor.0 + dx, drag.start_anchor.1 + dy);
        self.sim.drag_to(&run_id, ax, ay);
        cx.notify();
    }

    fn drag_ended(&mut self, cx: &mut Context<Self>) {
        if let Some(drag) = self.drag.take() {
            self.suppress_click = drag.moved;
            let now = self.sim.clock_ms();
            self.sim.end_drag(now);
            cx.notify();
        }
    }

    /// Node click → run drawer (a drag is not a click). Double-click →
    /// split-view agent feed is a banked next slice (agent-feed.js port).
    pub(super) fn node_clicked(&mut self, run_id: SharedString, cx: &mut Context<Self>) {
        if std::mem::take(&mut self.suppress_click) {
            return;
        }
        let drawer = cx.new(|cx| run_detail::RunDrawer::new(run_id, cx));
        cx.subscribe(&drawer, |this, _, _: &run_detail::DismissDrawer, cx| {
            this.drawer = None;
            cx.notify();
        })
        .detach();
        self.drawer = Some(drawer);
        cx.notify();
    }

    pub(super) fn set_hover(&mut self, run_id: SharedString, hovered: bool, cx: &mut Context<Self>) {
        if hovered {
            if self.hovered.as_ref() != Some(&run_id) {
                self.hovered = Some(run_id);
                cx.notify();
            }
        } else if self.hovered.as_ref() == Some(&run_id) {
            self.hovered = None;
            cx.notify();
        }
    }

    /// Root click → follow that conversation (the web's `selectConv`).
    pub(super) fn select_conversation(&mut self, conv: SharedString, cx: &mut Context<Self>) {
        self.store.update(cx, |store, cx| {
            store.set_conversation(Some(conv.to_string()), cx);
        });
    }

    fn arrange_clicked(&mut self, cx: &mut Context<Self>) {
        let now = self.sim.clock_ms();
        self.sim.auto_arrange(now);
        cx.notify();
    }

    /// The `.board-head` twin: title · arrange button.
    fn render_header(&self, has_nodes: bool, cx: &mut Context<Self>) -> Div {
        let colors = cx.theme().colors();
        h_flex()
            .flex_none()
            .items_center()
            .gap(px(10.))
            .px(px(16.))
            .pt(px(12.))
            .pb(px(10.))
            .border_b_1()
            .border_color(colors.border)
            .child(
                div()
                    .text_size(px(14.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(colors.text)
                    .child("Constellation"),
            )
            .child(div().flex_1())
            .when(has_nodes, |header| {
                header.child(
                    ui::IconButton::new("constellation-arrange", IconName::GitGraph)
                        .icon_size(ui::IconSize::Small)
                        .tooltip(ui::Tooltip::text(
                            "Auto-arrange the constellation (drags re-pin)",
                        ))
                        .on_click(cx.listener(|this, _, _, cx| this.arrange_clicked(cx))),
                )
            })
    }
}

impl Render for ConstellationPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.last_render_at = Instant::now();
        let now = self.sim.clock_ms();

        // Gather the frame's data: staged scenario or the shared store.
        let (board, convs, channels, connected) = if self.demo {
            let t = self.demo_start.elapsed().as_secs_f64();
            self.sim.set_overlap(demo::demo_overlap());
            (
                demo::demo_board(t),
                demo::demo_convs(t),
                demo::demo_channels(t),
                true,
            )
        } else {
            let store = self.store.read(cx);
            (
                store.ticked_board(),
                store.conversations().cloned().collect(),
                store.channels.clone(),
                store.connected,
            )
        };

        // Fold → advance (everything the draw layer reads is computed here,
        // before any element builds — the render path only reads).
        let width = {
            let measured = self.scroll.bounds().size.width.as_f32();
            if measured > 0. { measured.max(340.) } else { 440. }
        };
        self.sim.fold(&board, &convs, width, now);
        self.sim.fold_channels(&channels, now);
        let animating = self.sim.advance(now);

        let weak = cx.weak_entity();
        let hovered = self.hovered.clone();
        let has_nodes = self.sim.convs.iter().any(|conv| !conv.nodes.is_empty());

        let body: gpui::AnyElement = if !has_nodes {
            empty_state(
                IconName::Sparkle,
                "No spawn tree yet",
                "When the orchestrator delegates, subagents orbit their conversation root here.",
                cx,
            )
            .into_any_element()
        } else {
            let channel_sims = self.sim.channels.iter_sorted();
            let blocks: Vec<gpui::AnyElement> = self
                .sim
                .convs
                .iter()
                .map(|conv| {
                    let conv_channels: Vec<_> = channel_sims
                        .iter()
                        .filter(|c| c.row.conv == conv.conv_id)
                        .copied()
                        .collect();
                    conv_block(
                        conv,
                        &conv_channels,
                        &self.sim,
                        now,
                        hovered.as_ref(),
                        weak.clone(),
                        cx,
                    )
                })
                .collect();
            div()
                .id("constellation-scroll")
                .size_full()
                .overflow_y_scroll()
                .track_scroll(&self.scroll)
                .pt(px(8.))
                .children(blocks)
                .into_any_element()
        };

        // Frame pump: a populated constellation breathes (drift) — pump
        // while anything is on stage or a drag is live; an empty panel
        // schedules nothing (§8 idle cost).
        if animating || self.drag.is_some() {
            window.request_animation_frame();
        }

        let colors = cx.theme().colors();
        v_flex()
            .key_context("ConstellationPanel")
            .track_focus(&self.focus_handle)
            .relative()
            .size_full()
            .bg(colors.panel_background)
            .when(!connected && !self.demo, |this| this.opacity(0.5))
            .on_mouse_move(cx.listener(|this, event: &gpui::MouseMoveEvent, _, cx| {
                if this.drag.is_some() {
                    this.drag_moved(event.position, cx);
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.drag_ended(cx)),
            )
            .child(self.render_header(has_nodes, cx))
            .child(div().relative().flex_1().min_h_0().child(body))
            .children(self.drawer.clone())
    }
}

impl Focusable for ConstellationPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<PanelEvent> for ConstellationPanel {}

impl Panel for ConstellationPanel {
    fn persistent_name() -> &'static str {
        "ConstellationPanel"
    }

    fn panel_key() -> &'static str {
        "ConstellationPanel"
    }

    fn position(&self, _window: &Window, _cx: &App) -> DockPosition {
        self.position
    }

    fn position_is_valid(&self, position: DockPosition) -> bool {
        matches!(position, DockPosition::Left | DockPosition::Right)
    }

    fn set_position(&mut self, position: DockPosition, _: &mut Window, cx: &mut Context<Self>) {
        self.position = position;
        cx.notify();
    }

    fn default_size(&self, _window: &Window, _cx: &App) -> Pixels {
        px(460.)
    }

    fn icon(&self, _window: &Window, _cx: &App) -> Option<IconName> {
        Some(IconName::Sparkle)
    }

    fn icon_tooltip(&self, _window: &Window, _cx: &App) -> Option<&'static str> {
        Some("Constellation")
    }

    fn toggle_action(&self) -> Box<dyn Action> {
        Box::new(ToggleFocus)
    }

    fn activation_priority(&self) -> u32 {
        6
    }
}
