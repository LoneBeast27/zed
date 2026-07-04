//! Staged plan/wave data for the symphony panel under the shared agentic-demo
//! gate ([`crate::bridge::is_agentic_demo`]). NOT a product surface: a
//! believable two-wave plan whose cards span every check-off state (done /
//! running / queued / failed) plus an unlinked run, so the symphony score can
//! be reviewed without a live plan on the bridge. Decoded once (a plain
//! builder — the panel pumps its own frames off the store observer).

use crate::bridge::{PlanSnapshot, PlanSubtask, UnlinkedRun};

fn subtask(id: &str, task: &str, agent: &str, run: Option<&str>, status: &str) -> PlanSubtask {
    PlanSubtask {
        subtask_id: id.into(),
        task: task.into(),
        agent: agent.into(),
        reason: None,
        depends_on: Vec::new(),
        run_id: run.map(str::to_string),
        status: status.into(),
    }
}

/// A staged plan: wave 1 (the active band) scaffolds + parses in parallel,
/// wave 2 wires + verifies. Statuses walk done → running → queued → failed so
/// every card vocabulary + color renders; one unlinked run exercises the P6
/// banner.
pub fn demo_plan() -> PlanSnapshot {
    PlanSnapshot {
        plan_id: Some("demo-plan-7252c266".into()),
        conv: "demo".into(),
        summary: "Staged demo — two-wave port of the usage surface".into(),
        waves: vec![
            vec![
                subtask(
                    "t1",
                    "Scaffold the usage protocol fields (liveness, 429, call counts)",
                    "claude",
                    Some("demo-claude-1"),
                    "done",
                ),
                subtask(
                    "t2",
                    "Parse the /run detail session_id into the drawer head",
                    "codex",
                    Some("demo-codex-2"),
                    "running",
                ),
            ],
            vec![
                subtask(
                    "t3",
                    "Wire the liveness strip + per-pool call breakdown into the panel",
                    "gemini",
                    None,
                    "pending",
                ),
                subtask(
                    "t4",
                    "Prior-art sweep for the failure-grammar dot (researcher)",
                    "agy",
                    Some("demo-agy-4"),
                    "failed",
                ),
            ],
        ],
        unlinked_runs: vec![UnlinkedRun {
            run_id: "demo-unlinked-9".into(),
            agent: "claude".into(),
            status: "running".into(),
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_plan_is_present_and_walks_every_checkoff_state() {
        let plan = demo_plan();
        assert!(plan.is_present());
        let statuses: Vec<&str> = plan
            .waves
            .iter()
            .flatten()
            .map(|s| s.status.as_str())
            .collect();
        assert!(statuses.contains(&"done"));
        assert!(statuses.contains(&"running"));
        assert!(statuses.contains(&"pending"));
        assert!(statuses.contains(&"failed"));
        assert_eq!(plan.unlinked_runs.len(), 1);
    }
}
