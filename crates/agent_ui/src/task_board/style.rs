//! The board's shared visual vocabulary (PARITY_SPEC §4.2): out-of-chrome
//! surface tokens, status labels/phrases, relative-time formatting, and the
//! status-pill / agent-chip builders shared by inbox rows, graph nodes, and
//! the run drawer. Mirrors the web's `app.css` `.pill`/`.agent-chip` blocks +
//! `app.js` `rel()`/`chipReason()` helpers 1:1.

use gpui::{
    Animation, AnimationExt as _, ElementId, FontFeatures, FontWeight, Rgba, SharedString,
    Stateful,
};
use settings::Settings as _;
use theme_settings::ThemeSettings;
use ui::prelude::*;

use crate::agent_accents::{accent_for_agent, color_for_status, rgba_hex};

use super::motion::EFFECTS;

/// `.pill-elapsed` opacity reveal duration (board.css:48 `opacity .12s
/// var(--effects-curve)`).
const ELAPSED_REVEAL: std::time::Duration = std::time::Duration::from_millis(120);

// ── Out-of-chrome board tokens (web app.css values; deliberately NOT
//    ThemeColors fields — same rule as agent_accents) ──
/// `--surface-1: rgba(255,255,255,0.05)` — idle-pill fill, seg-toggle active.
pub(crate) const SURFACE_1: Rgba = rgba_hex(0xffffff0d);
/// `--surface-2: #141414` — graph node fill.
pub(crate) const SURFACE_2: Rgba = rgba_hex(0x141414ff);
/// `--surface-2b: #0c0c0c` — drawer `<pre>` fill.
pub(crate) const SURFACE_2B: Rgba = rgba_hex(0x0c0c0cff);
/// `--hairline-hi: rgba(255,255,255,0.12)` — graph node borders, edges.
pub(crate) const HAIRLINE_HI: Rgba = rgba_hex(0xffffff1f);

/// `font-variant-numeric: tabular-nums` (PARITY_SPEC §0: tabular-nums on
/// counters everywhere) — fixed-width digits so ticking values never reflow.
/// Mono surfaces are inherently tabular; this is for UI-font counters (the
/// pill elapsed, the drawer worked-for value, future Z2/Z4 numeric rolls).
pub fn tabular_nums() -> FontFeatures {
    FontFeatures(std::sync::Arc::new(vec![("tnum".to_string(), 1)]))
}

/// `rel()` from app.js — compact relative duration ("34s", "5m", "1.2h").
pub fn rel(seconds: f64) -> String {
    let s = seconds.max(0.0);
    if s < 90.0 {
        format!("{}s", s.round() as i64)
    } else if s < 5400.0 {
        format!("{}m", (s / 60.0).round() as i64)
    } else {
        format!("{:.1}h", s / 3600.0)
    }
}

/// `ago()` from app.js — relative wall-clock age for a unix-seconds ts
/// ("now", "5m", "3h", "2d"). Shared by the chat meta row + sidebar rows.
pub fn ago(ts: f64, now_unix: f64) -> String {
    let d = now_unix - ts;
    if d < 120.0 {
        "now".to_string()
    } else if d < 5400.0 {
        format!("{}m", (d / 60.0).round() as i64)
    } else if d < 90000.0 {
        format!("{}h", (d / 3600.0).round() as i64)
    } else {
        format!("{}d", (d / 86400.0).round() as i64)
    }
}

/// [`ago`] against the current system clock.
pub fn ago_now(ts: f64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|epoch| epoch.as_secs_f64())
        .unwrap_or(0.0);
    ago(ts, now)
}

/// `statusLabel()` from board.js — pill text (unknown statuses fall back to
/// the raw status string, like the web).
pub fn status_label(status: &str) -> String {
    match status {
        "running" => "Running".to_string(),
        "completed" => "Done".to_string(),
        "failed" => "Blocked".to_string(),
        "killed" => "Killed".to_string(),
        "pending" | "idle" => "Idle".to_string(),
        other => other.to_string(),
    }
}

/// `statusPhrase()` from graph.js — the Antigravity progress note
/// ("Working… · 34s").
pub fn status_phrase(status: &str, elapsed_s: f64) -> String {
    let t = rel(elapsed_s);
    match status {
        "running" => format!("Working… · {t}"),
        "completed" => format!("Done · {t}"),
        "failed" => format!("Blocked · {t}"),
        "killed" => "Killed".to_string(),
        "pending" | "idle" => "Queued".to_string(),
        other => other.to_string(),
    }
}

/// `routeReason()` from drawer.js — the drawer meta line's second chip
/// segment, WITHOUT the forced-@agent rewrite (the drawer runs the raw
/// vocabulary), falling back to "routed".
pub fn route_reason(chip: Option<&str>) -> String {
    let reason = chip
        .unwrap_or_default()
        .split('·')
        .map(str::trim)
        .nth(1)
        .unwrap_or("");
    if reason.is_empty() {
        "routed".to_string()
    } else {
        reason.to_string()
    }
}

/// `chipReason()` from app.js — routing chip is `agent · reason · conf%`;
/// "forced target"/"user override" reasons rewrite to `forced @agent`.
pub fn chip_reason(chip: Option<&str>, agent: &str) -> String {
    let segs: Vec<&str> = chip
        .unwrap_or_default()
        .split('·')
        .map(str::trim)
        .collect();
    let reason = segs.get(1).copied().unwrap_or("");
    let lower = reason.to_ascii_lowercase();
    if lower.contains("forced target") || lower.contains("user override") {
        format!("forced @{agent}")
    } else {
        reason.to_string()
    }
}

/// The `.pill-elapsed` reveal state — the elapsed segment lives inside the
/// pill and reveals on tracked row hover (the web's compact↔extended island
/// morph, board.css:45-55). At rest the segment is not rendered at all, so
/// resting geometry matches the web's `max-width: 0` collapsed pill; on
/// hover it appears at full width with a one-shot 120ms effects opacity
/// fade. (The width-spring morph itself lands with Z4's hover-retargetable
/// spring layer.)
pub struct ElapsedReveal {
    /// `rel(elapsed_s)` text.
    pub text: String,
    /// Stable per-row key (run id) — animation identity.
    pub id: SharedString,
    /// Whether the owning row is hovered right now.
    pub hovered: bool,
    /// Whether the hover flip is inside the crossfade window (attach the
    /// one-shot fade only then; settled hovers render the segment bare).
    pub fresh: bool,
}

/// The `.pill` element with the inbox vocabulary (`statusLabel()`:
/// "Running"/"Done"/…). 11px/500, full-round, 12%-tinted fill.
pub fn status_pill(
    id: impl Into<ElementId>,
    status: &str,
    elapsed: Option<ElapsedReveal>,
    cx: &App,
) -> Stateful<Div> {
    pill_with_label(id, status, status_label(status), elapsed, cx)
}

/// The drawer's pill: RAW lowercase status text (drawer.js:37-38 — the web
/// deliberately runs two vocabularies, capitalized inbox labels vs raw
/// drawer status).
pub fn raw_status_pill(id: impl Into<ElementId>, status: &str, cx: &App) -> Stateful<Div> {
    pill_with_label(id, status, status.to_string(), None, cx)
}

/// Shared `.pill` builder: status-colored label (+ pulsing dot while
/// running, + optional elapsed segment).
fn pill_with_label(
    id: impl Into<ElementId>,
    status: &str,
    label: String,
    elapsed: Option<ElapsedReveal>,
    cx: &App,
) -> Stateful<Div> {
    let color = color_for_status(status);
    let bg = match status {
        "pending" | "idle" => SURFACE_1.into(),
        _ => color.opacity(0.12),
    };
    let dot = (status == "running").then(|| {
        let dot = div().size(px(6.)).rounded_full().bg(color);
        // `.pill.running::before` pulse-dot: opacity .5 → 1 → .5, 1.2s loop.
        dot.with_animation(
            ElementId::NamedInteger("pill-pulse".into(), 0),
            Animation::new(std::time::Duration::from_millis(1200))
                .repeat()
                .with_easing(gpui::pulsating_between(0.5, 1.0)),
            |dot, value| dot.opacity(value),
        )
        .into_any_element()
    });
    let _ = cx; // kept for parity with sibling builders (theme-driven later)
    h_flex()
        .id(id)
        .flex_none()
        .items_center()
        .justify_center()
        .gap(px(6.))
        .px(px(10.))
        .py(px(5.))
        .rounded_full()
        .bg(bg)
        .text_size(px(11.))
        .font_weight(FontWeight::MEDIUM)
        // The elapsed segment ticks every second (board.css:47 tabular-nums).
        .font_features(tabular_nums())
        .text_color(color)
        .children(dot)
        .child(SharedString::from(label))
        .when_some(elapsed, |this, reveal| {
            // `.pill-elapsed` — collapsed (absent) at rest, revealed on
            // tracked row hover with a 120ms effects opacity fade.
            if !reveal.hovered {
                return this;
            }
            let segment = div()
                .text_color(color.opacity(0.85))
                .child(SharedString::from(reveal.text));
            if reveal.fresh {
                this.child(segment.with_animation(
                    ElementId::Name(format!("pill-elapsed-{}", reveal.id).into()),
                    Animation::new(ELAPSED_REVEAL).with_easing(EFFECTS.easing()),
                    |segment, t| segment.opacity(t),
                ))
            } else {
                this.child(segment)
            }
        })
}

/// The shared `.empty-state` block (icon · headline · copy), centered.
pub fn empty_state(
    icon: IconName,
    headline: &'static str,
    copy: &'static str,
    cx: &App,
) -> impl IntoElement {
    let colors = cx.theme().colors();
    v_flex()
        .size_full()
        .items_center()
        .justify_center()
        .text_center()
        .gap(px(10.))
        .p(px(40.))
        // 34px text-3 at 0.8 opacity (app.css:268 `.empty-state .ms`) — the
        // glyph recedes behind the headline on #000, never dominates it.
        .child(
            Icon::new(icon)
                .size(IconSize::Custom(rems_from_px(34.)))
                .color(Color::Custom(colors.text_placeholder.opacity(0.8))),
        )
        .child(
            div()
                .text_size(px(20.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(colors.text_muted)
                .child(headline),
        )
        .child(
            div()
                .max_w(px(380.))
                .text_size(px(13.))
                .text_color(colors.text_placeholder)
                .child(copy),
        )
}

/// The `.agent-chip` element: vendor-accent 7px dot + agent name, mono 12px
/// on a `#1a1a1a` rounded fill.
pub fn agent_chip(agent: &str, cx: &App) -> Div {
    let accent = accent_for_agent(agent);
    let colors = cx.theme().colors();
    let mono = ThemeSettings::get_global(cx).buffer_font.family.clone();
    h_flex()
        .flex_none()
        .items_center()
        .gap(px(6.))
        .px(px(8.))
        .py(px(3.))
        .rounded(px(8.))
        .bg(rgba_hex(0x1a1a1aff))
        .font_family(mono)
        .text_size(px(12.))
        .text_color(colors.text_muted)
        .child(div().size(px(7.)).rounded_full().bg(accent))
        .child(SharedString::from(agent.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rel_matches_web_breakpoints() {
        assert_eq!(rel(0.0), "0s");
        assert_eq!(rel(34.4), "34s");
        assert_eq!(rel(89.0), "89s");
        assert_eq!(rel(90.0), "2m"); // 1.5min rounds up, same as Math.round
        assert_eq!(rel(900.0), "15m");
        assert_eq!(rel(5400.0), "1.5h");
        assert_eq!(rel(-5.0), "0s");
    }

    #[test]
    fn ago_matches_app_js_breakpoints() {
        let now = 1_781_113_180.0;
        assert_eq!(ago(now - 30.0, now), "now");
        assert_eq!(ago(now - 119.0, now), "now");
        assert_eq!(ago(now - 120.0, now), "2m");
        assert_eq!(ago(now - 3600.0, now), "60m");
        assert_eq!(ago(now - 5400.0, now), "2h"); // 1.5h rounds up like JS
        assert_eq!(ago(now - 86400.0, now), "24h");
        assert_eq!(ago(now - 90000.0, now), "1d");
        assert_eq!(ago(now - 200000.0, now), "2d");
    }

    #[test]
    fn status_vocabulary_matches_board_js() {
        assert_eq!(status_label("running"), "Running");
        assert_eq!(status_label("completed"), "Done");
        assert_eq!(status_label("failed"), "Blocked");
        assert_eq!(status_label("killed"), "Killed");
        assert_eq!(status_label("pending"), "Idle");
        assert_eq!(status_label("idle"), "Idle");

        assert_eq!(status_phrase("running", 34.0), "Working… · 34s");
        assert_eq!(status_phrase("completed", 300.0), "Done · 5m");
        assert_eq!(status_phrase("failed", 10.0), "Blocked · 10s");
        assert_eq!(status_phrase("killed", 10.0), "Killed");
        assert_eq!(status_phrase("pending", 0.0), "Queued");
    }

    #[test]
    fn route_reason_extracts_without_rewrite() {
        // drawer.js: `segs[1] || "routed"` — NO forced-@agent rewrite.
        assert_eq!(
            route_reason(Some("claude · code-heavy task · 92%")),
            "code-heavy task"
        );
        assert_eq!(
            route_reason(Some("codex · Forced target · 100%")),
            "Forced target"
        );
        assert_eq!(route_reason(Some("gemini · user override")), "user override");
        assert_eq!(route_reason(None), "routed");
        assert_eq!(route_reason(Some("solo")), "routed");
    }

    #[test]
    fn chip_reason_extracts_and_rewrites() {
        assert_eq!(
            chip_reason(Some("claude · code-heavy task · 92%"), "claude"),
            "code-heavy task"
        );
        assert_eq!(
            chip_reason(Some("codex · Forced target · 100%"), "codex"),
            "forced @codex"
        );
        assert_eq!(
            chip_reason(Some("gemini · user override"), "gemini"),
            "forced @gemini"
        );
        assert_eq!(chip_reason(None, "claude"), "");
        assert_eq!(chip_reason(Some("solo"), "claude"), "");
    }
}
