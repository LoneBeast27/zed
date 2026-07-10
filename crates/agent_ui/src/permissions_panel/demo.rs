//! Staged permissions data under the shared agentic-demo gate
//! ([`crate::bridge::is_agentic_demo`]) — a believable policy state (a
//! session override over a user default, three rules with source badges,
//! every enforcement tone, TWO pending approvals, a short audit tail) so the
//! panel reviews without a live bridge. POSTs are NOT offered in demo (the
//! symphony `runnable` pattern): demo rows have no bridge to land on.

use crate::bridge::ApprovalRow;

use super::data::{AuditRow, ModeChain, PermissionsSnapshot, RuleRow};

fn now_unix() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|epoch| epoch.as_secs_f64())
        .unwrap_or(0.0)
}

/// The staged `GET /permissions` snapshot: effective `auto` via a session
/// override (user default `full`), the three-rule starter policy from the
/// design §2, all three enforcement tones (native / degraded / pending), and
/// a three-line audit tail (user allow · timeout deny · deny-at-spawn).
pub fn demo_snapshot() -> PermissionsSnapshot {
    let now = now_unix();
    let mut enforcement = serde_json::Map::new();
    enforcement.insert(
        "claude".into(),
        serde_json::Value::String(
            "native: --permission-mode acceptEdits (headless caveat: non-edit \
             tools that would prompt become tool denials the model routes around)"
                .into(),
        ),
    );
    enforcement.insert(
        "codex".into(),
        serde_json::Value::String(
            "degraded:worktree-only — windows workspace-write silently degrades \
             to read-only, so codex runs danger-full-access under worktree \
             containment (Decision 3), honestly labeled"
                .into(),
        ),
    );
    enforcement.insert(
        "gemini".into(),
        serde_json::Value::String(
            "pending: 'auto' is NOT enforced on this lane yet — vendor flag \
             translation lands Wave 2b; until then the worker launches with \
             today's full-access flags (spawn-gate rules still apply)"
                .into(),
        ),
    );
    PermissionsSnapshot {
        mode: ModeChain {
            effective: "auto".into(),
            source: "session".into(),
            session: Some("auto".into()),
            project: None,
            user: Some("full".into()),
        },
        enforcement,
        rules: vec![
            RuleRow {
                action: "ask".into(),
                source: "project".into(),
                index: 0,
                agent: Some("*".into()),
                cwd_outside_roots: Some(true),
                note: Some("escalation out of project".into()),
                ..RuleRow::default()
            },
            RuleRow {
                action: "deny".into(),
                source: "user".into(),
                index: 0,
                agent: Some("*".into()),
                cwd_prefix: Some(r"C:\Windows".into()),
                note: Some("system dir".into()),
                ..RuleRow::default()
            },
            RuleRow {
                action: "ask".into(),
                source: "user".into(),
                index: 1,
                agent: Some("gemini".into()),
                note: Some("rationed weekly window - human confirms spend".into()),
                ..RuleRow::default()
            },
        ],
        config_errors: Vec::new(),
        pending: demo_pending(),
        audit_tail: vec![
            AuditRow {
                ts: now - 2400.0,
                approval_id: Some("ap_demo01".into()),
                run_id: Some("demo-claude-3".into()),
                conv: Some("demo".into()),
                agent: Some("claude".into()),
                verdict: "allow".into(),
                rule: "ask: agent=* cwd_outside_roots".into(),
                decided_by: "user".into(),
                note: String::new(),
                mode: Some("auto".into()),
            },
            AuditRow {
                ts: now - 1500.0,
                approval_id: Some("ap_demo02".into()),
                run_id: Some("demo-gemini-5".into()),
                conv: Some("demo".into()),
                agent: Some("gemini".into()),
                verdict: "deny".into(),
                rule: "ask: agent=gemini".into(),
                decided_by: "timeout".into(),
                note: "approval request expired (300s) — denied by default".into(),
                mode: Some("auto".into()),
            },
            AuditRow {
                ts: now - 600.0,
                approval_id: None,
                run_id: Some("demo-codex-7".into()),
                conv: Some("demo".into()),
                agent: Some("codex".into()),
                verdict: "deny".into(),
                rule: r"deny: agent=* cwd_prefix=C:\Windows".into(),
                decided_by: "rule".into(),
                note: "system dir".into(),
                mode: Some("auto".into()),
            },
        ],
    }
}

/// The two staged pending approvals (design §5.1: "two pending rows") — one
/// out-of-roots codex spawn, one rationed gemini spawn, both mid-TTL.
pub fn demo_pending() -> Vec<ApprovalRow> {
    let now = now_unix();
    vec![
        ApprovalRow {
            id: "ap_demo11".into(),
            run_id: "demo-codex-9".into(),
            conv: "demo".into(),
            agent: "codex".into(),
            task_head: "deploy the arr stack config to the media box".into(),
            cwd: r"L:\Projects\other".into(),
            mode: "auto".into(),
            rule: "ask: agent=* cwd_outside_roots".into(),
            created_ts: now - 40.0,
            expires_ts: now + 260.0,
        },
        ApprovalRow {
            id: "ap_demo12".into(),
            run_id: "demo-gemini-11".into(),
            conv: "demo".into(),
            agent: "gemini".into(),
            task_head: "prior-art sweep for the failure-grammar dot".into(),
            cwd: r"L:\Projects\agentic-ide".into(),
            mode: "auto".into(),
            rule: "ask: agent=gemini".into(),
            created_ts: now - 120.0,
            expires_ts: now + 180.0,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permissions_panel::data::describe_rule;

    #[test]
    fn demo_snapshot_is_believable_and_complete() {
        let snapshot = demo_snapshot();
        // Mode chain internally consistent: session override wins.
        assert_eq!(snapshot.mode.effective, "auto");
        assert_eq!(snapshot.mode.source, "session");
        assert_eq!(snapshot.mode.session.as_deref(), Some("auto"));
        assert_eq!(snapshot.mode.user.as_deref(), Some("full"));
        // All three enforcement tones present, wire order.
        let rows = snapshot.enforcement_rows();
        assert_eq!(rows.len(), 3);
        assert!(rows[0].1.starts_with("native:"));
        assert!(rows[1].1.starts_with("degraded:"));
        assert!(rows[2].1.starts_with("pending:"));
        // Rules carry both source badges and render like the bridge.
        assert_eq!(snapshot.rules.len(), 3);
        assert!(snapshot.rules.iter().any(|r| r.source == "project"));
        assert!(snapshot.rules.iter().any(|r| r.source == "user"));
        assert_eq!(
            describe_rule(&snapshot.rules[0]),
            "ask: agent=* cwd_outside_roots"
        );
        // Two pending rows, both mid-TTL; audit tail spans user/timeout/rule.
        assert_eq!(snapshot.pending.len(), 2);
        assert!(snapshot.pending.iter().all(|row| row.expires_ts > row.created_ts));
        let deciders: Vec<&str> = snapshot
            .audit_tail
            .iter()
            .map(|row| row.decided_by.as_str())
            .collect();
        assert_eq!(deciders, ["user", "timeout", "rule"]);
    }
}
