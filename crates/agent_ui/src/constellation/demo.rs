//! Staged demo scenario (board-demo.js, exact script) — NOT a product
//! surface: a scripted parallel-converge team so the constellation's
//! choreography can be verified without live runs. Active when the panel is
//! built with `ZED_CONSTELLATION_DEMO=1` in the environment (the native
//! `?demo=1`). Zero bridge involvement.

use crate::bridge::{ChannelRow, ConversationRow, OverlapRow, RunRow, RunTokens};

struct Staged {
    at: f64,
    end: Option<f64>,
    end_status: &'static str,
    archetype: &'static str,
    tok0: f64,
    tok_rate: f64,
    ingest: f64,
    run_id: &'static str,
    agent: &'static str,
    task: &'static str,
}

const SCRIPT: [Staged; 6] = [
    Staged {
        at: 0.0,
        end: Some(9.),
        end_status: "completed",
        archetype: "orchestrator",
        tok0: 2000.,
        tok_rate: 900.,
        ingest: 420.,
        run_id: "demo-plan",
        agent: "claude",
        task: "Plan: lifecycle panel motion spec",
    },
    Staged {
        at: 1.2,
        end: None,
        end_status: "",
        archetype: "designer",
        tok0: 0.,
        tok_rate: 1400.,
        ingest: 800.,
        run_id: "demo-designer",
        agent: "gemini",
        task: "Designer: keyframe spec from hero recording",
    },
    Staged {
        at: 2.4,
        end: None,
        end_status: "",
        archetype: "implementation",
        tok0: 0.,
        tok_rate: 2600.,
        ingest: 1200.,
        run_id: "demo-impl-a",
        agent: "claude",
        task: "Implementation: absorption choreography",
    },
    Staged {
        at: 3.6,
        end: None,
        end_status: "",
        archetype: "implementation",
        tok0: 0.,
        tok_rate: 1100.,
        ingest: 600.,
        run_id: "demo-impl-b",
        agent: "codex",
        task: "Implementation: boost-edge particle",
    },
    Staged {
        at: 4.8,
        end: None,
        end_status: "",
        archetype: "tester",
        tok0: 0.,
        tok_rate: 700.,
        ingest: 500.,
        run_id: "demo-tester",
        agent: "claude",
        task: "Tester: frame-trace fluidity gate",
    },
    Staged {
        at: 6.0,
        end: Some(16.),
        end_status: "failed",
        archetype: "researcher",
        tok0: 0.,
        tok_rate: 0., // agy: no token surface — floor-size dot
        ingest: 0.,
        run_id: "demo-research",
        agent: "agy",
        task: "Researcher: motion prior-art sweep",
    },
];

/// Whether demo mode is on (`ZED_CONSTELLATION_DEMO=1`).
pub fn is_demo() -> bool {
    std::env::var("ZED_CONSTELLATION_DEMO").is_ok_and(|v| !v.is_empty() && v != "0")
}

/// The staged board at `t` seconds since panel build.
pub fn demo_board(t: f64) -> Vec<RunRow> {
    SCRIPT
        .iter()
        .filter(|s| t >= s.at)
        .map(|s| {
            let upto = s.end.map_or(t, |end| t.min(end));
            let weighted = s.tok0 + s.tok_rate * (upto - s.at).max(0.);
            RunRow {
                run_id: s.run_id.into(),
                agent: s.agent.into(),
                task: Some(s.task.into()),
                conv: "demo".into(),
                status: match s.end {
                    Some(end) if t >= end => s.end_status.into(),
                    _ => "running".into(),
                },
                elapsed_s: t - s.at,
                chip: Some(format!("{} · staged design demo · 100%", s.agent)),
                archetype: Some(s.archetype.into()),
                tokens: Some(RunTokens {
                    weighted,
                    ingest: s.ingest,
                }),
            }
        })
        .collect()
}

// Staged boost channel: designer↔tester crosstalk — opens at 6.5s, one batch
// every ~3s, converged at k=5 (edge retracts, badge absorbs). States walk
// the tri-class taxonomy (alive/held/terminal).
const CH_OPEN: f64 = 6.5;
const CH_EVERY: f64 = 3.;
const CH_MAX: u64 = 5;

pub fn demo_channels(t: f64) -> Vec<ChannelRow> {
    if t < CH_OPEN {
        return Vec::new();
    }
    let k = (((t - CH_OPEN) / CH_EVERY).floor() as u64).min(CH_MAX);
    let since = (t - CH_OPEN) % CH_EVERY;
    let state = if k >= CH_MAX {
        "converged"
    } else if k == 0 {
        "awaiting_a"
    } else if since < 1.1 {
        "working"
    } else if k % 2 == 1 {
        "awaiting_b"
    } else {
        "awaiting_a"
    };
    vec![ChannelRow {
        channel_id: "demo-ch1".into(),
        conv: "demo".into(),
        a: "demo-designer".into(),
        b: "demo-tester".into(),
        state: state.into(),
        k,
        max_batches: CH_MAX,
        reason: Some("unify keyframe spec with frame-trace verdicts".into()),
        last_from: Some(if k % 2 == 1 {
            "demo-designer".into()
        } else {
            "demo-tester".into()
        }),
    }]
}

/// Staged overlap venn rows: the two implementation tracks share artifacts
/// (score 3); designer↔tester share the keyframe spec (score 2).
pub fn demo_overlap() -> Vec<OverlapRow> {
    vec![
        OverlapRow {
            a: "demo-impl-a".into(),
            b: "demo-impl-b".into(),
            score: 3.,
        },
        OverlapRow {
            a: "demo-designer".into(),
            b: "demo-tester".into(),
            score: 2.,
        },
    ]
}

// Staged conversation meta: the root's ctx ramp with ONE compaction at
// t=22s (28K → ~8.6K) so the exhale + inward ring shows end to end.
const CTX0: f64 = 2600.;
const CTX_RATE: f64 = 1150.;
const COMPACT_AT: f64 = 22.;
const CTX_AFTER: f64 = 8600.;

pub fn demo_convs(t: f64) -> Vec<ConversationRow> {
    let compacted = t >= COMPACT_AT;
    vec![ConversationRow {
        id: "demo".into(),
        title: "Design demo".into(),
        busy: true,
        ctx_tokens: Some(if compacted {
            CTX_AFTER + 320. * (t - COMPACT_AT)
        } else {
            CTX0 + CTX_RATE * t
        }),
        compactions: u64::from(compacted),
        ..Default::default()
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_stages_every_state_the_panel_must_speak() {
        assert!(demo_board(0.).len() == 1, "plan alone at t=0");
        let board = demo_board(30.);
        assert_eq!(board.len(), 6, "full team by t=30");
        let by_id = |id: &str| board.iter().find(|r| r.run_id == id).unwrap();
        assert_eq!(by_id("demo-plan").status, "completed");
        assert_eq!(by_id("demo-research").status, "failed");
        assert_eq!(by_id("demo-designer").status, "running");
        // Mass freezes at a run's end, ramps while working.
        let plan = by_id("demo-plan").tokens.unwrap();
        assert_eq!(plan.weighted, 2000. + 900. * 9.);
        let impl_a = by_id("demo-impl-a").tokens.unwrap();
        assert!(impl_a.weighted > 60_000.);
    }

    #[test]
    fn channel_walks_open_working_awaiting_converged() {
        assert!(demo_channels(6.).is_empty());
        assert_eq!(demo_channels(6.6)[0].state, "awaiting_a");
        assert_eq!(demo_channels(CH_OPEN + CH_EVERY + 0.5)[0].state, "working");
        let done = demo_channels(CH_OPEN + CH_EVERY * 6.)[0].clone();
        assert_eq!(done.state, "converged");
        assert_eq!(done.k, CH_MAX);
    }

    #[test]
    fn ctx_ramps_then_compacts_once() {
        assert_eq!(demo_convs(0.)[0].compactions, 0);
        let after = &demo_convs(23.)[0];
        assert_eq!(after.compactions, 1);
        assert!(after.ctx_tokens.unwrap() < demo_convs(21.9)[0].ctx_tokens.unwrap());
    }
}
