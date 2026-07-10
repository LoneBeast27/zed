//! GRAPH-view interaction handlers (a `VaultBrowserPanel` impl split out of
//! `panel.rs` to hold the 500-line ceiling — single concern: node drag /
//! hover / reveal plumbing). All pure entity-state flips (§11: immediate
//! visual feedback, no layout mutation reached from these listeners).

use gpui::{Context, Pixels, Point, SharedString};

use super::panel::{VaultBrowserPanel, VaultView};

/// An in-flight graph node drag: pointer deltas re-aim the field anchor 1:1
/// (the constellation drag idiom).
pub(super) struct GraphDrag {
    id: SharedString,
    start_mouse: Point<Pixels>,
    start_anchor: (f32, f32),
    moved: bool,
}

/// An in-flight background pan (Q9 ruling 2026-07-11: plain wheel zooms, so
/// dragging empty graph space pans the viewport — the Obsidian grammar).
pub(super) struct GraphPan {
    start_mouse: Point<Pixels>,
    start_offset: Point<Pixels>,
}

impl VaultBrowserPanel {
    /// Begin dragging a graph node — record its current anchor so pointer
    /// deltas re-aim it 1:1 (constellation drag idiom). Pure state flip.
    pub fn begin_graph_drag(
        &mut self,
        id: SharedString,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let Some(anchor) = self
            .graph_field
            .nodes
            .iter()
            .find(|n| n.id == id.as_ref())
            .map(|n| (n.ax, n.ay))
        else {
            return;
        };
        self.graph_field.begin_drag(&id);
        self.graph_drag = Some(GraphDrag {
            id,
            start_mouse: position,
            start_anchor: anchor,
            moved: false,
        });
        cx.notify();
    }

    pub(super) fn graph_drag_moved(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let scale = self.graph_scale.max(0.05);
        let Some(drag) = &mut self.graph_drag else {
            return;
        };
        // Pointer deltas are VIEW pixels; anchors live in FIELD coordinates —
        // divide by the render scale so a drag lands where the cursor is at
        // any zoom (fit-mode s<1 included).
        let dx = (position.x - drag.start_mouse.x).as_f32() / scale;
        let dy = (position.y - drag.start_mouse.y).as_f32() / scale;
        if dx.abs() + dy.abs() > 4. {
            drag.moved = true;
        }
        let id = drag.id.clone();
        let (ax, ay) = (drag.start_anchor.0 + dx, drag.start_anchor.1 + dy);
        self.graph_field.drag_to(&id, ax, ay);
        cx.notify();
    }

    /// Wheel zoom (PLAIN wheel — Q9 ruling 2026-07-11, Obsidian grammar):
    /// multiply the fit-relative zoom, clamped [1, 6] — 1 is always "the
    /// whole field visible" (the window bound the user asked for). Panning
    /// when zoomed = background drag (`begin_graph_pan`).
    pub(super) fn zoom_graph(&mut self, factor: f32, cx: &mut Context<Self>) {
        let next = (self.graph_zoom * factor).clamp(1., 6.);
        if (next - self.graph_zoom).abs() > f32::EPSILON {
            self.graph_zoom = next;
            cx.notify();
        }
    }

    /// The "Fit" chip / background double-click: back to the whole field.
    pub fn reset_graph_zoom(&mut self, cx: &mut Context<Self>) {
        if self.graph_zoom != 1. {
            self.graph_zoom = 1.;
            cx.notify();
        }
    }

    /// Caption click → zoom INTO that cluster: scale so its bbox fills ~80%
    /// of the viewport and scroll it centered (the cluster navigation the
    /// user asked for, 2026-07-10).
    pub fn focus_graph_cluster(&mut self, project: &str, cx: &mut Context<Self>) {
        let bounds = self.graph_scroll.bounds().size;
        let (vw, vh) = (bounds.width.as_f32(), bounds.height.as_f32());
        if vw <= 0. || vh <= 0. {
            return;
        }
        let members: Vec<_> = self
            .graph_field
            .nodes
            .iter()
            .filter(|n| n.project == project)
            .collect();
        if members.is_empty() {
            return;
        }
        let pad = 60.;
        let min_x = members.iter().map(|n| n.out_x - n.r).fold(f32::MAX, f32::min) - pad;
        let max_x = members.iter().map(|n| n.out_x + n.r).fold(f32::MIN, f32::max) + pad;
        let min_y = members.iter().map(|n| n.out_y - n.r).fold(f32::MAX, f32::min) - pad;
        let max_y = members.iter().map(|n| n.out_y + n.r).fold(f32::MIN, f32::max) + pad;
        let (bw, bh) = ((max_x - min_x).max(1.), (max_y - min_y).max(1.));
        let fit_s = (vw / self.graph_field.width.max(1.))
            .min(vh / self.graph_field.height.max(1.))
            .min(1.);
        let target_s = ((vw / bw).min(vh / bh) * 0.85).clamp(fit_s, 4.);
        self.graph_zoom = (target_s / fit_s.max(0.001)).clamp(1., 6.);
        let s = fit_s * self.graph_zoom;
        // Scroll so the bbox center sits at the viewport center (offsets are
        // negative content displacement, clamped to the scrollable range).
        let content_w = (self.graph_field.width * s).max(vw);
        let content_h = (self.graph_field.height * s).max(vh);
        let cx_px = ((min_x + max_x) * 0.5) * s;
        let cy_px = ((min_y + max_y) * 0.5) * s;
        let off_x = (cx_px - vw * 0.5).clamp(0., (content_w - vw).max(0.));
        let off_y = (cy_px - vh * 0.5).clamp(0., (content_h - vh).max(0.));
        self.graph_scroll
            .set_offset(gpui::point(gpui::px(-off_x), gpui::px(-off_y)));
        cx.notify();
    }

    pub(super) fn graph_drag_ended(&mut self, cx: &mut Context<Self>) {
        if let Some(drag) = self.graph_drag.take() {
            self.graph_suppress_click = drag.moved;
            self.graph_field.end_drag(self.field_now());
            cx.notify();
        }
    }

    /// Begin a background pan — no-ops when a node drag is live (node
    /// mouse-down fires first in the bubble phase, so `graph_drag` is already
    /// set for node hits by the time the container's listener runs).
    pub fn begin_graph_pan(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        if self.graph_drag.is_some() {
            return;
        }
        self.graph_pan = Some(GraphPan {
            start_mouse: position,
            start_offset: self.graph_scroll.offset(),
        });
        cx.notify();
    }

    /// Pan: content follows the cursor 1:1 in VIEW pixels (offsets are
    /// negative content displacement; the scroll handle clamps to the
    /// scrollable range on paint, so out-of-range sets are safe).
    pub(super) fn graph_pan_moved(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(pan) = &self.graph_pan else {
            return;
        };
        let off = gpui::point(
            pan.start_offset.x + (position.x - pan.start_mouse.x),
            pan.start_offset.y + (position.y - pan.start_mouse.y),
        );
        self.graph_scroll.set_offset(off);
        cx.notify();
    }

    pub(super) fn graph_pan_ended(&mut self, cx: &mut Context<Self>) {
        if self.graph_pan.take().is_some() {
            cx.notify();
        }
    }

    /// The field sim clock (ms since the panel was created) — monotonic, so
    /// every field transition is measured against the same origin.
    pub(super) fn field_now(&self) -> f32 {
        self.field_epoch.elapsed().as_secs_f32() * 1000.
    }

    /// Hover a graph node (raise/lower its card). Pure state flip.
    pub fn set_graph_hover(&mut self, id: SharedString, hovered: bool, cx: &mut Context<Self>) {
        if hovered {
            if self.graph_hovered.as_ref() != Some(&id) {
                self.graph_hovered = Some(id);
                cx.notify();
            }
        } else if self.graph_hovered.as_ref() == Some(&id) {
            self.graph_hovered = None;
            cx.notify();
        }
    }

    /// Double-click a SESSION node → reveal it in the LIST view: flip the view
    /// toggle and stage the id so the list can scroll/select it (integration
    /// hook §3). A drag that moved is not a click.
    pub fn reveal_in_list(&mut self, id: SharedString, cx: &mut Context<Self>) {
        if std::mem::take(&mut self.graph_suppress_click) {
            return;
        }
        self.graph_reveal = Some(id);
        self.set_view(VaultView::List, cx);
    }

    /// The id a "reveal in list" staged (the list highlights it once). Consumed
    /// on read so the highlight is a one-shot.
    pub(super) fn take_graph_reveal(&mut self) -> Option<SharedString> {
        self.graph_reveal.take()
    }

    /// Show a list row's doc in the GRAPH ("show in graph" affordance §3): flip
    /// to the graph view. The field already carries every doc, so the node is
    /// present; a future slice can pan/select it.
    pub fn show_in_graph(&mut self, cx: &mut Context<Self>) {
        self.set_view(VaultView::Graph, cx);
    }
}
