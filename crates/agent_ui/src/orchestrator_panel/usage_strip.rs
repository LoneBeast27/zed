//! The inline usage cluster (user rulings 2026-07-08): usage lives IN the
//! chat deck's CONTROLS ROW — to the side, not a band on top ("add it to
//! the left or right") — as `[extending detail] [vendor dots] [session %]`
//! right of the spacer, beside the send circle. Clicking it EXTENDS the
//! per-pool detail HORIZONTALLY into the spacer's empty space.
//!
//! Dot hue = the pool's VENDOR (the 3-provider orange/green/blue read);
//! usage pressure lives in the NUMBERS — the extended detail's % text keeps
//! the tone tint (ok/warn/crit), so "who" and "how much left" never fight
//! over the same pixel.

use gpui::SharedString;
use ui::prelude::*;

use crate::agent_accents::{
    ACCENT_AGY, ACCENT_CLAUDE, ACCENT_CODEX, TEXT_3, Tone, tone_for_used, used_pct,
};
use crate::bridge::PoolRow;
use crate::islands::island_faces::short_pool;
use crate::usage_panel_groups::vendor_of;

/// The session metric (Amendment 2026-07-04 (4) item 2, user ruling:
/// "that's the useful one for pill") — the claude 5h window %.
const SESSION_POOL: &str = "claude_5h";
/// Pools with no cluster representation (Amendment 2026-07-04 (4b)): the
/// gemini heartbeat pool is plumbing — panel-only.
const PILL_HIDDEN_POOLS: [&str; 1] = ["gemini_free_rpd"];
/// Estimated width of one extended-detail segment (label + % + gap) — the
/// clip window's full-open width. Generous on purpose: the inner row is
/// right-justified, so overshoot parks empty space on the (invisible)
/// left edge while undershoot would clip a segment.
const SEG_W: f32 = 118.;

/// One pool as the cluster renders it.
#[derive(Clone, PartialEq)]
pub(super) struct StripPool {
    short: SharedString,
    used: Option<f64>,
    tone: Tone,
    vendor: gpui::Hsla,
}

/// The vendor identity hue for a pool name (3-provider ruling: agy and
/// gemini share the Google blue family — `vendor_of` folds both into
/// "google"). Unknown vendors stay neutral, never impersonating a known one.
fn vendor_accent(pool_name: &str) -> gpui::Hsla {
    match vendor_of(pool_name) {
        "claude" => ACCENT_CLAUDE.into(),
        "codex" => ACCENT_CODEX.into(),
        "google" => ACCENT_AGY.into(),
        _ => TEXT_3.into(),
    }
}

/// Extended-detail label for a pool: tighter than the raw pool name (the
/// vendor dot already carries WHO, so the label only needs the WINDOW —
/// sweep 2026-07-08: "claude_5h/claude_weekly_sonnet" read as debug output).
fn strip_label(name: &str) -> &str {
    match name {
        "claude_5h" => "5h",
        "claude_weekly" => "weekly",
        "claude_weekly_sonnet" => "sonnet",
        "codex_plan" => "codex",
        "antigravity_weekly" => "agy",
        other => short_pool(other),
    }
}

/// Fold the bridge pool rows into strip pools (hidden pools dropped, dot
/// row capped at 5 — the pill grammar).
pub(super) fn fold_pools(rows: &[PoolRow]) -> Vec<StripPool> {
    rows.iter()
        .filter(|pool| !PILL_HIDDEN_POOLS.contains(&pool.name.as_str()))
        .take(5)
        .map(|pool| {
            let used = used_pct(pool.headroom_pct);
            StripPool {
                short: SharedString::from(strip_label(&pool.name).to_string()),
                used,
                tone: tone_for_used(used),
                vendor: vendor_accent(&pool.name),
            }
        })
        .collect()
}

/// The session-% text (claude 5h used-%), `None` when absent/unmetered —
/// the cluster degrades to dots-only rather than borrowing another pool's
/// number (metric-source honesty).
pub(super) fn session_pct(rows: &[PoolRow]) -> Option<SharedString> {
    rows.iter()
        .find(|pool| pool.name == SESSION_POOL)
        .and_then(|pool| used_pct(pool.headroom_pct))
        .map(|used| SharedString::from(format!("{}%", used.round() as i64)))
}

/// The inline cluster element: `[detail (clip, width = open_t · full)]
/// [vendor dots] [%]`, sized to content — it sits in the controls row
/// between the spacer and the send circle. `open_t` is the horizontal-
/// extend progress (0 = compact, 1 = fully extended), driven by the
/// panel's StateFades so a mid-morph toggle re-bases instead of snapping.
pub(super) fn render_usage_strip(
    pools: &[StripPool],
    session: Option<SharedString>,
    open_t: f32,
    on_toggle: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
    cx: &App,
) -> AnyElement {
    let colors = cx.theme().colors();
    let detail_w = (pools.len() as f32) * SEG_W * open_t;

    let segments: Vec<AnyElement> = pools
        .iter()
        .map(|pool| {
            let pct: SharedString = match pool.used {
                Some(used) => format!("{}%", used.round() as i64).into(),
                None => "—".into(),
            };
            h_flex()
                .flex_none()
                .items_center()
                .gap(px(5.))
                .child(div().flex_none().size(px(6.)).rounded_full().bg(pool.vendor))
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(colors.text_muted)
                        .child(pool.short.clone()),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(pool.tone.color())
                        .child(pct),
                )
                .into_any_element()
        })
        .collect();

    let dots: Vec<AnyElement> = pools
        .iter()
        .map(|pool| {
            div()
                .flex_none()
                .size(px(6.))
                .rounded_full()
                .bg(pool.vendor)
                .into_any_element()
        })
        .collect();

    h_flex()
        .id("composer-usage-cluster")
        .flex_none()
        .h(px(32.))
        .items_center()
        .gap(px(8.))
        .px(px(6.))
        .rounded(px(8.))
        .cursor_pointer()
        .hover(|s| s.bg(colors.element_hover))
        .on_click(on_toggle)
        .child(
            // The horizontal-extend window: clipped, right-justified inner
            // row — growing the clip reveals segments leftward out of the
            // dot cluster, into the controls row's spacer space.
            div().overflow_hidden().w(px(detail_w)).h_full().child(
                h_flex()
                    .h_full()
                    .items_center()
                    .justify_end()
                    .gap(px(14.))
                    .pr(px(6.))
                    .children(segments),
            ),
        )
        .child(h_flex().items_center().gap(px(5.)).children(dots))
        .children(session.map(|pct| {
            div()
                .text_size(px(12.))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(colors.text)
                .child(pct)
                .into_any_element()
        }))
        .into_any_element()
}
