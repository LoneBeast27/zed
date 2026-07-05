//! Presentation for the vendor-grouped usage panel (PARITY_SPEC Amendment
//! 2026-07-04 (4) item 1). Free render functions so the panel entity
//! ([`crate::usage_panel::UsagePanel`]) keeps its lifecycle/state concern and
//! stays under the 500-line ceiling: a vendor cluster is a bordered card with
//! a header (name + liveness chip + worst-pool at-a-glance %) over its pools
//! as COMPACT rows. Every data element the old flat card carried survives —
//! meter, used-%, window, staleness, brain/ping calls, 429 — regrouped.
//!
//! The per-pool meter/roll animation state (keyed by pool name so fills morph
//! in place across store ticks — §5.6) lives on the entity and is threaded in
//! by `&mut` here; these functions are otherwise stateless render folds.

use std::collections::HashMap;

use gpui::{AnyElement, App, Div, ElementId, FontWeight, SharedString};
use settings::Settings as _;
use theme_settings::ThemeSettings;
use ui::Tooltip;
use ui::prelude::*;

use crate::agent_accents::{STATUS_BLOCKED, STATUS_ERROR, tone_for_used, used_pct};
use crate::bridge::protocol::VendorPlan;
use crate::bridge::{PoolRow, UsageMeta, VendorLiveness};
use crate::task_board::motion::RollValue;
use crate::task_board::style::{SURFACE_1, tabular_nums};
use crate::usage_panel_groups::{VendorGroup, vendor_label};
use crate::usage_panel_meter::{MeterState, render_meter};

/// Pool display names, vendor-RELATIVE (the vendor is the cluster header, so
/// the pool row drops the redundant vendor prefix): "claude_5h" → "5h session"
/// under the Claude header. Unknown pools fall back to their raw key so
/// nothing is ever anonymous.
pub(crate) fn pool_label(name: &str) -> &str {
    match name {
        "claude_5h" => "5h session",
        "claude_weekly" => "weekly (all models)",
        "claude_weekly_sonnet" => "weekly (Sonnet)",
        "claude_sdk_credit" => "SDK credit",
        "codex_plan" => "plan",
        "antigravity_weekly" => "weekly",
        "gemini_free_rpd" => "free RPD",
        other => other,
    }
}

/// JS-style number formatting: integers print bare ("82"), fractions keep one
/// decimal ("99.5") — matches the web's template-literal output. Shared with
/// the usage island (same `${used}%` rendering).
pub(crate) fn fmt_pct(value: f64) -> String {
    if (value - value.round()).abs() < 0.05 {
        format!("{}", value.round() as i64)
    } else {
        format!("{value:.1}")
    }
}

/// One vendor cluster: a bordered card holding the header over the vendor's
/// pools as compact rows.
pub(crate) fn render_vendor_cluster(
    group: &VendorGroup,
    meta: &UsageMeta,
    meters: &mut HashMap<String, MeterState>,
    pct_rolls: &mut HashMap<String, RollValue>,
    cx: &App,
) -> AnyElement {
    let colors = cx.theme().colors();
    let header = render_vendor_header(group, cx);
    let rows: Vec<AnyElement> = group
        .pools
        .iter()
        .map(|pool| render_pool_row(pool, meta, meters, pct_rolls, cx))
        .collect();
    v_flex()
        .w_full()
        .rounded(px(12.))
        .bg(SURFACE_1)
        .border_1()
        .border_color(colors.border)
        .px(px(16.))
        .py(px(14.))
        .child(header)
        .child(v_flex().mt(px(12.)).gap(px(14.)).children(rows))
        .into_any_element()
}

/// The vendor cluster HEADER: vendor name (15px/500) + its liveness chip (when
/// surfaced) + the worst-pool at-a-glance % right-aligned (a tone dot + "NN%
/// used", or "—" for an all-unknown cluster).
fn render_vendor_header(group: &VendorGroup, cx: &App) -> Div {
    let colors = cx.theme().colors();
    let mono = ThemeSettings::get_global(cx).buffer_font.family.clone();
    let tone = group.worst_tone();
    let glance = match group.worst_used() {
        Some(used) => format!("{}% used", fmt_pct(used)),
        None => "—".to_string(),
    };
    h_flex()
        .items_center()
        .gap(px(10.))
        .child(
            div()
                .text_size(px(15.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(colors.text)
                .child(SharedString::from(vendor_label(group.vendor).to_string())),
        )
        .children(
            group
                .liveness
                .map(|vendor| render_liveness_chip(vendor, cx)),
        )
        .child(
            // Worst-pool at-a-glance figure, right-aligned.
            h_flex()
                .ml_auto()
                .items_center()
                .gap(px(6.))
                .children(group.plan.map(|plan| render_plan_chip(plan, cx)))
                .children(group.plan.map(|plan| render_billing_button(plan)))
                .child(
                    div()
                        .size(px(6.))
                        .rounded_full()
                        .bg(tone.color())
                        .flex_none(),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .font_family(mono)
                        .font_features(tabular_nums())
                        .text_color(colors.text_muted)
                        .child(SharedString::from(glance)),
                ),
        )
}

/// The vendor cluster's plan chip, folded into the header next to liveness.
fn render_plan_chip(plan: &VendorPlan, cx: &App) -> Div {
    let colors = cx.theme().colors();
    let mono = ThemeSettings::get_global(cx).buffer_font.family.clone();
    let label_color = if plan.source == "detected" {
        colors.text
    } else {
        colors.text_muted
    };
    h_flex()
        .flex_none()
        .items_center()
        .px(px(9.))
        .py(px(4.))
        .rounded(px(8.))
        .bg(SURFACE_1)
        .child(
            div()
                .font_family(mono)
                .text_size(px(12.))
                .text_color(label_color)
                .child(SharedString::from(plan.label.clone())),
        )
}

/// External billing affordance for the vendor plan.
fn render_billing_button(plan: &VendorPlan) -> IconButton {
    let billing_url = plan.billing_url.clone();
    IconButton::new(
        SharedString::from(format!("usage-plan-billing-{}", plan.vendor)),
        IconName::ArrowUpRight,
    )
    .icon_size(IconSize::Small)
    .icon_color(Color::Muted)
    .tooltip(Tooltip::text("Manage billing"))
    .on_click(move |_, _, cx| {
        cx.open_url(&billing_url);
    })
}

/// The vendor cluster's liveness chip, folded into the header (2026-07-04
/// `liveness` map + Amendment (4) item 1: liveness now lives on the vendor it
/// describes, not a separate top strip). "is <vendor> alive RIGHT NOW": a tone
/// dot (green alive / amber rate_limited / red down) + word + age.
fn render_liveness_chip(vendor: &VendorLiveness, cx: &App) -> Div {
    let mono = ThemeSettings::get_global(cx).buffer_font.family.clone();
    let (dot, label_color) = liveness_tone(&vendor.state);
    let age = vendor
        .age_s
        .map(|age| format!("  ·  {}", crate::task_board::style::rel(age)))
        .unwrap_or_default();
    h_flex()
        .flex_none()
        .items_center()
        .gap(px(6.))
        .px(px(9.))
        .py(px(4.))
        .rounded(px(8.))
        .bg(SURFACE_1)
        .child(div().size(px(6.)).rounded_full().bg(dot).flex_none())
        .child(
            div()
                .font_family(mono)
                .text_size(px(12.))
                .text_color(label_color)
                .child(SharedString::from(format!(
                    "{}{age}",
                    liveness_word(&vendor.state)
                ))),
        )
}

/// One pool as a COMPACT row inside its vendor cluster: name + inline meter,
/// with the used-% + window on the right and the staleness / brain·ping / 429
/// sub-lines beneath. Preserves every data element the old full card carried
/// (§4.4 degrade-never-vanish included).
fn render_pool_row(
    pool: &PoolRow,
    meta: &UsageMeta,
    meters: &mut HashMap<String, MeterState>,
    pct_rolls: &mut HashMap<String, RollValue>,
    cx: &App,
) -> AnyElement {
    let colors = cx.theme().colors();
    let mono = ThemeSettings::get_global(cx).buffer_font.family.clone();
    let used = used_pct(pool.headroom_pct);
    let state = meters
        .entry(pool.name.clone())
        .or_insert_with(|| MeterState::new(tone_for_used(used)));
    state.update(used);
    let meter = render_meter(state, &pool.name);
    let window_label = pool.window.clone().unwrap_or_default();

    // `.pool-pct`: the % rolls its changed digits in lockstep with the meter
    // fill (keyed by pool name, survives store ticks — §5.6); the unknown case
    // degrades to the mono-italic "unknown / stale" tone.
    let pct_cell = match used {
        Some(used) => {
            let roll = pct_rolls
                .entry(pool.name.clone())
                .or_insert_with(|| RollValue::new(String::new()));
            roll.set(format!("{}% used", fmt_pct(used)));
            div().text_size(px(13.)).child(roll.element(
                ElementId::Name(format!("pool-pct-{}", pool.name).into()),
                px(13.),
                FontWeight::MEDIUM,
                colors.text,
            ))
        }
        None => div()
            .text_size(px(12.))
            .font_family(mono.clone())
            .italic()
            .text_color(colors.text_placeholder)
            .child("unknown / stale"),
    };

    // `.pool-stale` — mono 11px --text-3; the stale BANNER carries the tint.
    let status_line = match meta.scraped.as_ref() {
        Some(scrape) if scrape.stale => format!(
            "scraped {}h ago (stale)",
            scrape.age_h.map(fmt_pct).unwrap_or_else(|| "?".into())
        ),
        Some(scrape) => match &scrape.reset_phrase {
            Some(phrase) => format!("resets {phrase}"),
            None => "self-metered".to_string(),
        },
        None => "self-metered".to_string(),
    };

    v_flex()
        .w_full()
        .child(
            // Name row: pool label left, used-% + window right.
            h_flex()
                .items_baseline()
                .justify_between()
                .gap(px(10.))
                .mb(px(8.))
                .child(
                    // `.pool-name` 500 14px --text (vendor-relative label).
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(14.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(colors.text)
                        .truncate()
                        .child(SharedString::from(pool_label(&pool.name).to_string())),
                )
                .child(pct_cell)
                .child(
                    // `.pool-cap` 400 12px mono --text-3 tabular.
                    div()
                        .flex_none()
                        .text_size(px(12.))
                        .font_family(mono.clone())
                        .font_features(tabular_nums())
                        .text_color(colors.text_placeholder)
                        .child(SharedString::from(window_label)),
                ),
        )
        .child(meter)
        .child(
            div()
                .mt(px(8.))
                .text_size(px(11.))
                .font_family(mono.clone())
                .text_color(colors.text_placeholder)
                .child(SharedString::from(status_line)),
        )
        // Call-count breakdown (2026-07-04 fields): real brain calls vs
        // liveness/keepalive pings, rendered only when either is surfaced.
        .children(render_pool_calls(pool, &mono, cx))
        // `vendor_429_observed` — a red marker that the pool hit a real vendor
        // ceiling in this window.
        .when(pool.vendor_429_observed, |this| {
            this.child(
                h_flex()
                    .mt(px(6.))
                    .items_center()
                    .gap(px(6.))
                    .child(
                        div()
                            .size(px(6.))
                            .rounded_full()
                            .bg(gpui::Hsla::from(STATUS_ERROR))
                            .flex_none(),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .font_family(mono.clone())
                            .text_color(STATUS_ERROR)
                            .child("429 observed"),
                    ),
            )
        })
        .into_any_element()
}

/// The per-pool call-count line ("N brain · M ping"): real brain calls
/// separated from liveness/keepalive pings (2026-07-04). `None` when the
/// bridge surfaces neither count (older bridge / pool without the ping hack).
fn render_pool_calls(pool: &PoolRow, mono: &SharedString, cx: &App) -> Option<Div> {
    let brain = pool.brain_calls_in_window;
    let ping = pool.ping_calls_in_window;
    if brain.is_none() && ping.is_none() {
        return None;
    }
    let mut parts: Vec<String> = Vec::new();
    if let Some(brain) = brain {
        parts.push(format!("{brain} brain"));
    }
    if let Some(ping) = ping {
        parts.push(format!("{ping} ping"));
    }
    let colors = cx.theme().colors();
    Some(
        div()
            .mt(px(4.))
            .text_size(px(11.))
            .font_family(mono.clone())
            .font_features(tabular_nums())
            .text_color(colors.text_placeholder)
            .child(SharedString::from(parts.join("  ·  "))),
    )
}

/// The tone (status dot color + label color) for a vendor liveness state.
/// alive → green, rate_limited → amber, down/anything-error → red, unknown →
/// idle grey. Word and color agree (the P1 rule).
fn liveness_tone(state: &str) -> (gpui::Hsla, gpui::Hsla) {
    let color = match state {
        "alive" | "ok" | "up" => crate::agent_accents::STATUS_RUNNING,
        "rate_limited" | "limited" | "throttled" => STATUS_BLOCKED,
        "down" | "dead" | "error" | "unreachable" => STATUS_ERROR,
        _ => crate::agent_accents::STATUS_IDLE,
    };
    (color.into(), color.into())
}

/// The display word for a vendor liveness state (raw states pass through so a
/// future state is visible rather than swallowed).
fn liveness_word(state: &str) -> &str {
    match state {
        "alive" => "alive",
        "rate_limited" => "rate-limited",
        "down" => "down",
        "" => "unknown",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pool_labels_are_vendor_relative() {
        // Under a vendor cluster header the pool row drops the redundant
        // vendor prefix (Amendment 2026-07-04 (4) item 1).
        assert_eq!(pool_label("claude_5h"), "5h session");
        assert_eq!(pool_label("claude_weekly"), "weekly (all models)");
        assert_eq!(pool_label("claude_weekly_sonnet"), "weekly (Sonnet)");
        assert_eq!(pool_label("codex_plan"), "plan");
        assert_eq!(pool_label("antigravity_weekly"), "weekly");
        assert_eq!(pool_label("gemini_free_rpd"), "free RPD");
        assert_eq!(pool_label("future_pool"), "future_pool");
    }

    #[test]
    fn fmt_pct_prints_like_js() {
        assert_eq!(fmt_pct(82.0), "82");
        assert_eq!(fmt_pct(99.5), "99.5");
        assert_eq!(fmt_pct(0.0), "0");
        assert_eq!(fmt_pct(100.0), "100");
    }
}
