//! Staged usage data for the usage panel + island under the shared
//! agentic-demo gate ([`crate::bridge::is_agentic_demo`]). NOT a product
//! surface: believable pools spanning every meter band (ok / warn / crit /
//! unknown), a live `liveness` strip, a `vendor_429_observed` pool, and the
//! 2026-07-04 call-count fields — so the whole usage surface (panel + corner
//! island) can be reviewed without a live bridge. Decoded once (a plain
//! builder, no timers — the panel/island pump their own frames).

use crate::bridge::{PoolRow, ScrapeMeta, UsageMeta, VendorLiveness};

/// Staged six-pool set spanning the four vendor clusters (Amendment
/// 2026-07-04 (4) item 1): the CLAUDE cluster carries all three pools —
/// 5h (warn), weekly all-models (ok), weekly Sonnet (unknown/stale, degrades
/// never vanishes) — deliberately out of 5h-first order so the grouping's
/// intra-cluster sort is exercised; codex (ok + a 429); antigravity (crit);
/// gemini (0% used, carrying the new call-count fields + a liveness header).
pub fn demo_pools() -> Vec<PoolRow> {
    vec![
        // Claude weekly buckets appear BEFORE 5h on the wire → the grouping
        // must reorder them so 5h (the session pool) leads its cluster.
        PoolRow {
            name: "claude_weekly".into(),
            headroom_pct: Some(97.), // 3% used → ok band
            window: Some("week".into()),
            ..Default::default()
        },
        PoolRow {
            name: "claude_weekly_sonnet".into(),
            headroom_pct: None, // unknown/stale → degrades, never vanishes
            window: Some("week".into()),
            ..Default::default()
        },
        PoolRow {
            name: "claude_5h".into(),
            headroom_pct: Some(18.), // 82% used → warn band (worst in cluster)
            window: Some("5h".into()),
            ..Default::default()
        },
        PoolRow {
            name: "codex_plan".into(),
            headroom_pct: Some(88.), // 12% used → ok band
            window: Some("5h".into()),
            vendor_429_observed: true, // codex hit a real vendor ceiling once
            ..Default::default()
        },
        PoolRow {
            name: "antigravity_weekly".into(),
            headroom_pct: Some(6.), // 94% used → crit band
            window: Some("week".into()),
            ..Default::default()
        },
        PoolRow {
            name: "gemini_free_rpd".into(),
            headroom_pct: Some(100.), // 0% used
            window: Some("day".into()),
            brain_calls_in_window: Some(3),
            ping_calls_in_window: Some(12),
            vendor_429_observed: false,
        },
    ]
}

/// Staged usage meta: a fresh scrape + a three-vendor liveness strip walking
/// alive / rate-limited / down so every liveness tone renders.
pub fn demo_meta() -> UsageMeta {
    UsageMeta {
        source: Some("self-metering + vendor-page scrape (demo)".into()),
        scraped: Some(ScrapeMeta {
            stale: false,
            age_h: None,
            age_min: Some(4.),
            reset_phrase: Some("14:32".into()),
        }),
        liveness: vec![
            VendorLiveness {
                vendor: "gemini".into(),
                state: "alive".into(),
                last_status: Some("ok".into()),
                age_s: Some(120.),
                verified_by: Some("codex".into()),
                latency_ms: Some(1760.2),
            },
            VendorLiveness {
                vendor: "codex".into(),
                state: "rate_limited".into(),
                last_status: Some("429".into()),
                age_s: Some(30.),
                verified_by: None,
                latency_ms: Some(210.),
            },
            VendorLiveness {
                vendor: "claude".into(),
                state: "down".into(),
                last_status: Some("connection refused".into()),
                age_s: Some(600.),
                verified_by: None,
                latency_ms: None,
            },
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_accents::{Tone, tone_for_used, used_pct};

    #[test]
    fn demo_pools_span_every_meter_band() {
        let pools = demo_pools();
        let tone = |name: &str| {
            let pool = pools.iter().find(|p| p.name == name).unwrap();
            tone_for_used(used_pct(pool.headroom_pct))
        };
        assert_eq!(tone("claude_5h"), Tone::Warn);
        assert_eq!(tone("antigravity_weekly"), Tone::Crit);
        assert_eq!(tone("codex_plan"), Tone::Ok);
        assert_eq!(tone("claude_weekly"), Tone::Ok);
        assert_eq!(tone("claude_weekly_sonnet"), Tone::Unknown);
        // The new fields surface on the gemini pool.
        let gemini = pools.iter().find(|p| p.name == "gemini_free_rpd").unwrap();
        assert_eq!(gemini.brain_calls_in_window, Some(3));
        assert_eq!(gemini.ping_calls_in_window, Some(12));
        // And a 429 pool exists.
        assert!(pools.iter().any(|p| p.vendor_429_observed));
    }

    #[test]
    fn demo_meta_liveness_walks_every_tone() {
        let meta = demo_meta();
        let states: Vec<&str> = meta.liveness.iter().map(|v| v.state.as_str()).collect();
        assert_eq!(states, ["alive", "rate_limited", "down"]);
    }
}
