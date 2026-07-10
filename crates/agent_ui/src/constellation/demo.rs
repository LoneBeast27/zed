//! Staged demo scenario (board-demo.js, exact script) — NOT a product
//! surface: a scripted parallel-converge team so the constellation's
//! choreography can be verified without live runs. Active under the shared
//! agentic-demo gate ([`crate::bridge::is_agentic_demo`]): the canonical
//! `ZED_AGENTIC_DEMO=1` or the legacy `ZED_CONSTELLATION_DEMO=1` alias (this
//! panel's original var). Zero bridge involvement.

use crate::bridge::{ChannelRow, ConversationRow, OverlapRow, RunRow, RunTokens};
use crate::task_board::run_detail::{RunDetail, RunEvent};

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

/// Whether demo mode is on. Delegates to the shared agentic-demo gate — the
/// canonical `ZED_AGENTIC_DEMO=1` OR the legacy `ZED_CONSTELLATION_DEMO=1`
/// alias (this panel's original var) stages the constellation.
pub fn is_demo() -> bool {
    crate::bridge::is_agentic_demo()
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

/// Progress-note cadence in the staged drawer log (one note per 3s of run
/// time, capped so a long-open demo can't grow the log unboundedly).
const NOTE_EVERY: f64 = 3.;
const NOTE_CAP: usize = 12;

/// The staged run's DRAWER detail at `t` seconds — the same script as
/// [`demo_board`] plus canned summary/result/log content, so a demo node
/// click opens a fully-populated drawer with ZERO bridge involvement (the
/// synthetic runs don't exist on the bridge; fetching them would 404).
/// Built once per push (the panel's 1s tick), never per frame.
pub fn demo_run_detail(run_id: &str, t: f64) -> Option<RunDetail> {
    let s = SCRIPT.iter().find(|s| s.run_id == run_id)?;
    if t < s.at {
        return None;
    }
    let ended = s.end.is_some_and(|end| t >= end);
    let status = if ended { s.end_status } else { "running" };
    // Elapsed freezes at the run's end (the drawer is honest about a done
    // run's duration; only LIVE runs tick).
    let elapsed = (s.end.map_or(t, |end| t.min(end)) - s.at).max(0.);
    let weighted = s.tok0 + s.tok_rate * elapsed;

    let mut usage = serde_json::Map::new();
    usage.insert("weighted_tokens".into(), (weighted.round() as u64).into());
    usage.insert("ingest_tps".into(), (s.ingest.round() as u64).into());

    let chip = format!("{} · staged design demo · 100%", s.agent);
    let mut events = vec![
        RunEvent {
            kind: "spawn".into(),
            payload: serde_json::json!({ "archetype": s.archetype, "agent": s.agent }),
        },
        RunEvent {
            kind: "route".into(),
            payload: serde_json::json!({ "chip": chip }),
        },
    ];
    let notes = ((elapsed / NOTE_EVERY).floor() as usize).min(NOTE_CAP);
    for i in 1..=notes {
        events.push(RunEvent {
            kind: "note".into(),
            payload: serde_json::json!(format!(
                "checkpoint {i} — {} weighted tokens",
                (s.tok0 + s.tok_rate * NOTE_EVERY * i as f64).round() as u64
            )),
        });
    }
    let error = (ended && s.end_status == "failed").then(|| {
        "Staged failure: prior-art sweep hit the scripted dead end (demo scenario).".to_string()
    });
    if let Some(message) = &error {
        events.push(RunEvent {
            kind: "error".into(),
            payload: serde_json::json!(message),
        });
    }
    let text = (ended && s.end_status == "completed").then(|| {
        format!(
            "## {}\n\nStaged demo result — completed in {:.0}s with {} weighted tokens.\n\n\
             - keyframe spec handed to designer/tester crosstalk\n\
             - absorption choreography scoped for implementation",
            s.task,
            elapsed,
            weighted.round() as u64
        )
    });

    Some(RunDetail {
        run_id: s.run_id.into(),
        agent: s.agent.into(),
        status: status.into(),
        elapsed_s: elapsed,
        task: Some(s.task.into()),
        chip: Some(chip),
        archetype: Some(s.archetype.into()),
        error,
        text,
        usage: Some(usage),
        // A stable staged session id per run so the drawer head's session
        // line renders in demo mode (the field the bridge path fills from
        // `GET /run/<id>`).
        session_id: Some(format!("demo-session-{}", s.run_id)),
        events,
    })
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
        // Budget/close fields ride the wire row (channels section display);
        // the staged scenario leaves them at rest.
        ..Default::default()
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
    fn drawer_detail_speaks_the_full_summary_result_log_anatomy() {
        // Unknown runs and not-yet-spawned runs yield nothing.
        assert!(demo_run_detail("nope", 30.).is_none());
        assert!(demo_run_detail("demo-research", 1.).is_none());
        // A live run: running status, ticking elapsed, growing log, task
        // title + archetype for the header, no result yet.
        let designer = demo_run_detail("demo-designer", 10.).unwrap();
        assert_eq!(designer.status, "running");
        assert!((designer.elapsed_s - 8.8).abs() < 1e-6);
        assert_eq!(
            designer.task.as_deref(),
            Some("Designer: keyframe spec from hero recording")
        );
        assert_eq!(designer.archetype.as_deref(), Some("designer"));
        assert!(designer.text.is_none(), "no result while running");
        assert!(designer.events.len() > 2, "log grows past spawn+route");
        // Completed plan: FROZEN elapsed + canned markdown result + usage.
        let plan = demo_run_detail("demo-plan", 30.).unwrap();
        assert_eq!(plan.status, "completed");
        assert_eq!(plan.elapsed_s, 9.);
        assert!(plan.text.as_deref().unwrap().contains("Staged demo result"));
        assert!(plan.usage.as_ref().unwrap().contains_key("weighted_tokens"));
        // Failed researcher: canned error in both the field and the log.
        let research = demo_run_detail("demo-research", 30.).unwrap();
        assert_eq!(research.status, "failed");
        assert!(research.error.is_some());
        assert_eq!(research.events.last().unwrap().kind, "error");
        // Log growth is capped — a demo left open can't grow unboundedly.
        let long = demo_run_detail("demo-designer", 3600.).unwrap();
        assert!(long.events.len() <= 2 + NOTE_CAP + 1);
    }

    #[test]
    fn ctx_ramps_then_compacts_once() {
        assert_eq!(demo_convs(0.)[0].compactions, 0);
        let after = &demo_convs(23.)[0];
        assert_eq!(after.compactions, 1);
        assert!(after.ctx_tokens.unwrap() < demo_convs(21.9)[0].ctx_tokens.unwrap());
    }
}
