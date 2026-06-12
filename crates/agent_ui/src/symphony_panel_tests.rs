//! Tests for the symphony panel's pure data path: `extract_plans`
//! event filtering and the `toWaves` dependency-depth grouping
//! (unknown-dep + cycle edges ported from the JS semantics).

use super::*;

use super::*;

fn task(id: &str, deps: &[&str]) -> PlanTask {
    PlanTask {
        id: id.to_string(),
        task: format!("do {id}"),
        agent: "claude".to_string(),
        reason: None,
        depends_on: deps.iter().map(|d| d.to_string()).collect(),
    }
}

fn wave_ids(waves: &[Vec<PlanTask>]) -> Vec<Vec<&str>> {
    waves
        .iter()
        .map(|wave| wave.iter().map(|t| t.id.as_str()).collect())
        .collect()
}

#[test]
fn extract_plans_filters_escalate_plan_tool_events() {
    // Fixture truth: orchestrator/loop.py event dicts + judgment.py's
    // plan shape.
    let events: Vec<serde_json::Value> = serde_json::from_str(
        r#"[
            {"kind": "brain", "brain": "qwen"},
            {"kind": "tool", "name": "spawn_claude", "args": {}, "result": {"run_id": "r-1"}},
            {"kind": "tool", "name": "escalate_plan", "args": {},
             "result": {"plan": [
                 {"id": "t1", "task": "port the panel", "agent": "claude",
                  "reason": "code-heavy", "depends_on": []},
                 {"id": "t2", "task": "write the report", "agent": "gemini",
                  "depends_on": ["t1"]}
               ], "summary": "two-step port", "_judgment_cost_usd": 0.01}},
            {"kind": "tool", "name": "escalate_plan", "args": {},
             "result": {"error": "judgment tier failed: timeout"}}
        ]"#,
    )
    .unwrap();
    let plans = extract_plans(&events);
    assert_eq!(plans.len(), 1, "error results (no plan array) are skipped");
    assert_eq!(plans[0].summary, "two-step port");
    assert_eq!(plans[0].tasks.len(), 2);
    assert_eq!(plans[0].tasks[0].id, "t1");
    assert_eq!(plans[0].tasks[1].depends_on, vec!["t1"]);
    assert_eq!(plans[0].tasks[1].reason, None);
}

#[test]
fn waves_group_by_dependency_depth() {
    let waves = to_waves(&[
        task("t1", &[]),
        task("t2", &["t1"]),
        task("t3", &["t1"]),
        task("t4", &["t2", "t3"]),
    ]);
    assert_eq!(wave_ids(&waves), vec![vec!["t1"], vec!["t2", "t3"], vec!["t4"]]);
}

#[test]
fn unknown_dependency_ids_do_not_count() {
    // JS: `.filter(d => byId[d] …)` — a dep outside the plan is ignored.
    let waves = to_waves(&[task("t1", &["external-thing"]), task("t2", &["t1"])]);
    assert_eq!(wave_ids(&waves), vec![vec!["t1"], vec!["t2"]]);
}

#[test]
fn dependency_cycles_settle_like_the_js_seen_guard() {
    // t1 ⇄ t2: the seen-set guard dead-ends each side's recursion at
    // depth 1, so both land in the same band instead of recursing
    // forever (port of the JS `new Set([...seen, t.id])` semantics).
    let waves = to_waves(&[task("t1", &["t2"]), task("t2", &["t1"])]);
    assert_eq!(wave_ids(&waves), vec![vec!["t1", "t2"]]);
}

#[test]
fn no_dependencies_is_one_wave_in_insertion_order() {
    let waves = to_waves(&[task("b", &[]), task("a", &[]), task("c", &[])]);
    assert_eq!(wave_ids(&waves), vec![vec!["b", "a", "c"]]);
}
