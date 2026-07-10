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
        let Some(drag) = &mut self.graph_drag else {
            return;
        };
        let dx = (position.x - drag.start_mouse.x).as_f32();
        let dy = (position.y - drag.start_mouse.y).as_f32();
        if dx.abs() + dy.abs() > 4. {
            drag.moved = true;
        }
        let id = drag.id.clone();
        let (ax, ay) = (drag.start_anchor.0 + dx, drag.start_anchor.1 + dy);
        self.graph_field.drag_to(&id, ax, ay);
        cx.notify();
    }

    pub(super) fn graph_drag_ended(&mut self, cx: &mut Context<Self>) {
        if let Some(drag) = self.graph_drag.take() {
            self.graph_suppress_click = drag.moved;
            self.graph_field.end_drag(self.field_now());
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
