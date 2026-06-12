//! The Usage panel (PARITY_SPEC §4.4) — the `#/usage` full page, ported from
//! the approved web reference render (`bridge/ui/usage.js` + `panels.css`):
//! a responsive `.pool-grid` of pool CARDS (name above a full-width 4px
//! meter, used-% + window row beneath, mono staleness line) under a
//! scrape-staleness banner when `_scraped.stale`. Data is the push-fed
//! `Entity<BridgeStore>` (usage rows arrive via SSE with a 12s polling
//! fallback — no panel-owned timers).
//!
//! Meters NEVER silently vanish — a `None` used-% degrades to the unknown
//! tone at full width (the Antigravity quota-opacity lesson). Fill widths
//! animate value changes on the spatial curve (500ms) via the retargetable
//! [`AnimatedValue`]; tone flips crossfade 200ms on effects (§8.7: no
//! property snaps).

use std::collections::HashMap;

use gpui::{
    Action, Animation, AnimationExt as _, AnyElement, App, Context, Entity, EventEmitter,
    FocusHandle, Focusable, FontWeight, SharedString, Subscription, Window, actions,
};
use settings::Settings as _;
use theme_settings::ThemeSettings;
use ui::prelude::*;
use workspace::dock::{DockPosition, Panel, PanelEvent};

use crate::agent_accents::{STATUS_BLOCKED, tone_for_used, used_pct};
use crate::bridge::{self, BridgeStore, PoolRow, UsageMeta};
use crate::task_board::motion::{EFFECTS, StateFade};
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
        // `.meter-unknown` treatment instead (12px mono italic --text-3).
        let pct_cell = match used {
            Some(used) => div()
                .text_size(px(13.))
                .font_weight(FontWeight::MEDIUM)
                .font_features(tabular_nums())
                .text_color(colors.text)
                .child(SharedString::from(format!("{}% used", fmt_pct(used)))),
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
                    .font_family(mono)
                    .text_color(colors.text_placeholder)
                    .child(SharedString::from(status_line)),
            )
            .into_any_element()
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

}
