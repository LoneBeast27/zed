//! The Usage panel (PARITY_SPEC §4.4) — the `#/usage` full page, ported from
//! the approved web reference render (`bridge/ui/usage.js` + `panels.css`):
//! pool meter rows (name · thin tone-colored fill · used-% in tabular nums ·
//! window label · reset/staleness line) under a scrape-staleness banner when
//! `_scraped.stale`. Data is the push-fed `Entity<BridgeStore>` (usage rows
//! arrive via SSE with a 12s polling fallback — no panel-owned timers).
//!
//! Meters NEVER silently vanish — a `None` used-% degrades to the unknown
//! tone at full width (the Antigravity quota-opacity lesson). Fill widths
//! animate value changes on the spatial curve (500ms) via the retargetable
//! [`AnimatedValue`]; tone flips crossfade 200ms on effects (§8.7: no
//! property snaps).

use std::collections::HashMap;

use gpui::{
    Action, Animation, AnimationExt as _, AnyElement, App, Context, Entity, EventEmitter,
    FocusHandle, Focusable, FontWeight, SharedString, Subscription, Window, actions, relative,
};
use ui::prelude::*;
use workspace::dock::{DockPosition, Panel, PanelEvent};

use crate::agent_accents::{STATUS_BLOCKED, Tone, tone_for_used, used_pct};
use crate::bridge::{self, BridgeStore, PoolRow, UsageMeta};
use crate::task_board::motion::{AnimatedColor, AnimatedValue, EFFECTS, SPATIAL, StateFade};
use crate::task_board::style::tabular_nums;

actions!(
    usage_panel,
    [
        /// Toggles focus on the usage panel.
        ToggleFocus
    ]
);

/// Meter-fill morph duration — width changes ride the spatial curve
/// (PARITY_SPEC §4.9 geometry class).
const FILL_MORPH: std::time::Duration = std::time::Duration::from_millis(500);
/// Meter tone crossfade (effects class) — band flips only; width-only
/// changes hold the color steady (web: `background .3s` transitions only
/// when the band class actually swaps).
const TONE_FADE: std::time::Duration = std::time::Duration::from_millis(200);

/// `POOL_LABELS` from usage.js — display names for the known pools;
/// unknown pools fall back to their raw key.
fn pool_label(name: &str) -> &str {
    match name {
        "claude_sdk_credit" => "Claude SDK credit",
        "codex_plan" => "Codex plan",
        "antigravity_weekly" => "Antigravity weekly",
        "gemini_free_rpd" => "Gemini free RPD",
        other => other,
    }
}

/// Per-pool meter animation state: the retargetable width fill plus the
/// retargetable tone-color crossfade. Both are Instant-clocked, so a
/// width-only retarget restarting the shared animation wrapper can never
/// replay the previous band color (§8.7a — the tone holds steady unless
/// the band actually flips, and a flip crossfades from the color rendered
/// right now).
struct MeterState {
    width: AnimatedValue,
    tone: Tone,
    color: AnimatedColor,
}

impl MeterState {
    /// New pools fill from 0 to their value on first sight (the web's
    /// `width:0%` skeleton + first `updatePool`).
    fn new(tone: Tone) -> Self {
        Self {
            width: AnimatedValue::settled(0.0, SPATIAL, FILL_MORPH),
            tone,
            color: AnimatedColor::settled(tone.color(), EFFECTS, TONE_FADE),
        }
    }

    fn update(&mut self, used: Option<f64>) {
        // Unknown degrades to a full-width dim fill, never an empty bar.
        let target = used.unwrap_or(100.0) as f32;
        self.width.retarget(target);
        self.tone = tone_for_used(used);
        // Same-target retargets are no-ops: only a real band flip opens a
        // crossfade, and it re-bases from the interpolated current color.
        self.color.retarget(self.tone.color());
    }
}

pub struct UsagePanel {
    focus_handle: FocusHandle,
    store: Entity<BridgeStore>,
    position: DockPosition,
    /// Pool name → meter fill/tone animation state (persists across store
    /// ticks so fills morph in place — §5.6 keyed reconciliation).
    meters: HashMap<String, MeterState>,
    /// Bridge-offline grey-out (same native addition as the task board).
    was_connected: bool,
    connected_fade: StateFade,
    _store_subscription: Subscription,
}

impl UsagePanel {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let store = bridge::global_store(cx);
        let _store_subscription = cx.observe(&store, |_, _, cx| cx.notify());
        Self {
            focus_handle: cx.focus_handle(),
            store,
            position: DockPosition::Left,
            meters: HashMap::new(),
            was_connected: false,
            connected_fade: StateFade::default(),
            _store_subscription,
        }
    }

    /// `.panel-head`: "Usage" 18px/500 + the `_source` sub-line with scrape
    /// freshness appended (usage.js `renderScraped`).
    fn render_header(&self, meta: &UsageMeta, cx: &App) -> Div {
        let colors = cx.theme().colors();
        let mut sub = meta.source.clone().unwrap_or_default();
        if let Some(scraped) = &meta.scraped {
            if scraped.stale {
                if let Some(age_h) = scraped.age_h {
                    sub.push_str(&format!("  ·  scrape {age_h}h old (stale)"));
                }
            } else if let Some(age_min) = scraped.age_min {
                sub.push_str(&format!("  ·  scraped {age_min}m ago"));
            }
        }
        h_flex()
            .flex_none()
            .items_center()
            .gap(px(14.))
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
                    .child("Usage"),
            )
            .when(!sub.is_empty(), |this| {
                this.child(
                    div()
                        .text_size(px(13.))
                        .text_color(colors.text_placeholder)
                        .child(SharedString::from(sub)),
                )
            })
    }

    /// The scrape-staleness banner — shown only while `_scraped.stale`;
    /// scraped pools degrade to last-known values beneath it (§4.4).
    fn render_stale_banner(&self, meta: &UsageMeta) -> Option<Div> {
        let scraped = meta.scraped.as_ref().filter(|scraped| scraped.stale)?;
        let age = scraped
            .age_h
            .map(|age_h| format!(" ({age_h}h old)"))
            .unwrap_or_default();
        Some(
            h_flex()
                .flex_none()
                .px(px(28.))
                .py(px(8.))
                .bg(gpui::Hsla::from(STATUS_BLOCKED).opacity(0.10))
                .text_size(px(13.))
                .text_color(STATUS_BLOCKED)
                .child(SharedString::from(format!(
                    "Usage scrape went stale{age} — scraped pools show last-known values"
                ))),
        )
    }

    /// One pool row: name · meter · used-% (tabular nums) · window label,
    /// with the reset/staleness line beneath.
    fn render_pool_row(&mut self, pool: &PoolRow, meta: &UsageMeta, cx: &App) -> AnyElement {
        let colors = cx.theme().colors();
        let used = used_pct(pool.headroom_pct);
        let meter = self.render_meter(pool, used);

        let pct_label = match used {
            None => "unknown / stale".to_string(),
            Some(used) => format!("{}% used", fmt_pct(used)),
        };
        let window_label = pool.window.clone().unwrap_or_default();

        let scraped = meta.scraped.as_ref();
        let (status_line, status_color) = match scraped {
            Some(scrape) if scrape.stale => (
                format!(
                    "scraped {}h ago (stale)",
                    scrape.age_h.map(fmt_pct).unwrap_or_else(|| "?".into())
                ),
                STATUS_BLOCKED.into(),
            ),
            Some(scrape) => match &scrape.reset_phrase {
                Some(phrase) => (format!("resets {phrase}"), colors.text_placeholder),
                None => ("self-metered".to_string(), colors.text_placeholder),
            },
            None => ("self-metered".to_string(), colors.text_placeholder),
        };

        v_flex()
            .py(px(14.))
            .gap(px(6.))
            .border_b_1()
            .border_color(colors.border)
            .child(
                h_flex()
                    .items_center()
                    .gap(px(12.))
                    .child(
                        div()
                            .w(px(180.))
                            .flex_none()
                            .text_size(px(14.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(colors.text)
                            .truncate()
                            .child(SharedString::from(pool_label(&pool.name).to_string())),
                    )
                    .child(meter)
                    .child(
                        div()
                            .w(px(110.))
                            .flex_none()
                            .text_size(px(13.))
                            .font_features(tabular_nums())
                            .text_color(colors.text_muted)
                            .text_right()
                            .child(SharedString::from(pct_label)),
                    )
                    .child(
                        div()
                            .w(px(56.))
                            .flex_none()
                            .text_size(px(12.))
                            .text_color(colors.text_placeholder)
                            .truncate()
                            .child(SharedString::from(window_label)),
                    ),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(status_color)
                    .child(SharedString::from(status_line)),
            )
            .into_any_element()
    }

    /// The thin meter: tone-colored fill whose width morphs on the spatial
    /// curve (500ms) while its color crossfades on effects (200ms, band
    /// flips only) — ONE animation wrapper carrying both property classes
    /// so they are concurrent from the first frame (§8.7b). The wrapper is
    /// a frame pump over Instant-clocked values: a width-only retarget
    /// restarting it can never replay the previous band color.
    fn render_meter(&mut self, pool: &PoolRow, used: Option<f64>) -> AnyElement {
        let state = self
            .meters
            .entry(pool.name.clone())
            .or_insert_with(|| MeterState::new(tone_for_used(used)));
        state.update(used);

        let dim = state.tone == Tone::Unknown;

        let fill = div()
            .h_full()
            .rounded_full()
            .bg(state.color.target())
            .when(dim, |fill| fill.opacity(0.35));

        let track = div()
            .flex_1()
            .h(px(6.))
            .rounded_full()
            .bg(gpui::white().opacity(0.10))
            .overflow_hidden();

        if state.width.animating() || state.color.animating() {
            let value = state.width.clone();
            let color = state.color.clone();
            // Animation identity covers both the width retarget and the
            // tone flip — either bumps the key, extending the pump window.
            let generation = (state.width.generation() as u64) << 16
                | (state.color.generation() as u64 & 0xffff);
            track
                .child(fill.with_animation(
                    ElementId::NamedInteger(
                        format!("meter-fill-{}", pool.name).into(),
                        generation,
                    ),
                    // Frame pump: SPATIAL is mapped inside `current()` (the
                    // animator-closure rule for overshooting curves —
                    // motion GUARD). Overshoot past the track clips on
                    // overflow_hidden: the visible spring settle.
                    Animation::new(FILL_MORPH),
                    move |fill, _| {
                        fill.w(relative(value.current().max(0.0) / 100.0))
                            .bg(color.current())
                    },
                ))
                .into_any_element()
        } else {
            track
                .child(fill.w(relative(state.width.target().max(0.0) / 100.0)))
                .into_any_element()
        }
    }
}

/// JS-style number formatting: integers print bare ("82"), fractions keep
/// one decimal ("99.5") — matches the web's template-literal output.
/// Shared with the usage island (same `${used}%` rendering).
pub(crate) fn fmt_pct(value: f64) -> String {
    if (value - value.round()).abs() < 0.05 {
        format!("{}", value.round() as i64)
    } else {
        format!("{value:.1}")
    }
}

impl Render for UsagePanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let store = self.store.read(cx);
        let connected = store.connected;
        let pools = store.usage.clone();
        let meta = store.usage_meta.clone();
        if connected != self.was_connected {
            self.was_connected = connected;
            self.connected_fade.bump();
        }
        // Prune meter state for pools gone from the feed.
        let live: std::collections::HashSet<&str> =
            pools.iter().map(|pool| pool.name.as_str()).collect();
        self.meters.retain(|name, _| live.contains(name.as_str()));

        let body: AnyElement = if pools.is_empty() {
            crate::task_board::style::empty_state(
                IconName::SignalHigh,
                "No usage data yet",
                "Pool meters appear when the bridge reports usage.",
                cx,
            )
            .into_any_element()
        } else {
            let rows: Vec<AnyElement> = pools
                .iter()
                .map(|pool| self.render_pool_row(pool, &meta, cx))
                .collect();
            div()
                .id("usage-pools")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .px(px(28.))
                .child(v_flex().children(rows))
                .into_any_element()
        };

        // Bridge offline → grey out the kept snapshot, eased 200ms both
        // directions (same §8.7a treatment as the task board).
        let body_container = div().relative().flex_1().min_h_0().child(body);
        let body_container: AnyElement = if self.connected_fade.fresh() {
            let (from, to) = if connected { (0.5, 1.0) } else { (1.0, 0.5) };
            body_container
                .with_animation(
                    ElementId::NamedInteger(
                        "usage-offline-fade".into(),
                        self.connected_fade.generation() as u64,
                    ),
                    Animation::new(std::time::Duration::from_millis(200))
                        .with_easing(EFFECTS.easing()),
                    move |body, t| body.opacity(from + (to - from) * t),
                )
                .into_any_element()
        } else {
            body_container
                .when(!connected, |this| this.opacity(0.5))
                .into_any_element()
        };

        let colors = cx.theme().colors();
        v_flex()
            .key_context("UsagePanel")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(colors.panel_background)
            .child(self.render_header(&meta, cx))
            .children(self.render_stale_banner(&meta))
            .child(body_container)
    }
}

impl Focusable for UsagePanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<PanelEvent> for UsagePanel {}

impl Panel for UsagePanel {
    fn persistent_name() -> &'static str {
        "UsagePanel"
    }

    fn panel_key() -> &'static str {
        "UsagePanel"
    }

    fn position(&self, _window: &Window, _cx: &App) -> DockPosition {
        self.position
    }

    fn position_is_valid(&self, position: DockPosition) -> bool {
        matches!(position, DockPosition::Left | DockPosition::Right)
    }

    fn set_position(&mut self, position: DockPosition, _: &mut Window, cx: &mut Context<Self>) {
        // Runtime-only, mirroring the task board (Z1).
        self.position = position;
        cx.notify();
    }

    fn default_size(&self, _window: &Window, _cx: &App) -> Pixels {
        px(480.)
    }

    fn icon(&self, _window: &Window, _cx: &App) -> Option<IconName> {
        Some(IconName::SignalHigh)
    }

    fn icon_tooltip(&self, _window: &Window, _cx: &App) -> Option<&'static str> {
        Some("Usage")
    }

    fn toggle_action(&self) -> Box<dyn Action> {
        Box::new(ToggleFocus)
    }

    fn activation_priority(&self) -> u32 {
        6
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pool_labels_match_usage_js() {
        assert_eq!(pool_label("claude_sdk_credit"), "Claude SDK credit");
        assert_eq!(pool_label("codex_plan"), "Codex plan");
        assert_eq!(pool_label("antigravity_weekly"), "Antigravity weekly");
        assert_eq!(pool_label("gemini_free_rpd"), "Gemini free RPD");
        assert_eq!(pool_label("future_pool"), "future_pool");
    }

    #[test]
    fn fmt_pct_prints_like_js() {
        assert_eq!(fmt_pct(82.0), "82");
        assert_eq!(fmt_pct(99.5), "99.5");
        assert_eq!(fmt_pct(0.0), "0");
        assert_eq!(fmt_pct(100.0), "100");
    }

    #[test]
    fn meter_state_animates_value_changes_and_tone_flips() {
        let mut state = MeterState::new(tone_for_used(Some(40.0)));
        state.update(Some(40.0));
        // First sight: fills 0 → 40 (the web's width:0% skeleton).
        assert!(state.width.animating());
        assert_eq!(state.width.target(), 40.0);
        assert_eq!(state.tone, Tone::Ok);
        assert_eq!(state.color.generation(), 0, "same tone — no crossfade");

        // Crossing into warn: width retargets AND the tone crossfades.
        state.update(Some(80.0));
        assert_eq!(state.width.target(), 80.0);
        assert_eq!(state.tone, Tone::Warn);
        assert_eq!(state.color.generation(), 1);
        assert_eq!(state.color.target(), Tone::Warn.color());

        // Unknown degrades to a full-width fill, never an empty bar.
        state.update(None);
        assert_eq!(state.width.target(), 100.0);
        assert_eq!(state.tone, Tone::Unknown);
    }

    #[test]
    fn width_only_changes_hold_the_band_color() {
        // The previous-band flash: after Ok→Warn, every later width-only
        // retarget restarted the color animation from the OLD band color.
        // The tone must hold steady unless the band actually flips (web:
        // `background .3s` transitions only on a class swap).
        let mut state = MeterState::new(tone_for_used(Some(40.0)));
        state.update(Some(40.0));
        state.update(Some(80.0)); // Ok → Warn flip
        let flip_generation = state.color.generation();
        assert_eq!(flip_generation, 1);

        // Width-only changes inside the warn band: the color crossfade is
        // NEVER restarted (same-target retargets are no-ops).
        for used in [81.0, 79.5, 85.0, 89.9] {
            state.update(Some(used));
            assert_eq!(
                state.color.generation(),
                flip_generation,
                "width-only change at {used}% must not restart the tone fade"
            );
            assert_eq!(state.color.target(), Tone::Warn.color());
        }

        // The next REAL flip crossfades again.
        state.update(Some(95.0));
        assert_eq!(state.color.generation(), flip_generation + 1);
        assert_eq!(state.color.target(), Tone::Crit.color());
    }
}
