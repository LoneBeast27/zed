//! Pure-lifecycle tests for [`super::TasksIsland`] (sibling file per the
//! 500-line ceiling; a child module so private state stays reachable).

use super::*;


/// A bare island (no `db::kvp` global needed — the pure lifecycle
/// under test never touches persistence).
fn island() -> TasksIsland {
    TasksIsland {
        open: false,
        visible: false,
        emerge: AnimatedValue::settled(1., DECEL, EMERGE),
        fade: AnimatedValue::settled(0., EFFECTS, FADE),
        rows_reveal: AnimatedValue::settled(0., SPATIAL, ROWS_MORPH),
        chev: AnimatedValue::settled(0., SPATIAL, CHEV_MORPH),
        retracting: false,
        hide_task: None,
        rows: Vec::new(),
        count_roll: RollValue::new(String::new()),
    }
}

fn rows(ids: &[&str]) -> Vec<(SharedString, SharedString)> {
    ids.iter()
        .map(|id| (SharedString::from(id.to_string()), SharedString::from("t")))
        .collect()
}

#[test]
fn retract_freezes_the_last_snapshot_and_clears_only_at_hide_commit() {
    let mut island = island();
    let outcome = island.ingest(rows(&["r-1", "r-2"]));
    assert!(outcome.changed && !outcome.start_retract);
    assert!(island.visible);
    assert_eq!(island.rows.len(), 2);

    // Runs hit 0 → the retract begins; the LAST representation stays
    // frozen — the head must never read "0 subagent/tasks running"
    // over empty rows during the 400ms exit.
    let outcome = island.ingest(Vec::new());
    assert!(outcome.changed && outcome.start_retract);
    assert!(island.visible, "still in flow through the 400ms exit");
    assert!(island.retracting);
    assert_eq!(
        island.rows.len(),
        2,
        "content keeps the last snapshot through the retract — no '0 running' flash"
    );

    // Further empty ticks mid-retract neither restart the exit nor
    // wipe the frozen snapshot.
    let outcome = island.ingest(Vec::new());
    assert!(!outcome.changed && !outcome.start_retract);
    assert_eq!(island.rows.len(), 2);

    // The 400ms clock lands → only NOW the island leaves the flow and
    // the snapshot clears.
    assert!(island.commit_hide());
    assert!(!island.visible);
    assert!(!island.retracting);
    assert!(island.rows.is_empty());
}

#[test]
fn mid_retract_arrival_cancels_the_hide_and_a_stale_clock_is_a_no_op() {
    let mut island = island();
    island.ingest(rows(&["r-1"]));
    island.ingest(Vec::new());
    assert!(island.retracting);

    // A run arrives MID-retract: the island re-emerges with the fresh
    // set; the pending hide is cancelled.
    let outcome = island.ingest(rows(&["r-2"]));
    assert!(outcome.changed && !outcome.start_retract);
    assert!(!island.retracting);
    assert!(island.visible);
    assert_eq!(island.rows, rows(&["r-2"]));

    // A stale clock that somehow still fires commits nothing.
    assert!(!island.commit_hide());
    assert!(island.visible);
    assert_eq!(island.rows, rows(&["r-2"]));
}

fn run(id: &str, status: &str, task: Option<&str>, agent: &str) -> RunRow {
    RunRow {
        run_id: id.to_string(),
        status: status.to_string(),
        task: task.map(str::to_string),
        agent: agent.to_string(),
        ..Default::default()
    }
}

#[test]
fn rows_filter_running_and_fall_back_task_agent_task() {
    let board = vec![
        run("r-1", "running", Some("port wave Z3"), "claude"),
        run("r-2", "completed", Some("done thing"), "codex"),
        run("r-3", "running", None, "codex"),
        run("r-4", "running", Some(""), "gemini"),
        run("r-5", "running", None, ""),
        run("r-6", "pending", None, "agy"),
    ];
    let rows = running_rows(&board);
    let shaped: Vec<(&str, &str)> = rows
        .iter()
        .map(|(id, label)| (id.as_ref(), label.as_ref()))
        .collect();
    assert_eq!(
        shaped,
        [
            ("r-1", "port wave Z3"), // task wins
            ("r-3", "codex"),        // no task -> agent
            ("r-4", "gemini"),       // empty task -> agent (JS falsy "")
            ("r-5", "task"),         // nothing -> "task"
        ],
        "running-only, board order, task || agent || 'task' labels"
    );
}
