//! The Usage panel (PARITY_SPEC §4.4) — the `#/usage` full page, ported from
//! the approved web reference render (`bridge/ui/usage.js` + `panels.css`):
//! a responsive `.pool-grid` of pool CARDS (name above a full-width 4px
//! meter, used-% + window row beneath, mono staleness line) under a
//! scrape-staleness banner when `_scraped.stale`. Data is the push-fed
//! `Entity<BridgeStore>` (usage rows arrive via SSE with a 12s polling
//! fallback — no panel-owned timers).
//!
//! Meters NEVER silently vanish — a `None` used-% degrades to the unknown
//! tone at full width (the Antigravity quota-opacity lesson). Meter motion
//! (600ms spatial fills, 300ms effects tone fades on band flips only)
//! lives in [`crate::usage_panel_meter`].

use std::collections::HashMap;

use gpui::{
    Animation, AnimationExt as _, AnyElement, App, Context, Entity, FocusHandle, Focusable,
    FontWeight, SharedString, Subscription, Window, actions,
};
use settings::Settings as _;
use theme_settings::ThemeSettings;
use ui::prelude::*;

use crate::agent_accents::{STATUS_BLOCKED, STATUS_ERROR, tone_for_used, used_pct};
use crate::bridge::{self, BridgeStore, PoolRow, UsageMeta};
use crate::task_board::motion::{EFFECTS, RollValue, StateFade};
use crate::task_board::style::{SURFACE_1, tabular_nums};
use crate::usage_panel_meter::{MeterState, render_meter};

actions!(
    usage_panel,
    [
        /// Toggles focus on the usage panel.
        ToggleFocus
    ]
);

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

pub struct UsagePanel {
    focus_handle: FocusHandle,
    store: Entity<BridgeStore>,
    /// Pool name → meter fill/tone animation state (persists across store
    /// ticks so fills morph in place — §5.6 keyed reconciliation).
    meters: HashMap<String, MeterState>,
    /// Pool name → used-% numeric roll (web `rollNumber` on `.pool-pct`):
    /// the % digits slide on a usage change, in lockstep with the meter
    /// fill. Persisted across ticks, pruned alongside `meters`.
    pct_rolls: HashMap<String, RollValue>,
    /// Bridge-offline grey-out (same native addition as the task board).
    was_connected: bool,
    connected_fade: StateFade,
    /// Snapshot gate for the store observer (§8 idle cost): the store
    /// notifies at 1Hz while a run ticks for elapsed counters this panel
    /// doesn't render — repaint only when the panel's actual inputs
    /// (usage rows, scrape meta, connectivity) change, the same discipline
    /// as the islands.
    last_seen: Option<(Vec<PoolRow>, UsageMeta, bool)>,
    _store_subscription: Subscription,
}

impl UsagePanel {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let store = bridge::global_store(cx);
        let _store_subscription = cx.observe(&store, |this: &mut Self, store, cx| {
            let store = store.read(cx);
            let snapshot = (store.usage.clone(), store.usage_meta.clone(), store.connected);
            if this.last_seen.as_ref() != Some(&snapshot) {
                this.last_seen = Some(snapshot);
                cx.notify();
            }
        });
        Self {
            focus_handle: cx.focus_handle(),
            store,
            meters: HashMap::new(),
            pct_rolls: HashMap::new(),
            was_connected: false,
            connected_fade: StateFade::default(),
            last_seen: None,
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
            // `.panel-head`: baseline-aligned, gap 12 (panels.css:12-15).
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

    /// The per-vendor liveness strip (2026-07-04 `liveness` map): "is <vendor>
    /// alive RIGHT NOW", so the reader can see a KNOWN-dead vendor before a
    /// call fails. One chip per vendor — a tone dot (green alive / amber
    /// rate_limited / red down) + name + age. Rendered only when the bridge
    /// surfaces liveness (empty → no strip, never a blank row).
    fn render_liveness(&self, meta: &UsageMeta, cx: &App) -> Option<Div> {
        if meta.liveness.is_empty() {
            return None;
        }
        let colors = cx.theme().colors();
        let mono = ThemeSettings::get_global(cx).buffer_font.family.clone();
        let chips: Vec<AnyElement> = meta
            .liveness
            .iter()
            .map(|vendor| {
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
                    .child(
                        div()
                            .size(px(6.))
                            .rounded_full()
                            .bg(dot)
                            .flex_none(),
                    )
                    .child(
                        div()
                            .font_family(mono.clone())
                            .text_size(px(12.))
                            .text_color(label_color)
                            .child(SharedString::from(format!(
                                "{} {}{age}",
                                vendor.vendor, liveness_word(&vendor.state)
                            ))),
                    )
                    .into_any_element()
            })
            .collect();
        Some(
            h_flex()
                .flex_none()
                .flex_wrap()
                .gap(px(8.))
                .px(px(28.))
                .py(px(12.))
                .border_b_1()
                .border_color(colors.border)
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(colors.text_placeholder)
                        .child("Vendor liveness"),
                )
                .children(chips),
        )
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

    /// One `.pool-card` (panels.css:49-68): name above a full-width meter,
    /// the used-% + window `.pool-row` beneath, and the mono
    /// reset/staleness `.pool-stale` line.
    fn render_pool_card(&mut self, pool: &PoolRow, meta: &UsageMeta, cx: &App) -> AnyElement {
        let colors = cx.theme().colors();
        let mono = ThemeSettings::get_global(cx).buffer_font.family.clone();
        let used = used_pct(pool.headroom_pct);
        let state = self
            .meters
            .entry(pool.name.clone())
            .or_insert_with(|| MeterState::new(tone_for_used(used)));
        state.update(used);
        let meter = render_meter(state, &pool.name);
        let window_label = pool.window.clone().unwrap_or_default();

        // `.pool-pct` 500 13px --text tabular; the unknown case takes the
        // `.meter-unknown` treatment instead (12px mono italic --text-3). The
        // % rolls its changed digits (web `rollNumber`) in lockstep with the
        // meter fill; " used" and "%" are seps. The roll state is keyed by
        // pool name so it survives store ticks (§5.6).
        let pct_cell = match used {
            Some(used) => {
                let roll = self
                    .pct_rolls
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

        // `.pool-stale` — always mono 11px --text-3 (the web keeps text-3
        // even while stale; the stale BANNER carries the warning tint).
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
            // The `.pool-grid` cell: auto-fill minmax(240px, 1fr) emulated
            // as wrap + grow from a 240px basis.
            .flex_grow()
            .flex_basis(px(240.))
            .rounded(px(12.))
            .bg(SURFACE_1)
            .border_1()
            .border_color(colors.border)
            .px(px(16.))
            .py(px(15.))
            .child(
                // `.pool-name` 500 14px --text, 12px below.
                div()
                    .mb(px(12.))
                    .text_size(px(14.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(colors.text)
                    .truncate()
                    .child(SharedString::from(pool_label(&pool.name).to_string())),
            )
            .child(meter)
            .child(
                // `.pool-row`: baseline-aligned pct vs cap, 10px below the
                // meter.
                h_flex()
                    .mt(px(10.))
                    .items_baseline()
                    .justify_between()
                    .gap(px(8.))
                    .child(pct_cell)
                    .child(
                        // `.pool-cap` 400 12px mono --text-3 tabular.
                        div()
                            .text_size(px(12.))
                            .font_family(mono.clone())
                            .font_features(tabular_nums())
                            .text_color(colors.text_placeholder)
                            .truncate()
                            .child(SharedString::from(window_label)),
                    ),
            )
            .child(
                div()
                    .mt(px(8.))
                    .text_size(px(11.))
                    .font_family(mono.clone())
                    .text_color(colors.text_placeholder)
                    .child(SharedString::from(status_line)),
            )
            // Call-count breakdown (2026-07-04 fields): real orchestrator
            // brain calls vs liveness/keepalive pings, so the pool's genuine
            // burn stays legible next to the ping-hack traffic. Rendered only
            // when the bridge surfaces either count for this pool.
            .children(self.render_pool_calls(pool, &mono, cx))
            // `vendor_429_observed` — the vendor returned a rate-limit 429 in
            // this window. A red marker: the pool hit a real vendor ceiling.
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
    /// bridge surfaces neither count for the pool (older bridge / pool without
    /// the routine ping hack), so no empty line renders.
    fn render_pool_calls(
        &self,
        pool: &PoolRow,
        mono: &gpui::SharedString,
        cx: &App,
    ) -> Option<Div> {
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Agentic-demo gate: stage believable pools + liveness so the whole
        // usage surface reviews without a live bridge. `connected = true` so
        // the demo never greys itself out. Falls through to the live store
        // when the gate is off. (crate::usage_panel_demo — this surface's own
        // demo module, decoded once per frame, no timers.)
        let (connected, pools, meta) = if crate::bridge::is_agentic_demo() {
            (
                true,
                crate::usage_panel_demo::demo_pools(),
                crate::usage_panel_demo::demo_meta(),
            )
        } else {
            let store = self.store.read(cx);
            (store.connected, store.usage.clone(), store.usage_meta.clone())
        };
        if connected != self.was_connected {
            self.was_connected = connected;
            self.connected_fade.bump();
        }
        // Prune meter state for pools gone from the feed.
        let live: std::collections::HashSet<&str> =
            pools.iter().map(|pool| pool.name.as_str()).collect();
        self.meters.retain(|name, _| live.contains(name.as_str()));
        self.pct_rolls.retain(|name, _| live.contains(name.as_str()));

        let body: AnyElement = if pools.is_empty() {
            crate::task_board::style::empty_state(
                IconName::SignalHigh,
                "No usage data yet",
                "Pool meters appear when the bridge reports usage.",
                cx,
            )
            .into_any_element()
        } else {
            let cards: Vec<AnyElement> = pools
                .iter()
                .map(|pool| self.render_pool_card(pool, &meta, cx))
                .collect();
            // `.usage-scroll` padding 20/28/32 wrapping the `.pool-grid`
            // (wrap + 14px gaps ≈ auto-fill minmax(240px, 1fr)).
            div()
                .id("usage-pools")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .px(px(28.))
                .pt(px(20.))
                .pb(px(32.))
                .child(h_flex().flex_wrap().gap(px(14.)).children(cards))
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

        // Frame pump for the used-% rolls (the meter fill already self-pumps
        // its own `with_animation`, but a roll-only frame — value changed,
        // fill already settled — still needs ticking). Settled frames
        // schedule nothing (§8 idle cost).
        if self.pct_rolls.values().any(RollValue::animating) {
            window.request_animation_frame();
        }

        let colors = cx.theme().colors();
        v_flex()
            .key_context("UsagePanel")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(colors.panel_background)
            .child(self.render_header(&meta, cx))
            .children(self.render_liveness(&meta, cx))
            .children(self.render_stale_banner(&meta))
            .child(body_container)
    }
}

impl Focusable for UsagePanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl crate::mode_item::ModeSurface for UsagePanel {
    // Center-pane surface (Amendment 2026-07-04 (2)) — the dock `Panel`
    // era is retired. Push-fed via the shared store; no visibility-gated
    // watch, so the default `set_surface_active` no-op is correct.
    fn fallback_tab_title() -> SharedString {
        "Usage".into()
    }

    fn fallback_tab_icon() -> IconName {
        IconName::Sliders
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

}
