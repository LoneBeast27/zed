//! The spawn-tree graph (PARITY_SPEC §4.2 graph view) — 1:1 port of
//! `graph.js convTree`: per-conversation root node + cascading subagent
//! nodes on the EXACT deterministic layout (no layout crate — the approved
//! design is `y = Y0 + i*STEP`, COMPLEMENTARY_SOLUTIONS verdict 3).
//!
//! Motion (PARITY_SPEC §4.9 / §8.7): fresh edges draw in over 700ms on the
//! decel curve via partial-path paint (the in-tree dash machinery — same
//! `split_range` technique as gpui's own `dash_array`); fresh nodes spawn
//! Obsidian-style — born AT the parent root, translating to their slot on
//! the overshooting `--spatial` curve (evaluated exactly, mapped inside the
//! animator per the animation.rs:165 assert guard), staggered 0.15+0.07i s.
//! Status glow is a real `BoxShadow` (RUST_PORT_NOTES §1 native upgrade).

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use gpui::{
    Animation, AnimationExt as _, AnyElement, App, ElementId, Hsla, Path, PathBuilder, Pixels,
    Point, Rgba, ScrollHandle, SharedString, WeakEntity, canvas, point, px,
};
use ui::prelude::*;

use crate::agent_accents::STATUS_RUNNING;
use crate::bridge::RunRow;

use super::motion::{DECEL, SPATIAL};
use super::node::{node_label, root_node, status_dot};
use super::paint_cache::SharedPaintCache;
use super::panel::TaskBoardPanel;
use super::style::{HAIRLINE_HI, empty_state, status_phrase};

// ── Layout constants — graph.js `convTree`, EXACT ──
pub(super) const ROOTY: f32 = 24.;
pub(super) const X0: f32 = 30.;
const XC: f32 = 64.;
const Y0: f32 = 70.;
const STEP: f32 = 56.;

/// Edge draw-in duration (web `draw-edge .7s`).
const EDGE_DRAW: Duration = Duration::from_millis(700);
/// Node spawn duration (web `node-spawn .5s`).
const NODE_SPAWN: Duration = Duration::from_millis(500);
/// Margin added to each run's spawn-choreography deadline, absorbing render
/// latency between board arrival and the animation's first frame.
const FRESH_MARGIN: Duration = Duration::from_millis(500);

/// First-sight record for a graph run: when it appeared, and how long its
/// spawn choreography needs to complete (per-run — sibling index 34+ used
/// to outlive the old flat 3s window and snap in from nothing).
#[derive(Debug, Clone, Copy)]
pub struct GraphSeen {
    first_seen: Instant,
    fresh_for: Duration,
}

/// How long run `i`'s choreography runs: max(edge draw, stagger
/// `0.15 + 0.07·i` + spawn) + margin. `i` is known at insertion.
fn fresh_duration(i: usize) -> Duration {
    let spawn_end = 0.15 + 0.07 * i as f32 + NODE_SPAWN.as_secs_f32();
    Duration::from_secs_f32(spawn_end.max(EDGE_DRAW.as_secs_f32()) + FRESH_MARGIN.as_secs_f32())
}
/// Vertical slack around the scroll viewport before a conv tree is culled —
/// trees partially entering the view are always fully built.
const CULL_MARGIN: f32 = 200.;
/// Graph content top padding (kept in a const because tree culling computes
/// each tree's offset from it).
const CONTENT_PT: f32 = 12.;
/// Conv-tree bottom margin (`.gconv { margin-bottom: 28px }`).
const TREE_MB: f32 = 28.;

/// Whether a conv tree's vertical span `[tree_top, tree_top + height]` (in
/// content coordinates) intersects the scroll viewport plus margin.
fn tree_in_viewport(tree_top: f32, height: f32, scroll_top: f32, viewport_height: f32) -> bool {
    tree_top + height >= scroll_top - CULL_MARGIN
        && tree_top <= scroll_top + viewport_height + CULL_MARGIN
}

/// Builds the graph view. `seen` is the panel-owned `seenRuns` equivalent
/// (run id → first sight); it is pruned to live runs. Render cost is bounded
/// by what's visible (RUST_PORT_NOTES §8): conv trees fully outside the
/// scroll viewport (tracked via `scroll`) build a same-height placeholder —
/// no nodes, no edges, no paths — and every canvas paint closure early-outs
/// when its bounds don't intersect the window's content mask.
pub(super) fn graph_view(
    board: Vec<RunRow>,
    seen: &mut HashMap<SharedString, GraphSeen>,
    scroll: &ScrollHandle,
    cache: &SharedPaintCache,
    panel: WeakEntity<TaskBoardPanel>,
    cx: &mut App,
) -> AnyElement {
    if board.is_empty() {
        cache.borrow_mut().retain_convs(|_| false);
        return empty_state(
            IconName::ListTree,
            "No spawn tree yet",
            "When the orchestrator delegates, subagents grow under their conversation root here.",
            cx,
        )
        .into_any_element();
    }

    let now = Instant::now();
    let live: HashSet<&str> = board.iter().map(|run| run.run_id.as_str()).collect();
    seen.retain(|run_id, _| live.contains(run_id.as_ref()));

    // Group by conversation, preserving board order (web object-key order).
    let mut conv_order: Vec<String> = Vec::new();
    let mut by_conv: HashMap<String, Vec<RunRow>> = HashMap::new();
    for run in board {
        if !by_conv.contains_key(&run.conv) {
            conv_order.push(run.conv.clone());
        }
        by_conv.entry(run.conv.clone()).or_default().push(run);
    }
    cache
        .borrow_mut()
        .retain_convs(|conv| by_conv.contains_key(conv));

    // Viewport in content coordinates (offset.y goes negative as the user
    // scrolls down). Height 0 = unmeasured first frame → build everything.
    let viewport_height = scroll.bounds().size.height.as_f32();
    let scroll_top = -scroll.offset().y.as_f32();

    let mut tree_top = CONTENT_PT;
    let trees: Vec<AnyElement> = conv_order
        .into_iter()
        .map(|conv_id| {
            let runs = by_conv.remove(&conv_id).unwrap_or_default();
            let height = Y0 + runs.len() as f32 * STEP + 6.;
            // Stamp first sight at board arrival even when culled, so a
            // tree scrolled into view later doesn't replay settled spawns
            // (the web's seenRuns adds during every render pass). The
            // per-run deadline freezes at insertion, from the sibling index
            // that determines its stagger.
            let fresh: Vec<bool> = runs
                .iter()
                .enumerate()
                .map(|(i, run)| {
                    let entry = seen
                        .entry(SharedString::from(run.run_id.clone()))
                        .or_insert_with(|| GraphSeen {
                            first_seen: now,
                            fresh_for: fresh_duration(i),
                        });
                    now.duration_since(entry.first_seen) < entry.fresh_for
                })
                .collect();
            let visible = viewport_height <= 0.
                || tree_in_viewport(tree_top, height, scroll_top, viewport_height);
            tree_top += height + TREE_MB;
            if visible {
                conv_tree(conv_id, runs, fresh, cache.clone(), panel.clone(), cx)
            } else {
                // Same-height placeholder preserves scroll geometry.
                div().w_full().h(px(height)).mb(px(TREE_MB)).into_any_element()
            }
        })
        .collect();

    div()
        .id("task-board-graph")
        .size_full()
        .overflow_y_scroll()
        .track_scroll(scroll)
        .px(px(16.))
        .pt(px(CONTENT_PT))
        .pb(px(28.))
        .children(trees)
        .into_any_element()
}

/// One `.gconv` block: edge canvas + root node + cascading run nodes.
fn conv_tree(
    conv_id: String,
    runs: Vec<RunRow>,
    fresh: Vec<bool>,
    cache: SharedPaintCache,
    panel: WeakEntity<TaskBoardPanel>,
    cx: &App,
) -> AnyElement {
    let height = Y0 + runs.len() as f32 * STEP + 6.;

    // Settled edges paint together in one static canvas; fresh edges get
    // their own draw-in animation element each.
    let mut settled_edges: Vec<(f32, Hsla)> = Vec::new();
    let mut drawing_edges: Vec<AnyElement> = Vec::new();
    for (i, run) in runs.iter().enumerate() {
        let y = Y0 + i as f32 * STEP;
        let color = edge_color(&run.status);
        if fresh[i] {
            drawing_edges.push(drawing_edge(&run.run_id, y, color));
        } else {
            settled_edges.push((y, color));
        }
    }
    let edge_cache = cache.clone();
    let edge_conv = conv_id.clone();
    let settled_canvas = canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            // Offscreen culling: skip path building + paint entirely when
            // the tree is outside the clip (RUST_PORT_NOTES §8).
            if !bounds.intersects(&window.content_mask().bounds) {
                return;
            }
            // Static geometry: tessellate once per (origin, edge set), not
            // per frame (the spinner keeps the frame clock hot).
            let paths = edge_cache.borrow_mut().settled_edge_paths(
                &edge_conv,
                bounds.origin,
                &settled_edges,
                || {
                    settled_edges
                        .iter()
                        .filter_map(|(y, color)| {
                            edge_path(bounds.origin, *y, 1.0).map(|path| (path, *color))
                        })
                        .collect()
                },
            );
            for (path, color) in paths {
                window.paint_path(path, color);
            }
        },
    )
    .absolute()
    .size_full();

    let nodes: Vec<AnyElement> = runs
        .iter()
        .enumerate()
        .map(|(i, run)| graph_node(run, i, fresh[i], panel.clone(), cx))
        .collect();

    div()
        .relative()
        .w_full()
        .h(px(height))
        .mb(px(TREE_MB))
        .child(settled_canvas)
        .children(drawing_edges)
        .child(root_node(conv_id, cache, panel, cx))
        .children(nodes)
        .into_any_element()
}

/// `.gedge.running` is the running tint at 45%, others the high hairline.
fn edge_color(status: &str) -> Hsla {
    if status == "running" {
        Hsla::from(Rgba::from(STATUS_RUNNING)).opacity(0.45)
    } else {
        HAIRLINE_HI.into()
    }
}

/// The edge path, EXACT SVG `d` math from graph.js:
/// `M X0 ROOTY+16 L X0 y-14 C X0 y-3, X0+9 y, XC-12 y`.
/// `prefix` < 1 paints only the leading fraction (dash split — gpui's own
/// partial-path machinery).
fn edge_path(origin: Point<Pixels>, y: f32, prefix: f32) -> Option<Path<Pixels>> {
    if prefix <= 0.001 {
        return None;
    }
    let at = |x: f32, dy: f32| point(origin.x + px(x), origin.y + px(dy));
    let mut builder = PathBuilder::stroke(px(1.5));
    builder.move_to(at(X0, ROOTY + 16.));
    builder.line_to(at(X0, y - 14.));
    builder.cubic_bezier_to(at(XC - 12., y), at(X0, y - 3.), at(X0 + 9., y));
    if prefix < 1.0 {
        // Spine length + elbow chord-ish length (~28px) as the path length;
        // a slight overestimate just completes the draw marginally early.
        let length = (y - 14. - (ROOTY + 16.)).max(0.) + 28.;
        builder = builder.dash_array(&[px((length * prefix).max(0.01)), px(1_000_000.)]);
    }
    builder.build().ok()
}

/// A fresh edge: one-shot 700ms decel draw-in, painting the path prefix each
/// frame. The animator rebuilds the canvas with the eased `t` (the wrapper
/// div carries the element identity, so the draw fires exactly once per run).
fn drawing_edge(run_id: &str, y: f32, color: Hsla) -> AnyElement {
    div()
        .absolute()
        .size_full()
        .with_animation(
            ElementId::Name(format!("gedge-{run_id}").into()),
            Animation::new(EDGE_DRAW).with_easing(DECEL.easing()),
            move |wrapper, t| {
                wrapper.child(
                    canvas(
                        |_, _, _| (),
                        move |bounds, _, window, _| {
                            if !bounds.intersects(&window.content_mask().bounds) {
                                return;
                            }
                            if let Some(path) = edge_path(bounds.origin, y, t) {
                                window.paint_path(path, color);
                            }
                        },
                    )
                    .size_full(),
                )
            },
        )
        .into_any_element()
}

/// One `.gnode`: dot + (task name, progress note) label, absolutely placed
/// at its slot. Fresh nodes run the Obsidian spawn: hold 0.15+0.07i s, then
/// translate root→slot on the exact `--spatial` overshoot over 500ms.
fn graph_node(
    run: &RunRow,
    i: usize,
    fresh: bool,
    panel: WeakEntity<TaskBoardPanel>,
    cx: &App,
) -> AnyElement {
    let y = Y0 + i as f32 * STEP;
    let final_left = XC - 9.;
    let final_top = y - 10.;
    // Born at the parent root dot (web `--sx/--sy` math).
    let start_left = X0 - 11.;
    let start_top = ROOTY - 12.;

    let run_id = SharedString::from(run.run_id.clone());
    let name: String = run
        .task
        .clone()
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| {
            if run.agent.is_empty() {
                "task".to_string()
            } else {
                run.agent.clone()
            }
        })
        .chars()
        .take(34)
        .collect();
    let phrase = status_phrase(&run.status, run.elapsed_s);

    let click_panel = panel;
    let click_run_id = run_id.clone();
    let node = h_flex()
        .id(ElementId::Name(format!("gnode-{run_id}").into()))
        .absolute()
        .left(px(final_left))
        .top(px(final_top))
        .items_center()
        .gap(px(9.))
        .cursor_pointer()
        .on_click(move |_, _, cx| {
            click_panel
                .update(cx, |panel, cx| panel.open_run(click_run_id.clone(), cx))
                .ok();
        })
        .child(status_dot(&run.run_id, &run.status))
        .child(node_label(name.into(), phrase.into(), cx));

    if !fresh {
        return node.into_any_element();
    }
    let delay = 0.15 + i as f32 * 0.07;
    node.with_animations(
        ElementId::Name(format!("gspawn-{run_id}").into()),
        vec![
            // Stagger hold (web `animation-delay`), then the spring-out.
            Animation::new(Duration::from_secs_f32(delay)),
            Animation::new(NODE_SPAWN),
        ],
        move |node, animation_ix, delta| {
            // Overshoot mapped INSIDE the animator (animation.rs:165 guard).
            let s = if animation_ix == 0 {
                0.0
            } else {
                SPATIAL.eval(delta)
            };
            node.left(px(start_left + (final_left - start_left) * s))
                .top(px(start_top + (final_top - start_top) * s))
                .opacity(s.clamp(0.0, 1.0))
        },
    )
    .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The layout constants are the approved design — lock them.
    #[test]
    fn layout_constants_match_graph_js() {
        assert_eq!((ROOTY, X0, XC, Y0, STEP), (24., 30., 64., 70., 56.));
    }

    #[test]
    fn conv_height_matches_graph_js() {
        // H = Y0 + runs.len()*STEP + 6
        for n in [1usize, 3, 8] {
            assert_eq!(Y0 + n as f32 * STEP + 6., 70. + n as f32 * 56. + 6.);
        }
    }

    #[test]
    fn edge_path_builds_full_and_prefix() {
        let origin = point(px(0.), px(0.));
        assert!(edge_path(origin, Y0, 1.0).is_some());
        assert!(edge_path(origin, Y0 + 3. * STEP, 0.5).is_some());
        // Zero prefix paints nothing (guards the dash split_range edge case).
        assert!(edge_path(origin, Y0, 0.0).is_none());
    }

    #[test]
    fn fresh_duration_covers_every_sibling_index() {
        // i ≥ 34 outlived the old flat 3s window (0.15 + 34·0.07 + 0.5 =
        // 3.03s); the per-run deadline must clear the full choreography.
        for i in [0usize, 10, 33, 34, 41, 80] {
            let spawn_end = 0.15 + 0.07 * i as f32 + 0.5;
            let window = fresh_duration(i).as_secs_f32();
            assert!(
                window > spawn_end,
                "i={i}: window {window}s ends before spawn at {spawn_end}s"
            );
        }
        // Small indices still cover the 700ms edge draw-in.
        assert!(fresh_duration(0).as_secs_f32() > 0.7);
    }

    #[test]
    fn tree_culling_keeps_visible_and_margin_trees() {
        // Viewport: scroll_top 1000, height 600 → visible [1000, 1600],
        // with the 200px margin: [800, 1800].
        let (top, height) = (1000., 600.);
        assert!(tree_in_viewport(1200., 300., top, height), "fully inside");
        assert!(tree_in_viewport(700., 200., top, height), "straddles top edge + margin");
        assert!(tree_in_viewport(1750., 400., top, height), "enters bottom margin");
        assert!(!tree_in_viewport(0., 500., top, height), "far above → culled");
        assert!(!tree_in_viewport(2000., 300., top, height), "far below → culled");
        // Unscrolled viewport keeps the first trees.
        assert!(tree_in_viewport(12., 126., 0., 600.));
    }

    #[test]
    fn spawn_offsets_match_web_sx_sy() {
        // graph.js: sx = (X0-11)-(XC-9), sy = (ROOTY-12)-(y-10)
        let i = 2.;
        let y = Y0 + i * STEP;
        let sx = (X0 - 11.) - (XC - 9.);
        let sy = (ROOTY - 12.) - (y - 10.);
        // Our start/final formulation is the same vector.
        let (start_left, start_top) = (X0 - 11., ROOTY - 12.);
        let (final_left, final_top) = (XC - 9., y - 10.);
        assert_eq!(start_left - final_left, sx);
        assert_eq!(start_top - final_top, sy);
    }
}
