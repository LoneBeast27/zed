//! Daily briefing section builder. Pure data assembly only; the panel owns all
//! rendering.

#![allow(dead_code)]

use crate::bridge::protocol::{PoolRow, RunRow};
use crate::model_alloc::allocate;
use crate::model_roles::{ARCHETYPES, ROSTER, TIERS, role_assignment, roster_cost};

pub struct BriefItem {
    pub kind: &'static str,
    pub reference: String,
    pub text: String,
}

pub struct Section {
    pub key: &'static str,
    pub label: &'static str,
    pub items: Vec<BriefItem>,
}

pub struct Brief {
    pub sections: Vec<Section>,
}

pub fn build_brief(board: &[RunRow], usage: &[PoolRow], today: &str) -> Brief {
    let mut sections = Vec::new();

    let attention = attention_items(board, usage);
    if !attention.is_empty() {
        sections.push(Section {
            key: "attention",
            label: "Needs you",
            items: attention,
        });
    }

    sections.push(Section {
        key: "pricing",
        label: "Pricing",
        items: pricing_items(today),
    });

    let overnight = overnight_items(board);
    if !overnight.is_empty() {
        sections.push(Section {
            key: "overnight",
            label: "Overnight",
            items: overnight,
        });
    }

    let pools = pool_items(usage, 50., 3);
    if !pools.is_empty() {
        sections.push(Section {
            key: "pools",
            label: "Pools",
            items: pools,
        });
    }

    Brief { sections }
}

fn attention_items(board: &[RunRow], usage: &[PoolRow]) -> Vec<BriefItem> {
    let mut items = Vec::new();

    for run in board
        .iter()
        .filter(|run| run.status == "failed" || run.status == "killed")
    {
        items.push(BriefItem {
            kind: "run",
            reference: run.run_id.clone(),
            text: format!(
                "{} run {} - {}",
                run.agent,
                run.status,
                task_head(run.task.as_deref())
            ),
        });
    }

    for pool in usage
        .iter()
        .filter(|pool| pool.headroom_pct.is_some_and(|headroom| headroom < 25.))
    {
        items.push(pool_item(pool));
    }

    items.truncate(4);
    items
}

fn pricing_items(today: &str) -> Vec<BriefItem> {
    let assign = role_assignment(None, Some(today));
    let required = ARCHETYPES
        .iter()
        .map(|archetype| archetype.key)
        .collect::<Vec<_>>();
    let discounts = roster_discounts();
    let allocation = allocate(&required, 0.8, today, None, &discounts);

    let mut items = Vec::new();
    items.push(BriefItem {
        kind: "pricing",
        reference: "allocation".into(),
        text: allocation.map_or_else(
            || "No allocation covers all roles at 0.8".to_string(),
            |allocation| {
                format!(
                    "All {} roles covered for ${}/mo (you pay ${})",
                    required.len(),
                    allocation.cost,
                    roster_cost(None)
                )
            },
        ),
    });

    for archetype in ARCHETYPES {
        if let Some(model) = assign.get(archetype.key) {
            items.push(BriefItem {
                kind: "role",
                reference: archetype.key.into(),
                text: format!("{} -> {}", archetype.key, model),
            });
        }
    }

    items.truncate(8);
    items
}

fn overnight_items(board: &[RunRow]) -> Vec<BriefItem> {
    board
        .iter()
        .filter(|run| run.status == "completed")
        .take(4)
        .map(|run| BriefItem {
            kind: "run",
            reference: run.run_id.clone(),
            text: format!("{} completed - {}", run.agent, task_head(run.task.as_deref())),
        })
        .collect()
}

fn pool_items(usage: &[PoolRow], threshold: f64, cap: usize) -> Vec<BriefItem> {
    let mut pools = usage
        .iter()
        .filter(|pool| pool.headroom_pct.is_some_and(|headroom| headroom < threshold))
        .collect::<Vec<_>>();
    pools.sort_by(|left, right| {
        left.headroom_pct
            .unwrap_or(f64::INFINITY)
            .total_cmp(&right.headroom_pct.unwrap_or(f64::INFINITY))
    });
    pools.into_iter().take(cap).map(pool_item).collect()
}

fn pool_item(pool: &PoolRow) -> BriefItem {
    BriefItem {
        kind: "pool",
        reference: pool.name.clone(),
        text: format!(
            "{} - {}% headroom ({})",
            pool.name,
            format_headroom(pool.headroom_pct),
            pool.window.as_deref().unwrap_or("unknown window")
        ),
    }
}

fn roster_discounts() -> Vec<(&'static str, i64)> {
    ROSTER
        .iter()
        .filter_map(|entry| {
            let tier_price = TIERS
                .iter()
                .find_map(|tier| (tier.key == entry.tier).then_some(tier.price_usd))?;
            (entry.paid_usd < tier_price).then_some((entry.vendor, entry.paid_usd))
        })
        .collect()
}

fn task_head(task: Option<&str>) -> String {
    let task = task.unwrap_or("untitled");
    task.chars().take(60).collect()
}

fn format_headroom(headroom: Option<f64>) -> String {
    let Some(headroom) = headroom else {
        return "unknown".into();
    };
    if headroom.fract() == 0. {
        format!("{headroom:.0}")
    } else {
        format!("{headroom:.1}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(run_id: &str, status: &str, agent: &str, task: Option<&str>) -> RunRow {
        RunRow {
            run_id: run_id.into(),
            status: status.into(),
            agent: agent.into(),
            task: task.map(str::to_string),
            ..Default::default()
        }
    }

    fn pool(name: &str, headroom_pct: Option<f64>) -> PoolRow {
        PoolRow {
            name: name.into(),
            headroom_pct,
            window: Some("5h".into()),
            ..Default::default()
        }
    }

    fn section<'a>(brief: &'a Brief, key: &str) -> Option<&'a Section> {
        brief.sections.iter().find(|section| section.key == key)
    }

    #[test]
    fn brief_surfaces_failed_runs_completed_runs_and_low_pools() {
        let board = vec![
            run("r1", "failed", "codex", Some("broken thing")),
            run("r2", "completed", "claude", Some("finished thing")),
        ];
        let usage = vec![pool("codex_plan", Some(12.))];
        let brief = build_brief(&board, &usage, "2026-07-06");

        let attention = section(&brief, "attention").unwrap();
        assert!(attention.items.iter().any(|item| {
            item.kind == "run"
                && item.reference == "r1"
                && item.text == "codex run failed - broken thing"
        }));
        assert!(attention.items.iter().any(|item| {
            item.kind == "pool" && item.reference == "codex_plan" && item.text.contains("12%")
        }));

        let overnight = section(&brief, "overnight").unwrap();
        assert!(overnight.items.iter().any(|item| {
            item.kind == "run" && item.reference == "r2" && item.text.starts_with("claude completed")
        }));

        let pools = section(&brief, "pools").unwrap();
        assert!(pools.items.iter().any(|item| {
            item.kind == "pool" && item.reference == "codex_plan"
        }));
    }

    #[test]
    fn pricing_is_always_present_and_covers_windowed_roles() {
        let brief = build_brief(&[], &[], "2026-07-06");
        let pricing = section(&brief, "pricing").unwrap();

        assert_eq!(pricing.label, "Pricing");
        assert!(pricing.items[0].text.contains('$'));
        assert!(pricing
            .items
            .iter()
            .any(|item| item.kind == "role" && item.text == "long_horizon -> claude-fable-5"));
    }

    #[test]
    fn empty_inputs_only_emit_pricing() {
        let brief = build_brief(&[], &[], "2026-07-06");

        assert_eq!(brief.sections.len(), 1);
        assert_eq!(brief.sections[0].key, "pricing");
    }
}
