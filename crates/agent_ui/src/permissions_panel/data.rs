//! The PERMISSIONS panel's wire types + pure predicates (Phase-2 permissions
//! design §5.1 — `data.rs` of the routines 3-file split; render lives in
//! `view.rs`, fetch/POST in `actions.rs`).
//!
//! Wire shape pinned to the LIVE bridge (probed 2026-07-11 on 4530):
//! `GET /permissions?conv=` serves `{mode: {effective, source, session,
//! project, user}, enforcement: {vendor: honesty-string}, rules: [normalized
//! + source/index provenance], config_errors: [{scope, path, error}],
//! pending: [approval rows], audit_tail: [ledger lines]}` — shaped by
//! `bridge/features/permissions/routes.py` + `engine.py`. Liberal like the
//! rest of the protocol: every field defaults so bridge drift never breaks
//! the panel.
//!
//! Honesty laws carried from the design: the `enforcement` strings are the
//! TRUST SURFACE — rendered VERBATIM, never summarized (a mode that does not
//! bind reports itself as `pending:`/`degraded:` and the panel must show
//! exactly that); `config_errors` render verbatim in amber; a refusal body
//! (`{"error"}`) reads as a refusal, never as offline.

use serde::Deserialize;

use crate::agent_accents::{STATUS_BLOCKED, STATUS_DONE, STATUS_ERROR, STATUS_IDLE};
use crate::bridge::ApprovalRow;
use crate::task_board::style::rel;

/// The three mode ids + display names (design §1.1 / Decision 8 — ids
/// `readonly|auto|full`, display "Read Only / Auto Edit / Full Access").
pub const MODES: [(&str, &str); 3] = [
    ("readonly", "Read Only"),
    ("auto", "Auto Edit"),
    ("full", "Full Access"),
];

/// The effective-mode chain with provenance (`GET /permissions .mode`,
/// engine.effective_mode — session > project > user > built-in `full`).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct ModeChain {
    #[serde(default)]
    pub effective: String,
    /// Which scope won: `session | project | user | builtin`.
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub session: Option<String>,
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub user: Option<String>,
}

/// One normalized rule row (`rules.py normalize_rule` — match keys present
/// only when set; `source`/`index` provenance always ride).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct RuleRow {
    #[serde(default)]
    pub action: String,
    /// `project | user` — the scope badge.
    #[serde(default)]
    pub source: String,
    /// Position in its scope file (`-1` = the synthetic fail-closed floor).
    #[serde(default)]
    pub index: i64,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub cwd_prefix: Option<String>,
    #[serde(default)]
    pub cwd_outside_roots: Option<bool>,
    #[serde(default)]
    pub task_contains: Option<String>,
    #[serde(default)]
    pub archetype: Option<String>,
}

/// One config error (`engine.load_scopes` / `rules.parse_file`) — the exact
/// parse/validation error VERBATIM, plus which file produced it.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct ConfigError {
    #[serde(default)]
    pub scope: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub error: String,
}

/// One audit-ledger line (`engine.audit_decision` — the append-only JSONL,
/// last 20 served oldest-first as `audit_tail`).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct AuditRow {
    #[serde(default)]
    pub ts: f64,
    #[serde(default)]
    pub approval_id: Option<String>,
    #[serde(default)]
    pub run_id: Option<String>,
    #[serde(default)]
    pub conv: Option<String>,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub verdict: String,
    /// The matched rule, rendered (`describe_rule` bridge-side).
    #[serde(default)]
    pub rule: String,
    /// `user | timeout | kill | rule`.
    #[serde(default)]
    pub decided_by: String,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub mode: Option<String>,
}

/// The full `GET /permissions` snapshot. `enforcement` stays a raw map (the
/// workspace builds serde_json with `preserve_order`, so iteration is the
/// bridge's insertion order: claude, codex, gemini) — values are the
/// per-vendor honesty strings, rendered verbatim.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct PermissionsSnapshot {
    #[serde(default)]
    pub mode: ModeChain,
    #[serde(default)]
    pub enforcement: serde_json::Map<String, serde_json::Value>,
    #[serde(default)]
    pub rules: Vec<RuleRow>,
    #[serde(default)]
    pub config_errors: Vec<ConfigError>,
    #[serde(default)]
    pub pending: Vec<ApprovalRow>,
    #[serde(default)]
    pub audit_tail: Vec<AuditRow>,
}

impl PermissionsSnapshot {
    /// The enforcement map as ordered (vendor, verbatim-string) rows;
    /// non-string values are skipped (liberal-protocol rule).
    pub fn enforcement_rows(&self) -> Vec<(String, String)> {
        self.enforcement
            .iter()
            .filter_map(|(vendor, value)| {
                value
                    .as_str()
                    .map(|text| (vendor.clone(), text.to_string()))
            })
            .collect()
    }
}

// ── pure predicates + label helpers (unit-tested, zero I/O) ──

/// Whether clicking `target` at the scope a POST would write is a real
/// change (the client-side re-guard before I/O). With a followed
/// conversation the POST writes the SESSION override; without one it writes
/// the USER default — clicking a value that scope already holds is a no-op,
/// never a redundant write.
pub(super) fn mode_change_allowed(chain: &ModeChain, session_scope: bool, target: &str) -> bool {
    if session_scope {
        chain.session.as_deref() != Some(target)
    } else {
        chain.user.as_deref() != Some(target)
    }
}

/// Whether an Allow/Deny may dispatch for `id`: the row must still be
/// pending and not already in flight (defense in depth — the action
/// re-checks this before any I/O, like the routines AMBER law).
pub(super) fn decision_allowed(
    id: &str,
    pending: &[ApprovalRow],
    in_flight: &std::collections::HashSet<String>,
) -> bool {
    !in_flight.contains(id) && pending.iter().any(|row| row.id == id)
}

/// Mirror of the bridge's `rules.describe_rule` — the human string for a
/// rule row ("ask: agent=* cwd_outside_roots"), match keys in MATCH_KEYS
/// order, "(any spawn)" when no match key is set.
pub fn describe_rule(rule: &RuleRow) -> String {
    let action = if rule.action.is_empty() {
        "?"
    } else {
        rule.action.as_str()
    };
    let mut parts = vec![format!("{action}:")];
    if let Some(agent) = &rule.agent {
        parts.push(format!("agent={agent}"));
    }
    if let Some(prefix) = &rule.cwd_prefix {
        parts.push(format!("cwd_prefix={prefix}"));
    }
    match rule.cwd_outside_roots {
        Some(true) => parts.push("cwd_outside_roots".to_string()),
        Some(false) => parts.push("cwd_outside_roots=false".to_string()),
        None => {}
    }
    if let Some(task) = &rule.task_contains {
        parts.push(format!("task_contains={task}"));
    }
    if let Some(archetype) = &rule.archetype {
        parts.push(format!("archetype={archetype}"));
    }
    if parts.len() == 1 {
        parts.push("(any spawn)".to_string());
    }
    parts.join(" ")
}

/// Fold a non-2xx response body (`{"error"}` per the serve.py law) into a
/// note — the routines idiom: a refusal reads as a refusal, not offline.
pub(super) fn refusal_note(status: u16, raw: &str) -> String {
    #[derive(Default, Deserialize)]
    struct ErrorBody {
        #[serde(default)]
        error: String,
    }
    let msg = serde_json::from_str::<ErrorBody>(raw)
        .map(|body| body.error)
        .unwrap_or_default();
    if msg.is_empty() {
        format!("refused ({status})")
    } else {
        format!("refused ({status}) — {msg}")
    }
}

/// The mode card's provenance line — the full chain with its winner
/// ("source user · session — · project — · user full").
pub(super) fn provenance_line(chain: &ModeChain) -> String {
    let val = |v: &Option<String>| v.clone().unwrap_or_else(|| "—".to_string());
    format!(
        "source {} · session {} · project {} · user {}",
        if chain.source.is_empty() {
            "—".to_string()
        } else {
            chain.source.clone()
        },
        val(&chain.session),
        val(&chain.project),
        val(&chain.user),
    )
}

/// What scope a mode click will write — stated on the card so a click is
/// never a surprise (session override with a followed conversation, the
/// user-file default without one).
pub(super) fn scope_hint(conv: Option<&str>) -> String {
    match conv {
        Some(conv) => format!("sets a session override for conversation {conv}"),
        None => "sets the user default (no conversation followed)".to_string(),
    }
}

/// Whether an enforcement honesty string describes a lane the effective
/// mode does NOT truly bind (`degraded:` / `pending:` prefixes from
/// routes.py) — rendered amber per the design.
pub(super) fn enforcement_degraded(text: &str) -> bool {
    text.starts_with("degraded") || text.starts_with("pending")
}

/// Audit verdict → color bucket (MONO ruling: safety states keep hue).
/// `deny` reads red, `ask` amber, `allow` calm-neutral; unknown future
/// verdicts degrade to idle, never invented-green.
pub(super) fn verdict_color(verdict: &str) -> gpui::Rgba {
    match verdict {
        "deny" => STATUS_ERROR,
        "ask" => STATUS_BLOCKED,
        "allow" => STATUS_DONE,
        _ => STATUS_IDLE,
    }
}

/// The pending row's TTL line ("expires in 4m" / "expired — denied by
/// default bridge-side"). Empty when the bridge served no deadline.
pub(super) fn expires_line(expires_ts: f64, now: f64) -> String {
    if expires_ts <= 0.0 {
        return String::new();
    }
    let remaining = expires_ts - now;
    if remaining <= 0.0 {
        "expired — denied by default bridge-side".to_string()
    } else {
        format!("expires in {}", rel(remaining))
    }
}

#[cfg(test)]
#[path = "data_tests.rs"]
mod tests;
