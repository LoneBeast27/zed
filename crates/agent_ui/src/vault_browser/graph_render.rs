//! The GRAPH field renderer — draws the live [`VaultField`] (physics-relaxed
//! node positions) as absolutely-placed node chips over a `canvas()` edge
//! layer, mirroring the constellation's draw discipline (paint closures replay
//! a per-frame snapshot; the render path only reads sim outputs). Split from
//! `graph.rs` for the one-concern-per-file ceiling: this file is the field
//! view; `graph.rs` keeps edge resolution + the static fallback + the entry.
//!
//! Node identity is the doc id (ElementId per doc — the animation identity
//! law): drag re-pins, click opens the md in a center tab (deferred §11),
//! dblclick on a SESSION reveals it in the LIST view.

use std::collections::HashMap;

use gpui::{
    App, BoxShadow, ElementId, Hsla, MouseButton, PathBuilder, Pixels, Point, SharedString,
    WeakEntity, canvas, point, px,
};
use ui::prelude::*;

use crate::agent_accents::STATUS_DONE;

use super::field::VaultField;
use super::graph::{Edge, EdgeKind, node_color, node_size};
use super::index::{DocKind, VaultDoc};
use super::panel::VaultBrowserPanel;
use super::style::{HAIRLINE_HI, SURFACE_1};

/// A snapshot of one edge as screen segments, resolved from live field node
/// positions — recomputed each frame off the sim outputs.
struct EdgeSeg {
    a: Point<Pixels>,
    b: Point<Pixels>,
    color: Hsla,
    kind: EdgeKind,
}

/// Build the physics-field graph body: the edge canvas + the node chips over a
/// scroll surface sized to the field extent. `hovered` is the doc id whose
/// hover card is up (owned by the panel).
/// The graph's navigation frame (user ask 2026-07-10): `scale` = fit×zoom
/// applied to every rendered coordinate (zoom 1 bounds the WHOLE field by
/// the viewport); `zoomed` shows the Fit chip; `filter` dims non-matching
/// nodes (the galaxy backlog's filter↔graph sync).
pub(super) struct GraphNav {
    pub scale: f32,
    pub zoomed: bool,
    pub viewport: (f32, f32),
    pub filter: String,
}

pub(super) fn field_view(
    field: &VaultField,
    docs: &[&VaultDoc],
    edges: &[Edge],
    scroll: &gpui::ScrollHandle,
    hovered: Option<&str>,
    nav: GraphNav,
    panel: WeakEntity<VaultBrowserPanel>,
    cx: &App,
) -> gpui::AnyElement {
    let s = nav.scale.max(0.05);
    // Center the scaled field inside the viewport axis-wise when it is
    // smaller (fit mode leaves one axis short).
    let content_w = (field.width * s).max(nav.viewport.0.max(1.));
    let content_h = (field.height * s).max(nav.viewport.1.max(1.));
    let off = (
        (content_w - field.width * s) * 0.5,
        (content_h - field.height * s) * 0.5,
    );
    let filter = nav.filter.trim().to_lowercase();

    // doc id → live visual center (from the advanced field), view-space.
    let mut pos: HashMap<&str, (f32, f32)> = HashMap::with_capacity(field.nodes.len());
    for node in &field.nodes {
        pos.insert(
            node.id.as_str(),
            (node.out_x * s + off.0, node.out_y * s + off.1),
        );
    }

    // Resolve edges to live segments (docs[from]/docs[to] → their node pos).
    let segs: Vec<EdgeSeg> = edges
        .iter()
        .filter_map(|edge| {
            let from_id = docs.get(edge.from)?.id.as_str();
            let to_id = docs.get(edge.to)?.id.as_str();
            let (ax, ay) = *pos.get(from_id)?;
            let (bx, by) = *pos.get(to_id)?;
            Some(EdgeSeg {
                a: point(px(ax), px(ay)),
                b: point(px(bx), px(by)),
                color: edge_color(edge.kind),
                kind: edge.kind,
            })
        })
        .collect();

    let edge_canvas = canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            if !bounds.intersects(&window.content_mask().bounds) {
                return;
            }
            let origin = bounds.origin;
            for seg in &segs {
                paint_edge(window, origin, seg);
            }
        },
    )
    .absolute()
    .size_full();

    // Map doc id → its doc for the node chips + hover cards.
    let by_id: HashMap<&str, &VaultDoc> = docs.iter().map(|d| (d.id.as_str(), *d)).collect();
    // Label budget is PER CLUSTER, not global ("the dots look lost" — user,
    // 2026-07-10 round 2: the blunt global gate starved meaning). A roomy
    // cluster labels everything; a crowded one keeps its anchors (hub /
    // index / big / hovered) and the caption carries the group's meaning.
    let mut cluster_n: HashMap<&str, usize> = HashMap::new();
    for n in &field.nodes {
        *cluster_n.entry(n.project.as_str()).or_default() += 1;
    }
    // Labels need room to read: below ~0.7× the captions carry the meaning
    // and per-node labels rest (hover still reveals everything).
    let labels_visible = s >= 0.7;
    let nodes: Vec<gpui::AnyElement> = field
        .nodes
        .iter()
        .filter_map(|n| by_id.get(n.id.as_str()).map(|doc| (n, *doc)))
        .map(|(n, doc)| {
            let is_hovered = hovered == Some(n.id.as_str());
            let size = (node_size(doc) * s).max(4.);
            let roomy = cluster_n
                .get(n.project.as_str())
                .is_none_or(|&count| count <= LABEL_DENSE_N);
            let labeled = is_hovered
                || (labels_visible
                    && (roomy
                        || matches!(doc.kind, DocKind::Project | DocKind::Index)
                        || size >= LABEL_MIN_SIZE));
            // Filter↔graph sync (galaxy backlog): non-matches DIM, never
            // vanish — the layout stays stable while the eye finds matches.
            let dimmed = !filter.is_empty() && !doc.search_key.contains(&filter);
            let (vx, vy) = pos.get(n.id.as_str()).copied().unwrap_or((0., 0.));
            node_chip(
                doc,
                vx,
                vy,
                size,
                is_hovered,
                labeled,
                dimmed,
                panel.clone(),
                cx,
            )
        })
        .collect();

    // Cluster captions: the project name floats above each cluster's top so
    // every dot has an immediate group meaning even unlabeled. Clicking one
    // ZOOMS INTO that cluster (the navigation handle).
    let captions: Vec<gpui::AnyElement> = field
        .clusters
        .iter()
        .map(|(project, cluster)| {
            let top = field
                .nodes
                .iter()
                .filter(|n| &n.project == project)
                .map(|n| (n.out_y - n.r) * s + off.1)
                .fold(cluster.cy * s + off.1, f32::min);
            let name: SharedString = if project.is_empty() {
                "(unassigned)".into()
            } else {
                project.clone().into()
            };
            let any_match = filter.is_empty()
                || field.nodes.iter().any(|n| {
                    &n.project == project
                        && by_id
                            .get(n.id.as_str())
                            .is_some_and(|d| d.search_key.contains(&filter))
                });
            let caption_panel = panel.clone();
            let caption_project = project.clone();
            div()
                .id(ElementId::Name(format!("cluster-cap-{project}").into()))
                .absolute()
                .left(px(cluster.cx * s + off.0 - 80.))
                .top(px(top - 34.))
                .w(px(160.))
                .text_size(px(11.))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(cx.theme().colors().text_muted)
                .text_center()
                .when(!any_match, |el| el.opacity(0.25))
                .cursor_pointer()
                .tooltip(ui::Tooltip::text("Zoom into this cluster"))
                .on_click(move |_, _, cx| {
                    caption_panel
                        .update(cx, |panel, cx| {
                            panel.focus_graph_cluster(&caption_project, cx)
                        })
                        .ok();
                })
                .child(name)
                .into_any_element()
        })
        .collect();

    // Ctrl+wheel = zoom (plain wheel stays the scroll surface's pan).
    let wheel_panel = panel.clone();
    let pan_panel = panel.clone();
    // "Fit" chip — back to the whole-field view whenever zoomed in.
    let fit_chip = nav.zoomed.then(|| {
        let colors = cx.theme().colors();
        let fit_panel = panel.clone();
        div()
            .id("graph-fit")
            .absolute()
            .top(px(10.))
            .right(px(14.))
            .px(px(10.))
            .py(px(4.))
            .rounded(px(8.))
            .bg(SURFACE_1)
            .border_1()
            .border_color(colors.border)
            .text_size(px(11.))
            .text_color(colors.text_muted)
            .cursor_pointer()
            .hover(|el| el.bg(colors.element_hover))
            .on_click(move |_, _, cx| {
                fit_panel
                    .update(cx, |panel, cx| panel.reset_graph_zoom(cx))
                    .ok();
            })
            .child("Fit")
    });

    div()
        .relative()
        .size_full()
        .child(
            div()
                .id("vault-field-scroll")
                .size_full()
                .overflow_scroll()
                .track_scroll(scroll)
                // PLAIN wheel zooms (Q9 ruling 2026-07-11 — the Obsidian
                // grammar; ctrl+wheel still lands here too). Panning when
                // zoomed = background drag (`begin_graph_pan` below).
                .on_scroll_wheel(move |event, _, cx| {
                    let dy = match event.delta {
                        gpui::ScrollDelta::Lines(delta) => delta.y,
                        gpui::ScrollDelta::Pixels(delta) => delta.y.as_f32() / 40.,
                    };
                    if dy != 0. {
                        let factor = if dy > 0. { 1.15 } else { 1. / 1.15 };
                        wheel_panel
                            .update(cx, |panel, cx| panel.zoom_graph(factor, cx))
                            .ok();
                        cx.stop_propagation();
                    }
                })
                .on_mouse_down(gpui::MouseButton::Left, move |event, _, cx| {
                    // Background pan-begin — a node hit already set its drag
                    // (child listeners fire first), so this no-ops there.
                    pan_panel
                        .update(cx, |panel, cx| panel.begin_graph_pan(event.position, cx))
                        .ok();
                })
                .child(
                    div()
                        .relative()
                        .w(px(content_w))
                        .h(px(content_h))
                        .child(edge_canvas)
                        .children(captions)
                        .children(nodes),
                ),
        )
        .children(fit_chip)
        .into_any_element()
}

/// Edge color by kind: links on the hairline, supersedes in the done-blue,
/// structural spine dimmed well under both, project hub spokes faintest of
/// all (connective tissue, never competing with semantic links).
fn edge_color(kind: EdgeKind) -> Hsla {
    match kind {
        EdgeKind::Supersedes => Hsla::from(STATUS_DONE).opacity(0.5),
        EdgeKind::Link => HAIRLINE_HI.into(),
        EdgeKind::Structural => Hsla::from(HAIRLINE_HI).opacity(0.35),
        EdgeKind::Project => Hsla::from(HAIRLINE_HI).opacity(0.22),
    }
}

/// Paint one edge: solid for links, dashed + an arrowhead for a directional
/// supersedes correction, a faint thin line for the structural spine and the
/// fainter project spokes.
fn paint_edge(window: &mut Window, origin: Point<Pixels>, seg: &EdgeSeg) {
    let a = point(origin.x + seg.a.x, origin.y + seg.a.y);
    let b = point(origin.x + seg.b.x, origin.y + seg.b.y);
    match seg.kind {
        EdgeKind::Supersedes => {
            paint_dashed(window, a, b, seg.color);
            paint_arrowhead(window, a, b, seg.color);
        }
        EdgeKind::Link => paint_line(window, a, b, seg.color, 1.),
        EdgeKind::Structural => paint_line(window, a, b, seg.color, 0.75),
        EdgeKind::Project => paint_line(window, a, b, seg.color, 0.75),
    }
}

/// A solid stroked line.
fn paint_line(window: &mut Window, a: Point<Pixels>, b: Point<Pixels>, color: Hsla, w: f32) {
    let mut builder = PathBuilder::stroke(px(w));
    builder.move_to(a);
    builder.line_to(b);
    if let Ok(path) = builder.build() {
        window.paint_path(path, color);
    }
}

/// A dashed line (supersedes) — fixed-length dashes with gaps along the segment.
fn paint_dashed(window: &mut Window, a: Point<Pixels>, b: Point<Pixels>, color: Hsla) {
    const DASH: f32 = 6.;
    const GAP: f32 = 4.;
    let (ax, ay) = (a.x.as_f32(), a.y.as_f32());
    let (bx, by) = (b.x.as_f32(), b.y.as_f32());
    let len = (bx - ax).hypot(by - ay).max(0.001);
    let (ux, uy) = ((bx - ax) / len, (by - ay) / len);
    let step = DASH + GAP;
    let mut t = 0.;
    while t < len {
        let s = t;
        let e = (t + DASH).min(len);
        let mut builder = PathBuilder::stroke(px(1.));
        builder.move_to(point(px(ax + ux * s), px(ay + uy * s)));
        builder.line_to(point(px(ax + ux * e), px(ay + uy * e)));
        if let Ok(path) = builder.build() {
            window.paint_path(path, color);
        }
        t += step;
    }
}

/// A small arrowhead at `b` pointing FROM `a` (supersedes is directional: the
/// newer doc points at the doc it corrects).
fn paint_arrowhead(window: &mut Window, a: Point<Pixels>, b: Point<Pixels>, color: Hsla) {
    const LEN: f32 = 8.;
    const SPREAD: f32 = 0.5; // radians off the shaft
    let (ax, ay) = (a.x.as_f32(), a.y.as_f32());
    let (bx, by) = (b.x.as_f32(), b.y.as_f32());
    let ang = (by - ay).atan2(bx - ax);
    // Pull the tip back off the node rim a touch so it doesn't bury in the dot.
    let tip = point(px(bx), px(by));
    for side in [-SPREAD, SPREAD] {
        let wa = ang + std::f32::consts::PI + side;
        let mut builder = PathBuilder::stroke(px(1.));
        builder.move_to(tip);
        builder.line_to(point(px(bx + LEN * wa.cos()), px(by + LEN * wa.sin())));
        if let Ok(path) = builder.build() {
            window.paint_path(path, color);
        }
    }
}

/// Past this node count resting labels thin out to the anchors of meaning
/// (project hubs, index, big nodes) — hover reveals everything else.
const LABEL_DENSE_N: usize = 25;
/// A node at/above this dot size keeps its resting label even when dense.
const LABEL_MIN_SIZE: f32 = 18.;

/// One field node: the type/vendor-colored dot (mass-sized, bloom shadow) + a
/// truncated title (density-gated — see `LABEL_DENSE_N`), absolutely placed
/// at its live center. Mouse-down begins a drag; click opens the md; dblclick
/// on a session reveals it in the list; hover raises the card.
fn node_chip(
    doc: &VaultDoc,
    out_x: f32,
    out_y: f32,
    size: f32,
    is_hovered: bool,
    labeled: bool,
    dimmed: bool,
    panel: WeakEntity<VaultBrowserPanel>,
    cx: &App,
) -> gpui::AnyElement {
    let colors = cx.theme().colors();
    let color = node_color(doc);
    let id: SharedString = doc.id.clone().into();
    let abs_path = doc.abs_path.clone();
    let is_session = doc.kind == DocKind::Session;
    let title: SharedString = doc.title.chars().take(28).collect::<String>().into();

    let down_panel = panel.clone();
    let down_id = id.clone();
    let click_panel = panel.clone();
    let click_id = id.clone();
    let click_path = abs_path;
    let hover_panel = panel;
    let hover_id = id.clone();

    let mut chip = h_flex()
        .id(ElementId::Name(format!("vnode-{id}").into()))
        .absolute()
        .left(px(out_x - size * 0.5))
        .top(px(out_y - size * 0.5))
        // Filter dim (galaxy backlog: dim non-matches, never remove).
        .when(dimmed, |el| el.opacity(0.15))
        .items_center()
        .gap(px(9.))
        .cursor_pointer()
        .on_mouse_down(
            MouseButton::Left,
            move |event, _, cx| {
                down_panel
                    .update(cx, |panel, cx| {
                        panel.begin_graph_drag(down_id.clone(), event.position, cx)
                    })
                    .ok();
            },
        )
        .on_click(move |event, window, cx| {
            let (path, id) = (click_path.clone(), click_id.clone());
            if event.click_count() >= 2 && is_session {
                // Double-click a session → reveal it in the LIST view.
                click_panel
                    .update(cx, |panel, cx| panel.reveal_in_list(id, cx))
                    .ok();
            } else {
                click_panel
                    .update(cx, |panel, cx| panel.request_open(path, window, cx))
                    .ok();
            }
        })
        .on_hover(move |hovered, _, cx| {
            let hovered = *hovered;
            hover_panel
                .update(cx, |panel, cx| {
                    panel.set_graph_hover(hover_id.clone(), hovered, cx)
                })
                .ok();
        })
        .child(
            div()
                .size(px(size))
                .rounded_full()
                .bg(color)
                .shadow(vec![BoxShadow {
                    color: color.opacity(0.25),
                    offset: point(px(0.), px(0.)),
                    blur_radius: px(12.),
                    spread_radius: px(0.),
                }]),
        );
    if labeled {
        chip = chip.child(
            div()
                .max_w(px(160.))
                .text_size(px(12.))
                .text_color(colors.text_muted)
                .truncate()
                .child(title),
        );
    }

    if is_hovered {
        chip = chip.child(hover_card(doc, size, cx));
    }
    chip.into_any_element()
}

/// The hover card — title · type · project · updated, plus a redactions line
/// for sessions. Reuses the browser's chip vocabulary (one visual source).
fn hover_card(doc: &VaultDoc, dot_size: f32, cx: &App) -> gpui::AnyElement {
    use super::style::{redactions_badge, type_chip, updated_age, vendor_chip};
    let colors = cx.theme().colors();
    let title: SharedString = doc.title.clone().into();
    let age = updated_age(&doc.updated);

    let mut meta = h_flex().items_center().gap(px(6.)).flex_wrap();
    meta = meta.children(type_chip(&doc.doc_type, cx));
    meta = meta.children(vendor_chip(&doc.vendor, cx));
    if doc.kind == DocKind::Session {
        meta = meta.children(redactions_badge(doc.redactions, cx));
    }

    let mut card = div()
        .absolute()
        .left(px(dot_size + 16.))
        .top(px(-6.))
        .w(px(220.))
        .p(px(11.))
        .rounded(px(12.))
        .bg(super::style::SURFACE_1)
        .border_1()
        .border_color(HAIRLINE_HI)
        .shadow(vec![BoxShadow {
            color: gpui::hsla(0., 0., 0., 0.55),
            offset: point(px(0.), px(12.)),
            blur_radius: px(32.),
            spread_radius: px(0.),
        }])
        .flex()
        .flex_col()
        .gap(px(7.))
        .child(
            div()
                .text_size(px(13.))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(colors.text)
                .child(title),
        )
        .child(meta);

    if !doc.project.is_empty() {
        card = card.child(
            div()
                .text_size(px(11.))
                .text_color(colors.text_placeholder)
                .child(SharedString::from(format!("project · {}", doc.project))),
        );
    }
    if !age.is_empty() {
        card = card.child(
            div()
                .text_size(px(11.))
                .text_color(colors.text_placeholder)
                .child(SharedString::from(format!("updated {age}"))),
        );
    }
    if doc.link_scan_truncated {
        card = card.child(
            div()
                .text_size(px(11.))
                .text_color(colors.text_placeholder)
                .child("links partial (large body)"),
        );
    }
    card.into_any_element()
}
