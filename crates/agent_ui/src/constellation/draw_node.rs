//! Node visuals for the constellation draw layer: the mass-sized Atlas
//! checkpoint dot (hollow / sweeping rim arc / solid / stilled hard ring +
//! real-BoxShadow bloom), the compact archetype tag, and the hover card
//! dropped along the panel's major axis. Split from draw.rs (conv block /
//! canvas / root) purely for the one-concern-per-file ceiling.

use gpui::{
    AnyElement, App, BoxShadow, ElementId, FontWeight, Hsla, MouseButton, PathBuilder,
    SharedString, WeakEntity, canvas, point, px,
};
use settings::Settings as _;
use theme_settings::ThemeSettings;
use ui::prelude::*;

use crate::agent_accents::constellation_node_color;
use crate::task_board::style::{HAIRLINE_HI, SURFACE_2, agent_chip, status_phrase};

use super::panel::ConstellationPanel;
use super::sim::{ConvSim, NodeSim};

/// One subagent node: checkpoint dot (mass-sized) + compact tag + hover
/// card. Position/scale/opacity come straight off the sim outputs.
pub(super) fn node_el(
    node: &NodeSim,
    conv: &ConvSim,
    now: f32,
    is_hovered: bool,
    panel: WeakEntity<ConstellationPanel>,
    cx: &App,
) -> AnyElement {
    let run_id = SharedString::from(node.run_id.clone());
    let dot_size = node.mass_cur * node.out_scale;
    let spawn_opacity = (node.out_spawn / 0.6).clamp(0., 1.);
    let mono = ThemeSettings::get_global(cx).buffer_font.family.clone();
    let colors = cx.theme().colors();

    let tag_text: SharedString = node
        .archetype
        .clone()
        .filter(|a| !a.is_empty())
        .unwrap_or_else(|| node.agent.clone())
        .into();

    let down_panel = panel.clone();
    let down_id = run_id.clone();
    let click_panel = panel.clone();
    let click_id = run_id.clone();
    let hover_panel = panel;
    let hover_id = run_id.clone();

    let mut element = div()
        .id(ElementId::Name(format!("cnode-{run_id}").into()))
        .absolute()
        .left(px(node.out_x - 36.))
        .top(px(node.out_y - dot_size / 2.))
        .w(px(72.))
        .flex()
        .flex_col()
        .items_center()
        .gap(px(5.))
        .cursor_pointer()
        .opacity(spawn_opacity)
        .on_mouse_down(
            MouseButton::Left,
            move |event, _, cx| {
                down_panel
                    .update(cx, |panel, cx| {
                        panel.begin_node_drag(down_id.clone(), event.position, cx)
                    })
                    .ok();
            },
        )
        .on_click(move |_, _, cx| {
            click_panel
                .update(cx, |panel, cx| panel.node_clicked(click_id.clone(), cx))
                .ok();
        })
        .on_hover(move |hovered, _, cx| {
            let hovered = *hovered;
            hover_panel
                .update(cx, |panel, cx| {
                    panel.set_hover(hover_id.clone(), hovered, cx)
                })
                .ok();
        })
        .child(checkpoint_dot(&node.agent, &node.status, dot_size, now))
        .child(
            div()
                .max_w(px(70.))
                .font_family(mono)
                .text_size(px(11.))
                .text_color(colors.text_placeholder)
                .truncate()
                .when(node.absorbing.is_some(), |tag| {
                    tag.opacity(node.out_edge_alpha)
                })
                .child(tag_text),
        );

    if is_hovered && node.absorbing.is_none() {
        element = element.child(hover_card(node, conv, dot_size, cx));
    }
    element.into_any_element()
}

/// The Atlas checkpoint-dot grammar (board.css `.gdot`, mass-sized):
/// hollow = pending · sweeping rim arc = running · solid = done · stilled
/// hard ring = failed. Bloom = real BoxShadow (§1 native upgrade).
/// HUE carries the vendor (3-provider ruling 2026-07-08); the SHAPE above
/// carries lifecycle; failed/killed stay error-red via
/// [`constellation_node_color`].
fn checkpoint_dot(agent: &str, status: &str, size: f32, now: f32) -> AnyElement {
    let color = constellation_node_color(agent, status);
    let mut shadows: Vec<BoxShadow> = Vec::with_capacity(2);
    // `awaiting_approval` (Phase-2 §5.2) blooms too: the held amber glow —
    // [`constellation_node_color`] already resolved awaiting_* to the amber
    // safety hue, and the SHAPE below stays hollow (nothing runs, nothing
    // settled) — the constellation's held-state grammar.
    if matches!(status, "running" | "completed" | "failed" | "killed")
        || status.starts_with("awaiting")
    {
        shadows.push(BoxShadow {
            color: color.opacity(0.25),
            offset: point(px(0.), px(0.)),
            blur_radius: px(20.),
            spread_radius: px(6.),
        });
    }
    if matches!(status, "failed" | "killed") {
        // The stilled hard ring — a dead solid dot among breathing siblings;
        // the stillness is the signal.
        shadows.push(BoxShadow {
            color: color.opacity(0.20),
            offset: point(px(0.), px(0.)),
            blur_radius: px(0.),
            spread_radius: px(3.),
        });
    }
    let dot = div()
        .relative()
        .flex_none()
        .size(px(size))
        .rounded_full()
        .when(!shadows.is_empty(), |dot| dot.shadow(shadows));
    match status {
        "completed" => dot.bg(color).border_2().border_color(color),
        "failed" | "killed" => dot.bg(color).border_2().border_color(color),
        "running" => dot
            .border_2()
            .border_color(color.opacity(0.30))
            .child(rim_arc(color, now)),
        // Pending: hollow, but the ring already carries the vendor hue at
        // half strength — identity is visible from the moment of spawn.
        _ => dot.border_2().border_color(color.opacity(0.5)),
    }
    .into_any_element()
}

/// The sweeping rim arc — a quarter ring orbiting the dot at 0.9s/rev,
/// phased off the sim clock (the sim keeps frames coming while the
/// constellation is populated, so no `with_animation` wrapper is needed).
fn rim_arc(color: Hsla, now: f32) -> AnyElement {
    let phase = (now % 900.) / 900.;
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            if !bounds.intersects(&window.content_mask().bounds) {
                return;
            }
            let center = bounds.center();
            let radius = bounds.size.width.as_f32() / 2.0 + 1.;
            let start = phase * std::f32::consts::TAU;
            let mut builder = PathBuilder::stroke(px(2.));
            const SEGMENTS: usize = 14;
            for k in 0..=SEGMENTS {
                let a = start + (k as f32 / SEGMENTS as f32) * (std::f32::consts::TAU / 4.);
                let p = point(center.x + px(radius * a.cos()), center.y + px(radius * a.sin()));
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
    .absolute()
    .inset(px(-2.))
    .into_any_element()
}

/// The hover card — the node's "full view", dropped along the major axis
/// (wide panel → sideways, tall → below the dot).
fn hover_card(node: &NodeSim, conv: &ConvSim, dot_size: f32, cx: &App) -> AnyElement {
    let colors = cx.theme().colors();
    let mono = ThemeSettings::get_global(cx).buffer_font.family.clone();
    let task: SharedString = node
        .task
        .clone()
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| "task".into())
        .into();
    let phrase: SharedString = status_phrase(&node.status, node.elapsed_s).into();
    let card = div()
        .absolute()
        .w(px(210.))
        .p(px(11.))
        .rounded(px(12.))
        .bg(SURFACE_2)
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
                .font_weight(FontWeight::MEDIUM)
                .text_color(colors.text)
                .child(task),
        )
        .child(
            h_flex()
                .items_center()
                .gap(px(8.))
                .child(agent_chip(&node.agent, cx))
                .child(
                    div()
                        .font_family(mono)
                        .text_size(px(12.))
                        .text_color(colors.text_placeholder)
                        .truncate()
                        .child(phrase),
                ),
        );
    if conv.layout.wide {
        card.left(px(76.)).top(px(-6.))
    } else {
        card.left(px(36. - 105.)).top(px(dot_size + 24.))
    }
    .into_any_element()
}
