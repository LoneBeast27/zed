//! Vendor grouping for the usage panel (PARITY_SPEC Amendment 2026-07-04 (4)
//! item 1) — the render-side fold that clusters the flat `/usage` pool rows
//! BY VENDOR. One [`VendorGroup`] per vendor (claude / codex / gemini / agy,
//! then any unknown vendor honestly, never dropped), with the vendor's
//! liveness state matched in and its worst-pool at-a-glance % computed.
//!
//! Pure logic, no gpui: the panel calls [`group_pools`] ONCE per frame
//! (I/O-first — a fold over the already-typed payload, no new fetch, no
//! per-pool re-derivation) and renders the returned clusters. All ordering,
//! vendor attribution, and roll-up math is unit-tested here.

use crate::agent_accents::{Tone, tone_for_used, used_pct};
use crate::bridge::{PoolRow, UsageMeta, VendorLiveness};

/// Canonical vendor ordering for the clusters (the ruling's order:
/// claude, codex, gemini, agy). Unknown vendors sort after these, in first-
/// seen (wire) order, so a new vendor surfaces instead of vanishing.
const VENDOR_ORDER: [&str; 4] = ["claude", "codex", "gemini", "agy"];

/// Within the claude cluster the 5h (session) pool leads, then the weekly
/// buckets. Pools not named here keep their wire order after the ranked ones.
fn pool_rank(name: &str) -> u8 {
    match name {
        "claude_5h" => 0,
        "claude_weekly" => 1,
        "claude_weekly_sonnet" => 2,
        _ => u8::MAX,
    }
}

/// The vendor a pool key belongs to. Known prefixes map to the canonical
/// short vendor name; anything else derives its vendor from the key's first
/// `_`-segment so an unrecognized pool still clusters honestly (new outranks
/// old — an unmapped vendor is surfaced, not swallowed into a catch-all).
pub fn vendor_of(pool_name: &str) -> &str {
    match pool_name {
        n if n.starts_with("claude") => "claude",
        n if n.starts_with("codex") => "codex",
        n if n.starts_with("gemini") => "gemini",
        n if n.starts_with("antigravity") => "agy",
        n => n.split('_').next().unwrap_or(n),
    }
}

/// The human vendor header label for a canonical vendor key.
pub fn vendor_label(vendor: &str) -> &str {
    match vendor {
        "claude" => "Claude",
        "codex" => "Codex",
        "gemini" => "Gemini",
        "agy" => "Antigravity",
        other => other,
    }
}

/// One vendor cluster: the vendor's header data (liveness, worst-pool %) over
/// its pools in display order. Borrows the pool rows + liveness from the
/// panel's per-frame snapshot (no clone — the fold outlives only the render).
pub struct VendorGroup<'a> {
    /// Canonical vendor key ("claude" / "codex" / "gemini" / "agy" / raw).
    pub vendor: &'a str,
    /// The vendor's pools, ordered ([`pool_rank`] then wire order).
    pub pools: Vec<&'a PoolRow>,
    /// The vendor's liveness chip, matched from [`UsageMeta::liveness`] by
    /// vendor name. `None` when the bridge surfaces no liveness for it (no
    /// blank chip — the header just omits it).
    pub liveness: Option<&'a VendorLiveness>,
}

impl VendorGroup<'_> {
    /// The cluster's worst-pool used-% for the at-a-glance header figure: the
    /// highest used-% among the vendor's METERED pools (a fuller pool is
    /// worse). `None` when every pool in the cluster is unknown/unmetered, so
    /// the header degrades to "—" instead of inventing a number.
    pub fn worst_used(&self) -> Option<f64> {
        self.pools
            .iter()
            .filter_map(|pool| used_pct(pool.headroom_pct))
            .fold(None, |acc: Option<f64>, used| {
                Some(acc.map_or(used, |worst| worst.max(used)))
            })
    }

    /// The tone for the cluster header dot/figure — driven by the worst pool
    /// (an Unknown-only cluster stays Unknown, degrading visibly).
    pub fn worst_tone(&self) -> Tone {
        tone_for_used(self.worst_used())
    }
}

/// Fold the flat pool rows into ordered vendor clusters. Called once per
/// frame by the panel: groups by [`vendor_of`], orders pools within each
/// cluster ([`pool_rank`]), matches each vendor's liveness, and orders the
/// clusters ([`VENDOR_ORDER`] then first-seen). Empty input → no clusters
/// (the panel's honest-empty path owns that case).
pub fn group_pools<'a>(pools: &'a [PoolRow], meta: &'a UsageMeta) -> Vec<VendorGroup<'a>> {
    // Preserve first-seen order for unknown vendors (and as the stable tie-
    // break) by remembering when each vendor first appeared.
    let mut order: Vec<&str> = Vec::new();
    let mut groups: Vec<VendorGroup<'a>> = Vec::new();
    for pool in pools {
        let vendor = vendor_of(&pool.name);
        let ix = match order.iter().position(|v| *v == vendor) {
            Some(ix) => ix,
            None => {
                order.push(vendor);
                groups.push(VendorGroup {
                    vendor,
                    pools: Vec::new(),
                    liveness: liveness_for(vendor, meta),
                });
                groups.len() - 1
            }
        };
        groups[ix].pools.push(pool);
    }
    // Order pools within each cluster: ranked leaders first, rest in wire
    // order (a stable sort keeps the wire order among equal ranks).
    for group in &mut groups {
        group.pools.sort_by_key(|pool| pool_rank(&pool.name));
    }
    // Order the clusters: canonical vendors first (in VENDOR_ORDER), then any
    // remaining vendors in first-seen order.
    groups.sort_by_key(|group| {
        VENDOR_ORDER
            .iter()
            .position(|v| *v == group.vendor)
            .unwrap_or(usize::MAX)
    });
    groups
}

/// Match a vendor's liveness entry (by vendor name) from the usage meta.
fn liveness_for<'a>(vendor: &str, meta: &'a UsageMeta) -> Option<&'a VendorLiveness> {
    meta.liveness.iter().find(|entry| entry.vendor == vendor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::ScrapeMeta;

    fn pool(name: &str, headroom: Option<f64>) -> PoolRow {
        PoolRow {
            name: name.into(),
            headroom_pct: headroom,
            window: Some("5h".into()),
            ..Default::default()
        }
    }

    fn meta_with_liveness(liveness: Vec<VendorLiveness>) -> UsageMeta {
        UsageMeta {
            source: None,
            scraped: Some(ScrapeMeta::default()),
            liveness,
        }
    }

    #[test]
    fn vendor_of_maps_known_prefixes() {
        assert_eq!(vendor_of("claude_5h"), "claude");
        assert_eq!(vendor_of("claude_weekly_sonnet"), "claude");
        assert_eq!(vendor_of("codex_plan"), "codex");
        assert_eq!(vendor_of("gemini_free_rpd"), "gemini");
        assert_eq!(vendor_of("antigravity_weekly"), "agy");
        // Unknown pool derives its vendor from the first segment, honestly.
        assert_eq!(vendor_of("mistral_daily"), "mistral");
        assert_eq!(vendor_of("solo"), "solo");
    }

    #[test]
    fn clusters_order_is_claude_codex_gemini_agy() {
        // Deliberately shuffled input; agy pool appears first on the wire.
        let pools = vec![
            pool("antigravity_weekly", Some(100.)),
            pool("gemini_free_rpd", Some(99.)),
            pool("claude_weekly", Some(97.)),
            pool("codex_plan", Some(100.)),
            pool("claude_5h", Some(100.)),
        ];
        let meta = meta_with_liveness(vec![]);
        let groups = group_pools(&pools, &meta);
        let vendors: Vec<&str> = groups.iter().map(|g| g.vendor).collect();
        assert_eq!(vendors, ["claude", "codex", "gemini", "agy"]);
    }

    #[test]
    fn claude_cluster_puts_5h_first() {
        let pools = vec![
            pool("claude_weekly_sonnet", Some(99.)),
            pool("claude_weekly", Some(97.)),
            pool("claude_5h", Some(100.)),
        ];
        let meta = meta_with_liveness(vec![]);
        let groups = group_pools(&pools, &meta);
        assert_eq!(groups.len(), 1);
        let names: Vec<&str> = groups[0].pools.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["claude_5h", "claude_weekly", "claude_weekly_sonnet"]);
    }

    #[test]
    fn worst_used_is_the_fullest_metered_pool() {
        // 82% used (headroom 18) is worse than 12% used (headroom 88).
        let pools = vec![pool("claude_5h", Some(18.)), pool("claude_weekly", Some(88.))];
        let meta = meta_with_liveness(vec![]);
        let groups = group_pools(&pools, &meta);
        assert_eq!(groups[0].worst_used(), Some(82.));
        assert_eq!(groups[0].worst_tone(), Tone::Warn);
    }

    #[test]
    fn all_unknown_cluster_degrades_to_none() {
        let pools = vec![pool("claude_5h", None), pool("claude_weekly", None)];
        let meta = meta_with_liveness(vec![]);
        let groups = group_pools(&pools, &meta);
        assert_eq!(groups[0].worst_used(), None);
        assert_eq!(groups[0].worst_tone(), Tone::Unknown);
    }

    #[test]
    fn liveness_matches_by_vendor_name() {
        let pools = vec![pool("gemini_free_rpd", Some(99.)), pool("claude_5h", Some(100.))];
        let meta = meta_with_liveness(vec![VendorLiveness {
            vendor: "gemini".into(),
            state: "alive".into(),
            ..Default::default()
        }]);
        let groups = group_pools(&pools, &meta);
        let gemini = groups.iter().find(|g| g.vendor == "gemini").unwrap();
        assert_eq!(gemini.liveness.map(|l| l.state.as_str()), Some("alive"));
        let claude = groups.iter().find(|g| g.vendor == "claude").unwrap();
        assert!(claude.liveness.is_none());
    }

    #[test]
    fn unknown_vendors_sort_after_canonical_in_wire_order() {
        let pools = vec![
            pool("mistral_daily", Some(50.)),
            pool("claude_5h", Some(100.)),
            pool("cohere_rpm", Some(50.)),
        ];
        let meta = meta_with_liveness(vec![]);
        let groups = group_pools(&pools, &meta);
        let vendors: Vec<&str> = groups.iter().map(|g| g.vendor).collect();
        assert_eq!(vendors, ["claude", "mistral", "cohere"]);
    }
}
