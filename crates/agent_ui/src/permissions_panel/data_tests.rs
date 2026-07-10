//! Tests for the permissions wire types + pure predicates (`data.rs`),
//! pinned to the LIVE `GET /permissions` probe (2026-07-11) + the
//! routes.py/engine.py row shapes.

use super::*;
use std::collections::HashSet;

/// Fixture truth: the LIVE `GET /permissions` body, probed 2026-07-11
/// (curl http://127.0.0.1:4530/permissions, default config).
const LIVE_BODY: &str = r#"{
    "mode": {"effective": "full", "source": "builtin", "session": null,
             "project": null, "user": null},
    "enforcement": {
        "claude": "host-implicit: no flag passed — full access rides the host settings (Decision 4 staged); containment = worktree isolation + host safety classifier",
        "codex": "native: sandbox=danger-full-access approvalPolicy=never (today's exact params); containment = worktree isolation",
        "gemini": "host-implicit: no flag passed (closed harness); containment = worktree isolation + host safety classifier"},
    "rules": [], "config_errors": [], "pending": [], "audit_tail": []}"#;

#[test]
fn live_probe_body_decodes() {
    let snapshot: PermissionsSnapshot = serde_json::from_str(LIVE_BODY).unwrap();
    assert_eq!(snapshot.mode.effective, "full");
    assert_eq!(snapshot.mode.source, "builtin");
    assert_eq!(snapshot.mode.session, None);
    let rows = snapshot.enforcement_rows();
    // preserve_order: wire order (claude, codex, gemini), never sorted.
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].0, "claude");
    assert!(rows[0].1.starts_with("host-implicit:"));
    assert_eq!(rows[1].0, "codex");
    assert!(rows[1].1.starts_with("native:"));
    assert_eq!(rows[2].0, "gemini");
    assert!(snapshot.rules.is_empty());
    assert!(snapshot.audit_tail.is_empty());
}

#[test]
fn populated_snapshot_decodes_rules_errors_pending_audit() {
    // Shaped per routes.py/engine.py: normalized rule + provenance, a
    // parse error verbatim, a pending row, one ledger line.
    let snapshot: PermissionsSnapshot = serde_json::from_str(
        r#"{
        "mode": {"effective": "auto", "source": "session",
                 "session": "auto", "project": null, "user": "full"},
        "enforcement": {"claude": "pending: 'auto' is NOT enforced on this lane yet — vendor flag translation lands Wave 2b"},
        "rules": [
            {"action": "ask", "source": "project", "index": 0,
             "agent": "*", "cwd_outside_roots": true,
             "note": "escalation out of project"},
            {"action": "deny", "source": "user", "index": 1,
             "cwd_prefix": "C:\\Windows", "unknown_future_key": 7}
        ],
        "config_errors": [{"scope": "user",
            "path": "L:\\Projects\\agentic-ide\\.permissions.json",
            "error": "Expecting ',' delimiter: line 4 column 3 (char 61)"}],
        "pending": [{"id": "ap_9f2c1e", "run_id": "codex-1", "conv": "c-9",
            "agent": "codex", "task_head": "deploy the arr stack",
            "cwd": "L:\\Projects\\other", "mode": "auto",
            "rule": "ask: agent=* cwd_outside_roots",
            "created_ts": 1783100000.5, "expires_ts": 1783100300.5}],
        "audit_tail": [{"ts": 1783100100.0, "approval_id": "ap_9f2c1e",
            "run_id": "codex-1", "conv": "c-9", "agent": "codex",
            "verdict": "deny", "rule": "ask: agent=* cwd_outside_roots",
            "decided_by": "timeout",
            "note": "approval request expired (300s)", "mode": "auto"}]}"#,
    )
    .unwrap();
    assert_eq!(snapshot.mode.session.as_deref(), Some("auto"));
    assert_eq!(snapshot.rules.len(), 2);
    assert_eq!(snapshot.rules[0].source, "project");
    assert_eq!(snapshot.rules[0].cwd_outside_roots, Some(true));
    // Unknown rule keys drop silently — the tolerance law.
    assert_eq!(snapshot.rules[1].cwd_prefix.as_deref(), Some(r"C:\Windows"));
    assert_eq!(
        snapshot.config_errors[0].error,
        "Expecting ',' delimiter: line 4 column 3 (char 61)"
    );
    assert_eq!(snapshot.pending[0].id, "ap_9f2c1e");
    let audit = &snapshot.audit_tail[0];
    assert_eq!(audit.verdict, "deny");
    assert_eq!(audit.decided_by, "timeout");
    assert_eq!(audit.mode.as_deref(), Some("auto"));
    // A bare snapshot degrades to defaults, never errors.
    let bare: PermissionsSnapshot = serde_json::from_str("{}").unwrap();
    assert_eq!(bare, PermissionsSnapshot::default());
}

#[test]
fn describe_rule_mirrors_the_bridge_renderer() {
    let rule: RuleRow = serde_json::from_str(
        r#"{"action": "ask", "source": "project", "index": 0,
            "agent": "*", "cwd_outside_roots": true}"#,
    )
    .unwrap();
    assert_eq!(describe_rule(&rule), "ask: agent=* cwd_outside_roots");
    let deny: RuleRow = serde_json::from_str(
        r#"{"action": "deny", "source": "user", "index": 1,
            "cwd_prefix": "C:\\Windows", "task_contains": "deploy"}"#,
    )
    .unwrap();
    assert_eq!(
        describe_rule(&deny),
        r"deny: cwd_prefix=C:\Windows task_contains=deploy"
    );
    // No match keys → the bridge's "(any spawn)" wildcard phrasing.
    let bare: RuleRow =
        serde_json::from_str(r#"{"action": "allow", "source": "user", "index": 2}"#).unwrap();
    assert_eq!(describe_rule(&bare), "allow: (any spawn)");
}

#[test]
fn refusal_note_surfaces_the_bridge_error_verbatim() {
    assert_eq!(
        refusal_note(409, r#"{"error": "already resolved: deny"}"#),
        "refused (409) — already resolved: deny"
    );
    assert_eq!(
        refusal_note(
            400,
            r#"{"error": "unknown mode 'yolo' — expected one of readonly|auto|full"}"#
        ),
        "refused (400) — unknown mode 'yolo' — expected one of readonly|auto|full"
    );
    assert_eq!(refusal_note(500, "not json"), "refused (500)");
}

#[test]
fn mode_change_is_guarded_per_scope() {
    let chain = ModeChain {
        effective: "auto".into(),
        source: "session".into(),
        session: Some("auto".into()),
        project: None,
        user: Some("full".into()),
    };
    // Session scope: re-clicking the held override is a no-op…
    assert!(!mode_change_allowed(&chain, true, "auto"));
    // …but any other target is a real change.
    assert!(mode_change_allowed(&chain, true, "full"));
    assert!(mode_change_allowed(&chain, true, "readonly"));
    // User scope: the user file already says full.
    assert!(!mode_change_allowed(&chain, false, "full"));
    assert!(mode_change_allowed(&chain, false, "auto"));
    // Builtin-only chain (nothing explicit anywhere): every click at
    // either scope is a real (explicit) write — even "full".
    let builtin = ModeChain {
        effective: "full".into(),
        source: "builtin".into(),
        ..ModeChain::default()
    };
    assert!(mode_change_allowed(&builtin, true, "full"));
    assert!(mode_change_allowed(&builtin, false, "full"));
}

#[test]
fn decisions_require_a_live_pending_row_and_no_inflight() {
    let pending = vec![ApprovalRow {
        id: "ap_1".into(),
        ..ApprovalRow::default()
    }];
    let mut in_flight = HashSet::new();
    assert!(decision_allowed("ap_1", &pending, &in_flight));
    // Resolved elsewhere (row left the set) → refuse before I/O.
    assert!(!decision_allowed("ap_gone", &pending, &in_flight));
    // Already in flight → a double-click must not double-POST.
    in_flight.insert("ap_1".to_string());
    assert!(!decision_allowed("ap_1", &pending, &in_flight));
}

#[test]
fn enforcement_tone_flags_degraded_and_pending_lanes() {
    assert!(enforcement_degraded(
        "degraded:worktree-only — windows workspace-write silently degrades"
    ));
    assert!(enforcement_degraded(
        "pending: 'auto' is NOT enforced on this lane yet"
    ));
    assert!(!enforcement_degraded("native: --permission-mode plan"));
    assert!(!enforcement_degraded("host-implicit: no flag passed"));
}

#[test]
fn verdict_colors_keep_safety_hue_only() {
    assert_eq!(verdict_color("deny"), STATUS_ERROR);
    assert_eq!(verdict_color("ask"), STATUS_BLOCKED);
    assert_eq!(verdict_color("allow"), STATUS_DONE);
    // Unknown future verdicts degrade to idle, never invented-green.
    assert_eq!(verdict_color("someday-new"), STATUS_IDLE);
}

#[test]
fn expires_line_counts_down_and_fails_closed_wording() {
    let now = 1_783_100_000.0;
    assert_eq!(expires_line(now + 240.0, now), "expires in 4m");
    assert_eq!(expires_line(now + 34.0, now), "expires in 34s");
    assert_eq!(
        expires_line(now - 1.0, now),
        "expired — denied by default bridge-side"
    );
    // No deadline served → no invented countdown.
    assert_eq!(expires_line(0.0, now), "");
}

#[test]
fn provenance_and_scope_lines_are_exact() {
    let builtin = ModeChain {
        effective: "full".into(),
        source: "builtin".into(),
        ..ModeChain::default()
    };
    assert_eq!(
        provenance_line(&builtin),
        "source builtin · session — · project — · user —"
    );
    let chain = ModeChain {
        effective: "auto".into(),
        source: "session".into(),
        session: Some("auto".into()),
        project: None,
        user: Some("full".into()),
    };
    assert_eq!(
        provenance_line(&chain),
        "source session · session auto · project — · user full"
    );
    assert_eq!(
        scope_hint(Some("c-9")),
        "sets a session override for conversation c-9"
    );
    assert_eq!(
        scope_hint(None),
        "sets the user default (no conversation followed)"
    );
}
