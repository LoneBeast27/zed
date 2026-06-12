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
    Animation, AnimationExt as _, AnyElement, App, BoxShadow, ElementId, FontWeight, Hsla, Path,
    PathBuilder, Pixels, Point, Rgba, SharedString, WeakEntity, canvas, point, px,
};
use settings::Settings as _;
use theme_settings::ThemeSettings;
use ui::prelude::*;

use crate::agent_accents::{ACCENT_AGY, ACCENT_GEMINI, STATUS_RUNNING, color_for_status};
use crate::bridge::RunRow;

use super::motion::{DECEL, SPATIAL};
use super::panel::TaskBoardPanel;
use super::style::{HAIRLINE_HI, SURFACE_2, empty_state, status_phrase};

// ── Layout constants — graph.js `convTree`, EXACT ──
const ROOTY: f32 = 24.;
const X0: f32 = 30.;
const XC: f32 = 64.;
const Y0: f32 = 70.;
const STEP: f32 = 56.;

/// Edge draw-in duration (web `draw-edge .7s`).
const EDGE_DRAW: Duration = Duration::from_millis(700);
/// Node spawn duration (web `node-spawn .5s`).
const NODE_SPAWN: Duration = Duration::from_millis(500);
/// How long a run counts as fresh after first sight on the graph: covers the
/// longest stagger + spawn + edge draw with margin, so mid-animation renders
/// keep their animated wrappers (the web's `seenRuns` keeps the DOM node).
const FRESH_WINDOW: Duration = Duration::from_secs(3);

/// Builds the graph view. `seen` is the panel-owned `seenRuns` equivalent
/// (run id → first time it rendered on the graph); it is pruned to live runs
/// and only ever updated when the graph actually renders, like the web.
pub fn graph_view(
    board: Vec<RunRow>,
    seen: &mut HashMap<SharedString, Instant>,
    panel: WeakEntity<TaskBoardPanel>,
    cx: &mut App,
) -> AnyElement {
    if board.is_empty() {
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

    let trees: Vec<AnyElement> = conv_order
        .into_iter()
        .map(|conv_id| {
            let runs = by_conv.remove(&conv_id).unwrap_or_default();
            conv_tree(conv_id, runs, seen, now, panel.clone(), cx)
        })
        .collect();

    div()
        .id("task-board-graph")
        .size_full()
        .overflow_y_scroll()
        .px(px(16.))
        .pt(px(12.))
        .pb(px(28.))
        .children(trees)
        .into_any_element()
}

/// One `.gconv` block: edge canvas + root node + cascading run nodes.
fn conv_tree(
    conv_id: String,
    runs: Vec<RunRow>,
    seen: &mut HashMap<SharedString, Instant>,
    now: Instant,
    panel: WeakEntity<TaskBoardPanel>,
    cx: &App,
) -> AnyElement {
    let height = Y0 + runs.len() as f32 * STEP + 6.;

    let fresh: Vec<bool> = runs
        .iter()
        .map(|run| {
            let first_seen = *seen
                .entry(SharedString::from(run.run_id.clone()))
                .or_insert(now);
            now.duration_since(first_seen) < FRESH_WINDOW
        })
        .collect();

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
    let settled_canvas = canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            for (y, color) in &settled_edges {
                if let Some(path) = edge_path(bounds.origin, *y, 1.0) {
                    window.paint_path(path, *color);
                }
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
        .mb(px(28.))
        .child(settled_canvas)
        .children(drawing_edges)
        .child(root_node(conv_id, panel.clone(), cx))
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

    let click_panel = panel.clone();
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
        .child(node_label(name.into(), phrase.into(), false, cx));

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

/// The `.gdot`: 16px circle, surface fill, high hairline border, status glow
/// as a real BoxShadow; completed/failed/killed = filled center dot, running
/// = open-circle spinner arc rotating on the rim.
fn status_dot(run_id: &str, status: &str) -> AnyElement {
    let color = color_for_status(status);
    let glowing = matches!(status, "running" | "completed" | "failed" | "killed");
    let dot = div()
        .relative()
        .flex_none()
        .size(px(16.))
        .rounded_full()
        .bg(SURFACE_2)
        .border_1()
        .border_color(HAIRLINE_HI)
        .when(glowing, |dot| {
            // Web: blurred pseudo-element, inset -6px, blur 20, opacity .25.
            dot.shadow(vec![BoxShadow {
                color: color.opacity(0.25),
                offset: point(px(0.), px(0.)),
                blur_radius: px(20.),
                spread_radius: px(6.),
            }])
        });
    match status {
        "completed" | "failed" | "killed" => dot
            .child(
                div()
                    .absolute()
                    .inset(px(4.))
                    .rounded_full()
                    .bg(color),
            )
            .into_any_element(),
        "running" => dot.child(spinner_arc(run_id, color)).into_any_element(),
        _ => dot.into_any_element(),
    }
}

/// The open-circle spinner: a quarter-ring arc orbiting the dot rim at
/// 0.9s/rev (web `gspin` border-top trick, painted as a real arc).
fn spinner_arc(run_id: &str, color: Hsla) -> AnyElement {
    div()
        .absolute()
        .inset(px(-1.))
        .with_animation(
            ElementId::Name(format!("gspin-{run_id}").into()),
            Animation::new(Duration::from_millis(900)).repeat(),
            move |wrapper, t| {
                let start_angle = t * std::f32::consts::TAU;
                wrapper.child(
                    canvas(
                        |_, _, _| (),
                        move |bounds, _, window, _| {
                            let center = bounds.center();
                            let radius = (bounds.size.width.as_f32() / 2.0) - 0.75;
                            let mut builder = PathBuilder::stroke(px(1.5));
                            const SEGMENTS: usize = 14;
                            for k in 0..=SEGMENTS {
                                let a = start_angle
                                    + (k as f32 / SEGMENTS as f32)
                                        * (std::f32::consts::TAU / 4.0);
                                let p = point(
                                    center.x + px(radius * a.cos()),
                                    center.y + px(radius * a.sin()),
                                );
                                if k == 0 {
                                    builder.move_to(p);
                                } else {
                                    builder.line_to(p);
                                }
                            }
                            if let Ok(path) = builder.build() {
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

/// The `.groot`: 22px duotone conic ring (accent blue → violet → back, NO
/// full rainbows) with a ✦ core, + "conversation / orchestrator" label.
fn root_node(conv_id: String, panel: WeakEntity<TaskBoardPanel>, cx: &App) -> AnyElement {
    let title: SharedString = if conv_id.is_empty() {
        "conversation".into()
    } else {
        conv_id.clone().into()
    };
    let text = cx.theme().colors().text;

    let ring = canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            // Two-color conic approximation: stroked arc segments lerping
            // ACCENT_AGY → ACCENT_GEMINI → ACCENT_AGY around the ring.
            let center = bounds.center();
            let radius = (bounds.size.width.as_f32() / 2.0) - 1.5; // 3px-stroke ring
            const SEGMENTS: usize = 24;
            for k in 0..SEGMENTS {
                let f = k as f32 / SEGMENTS as f32;
                let mix = if f < 0.5 { f * 2.0 } else { 2.0 - f * 2.0 };
                let color = lerp_rgba(ACCENT_AGY, ACCENT_GEMINI, mix);
                let mut builder = PathBuilder::stroke(px(3.));
                // Overlap each segment slightly to avoid hairline joints.
                for (step, sub) in [0.0_f32, 0.55, 1.1].into_iter().enumerate() {
                    let a = (f + sub / SEGMENTS as f32) * std::f32::consts::TAU;
                    let p = point(
                        center.x + px(radius * a.cos()),
                        center.y + px(radius * a.sin()),
                    );
                    if step == 0 {
                        builder.move_to(p);
                    } else {
                        builder.line_to(p);
                    }
                }
                if let Ok(path) = builder.build() {
                    window.paint_path(path, color);
                }
            }
        },
    )
    .absolute()
    .inset_0();

    let click_panel = panel;
    let click_conv = SharedString::from(conv_id.clone());
    h_flex()
        .id(ElementId::Name(format!("groot-{conv_id}").into()))
        .absolute()
        .left(px(X0 - 11.))
        .top(px(ROOTY - 12.))
        .items_center()
        .gap(px(9.))
        .cursor_pointer()
        .on_click(move |_, _, cx| {
            click_panel
                .update(cx, |panel, cx| {
                    panel.select_conversation(click_conv.clone(), cx)
                })
                .ok();
        })
        .child(
            div()
                .relative()
                .flex_none()
                .size(px(22.))
                .child(ring)
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(9.))
                        .text_color(text)
                        .child("✦"),
                ),
        )
        .child(node_label(title, "orchestrator".into(), true, cx))
        .into_any_element()
}

/// The `.glabel`: name line (13px/500, ≤160px ellipsis) over the mono
/// progress-note line (12px, text-3, tabular nums).
fn node_label(name: SharedString, note: SharedString, root: bool, cx: &App) -> AnyElement {
    let colors = cx.theme().colors();
    let mono = ThemeSettings::get_global(cx).buffer_font.family.clone();
    let _ = root;
    v_flex()
        .min_w_0()
        .child(
            div()
                .max_w(px(160.))
                .text_size(px(13.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(colors.text)
                .truncate()
                .child(name),
        )
        .child(
            div()
                .max_w(px(160.))
                .font_family(mono)
                .text_size(px(12.))
                .text_color(colors.text_placeholder)
                .truncate()
                .child(note),
        )
        .into_any_element()
}

/// Componentwise sRGB lerp between two accent stops.
fn lerp_rgba(a: Rgba, b: Rgba, t: f32) -> Hsla {
    Rgba {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: 1.0,
    }
    .into()
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
