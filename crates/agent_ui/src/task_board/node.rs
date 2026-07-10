//! Graph node visuals (PARITY_SPEC §4.2 node anatomy): the 16px status dot
//! with real-BoxShadow glow (RUST_PORT_NOTES §1 native upgrade) and rotating
//! open-circle spinner arc, the duotone-ring conversation root (NO full
//! rainbows — two-hue arc only), and the name + progress-note label.

use std::time::Duration;

use gpui::{
    Animation, AnimationExt as _, AnyElement, App, Bounds, BoxShadow, ElementId, FontWeight,
    Hsla, Path, PathBuilder, Pixels, Rgba, SharedString, WeakEntity, canvas, point, px,
};
use settings::Settings as _;
use theme_settings::ThemeSettings;
use ui::prelude::*;

use crate::agent_accents::{ACCENT_AGY, ACCENT_GEMINI, color_for_status};

use super::paint_cache::SharedPaintCache;
use super::panel::TaskBoardPanel;
use super::style::{HAIRLINE_HI, SURFACE_2};

// Root layout anchors (graph.js: root dot top-left at X0-11, ROOTY-12).
use super::graph::{ROOTY, X0};

/// The `.gdot`: 16px circle, surface fill, high hairline border, status glow
/// as a real BoxShadow; completed/failed/killed = filled center dot, running
/// = open-circle spinner arc rotating on the rim.
pub(super) fn status_dot(run_id: &str, status: &str) -> AnyElement {
    let color = color_for_status(status);
    // `awaiting_approval` (Phase-2 §5.2) joins the glow set: the held amber
    // bloom — hollow center (nothing runs, nothing settled), never the
    // spinner, never a fill.
    let glowing = matches!(status, "running" | "completed" | "failed" | "killed")
        || status.starts_with("awaiting");
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
                            // Offscreen culling (RUST_PORT_NOTES §8).
                            if !bounds.intersects(&window.content_mask().bounds) {
                                return;
                            }
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

/// Builds the duotone ring's 24 stroked arc-segment paths for `bounds` —
/// static geometry, cached by the caller and rebuilt only when bounds move.
fn ring_segment_paths(bounds: Bounds<Pixels>) -> Vec<(Path<Pixels>, Hsla)> {
    // Two-color conic approximation: stroked arc segments lerping
    // ACCENT_AGY → ACCENT_GEMINI → ACCENT_AGY around the ring.
    let center = bounds.center();
    let radius = (bounds.size.width.as_f32() / 2.0) - 1.5; // 3px-stroke ring
    const SEGMENTS: usize = 24;
    let mut paths = Vec::with_capacity(SEGMENTS);
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
            paths.push((path, color));
        }
    }
    paths
}

/// The `.groot`: 22px duotone conic ring (accent blue → violet → back, NO
/// full rainbows) with a ✦ core, + "conversation / orchestrator" label.
pub(super) fn root_node(
    conv_id: String,
    cache: SharedPaintCache,
    panel: WeakEntity<TaskBoardPanel>,
    cx: &App,
) -> AnyElement {
    let title: SharedString = if conv_id.is_empty() {
        "conversation".into()
    } else {
        conv_id.clone().into()
    };
    let text = cx.theme().colors().text;

    let ring_conv = conv_id.clone();
    let ring = canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            // Offscreen culling (RUST_PORT_NOTES §8).
            if !bounds.intersects(&window.content_mask().bounds) {
                return;
            }
            // Static geometry: 24 PathBuilder builds once per bounds, not
            // per frame.
            let paths = cache
                .borrow_mut()
                .ring_paths(&ring_conv, bounds, || ring_segment_paths(bounds));
            for (path, color) in paths {
                window.paint_path(path, color);
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
        .child(node_label(title, "orchestrator".into(), cx))
        .into_any_element()
}

/// The `.glabel`: name line (13px/500, ≤160px ellipsis) over the mono
/// progress-note line (12px, text-3, tabular nums).
pub(super) fn node_label(name: SharedString, note: SharedString, cx: &App) -> AnyElement {
    let colors = cx.theme().colors();
    let mono = ThemeSettings::get_global(cx).buffer_font.family.clone();
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
