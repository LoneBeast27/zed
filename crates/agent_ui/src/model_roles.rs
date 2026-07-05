//! Model-role meta table: pure data and pure helpers for assigning available
//! models to run archetypes. No UI and no gpui dependencies; briefing panels
//! and routing code can read this table directly.

#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet};

pub struct Tier {
    pub key: &'static str,
    pub price_usd: i64,
    pub mult: i64,
    pub limit_unit: &'static str,
}

pub struct Archetype {
    pub key: &'static str,
    pub description: &'static str,
}

pub struct Model {
    pub id: &'static str,
    pub vendor: &'static str,
    pub min_tier: &'static str,
    pub api_cost: Option<(f64, f64)>,
    pub swe_bench: f64,
    pub window: Option<&'static str>,
    pub fit: &'static [(&'static str, f64)],
    pub note: &'static str,
}

pub struct RosterEntry {
    pub vendor: &'static str,
    pub tier: &'static str,
    pub paid_usd: i64,
    pub until: Option<&'static str>,
    pub then_tier: Option<&'static str>,
    pub why: Option<&'static str>,
}

pub const TIERS: &[Tier] = &[
    Tier {
        key: "claude_pro",
        price_usd: 20,
        mult: 1,
        limit_unit: "tokens",
    },
    Tier {
        key: "claude_max_5x",
        price_usd: 100,
        mult: 5,
        limit_unit: "tokens",
    },
    Tier {
        key: "claude_max_20x",
        price_usd: 200,
        mult: 20,
        limit_unit: "tokens",
    },
    Tier {
        key: "codex_plus",
        price_usd: 20,
        mult: 1,
        limit_unit: "messages",
    },
    Tier {
        key: "codex_pro_100",
        price_usd: 100,
        mult: 5,
        limit_unit: "messages",
    },
    Tier {
        key: "codex_pro_200",
        price_usd: 200,
        mult: 20,
        limit_unit: "messages",
    },
    Tier {
        key: "google_ai_pro",
        price_usd: 20,
        mult: 1,
        limit_unit: "compute",
    },
    Tier {
        key: "google_ultra_100",
        price_usd: 100,
        mult: 5,
        limit_unit: "compute",
    },
    Tier {
        key: "google_ultra_200",
        price_usd: 200,
        mult: 20,
        limit_unit: "compute",
    },
];

const CLAUDE_LADDER: &[&str] = &["claude_pro", "claude_max_5x", "claude_max_20x"];
const CODEX_LADDER: &[&str] = &["codex_plus", "codex_pro_100", "codex_pro_200"];
const GOOGLE_LADDER: &[&str] = &["google_ai_pro", "google_ultra_100", "google_ultra_200"];

pub const ARCHETYPES: &[Archetype] = &[
    Archetype {
        key: "orchestrator",
        description: "plans a large task, fans out subagents, verifies output",
    },
    Archetype {
        key: "long_horizon",
        description: "hardest multi-complex, long-context fixes end-to-end",
    },
    Archetype {
        key: "coder",
        description: "multifile single-objective implementation",
    },
    Archetype {
        key: "designer",
        description: "visual / UI / motion - multimodal",
    },
    Archetype {
        key: "reviewer",
        description: "code review / adversarial verification",
    },
    Archetype {
        key: "judge",
        description: "cheap, fast triage + structured decisions",
    },
];

pub const MODELS: &[Model] = &[
    Model {
        id: "claude-fable-5",
        vendor: "claude",
        min_tier: "claude_max_20x",
        api_cost: Some((10., 50.)),
        swe_bench: 95.0,
        window: Some("2026-07-08"),
        fit: &[
            ("long_horizon", 0.98),
            ("coder", 0.90),
            ("orchestrator", 0.85),
            ("reviewer", 0.85),
        ],
        note: "frontier peak (95% SWE-bench V, top BenchLM); windowed release",
    },
    Model {
        id: "claude-opus-4-8",
        vendor: "claude",
        min_tier: "claude_pro",
        api_cost: Some((5., 25.)),
        swe_bench: 88.6,
        window: None,
        fit: &[
            ("orchestrator", 0.95),
            ("coder", 0.90),
            ("long_horizon", 0.85),
            ("reviewer", 0.90),
        ],
        note: "the default; Dynamic Workflows (plan+fan-out+verify), best long-context coherence",
    },
    Model {
        id: "claude-sonnet-5",
        vendor: "claude",
        min_tier: "claude_pro",
        api_cost: Some((3., 15.)),
        swe_bench: 80.0,
        window: None,
        fit: &[
            ("coder", 0.90),
            ("reviewer", 0.85),
            ("orchestrator", 0.70),
        ],
        note: "~ties Opus 4.8 on reasoning WITH TOOLS at a fraction of cost",
    },
    Model {
        id: "claude-sonnet-4-6",
        vendor: "claude",
        min_tier: "claude_pro",
        api_cost: Some((3., 15.)),
        swe_bench: 77.0,
        window: None,
        fit: &[("coder", 0.80), ("reviewer", 0.75)],
        note: "everyday coding subworker",
    },
    Model {
        id: "agy-claude-sonnet-4-6",
        vendor: "google",
        min_tier: "google_ai_pro",
        api_cost: None,
        swe_bench: 77.0,
        window: None,
        fit: &[
            ("reviewer", 0.80),
            ("coder", 0.75),
            ("judge", 0.85),
            ("designer", 0.60),
        ],
        note: "free via agy CLI (--model 'Claude Sonnet 4.6 (Thinking)'); small tasks",
    },
    Model {
        id: "agy-claude-opus-4-6",
        vendor: "google",
        min_tier: "google_ai_pro",
        api_cost: None,
        swe_bench: 82.0,
        window: None,
        fit: &[
            ("reviewer", 0.85),
            ("orchestrator", 0.75),
            ("coder", 0.78),
        ],
        note: "free via agy CLI (--model 'Claude Opus 4.6 (Thinking)'); small tasks / the pricing recommendation",
    },
    Model {
        id: "claude-haiku-4-5",
        vendor: "claude",
        min_tier: "claude_pro",
        api_cost: Some((1., 5.)),
        swe_bench: 60.0,
        window: None,
        fit: &[("judge", 0.90), ("coder", 0.55)],
        note: "fast/cheap judgment + triage tier",
    },
    Model {
        id: "gpt-5.5",
        vendor: "codex",
        min_tier: "codex_plus",
        api_cost: None,
        swe_bench: 82.0,
        window: None,
        fit: &[
            ("coder", 0.85),
            ("reviewer", 0.80),
            ("orchestrator", 0.70),
        ],
        note: "alt-vendor coder (non-Anthropic leg - spares the Max quota)",
    },
    Model {
        id: "gemini-3.1-pro",
        vendor: "google",
        min_tier: "google_ai_pro",
        api_cost: None,
        swe_bench: 78.0,
        window: None,
        fit: &[
            ("designer", 0.92),
            ("long_horizon", 0.80),
            ("coder", 0.70),
            ("reviewer", 0.70),
        ],
        note: "multimodal (video/visual refs); routed via Antigravity",
    },
];

pub const ROSTER: &[RosterEntry] = &[
    RosterEntry {
        vendor: "claude",
        tier: "claude_max_20x",
        paid_usd: 200,
        until: Some("2026-07-08"),
        then_tier: Some("claude_max_5x"),
        why: Some("temporary - reach Fable 5 during its window, then demote"),
    },
    RosterEntry {
        vendor: "codex",
        tier: "codex_plus",
        paid_usd: 20,
        until: None,
        then_tier: None,
        why: None,
    },
    RosterEntry {
        vendor: "google",
        tier: "google_ai_pro",
        paid_usd: 0,
        until: None,
        then_tier: None,
        why: Some("AI Pro via a ~1-year free coupon -> $0 effective"),
    },
];

pub fn ladder_grants(tier: &str) -> BTreeSet<String> {
    for ladder in [CLAUDE_LADDER, CODEX_LADDER, GOOGLE_LADDER] {
        if let Some(ix) = ladder.iter().position(|entry| *entry == tier) {
            return ladder[..=ix].iter().map(|entry| entry.to_string()).collect();
        }
    }
    std::iter::once(tier.to_string()).collect()
}

pub fn available_models(roster: Option<&[RosterEntry]>) -> Vec<&'static str> {
    let roster = roster.unwrap_or(ROSTER);
    let granted = roster
        .iter()
        .flat_map(|sub| ladder_grants(sub.tier))
        .collect::<BTreeSet<_>>();
    MODELS
        .iter()
        .filter(|model| granted.contains(model.min_tier))
        .map(|model| model.id)
        .collect()
}

pub fn in_window(model: &str, today: &str) -> bool {
    model_spec(model)
        .and_then(|model| model.window)
        .map_or(true, |window| today <= window)
}

pub fn best_for(
    archetype: &str,
    roster: Option<&[RosterEntry]>,
    today: Option<&str>,
) -> Option<&'static str> {
    available_models(roster)
        .into_iter()
        .filter_map(|model_id| {
            let model = model_spec(model_id)?;
            let fit = fit_score(model, archetype);
            if fit > 0. && today.map_or(true, |today| in_window(model_id, today)) {
                Some((fit, model_id))
            } else {
                None
            }
        })
        .max_by(|(left_fit, left_model), (right_fit, right_model)| {
            left_fit
                .total_cmp(right_fit)
                .then_with(|| left_model.cmp(right_model))
        })
        .map(|(_, model_id)| model_id)
}

pub fn role_assignment(
    roster: Option<&[RosterEntry]>,
    today: Option<&str>,
) -> BTreeMap<&'static str, &'static str> {
    ARCHETYPES
        .iter()
        .filter_map(|archetype| {
            best_for(archetype.key, roster, today).map(|model| (archetype.key, model))
        })
        .collect()
}

pub fn roster_cost(roster: Option<&[RosterEntry]>) -> i64 {
    roster
        .unwrap_or(ROSTER)
        .iter()
        .map(|sub| sub.paid_usd)
        .sum()
}

fn model_spec(model: &str) -> Option<&'static Model> {
    MODELS.iter().find(|spec| spec.id == model)
}

fn fit_score(model: &Model, archetype: &str) -> f64 {
    model
        .fit
        .iter()
        .find_map(|(key, score)| (*key == archetype).then_some(*score))
        .unwrap_or(0.)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAX_5X_ROSTER: &[RosterEntry] = &[
        RosterEntry {
            vendor: "claude",
            tier: "claude_max_5x",
            paid_usd: 100,
            until: None,
            then_tier: None,
            why: None,
        },
        RosterEntry {
            vendor: "codex",
            tier: "codex_plus",
            paid_usd: 20,
            until: None,
            then_tier: None,
            why: None,
        },
        RosterEntry {
            vendor: "google",
            tier: "google_ai_pro",
            paid_usd: 0,
            until: None,
            then_tier: None,
            why: None,
        },
    ];

    #[test]
    fn max_20x_ladder_grants_claude_lower_tiers() {
        assert_eq!(
            ladder_grants("claude_max_20x"),
            ["claude_pro", "claude_max_5x", "claude_max_20x"]
                .into_iter()
                .map(String::from)
                .collect()
        );
    }

    #[test]
    fn available_models_reflect_roster_tiers() {
        let seed = available_models(None);
        assert!(seed.contains(&"claude-fable-5"));
        assert!(seed.contains(&"claude-opus-4-8"));
        assert!(seed.contains(&"gpt-5.5"));
        assert!(seed.contains(&"gemini-3.1-pro"));

        let max_5x = available_models(Some(MAX_5X_ROSTER));
        assert!(!max_5x.contains(&"claude-fable-5"));
    }

    #[test]
    fn windowed_models_drop_after_window() {
        assert!(in_window("claude-fable-5", "2026-07-06"));
        assert!(!in_window("claude-fable-5", "2026-07-09"));
        assert!(in_window("claude-opus-4-8", "2026-07-09"));
    }

    #[test]
    fn best_for_picks_highest_fit_available_in_window_model() {
        assert_eq!(
            best_for("long_horizon", None, Some("2026-07-06")),
            Some("claude-fable-5")
        );
        assert_eq!(
            best_for("orchestrator", None, Some("2026-07-06")),
            Some("claude-opus-4-8")
        );
        assert_eq!(
            best_for("designer", None, Some("2026-07-06")),
            Some("gemini-3.1-pro")
        );
        assert_eq!(
            best_for("judge", None, Some("2026-07-06")),
            Some("claude-haiku-4-5")
        );
        assert_eq!(
            best_for("long_horizon", None, Some("2026-07-09")),
            Some("claude-opus-4-8")
        );
    }

    #[test]
    fn role_assignment_covers_all_archetypes() {
        let assignment = role_assignment(None, Some("2026-07-06"));
        assert_eq!(assignment.len(), 6);
        for archetype in ARCHETYPES {
            assert!(assignment.contains_key(archetype.key));
        }
    }

    #[test]
    fn roster_cost_counts_effective_paid_usd() {
        assert_eq!(roster_cost(None), 220);
    }
}
