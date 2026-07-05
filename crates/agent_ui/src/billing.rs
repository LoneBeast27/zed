//! Vendor billing/plan registry: pure data and pure helpers for resolving
//! vendor plan metadata and per-pool cap overrides. No UI and no gpui.

#![allow(dead_code)]

pub struct Plan {
    pub vendor: &'static str,
    pub billing_url: &'static str,
    pub usage_url: Option<&'static str>,
    pub default_tier: &'static str,
    pub tiers: &'static [Tier],
}

pub struct Tier {
    pub key: &'static str,
    pub label: &'static str,
    pub price_usd: f64,
    pub detect: &'static [&'static str],
    pub caps: &'static [(&'static str, i64)],
}

/// Mirrors `crate::bridge::protocol::VendorPlan` without importing bridge types.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanMeta {
    pub tier: String,
    pub label: String,
    pub price_usd: Option<f64>,
    pub billing_url: String,
    pub usage_url: Option<String>,
    pub source: String,
}

pub const PLANS: &[Plan] = &[
    Plan {
        vendor: "claude",
        billing_url: "https://claude.ai/settings/billing",
        usage_url: Some("https://claude.ai/settings/usage"),
        default_tier: "max_5x",
        tiers: &[
            Tier {
                key: "max_20x",
                label: "Max 20×",
                price_usd: 200.,
                detect: &["max 20", "max plan (20", "20x", "max 20x"],
                caps: &[
                    ("claude_5h", 160),
                    ("claude_weekly", 1600),
                    ("claude_weekly_sonnet", 3200),
                ],
            },
            Tier {
                key: "max_5x",
                label: "Max 5×",
                price_usd: 100.,
                detect: &["max 5", "max 5x", "max plan", "max"],
                caps: &[
                    ("claude_5h", 40),
                    ("claude_weekly", 400),
                    ("claude_weekly_sonnet", 800),
                ],
            },
            Tier {
                key: "pro",
                label: "Pro",
                price_usd: 20.,
                detect: &["pro plan", "pro"],
                caps: &[
                    ("claude_5h", 8),
                    ("claude_weekly", 80),
                    ("claude_weekly_sonnet", 160),
                ],
            },
            Tier {
                key: "free",
                label: "Free",
                price_usd: 0.,
                detect: &["free plan", "free"],
                caps: &[
                    ("claude_5h", 2),
                    ("claude_weekly", 0),
                    ("claude_weekly_sonnet", 0),
                ],
            },
        ],
    },
    Plan {
        vendor: "codex",
        billing_url: "https://chatgpt.com/#settings/Subscription",
        usage_url: Some("https://chatgpt.com/codex/settings/usage"),
        default_tier: "base",
        tiers: &[
            Tier {
                key: "pro",
                label: "Pro ($200)",
                price_usd: 200.,
                detect: &["pro plan", "chatgpt pro"],
                caps: &[("codex_plan", 400)],
            },
            Tier {
                key: "plus",
                label: "Plus ($20)",
                price_usd: 20.,
                detect: &["plus plan", "chatgpt plus", "plus"],
                caps: &[("codex_plan", 80)],
            },
            Tier {
                key: "base",
                label: "Base ($10)",
                price_usd: 10.,
                detect: &["go plan", "chatgpt go", "basic", "$10"],
                caps: &[("codex_plan", 40)],
            },
            Tier {
                key: "free",
                label: "Free",
                price_usd: 0.,
                detect: &["free plan", "free"],
                caps: &[("codex_plan", 0)],
            },
        ],
    },
    Plan {
        vendor: "google",
        billing_url: "https://one.google.com/ai",
        usage_url: Some("https://one.google.com/ai"),
        default_tier: "base_10",
        tiers: &[
            Tier {
                key: "ultra",
                label: "AI Ultra",
                price_usd: 250.,
                detect: &["ai ultra", "ultra"],
                caps: &[],
            },
            Tier {
                key: "pro",
                label: "AI Pro ($20)",
                price_usd: 20.,
                detect: &["ai pro", "ai premium", "pro"],
                caps: &[],
            },
            Tier {
                key: "base_10",
                label: "AI ($10)",
                price_usd: 10.,
                detect: &["ai plan", "$10", "basic"],
                caps: &[],
            },
            Tier {
                key: "free",
                label: "Free",
                price_usd: 0.,
                detect: &["free plan", "free"],
                caps: &[],
            },
        ],
    },
];

pub fn vendor_of(pool_name: &str) -> &str {
    match pool_name {
        n if n.starts_with("claude") => "claude",
        n if n.starts_with("codex") => "codex",
        n if n.starts_with("gemini") => "google",
        n if n.starts_with("antigravity") => "google",
        n => n.split('_').next().unwrap_or(n),
    }
}

pub fn detect_tier(vendor: &str, page_text: &str) -> Option<&'static str> {
    let spec = plan_spec(vendor)?;
    if page_text.is_empty() {
        return None;
    }
    let low = page_text.to_lowercase();
    spec.tiers
        .iter()
        .find(|tier| tier.detect.iter().any(|phrase| low.contains(phrase)))
        .map(|tier| tier.key)
}

pub fn plan_meta(vendor: &str, detected_tier: Option<&str>) -> Option<PlanMeta> {
    let spec = plan_spec(vendor)?;
    let detected = detected_tier.and_then(|tier_key| tier_spec(spec, tier_key));
    let source = if detected.is_some() {
        "detected"
    } else {
        "default"
    };
    let tier = detected.or_else(|| tier_spec(spec, spec.default_tier))?;

    Some(PlanMeta {
        tier: tier.key.into(),
        label: tier.label.into(),
        price_usd: Some(tier.price_usd),
        billing_url: spec.billing_url.into(),
        usage_url: spec.usage_url.map(Into::into),
        source: source.into(),
    })
}

pub fn resolve_all(
    detected: &[(&str, Option<&str>)],
) -> (Vec<(&'static str, PlanMeta)>, Vec<(&'static str, i64)>) {
    let mut plans = Vec::new();
    let mut caps = Vec::new();

    for spec in PLANS {
        let detected_tier = detected
            .iter()
            .find_map(|(vendor, tier)| (*vendor == spec.vendor).then_some(*tier).flatten());
        if let Some(meta) = plan_meta(spec.vendor, detected_tier) {
            if let Some(tier) = tier_spec(spec, &meta.tier) {
                caps.extend(tier.caps.iter().copied());
            }
            plans.push((spec.vendor, meta));
        }
    }

    (plans, caps)
}

fn plan_spec(vendor: &str) -> Option<&'static Plan> {
    PLANS.iter().find(|spec| spec.vendor == vendor)
}

fn tier_spec<'a>(spec: &'a Plan, tier_key: &str) -> Option<&'a Tier> {
    spec.tiers.iter().find(|tier| tier.key == tier_key)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan<'a>(plans: &'a [(&'static str, PlanMeta)], vendor: &str) -> &'a PlanMeta {
        plans
            .iter()
            .find_map(|(candidate, meta)| (*candidate == vendor).then_some(meta))
            .expect("vendor plan exists")
    }

    fn cap(caps: &[(&'static str, i64)], pool: &str) -> i64 {
        caps.iter()
            .find_map(|(candidate, cap)| (*candidate == pool).then_some(*cap))
            .expect("pool cap exists")
    }

    #[test]
    fn detects_tier_by_ordered_phrase_scan() {
        assert_eq!(
            detect_tier("claude", "You are on the Max plan"),
            Some("max_5x")
        );
        assert_eq!(
            detect_tier("claude", "Max 20x plan -- 200/mo"),
            Some("max_20x")
        );
        assert_eq!(
            detect_tier("codex", "ChatGPT Plus subscription"),
            Some("plus")
        );
        assert_eq!(detect_tier("claude", "are you a bot"), None);
        assert_eq!(detect_tier("nope", "Max"), None);
    }

    #[test]
    fn resolve_all_defaults_all_vendors() {
        let (plans, caps) = resolve_all(&[]);

        assert_eq!(plans.len(), 3);
        assert!(plans.iter().all(|(_, meta)| meta.source == "default"));
        assert_eq!(plan(&plans, "claude").tier, "max_5x");
        assert_eq!(plan(&plans, "codex").price_usd, Some(10.));
        assert_eq!(plan(&plans, "google").price_usd, Some(10.));
        assert_eq!(cap(&caps, "claude_5h"), 40);
        assert_eq!(cap(&caps, "codex_plan"), 40);
    }

    #[test]
    fn resolve_all_uses_detected_tiers_and_defaults_absent_vendors() {
        let (plans, caps) = resolve_all(&[("claude", Some("max_20x")), ("codex", Some("plus"))]);

        assert_eq!(plan(&plans, "claude").source, "detected");
        assert_eq!(cap(&caps, "claude_5h"), 160);
        assert_eq!(cap(&caps, "codex_plan"), 80);
        assert_eq!(plan(&plans, "google").source, "default");
    }

    #[test]
    fn vendor_of_maps_pool_to_cluster_vendor() {
        assert_eq!(vendor_of("claude_5h"), "claude");
        assert_eq!(vendor_of("codex_plan"), "codex");
        assert_eq!(vendor_of("gemini_free_rpd"), "google");
        assert_eq!(vendor_of("antigravity_weekly"), "google");
        assert_eq!(vendor_of("mistral_daily"), "mistral");
    }
}
