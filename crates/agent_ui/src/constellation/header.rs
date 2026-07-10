//! The panel header — the shared `.panel-head` twin: title · live stats ·
//! provider legend · channels toggle · arrange button. Extracted from
//! panel.rs (which sat over the 500-line ceiling) when the channels toggle
//! landed; pure element building over the already-folded sim, no state of
//! its own.

use gpui::{Context, FontWeight, SharedString};
use ui::prelude::*;

use super::panel::ConstellationPanel;

/// Build the header. The stats + legend + arrange land with the first node
/// (an empty panel keeps the plain subtitle); the channels toggle is ALWAYS
/// present — the observe surface must stay discoverable even on an empty
/// graph (its empty state is honest, never blank).
pub(super) fn render_header(
    panel: &ConstellationPanel,
    has_nodes: bool,
    cx: &mut Context<ConstellationPanel>,
) -> Div {
    let colors = cx.theme().colors();
    // Live density stats (backlog item: "N nodes · M channels · ctx") —
    // read off the already-folded sim, no extra derivation.
    let node_count: usize = panel.sim.convs.iter().map(|conv| conv.nodes.len()).sum();
    let channel_count = panel.sim.channels.iter_sorted().len();
    let ctx_tokens: f64 = panel
        .sim
        .convs
        .iter()
        .filter_map(|conv| conv.root_mass.as_ref())
        .map(|mass| mass.tokens)
        .sum();
    let stats = if has_nodes {
        let mut s = format!("{node_count} node{}", if node_count == 1 { "" } else { "s" });
        if channel_count > 0 {
            s.push_str(&format!(
                " · {channel_count} channel{}",
                if channel_count == 1 { "" } else { "s" }
            ));
        }
        if ctx_tokens > 0. {
            s.push_str(&format!(" · {:.1}K ctx", ctx_tokens / 1000.));
        }
        SharedString::from(s)
    } else {
        "agent relationships and message flow".into()
    };
    // The 3-provider legend (user ruling 2026-07-08: hue = WHO) — the
    // color coding must never need prior knowledge to read.
    let legend_dot = |accent: gpui::Rgba, label: &'static str| {
        h_flex()
            .items_center()
            .gap(px(4.))
            .child(div().flex_none().size(px(6.)).rounded_full().bg(accent))
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(colors.text_placeholder)
                    .child(label),
            )
    };
    h_flex()
        .flex_none()
        .items_baseline()
        .gap(px(12.))
        .px(px(28.))
        .pt(px(18.))
        .pb(px(14.))
        .border_b_1()
        .border_color(colors.border)
        .child(
            div()
                .text_size(px(18.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(colors.text)
                .child("Constellation"),
        )
        .child(
            div()
                .text_size(px(13.))
                .text_color(colors.text_placeholder)
                .child(stats),
        )
        .child(div().flex_1())
        .when(has_nodes, |header| {
            header
                .child(
                    h_flex()
                        .items_center()
                        .gap(px(10.))
                        .child(legend_dot(crate::agent_accents::ACCENT_CLAUDE, "claude"))
                        .child(legend_dot(crate::agent_accents::ACCENT_CODEX, "codex"))
                        .child(legend_dot(crate::agent_accents::ACCENT_AGY, "google")),
                )
                .child(
                    ui::IconButton::new("constellation-arrange", IconName::GitGraph)
                        .icon_size(ui::IconSize::Small)
                        .tooltip(ui::Tooltip::text(
                            "Auto-arrange the constellation (drags re-pin)",
                        ))
                        .on_click(cx.listener(|this, _, _, cx| this.arrange_clicked(cx))),
                )
        })
        .child(
            // Channels observe-surface toggle (T2) — mirrors the arrange
            // idiom, but never gated behind has_nodes.
            ui::IconButton::new("constellation-channels", IconName::ArrowRightLeft)
                .icon_size(ui::IconSize::Small)
                .toggle_state(panel.channels_shown)
                .tooltip(ui::Tooltip::text(
                    "Show/hide boost channels (grants, receipts, overlap)",
                ))
                .on_click(cx.listener(|this, _, _, cx| this.toggle_channels(cx))),
        )
}
