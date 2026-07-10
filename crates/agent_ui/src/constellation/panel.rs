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
    Action, AnimationExt as _, App, AppContext as _, Context, Entity, EventEmitter, FocusHandle,
    Focusable, MouseButton, Pixels, Point, SharedString, Subscription, Task, Window, actions,
};
use ui::prelude::*;
use workspace::dock::{DockPosition, Panel, PanelEvent};

use crate::bridge::{self, BridgeStore, ChannelEventRow, OverlapRow};
use crate::task_board::run_detail;
use crate::task_board::style::empty_state;

use super::channels_section::channels_section;
use super::demo;
use super::draw::conv_block;
use super::feed;
use super::header::render_header;
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
    pub(super) sim: Sim,
    /// Staged-scenario mode (`ZED_CONSTELLATION_DEMO=1`) — no bridge I/O.
    demo: bool,
    demo_start: Instant,
    drag: Option<DragState>,
    /// A drag that moved suppresses the click it lands on.
    suppress_click: bool,
    hovered: Option<SharedString>,
    /// The run drawer (task_board's — same drawer, same anatomy).
    drawer: Option<Entity<run_detail::RunDrawer>>,
    /// In demo mode, the run the open drawer is showing — the render loop
    /// pushes fresh staged detail into it (demo runs don't exist on the
    /// bridge; a fetch would 404).
    demo_drawer_run: Option<SharedString>,
    /// Whole-second stamp of the last demo push — the drawer refresh rides
    /// the panel's EXISTING frame pump but only lands once per second (the
    /// web's 1s drawer tick), not per frame.
    demo_drawer_pushed_s: Option<u64>,
    /// Stamped every render — the feed poll's visibility gate.
    last_render_at: Instant,
    /// Empty→populated transition (backlog: the blank-to-graph flip was
    /// abrupt): bumped when the first node arrives; the graph fades in
    /// 300ms instead of snapping.
    was_populated: bool,
    populated_fade: crate::task_board::motion::StateFade,
    /// Channels observe surface (T2): shown in the below-graph detail area
    /// while no run drawer is open; the header toggle hides/shows it.
    pub(super) channels_shown: bool,
    /// The Tier-0 receipt feed off the supplemental `/channels` poll
    /// (newest LAST, wire order) — display data for the channels section.
    pub(super) channel_events: Vec<ChannelEventRow>,
    /// The latest overlap venn rows, kept for DISPLAY (the sim holds its own
    /// copy for arrange weights).
    pub(super) overlap_rows: Vec<OverlapRow>,
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
            demo_drawer_run: None,
            demo_drawer_pushed_s: None,
            last_render_at: Instant::now(),
            was_populated: false,
            populated_fade: crate::task_board::motion::StateFade::default(),
            channels_shown: true,
            channel_events: Vec::new(),
            overlap_rows: Vec::new(),
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
    ///
    /// Demo nodes are synthetic — they don't exist on the bridge, so the
    /// drawer is fed from the staged store ([`demo::demo_run_detail`])
    /// instead of fetching (which would 404 into an error state).
    pub(super) fn node_clicked(&mut self, run_id: SharedString, cx: &mut Context<Self>) {
        if std::mem::take(&mut self.suppress_click) {
            return;
        }
        let drawer = if self.demo {
            let t = self.demo_start.elapsed().as_secs_f64();
            let detail =
                demo::demo_run_detail(&run_id, t).unwrap_or_else(|| run_detail::RunDetail {
                    run_id: run_id.to_string(),
                    ..Default::default()
                });
            self.demo_drawer_run = Some(run_id);
            self.demo_drawer_pushed_s = Some(t as u64);
            cx.new(|cx| {
                let mut drawer = run_detail::RunDrawer::local(detail, cx);
                drawer.embed();
                drawer
            })
        } else {
            cx.new(|cx| {
                let mut drawer = run_detail::RunDrawer::new(run_id, cx);
                drawer.embed();
                drawer
            })
        };
        cx.subscribe(&drawer, |this, _, _: &run_detail::DismissDrawer, cx| {
            this.drawer = None;
            this.demo_drawer_run = None;
            this.demo_drawer_pushed_s = None;
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

    pub(super) fn arrange_clicked(&mut self, cx: &mut Context<Self>) {
        let now = self.sim.clock_ms();
        self.sim.auto_arrange(now);
        cx.notify();
    }
}

impl Render for ConstellationPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.last_render_at = Instant::now();
        let now = self.sim.clock_ms();

        // Gather the frame's data: staged scenario or the shared store.
        let (board, convs, channels, connected) = if self.demo {
            let t = self.demo_start.elapsed().as_secs_f64();
            let overlap = demo::demo_overlap();
            if self.overlap_rows != overlap {
                // Mid-render: assign only — a notify here would spin the
                // frame loop (we are already producing this frame).
                self.overlap_rows = overlap.clone();
            }
            self.sim.set_overlap(overlap);
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

        // Demo drawer refresh: rides THIS render's frame pump (no timer of
        // its own) but lands only once per whole second — elapsed ticks,
        // status flips arrive, the staged log grows.
        if self.demo
            && let (Some(run_id), Some(drawer)) =
                (self.demo_drawer_run.clone(), self.drawer.clone())
        {
            let t = self.demo_start.elapsed().as_secs_f64();
            let stamp = t as u64;
            if self.demo_drawer_pushed_s != Some(stamp) {
                self.demo_drawer_pushed_s = Some(stamp);
                if let Some(detail) = demo::demo_run_detail(&run_id, t) {
                    drawer.update(cx, |drawer, cx| drawer.push_local_detail(detail, cx));
                }
            }
        }

        let weak = cx.weak_entity();
        let hovered = self.hovered.clone();
        let has_nodes = self.sim.convs.iter().any(|conv| !conv.nodes.is_empty());
        if has_nodes != self.was_populated {
            self.was_populated = has_nodes;
            if has_nodes {
                // First node arrived — ease the graph in (300ms) instead of
                // the old instant empty→populated snap.
                self.populated_fade.bump();
            }
        }

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
            let graph = div()
                .id("constellation-scroll")
                .size_full()
                .overflow_y_scroll()
                .track_scroll(&self.scroll)
                .pt(px(8.))
                .children(blocks);
            if self.populated_fade.fresh() {
                graph
                    .with_animation(
                        gpui::ElementId::NamedInteger(
                            "constellation-populate".into(),
                            self.populated_fade.generation() as u64,
                        ),
                        gpui::Animation::new(std::time::Duration::from_millis(300))
                            .with_easing(crate::task_board::motion::EFFECTS.easing()),
                        |graph, t| graph.opacity(t),
                    )
                    .into_any_element()
            } else {
                graph.into_any_element()
            }
        };

        // Frame pump: a populated constellation breathes (drift) — pump
        // while anything is on stage or a drag is live; an empty panel
        // schedules nothing (§8 idle cost). An open DEMO drawer also keeps
        // the pump alive so its 1s elapsed tick can land even if the sim
        // settles (demo-only cost).
        if animating || self.drag.is_some() || (self.demo && self.demo_drawer_run.is_some()) {
            window.request_animation_frame();
        }

        // Channels observe surface (T2): rides the same below-graph detail
        // area while no run drawer holds it; the header toggle hides it.
        // Built OUTSIDE the element chain so the section is a pure read of
        // this frame's already-gathered snapshot (§11: no entity access).
        let channels_detail = (self.drawer.is_none() && self.channels_shown).then(|| {
            channels_section(
                &channels,
                &self.channel_events,
                &self.overlap_rows,
                connected || self.demo,
                cx,
            )
        });

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
            .child(render_header(self, has_nodes, cx))
            .child(div().relative().flex_1().min_h_0().child(body))
            // Unified surface (PARITY_SPEC amendment 2026-07-04): the agent
            // detail is a section BELOW the graph on the same background —
            // no overlay, no scrim, no seam. Graph keeps the larger share.
            .children(channels_detail)
            .children(self.drawer.clone().map(|drawer| {
                div()
                    .id("constellation-detail")
                    .flex_none()
                    .h(relative(0.45))
                    .min_h_0()
                    .child(drawer)
            }))
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
        16
    }
}
