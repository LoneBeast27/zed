//! Constellation draw layer — builds one conversation block's elements from
//! the sim's per-frame outputs. Pure read: the sim advanced BEFORE this
//! runs; the canvas paint closures replay a small snapshot (star edges,
//! channel edges, particles, rings) so the whole breathing body — anchors +
//! physics + drift + flights — moves as one (RUST_PORT_NOTES §10:
//! flight-tracked edges read animated positions; we own the clock).
//!
//! Visual grammar (board.css, exact): Atlas checkpoint dots (hollow=pending
//! · sweeping rim arc=running · solid=done · stilled hard ring=failed), the
//! root the SAME language one rank bigger with the accent ring (no conic, no
//! glyph), status bloom as real BoxShadow (§1 native upgrade), violet
//! Kiali channel edges with midpoint k/max badges.

use gpui::{
    AnyElement, App, BoxShadow, ElementId, FontWeight, Hsla, PathBuilder, Pixels, Point, Rgba,
    SharedString, WeakEntity, canvas, point, px,
};
use settings::Settings as _;
use theme_settings::ThemeSettings;
use ui::prelude::*;

use crate::agent_accents::{ACCENT, STATUS_ERROR, rgba_hex};

use super::advance::{RING_MS, exhale_scale, gulp_scale};
use super::draw_node::node_el;
use super::edges::{CHANNEL_TRIM, ChannelSim, RETRACT_TRIM, retract_endpoints, trimmed_segment};
use super::panel::ConstellationPanel;
use super::sim::{ConvSim, Sim};

// ── channel palette (board.css .gchan*) ──
const CHAN_GLOW: Rgba = rgba_hex(0xa78bfa1f); // 0.12
const CHAN_DOT: Rgba = rgba_hex(0xa78bfaff);
const CHAN_BADGE_TEXT: Rgba = rgba_hex(0xc4b5fdff);
const CHAN_BADGE_BG: Rgba = rgba_hex(0x141022d9); // rgba(20,16,34,.85)
const PULSE_VIOLET: Rgba = rgba_hex(0xa78bfaff);
/// Root ring glow: rgba(138,180,248,0.30).
const ROOT_GLOW: Rgba = rgba_hex(0x8ab4f84d);

/// Channel line alpha by state class (working = alive, awaiting = held).
fn channel_line_alpha(state: &str) -> f32 {
    if state == "working" {
        0.85
    } else if state.starts_with("awaiting") {
        0.35
    } else {
        0.55
    }
}

/// One paintable segment for the conv canvas.
enum PaintCmd {
    /// Stroked line segment: endpoints, width, color.
    Seg((f32, f32), (f32, f32), f32, Hsla),
    /// Filled dot: center, radius, color.
    Dot((f32, f32), f32, Hsla),
    /// Stroked circle: center, radius, width, color.
    Ring((f32, f32), f32, f32, Hsla),
}

/// Build one conversation's constellation block.
pub(super) fn conv_block(
    conv: &ConvSim,
    channels: &[&ChannelSim],
    sim: &Sim,
    now: f32,
    hovered: Option<&SharedString>,
    panel: WeakEntity<ConstellationPanel>,
    cx: &App,
) -> AnyElement {
    let mut cmds: Vec<PaintCmd> = Vec::with_capacity(conv.nodes.len() + channels.len() * 2 + 4);
    let root = (conv.layout.rx, conv.layout.ry);
    let root_draw = conv.root_cur * gulp_scale(conv.gulp_at, now) * exhale_scale(&conv.exhales, now);
    let root_r = root_draw / 2.;

    // Star edges: root rim → node rim, live trims (T4), status tint, spawn
    // opacity riding the flight (the edge stretches with the dot).
    for node in &conv.nodes {
        let alpha = node.out_edge_alpha * node.out_spawn.clamp(0., 1.);
        if alpha <= 0.01 {
            continue;
        }
        let (a, b) = trimmed_segment(
            root.0,
            root.1,
            node.out_x,
            node.out_y,
            root_r + 3.,
            node.mass_cur * node.out_scale / 2. + 4.,
        );
        cmds.push(PaintCmd::Seg(
            a,
            b,
            1.5,
            star_edge_color(&node.agent, &node.status).opacity(alpha),
        ));
    }

    // Boost-channel edges (Kiali grammar): glow + line + particles, terminal
    // retract easing both endpoints into the midpoint.
    for channel in channels {
        let (Some(pa), Some(pb)) = (
            sim.position_of(&channel.row.a),
            sim.position_of(&channel.row.b),
        ) else {
            continue; // an endpoint left the stage (gobbled)
        };
        let p = channel.retract(now);
        let (a, b, _mid) = retract_endpoints(pa, pb, p);
        let trim = if p > 0. { RETRACT_TRIM } else { CHANNEL_TRIM };
        let (a, b) = trimmed_segment(a.0, a.1, b.0, b.1, trim, trim);
        let fade = 1. - p;
        cmds.push(PaintCmd::Seg(a, b, 6., Hsla::from(CHAN_GLOW).opacity(fade)));
        let line: Hsla = Hsla::from(CHAN_DOT).opacity(channel_line_alpha(&channel.row.state) * fade);
        cmds.push(PaintCmd::Seg(a, b, 1.5, line));
        for particle in &channel.particles {
            let (from, to) = if particle.rev { (pb, pa) } else { (pa, pb) };
            if let Some(pos) = particle.pos_at(now, from, to) {
                cmds.push(PaintCmd::Dot(pos, 3., CHAN_DOT.into()));
            }
        }
    }

    // Root rings: outward feed pulse (gobble) + inward compaction exhale.
    for t0 in &conv.pulses {
        let k = ((now - t0) / RING_MS).clamp(0., 1.);
        let eased = crate::task_board::motion::DECEL.eval(k);
        let scale = 1. + 1.4 * eased;
        cmds.push(PaintCmd::Ring(
            root,
            11. * scale,
            2.,
            Hsla::from(PULSE_VIOLET).opacity(0.85 * (1. - eased)),
        ));
    }
    for t0 in &conv.exhales {
        let k = ((now - t0) / RING_MS).clamp(0., 1.);
        if k >= 1. {
            continue;
        }
        let eased = crate::task_board::motion::DECEL.eval(k);
        let scale = 2.3 + (0.7 - 2.3) * eased;
        let alpha = if k < 0.3 { 0.8 * (k / 0.3) } else { 0.8 * (1. - k) / 0.7 };
        cmds.push(PaintCmd::Ring(
            root,
            11. * scale,
            2.,
            Hsla::from(ACCENT).opacity(alpha),
        ));
    }

    let edge_canvas = canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            if !bounds.intersects(&window.content_mask().bounds) {
                return;
            }
            let origin = bounds.origin;
            for cmd in &cmds {
                match *cmd {
                    PaintCmd::Seg(a, b, width, color) => {
                        let mut builder = PathBuilder::stroke(px(width));
                        builder.move_to(at(origin, a));
                        builder.line_to(at(origin, b));
                        if let Ok(path) = builder.build() {
                            window.paint_path(path, color);
                        }
                    }
                    PaintCmd::Dot(center, radius, color) => {
                        if let Some(path) = circle_path(origin, center, radius, None) {
                            window.paint_path(path, color);
                        }
                    }
                    PaintCmd::Ring(center, radius, width, color) => {
                        if let Some(path) = circle_path(origin, center, radius, Some(width)) {
                            window.paint_path(path, color);
                        }
                    }
                }
            }
        },
    )
    .absolute()
    .size_full();

    // Node elements — the hovered node builds LAST so its card paints above
    // its siblings (paint order = child order).
    let mut nodes: Vec<AnyElement> = Vec::with_capacity(conv.nodes.len());
    let mut hovered_el: Option<AnyElement> = None;
    for node in &conv.nodes {
        let is_hovered = hovered.is_some_and(|id| id.as_ref() == node.run_id);
        let element = node_el(node, conv, now, is_hovered, panel.clone(), cx);
        if is_hovered {
            hovered_el = Some(element);
        } else {
            nodes.push(element);
        }
    }

    // Channel badges (k/max at the midpoint, absorbing on the retract beat).
    let badges: Vec<AnyElement> = channels
        .iter()
        .filter_map(|channel| {
            let pa = sim.position_of(&channel.row.a)?;
            let pb = sim.position_of(&channel.row.b)?;
            let p = channel.retract(now);
            Some(channel_badge(channel, pa, pb, p, cx))
        })
        .collect();

    div()
        .relative()
        .w_full()
        .h(px(conv.layout.h))
        .mb(px(24.))
        .child(edge_canvas)
        .child(root_el(conv, root_draw, panel, cx))
        .children(nodes)
        .children(hovered_el)
        .children(badges)
        .into_any_element()
}

fn at(origin: Point<Pixels>, p: (f32, f32)) -> Point<Pixels> {
    point(origin.x + px(p.0), origin.y + px(p.1))
}

/// A polygonal circle path — filled (`width: None`) or stroked.
fn circle_path(
    origin: Point<Pixels>,
    center: (f32, f32),
    radius: f32,
    width: Option<f32>,
) -> Option<gpui::Path<Pixels>> {
    const SEGMENTS: usize = 20;
    let mut builder = match width {
        Some(w) => PathBuilder::stroke(px(w)),
        None => PathBuilder::fill(),
    };
    for k in 0..=SEGMENTS {
        let a = k as f32 / SEGMENTS as f32 * std::f32::consts::TAU;
        let p = at(origin, (center.0 + radius * a.cos(), center.1 + radius * a.sin()));
        if k == 0 {
            builder.move_to(p);
        } else {
            builder.line_to(p);
        }
    }
    builder.build().ok()
}

/// `.gedge` tint (3-provider ruling 2026-07-08): the edge carries its
/// node's VENDOR hue — bright while running (45%), quiet when settled
/// (20%) — so provider clusters read at a glance; failure stays red.
fn star_edge_color(agent: &str, status: &str) -> Hsla {
    match status {
        "failed" | "killed" => Hsla::from(STATUS_ERROR).opacity(0.30),
        "running" => crate::agent_accents::accent_for_agent(agent).opacity(0.45),
        _ => crate::agent_accents::accent_for_agent(agent).opacity(0.20),
    }
}

/// The conversation root: SAME checkpoint language one rank bigger — a
/// hollow accent ring ("always listening") with a quiet bloom; title above
/// so the fan stays clear. Mass gain grows the same ring.
fn root_el(
    conv: &ConvSim,
    root_draw: f32,
    panel: WeakEntity<ConstellationPanel>,
    cx: &App,
) -> AnyElement {
    let colors = cx.theme().colors();
    let conv_id = SharedString::from(conv.conv_id.clone());
    let click_id = conv_id.clone();
    div()
        .id(ElementId::Name(format!("croot-{conv_id}").into()))
        .absolute()
        .left(px(conv.layout.rx - 60.))
        .top(px(conv.layout.ry - root_draw / 2. - 21.))
        .w(px(120.))
        .flex()
        .flex_col()
        .items_center()
        .gap(px(4.))
        .cursor_pointer()
        .on_click(move |_, _, cx| {
            panel
                .update(cx, |panel, cx| {
                    panel.select_conversation(click_id.clone(), cx)
                })
                .ok();
        })
        .child(
            div()
                .max_w(px(120.))
                .text_size(px(13.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(colors.text)
                .truncate()
                .child(conv.title.clone()),
        )
        .child(
            div()
                .flex_none()
                .size(px(root_draw))
                .rounded_full()
                .border_2()
                .border_color(Hsla::from(ACCENT))
                .shadow(vec![BoxShadow {
                    color: ROOT_GLOW.into(),
                    offset: point(px(0.), px(0.)),
                    blur_radius: px(16.),
                    spread_radius: px(0.),
                }]),
        )
        .into_any_element()
}

/// The k/max batch badge at the channel midpoint (n8n items-count steal),
/// absorbing (fading) on the retract beat.
fn channel_badge(
    channel: &ChannelSim,
    pa: (f32, f32),
    pb: (f32, f32),
    retract: f32,
    cx: &App,
) -> AnyElement {
    let mono = ThemeSettings::get_global(cx).buffer_font.family.clone();
    let text = format!("{}/{}", channel.row.k, channel.row.max_batches);
    let width_est = text.len() as f32 * 6.5 + 12.;
    let mid = ((pa.0 + pb.0) / 2., (pa.1 + pb.1) / 2.);
    let border_alpha = if channel.row.state == "working" { 0.7 } else { 0.35 };
    div()
        .absolute()
        .left(px(mid.0 - width_est / 2.))
        .top(px(mid.1 - 9.))
        .px(px(6.))
        .py(px(1.))
        .rounded(px(8.))
        .bg(Hsla::from(CHAN_BADGE_BG))
        .border_1()
        .border_color(Hsla::from(CHAN_DOT).opacity(border_alpha))
        .font_family(mono)
        .text_size(px(10.))
        .text_color(Hsla::from(CHAN_BADGE_TEXT))
        .opacity(1. - retract)
        .child(SharedString::from(text))
        .into_any_element()
}
