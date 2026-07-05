//! Brute-force allocator for covering model roles at the lowest effective tier cost.
//! Pure logic only; no UI and no gpui dependencies.

#![allow(dead_code)]

use crate::model_roles::{MODELS, RosterEntry, TIERS, available_models, in_window, roster_cost};

pub struct Allocation {
    pub tiers: Vec<(&'static str, &'static str)>,
    pub cost: i64,
    pub assignment: Vec<(&'static str, &'static str)>,
    pub total_fit: f64,
}

const CLAUDE_LADDER: &[Option<&str>] = &[
    None,
    Some("claude_pro"),
    Some("claude_max_5x"),
    Some("claude_max_20x"),
];
const CODEX_LADDER: &[Option<&str>] = &[
    None,
    Some("codex_plus"),
    Some("codex_pro_100"),
    Some("codex_pro_200"),
];
const GOOGLE_LADDER: &[Option<&str>] = &[
    None,
    Some("google_ai_pro"),
    Some("google_ultra_100"),
    Some("google_ultra_200"),
];

pub fn allocate(
    required: &[&str],
    threshold: f64,
    today: &str,
    max_budget: Option<i64>,
    discounts: &[(&str, i64)],
) -> Option<Allocation> {
    let mut best = None;

    for claude in CLAUDE_LADDER {
        for codex in CODEX_LADDER {
            for google in GOOGLE_LADDER {
                let roster = candidate_roster([
                    ("claude", *claude),
                    ("codex", *codex),
                    ("google", *google),
                ], discounts)?;
                let cost = roster_cost(Some(&roster));

                if max_budget.is_some_and(|max_budget| cost > max_budget) {
                    continue;
                }

                let Some((assignment, total_fit)) = assign_roles(required, threshold, today, &roster)
                else {
                    continue;
                };
                let tiers = roster.iter().map(|entry| (entry.vendor, entry.tier)).collect();
                let allocation = Allocation {
                    tiers,
                    cost,
                    assignment,
                    total_fit,
                };

                if is_better(&allocation, best.as_ref()) {
                    best = Some(allocation);
                }
            }
        }
    }

    best
}

fn candidate_roster(
    choices: [(&'static str, Option<&'static str>); 3],
    discounts: &[(&str, i64)],
) -> Option<Vec<RosterEntry>> {
    choices
        .into_iter()
        .filter_map(|(vendor, tier)| tier.map(|tier| (vendor, tier)))
        .map(|(vendor, tier)| {
            let paid_usd = effective_price(vendor, tier, discounts)?;
            Some(RosterEntry {
                vendor,
                tier,
                paid_usd,
                until: None,
                then_tier: None,
                why: None,
            })
        })
        .collect()
}

fn effective_price(vendor: &str, tier: &str, discounts: &[(&str, i64)]) -> Option<i64> {
    discounts
        .iter()
        .find_map(|(discount_vendor, price)| (*discount_vendor == vendor).then_some(*price))
        .or_else(|| {
            TIERS
                .iter()
                .find_map(|spec| (spec.key == tier).then_some(spec.price_usd))
        })
}

fn assign_roles(
    required: &[&str],
    threshold: f64,
    today: &str,
    roster: &[RosterEntry],
) -> Option<(Vec<(&'static str, &'static str)>, f64)> {
    let mut assignment = Vec::new();
    let mut total_fit = 0.;

    for required in required {
        let (model, fit) = best_fit(required, today, roster)?;
        if fit < threshold {
            return None;
        }
        assignment.push((static_role(required)?, model));
        total_fit += fit;
    }

    Some((assignment, total_fit))
}

fn best_fit(role: &str, today: &str, roster: &[RosterEntry]) -> Option<(&'static str, f64)> {
    available_models(Some(roster))
        .into_iter()
        .filter(|model| in_window(model, today))
        .filter_map(|model| {
            MODELS
                .iter()
                .find(|spec| spec.id == model)
                .map(|spec| (model, fit_score(spec.fit, role)))
        })
        .max_by(|(left_model, left_fit), (right_model, right_fit)| {
            left_fit
                .total_cmp(right_fit)
                .then_with(|| left_model.cmp(right_model))
        })
}

fn fit_score(fit: &[(&str, f64)], role: &str) -> f64 {
    fit.iter()
        .find_map(|(key, score)| (*key == role).then_some(*score))
        .unwrap_or(0.)
}

fn static_role(role: &str) -> Option<&'static str> {
    MODELS
        .iter()
        .flat_map(|model| model.fit.iter().map(|(key, _)| *key))
        .find(|key| *key == role)
}

fn is_better(candidate: &Allocation, incumbent: Option<&Allocation>) -> bool {
    incumbent.is_none_or(|incumbent| {
        candidate.cost < incumbent.cost
            || (candidate.cost == incumbent.cost && candidate.total_fit > incumbent.total_fit)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model_roles::ARCHETYPES;

    fn all_roles() -> Vec<&'static str> {
        ARCHETYPES.iter().map(|archetype| archetype.key).collect()
    }

    #[test]
    fn allocates_lowest_cost_cover_for_all_roles_at_standard_threshold() {
        let required = all_roles();
        let allocation = allocate(&required, 0.8, "2026-07-06", None, &[]).unwrap();

        assert_eq!(allocation.cost, 40);
        assert!(allocation.tiers.contains(&("claude", "claude_pro")));
        assert!(allocation.tiers.contains(&("google", "google_ai_pro")));
        assert!(!allocation.tiers.iter().any(|(vendor, _)| *vendor == "codex"));
        assert_eq!(allocation.assignment.len(), 6);
        for role in &required {
            assert!(allocation.assignment.iter().any(|(covered, _)| covered == role));
        }
    }

    #[test]
    fn vendor_discount_overrides_chosen_tier_price() {
        let required = all_roles();
        let allocation = allocate(&required, 0.8, "2026-07-06", None, &[("google", 0)]).unwrap();

        assert_eq!(allocation.cost, 20);
        assert!(allocation.tiers.contains(&("claude", "claude_pro")));
        assert!(allocation.tiers.contains(&("google", "google_ai_pro")));
    }

    #[test]
    fn high_threshold_justifies_claude_max_20x_during_fable_window() {
        let required = all_roles();
        let allocation = allocate(&required, 0.9, "2026-07-06", None, &[]).unwrap();

        assert_eq!(allocation.cost, 220);
        assert!(allocation.tiers.contains(&("claude", "claude_max_20x")));
        assert!(allocation.tiers.contains(&("google", "google_ai_pro")));
        assert!(allocation.assignment.contains(&("long_horizon", "claude-fable-5")));
        assert!(allocation.assignment.contains(&("orchestrator", "claude-opus-4-8")));
        assert!(allocation.assignment.contains(&("designer", "gemini-3.1-pro")));
    }

    #[test]
    fn budget_cap_filters_out_required_cover() {
        let required = all_roles();

        assert!(allocate(&required, 0.8, "2026-07-06", Some(30), &[]).is_none());
    }
}
