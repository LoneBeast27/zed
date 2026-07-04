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
use ui::prelude::*;

use crate::agent_accents::STATUS_BLOCKED;
use crate::bridge::{self, BridgeStore, PoolRow, UsageMeta};
use crate::task_board::motion::{EFFECTS, RollValue, StateFade};
use crate::usage_panel_groups::group_pools;
use crate::usage_panel_meter::MeterState;
use crate::usage_panel_render::render_vendor_cluster;

actions!(
    usage_panel,
    [
        /// Toggles focus on the usage panel.
        ToggleFocus
    ]
);

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
            // Fold the flat pool rows into vendor clusters ONCE per frame
            // (Amendment 2026-07-04 (4) item 1; I/O-first — a render-side fold
            // over the already-typed payload, no new fetch, no per-pool
            // re-derivation). One card per vendor, stacked top-to-bottom.
            let groups = group_pools(&pools, &meta);
            let clusters: Vec<AnyElement> = groups
                .iter()
                .map(|group| {
                    render_vendor_cluster(
                        group,
                        &meta,
                        &mut self.meters,
                        &mut self.pct_rolls,
                        cx,
                    )
                })
                .collect();
            // `.usage-scroll` padding 20/28/32; the clusters live in a bounded,
            // centered column (the surface-centering discipline — usage's grid
            // is now a vendor stack, so it reads as a centered column like the
            // other converted surfaces).
            div()
                .id("usage-pools")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .px(px(28.))
                .pt(px(20.))
                .pb(px(32.))
                .child(
                    h_flex().w_full().justify_center().child(
                        v_flex()
                            .w_full()
                            .max_w(px(720.))
                            .gap(px(16.))
                            .children(clusters),
                    ),
                )
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
