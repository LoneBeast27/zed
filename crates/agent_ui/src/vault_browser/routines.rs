//! The ROUTINES view — wire types + policy + state (the fetch/POST actions
//! live in `routines_actions.rs`, the render half in `routines_view.rs`;
//! split at the 500-line ceiling): the bridge's croniter daemon surfaced for
//! review (`GET /routines`) with per-row Fire-now (`POST /routines/<id>/
//! run-now`), Enable/Disable (`…/toggle`) and the AMBER consent commit
//! (`…/consent`).
//!
//! Wire shape pinned to the live bridge (probed 2026-07-10): rows carry
//! `{id, title, status active|paused|disabled, valid, problems[], when,
//! next_fire_in, last: {at, error, skipped, skip_reason, count},
//! hack: {verdict, consent_ok, verify_first, banked, guards[],
//! ping: {vendor, last: {status, latency_ms}}} | null, health}`.
//!
//! AMBER LAW (TOKEN_CONSOLE §2.4): a routine whose consent is not granted
//! (`hack.verdict == "amber" && !consent_ok`) NEVER fires from this client —
//! [`fire_allowed`] gates both the button render and the action itself
//! (defense in depth); the row renders the amber Consent affordance instead.
//!
//! Honesty laws (the axioms-view degrade contract): a run-now response is
//! rendered VERBATIM (`fired` / `skipped — <reason>`, e.g. a fail-closed
//! guard); a 4xx/5xx refusal renders the bridge's own `{"error": …}` text;
//! a failed fetch keeps the last-known snapshot under a stale note; a bridge
//! never reached reads "Routines unavailable" — no fake data, ever.

use std::collections::{HashMap, HashSet};

use gpui::Task;
use serde::Deserialize;

/// One `/routines` row. Unknown/absent fields default (a plain routine has
/// `hack: null`; an invalid one still carries every key).
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct RoutineRow {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub title: String,
    /// Frontmatter claim: `active | paused | disabled`.
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub valid: bool,
    #[serde(default)]
    pub problems: Vec<String>,
    /// The human-words schedule ("every 30m", "daily 23:00"; "" = unscheduled).
    #[serde(default)]
    pub when: String,
    /// "in 10m" — pre-rendered by the bridge, `null` when unscheduled.
    #[serde(default)]
    pub next_fire_in: Option<String>,
    #[serde(default)]
    pub last: LastFire,
    /// The console block — `null` on plain (non-hack) routines.
    #[serde(default)]
    pub hack: Option<HackBlock>,
    /// The bridge's health claim: ok | firing | late | failed | disabled |
    /// invalid | banned | banked | continuous | manual | warning.
    #[serde(default)]
    pub health: String,
}

/// The sidecar's last-fire ledger for a row.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct LastFire {
    /// Unix seconds of the last dispatched fire.
    #[serde(default)]
    pub at: Option<f64>,
    /// The last fire's error text (`null` = it dispatched clean).
    #[serde(default)]
    pub error: Option<String>,
    /// Unix seconds of the last gated/missed SKIP.
    #[serde(default)]
    pub skipped: Option<f64>,
    #[serde(default)]
    pub skip_reason: Option<String>,
    #[serde(default)]
    pub count: u64,
}

/// The `type: hack` console block (verdict + consent + guards + ping).
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct HackBlock {
    /// green | amber | red.
    #[serde(default)]
    pub verdict: String,
    /// Missing key deserializes to `false` — fail closed, per AMBER LAW.
    #[serde(default)]
    pub consent_ok: bool,
    #[serde(default)]
    pub verify_first: bool,
    #[serde(default)]
    pub banked: bool,
    #[serde(default)]
    pub guards: Vec<String>,
    #[serde(default)]
    pub ping: Option<PingBlock>,
}

/// The liveness inter-ping block (present only on ping hacks).
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct PingBlock {
    #[serde(default)]
    pub vendor: String,
    #[serde(default)]
    pub last: Option<PingResult>,
}

/// The last recorded ping outcome.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct PingResult {
    /// ok | error | timeout | 429.
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub latency_ms: Option<f64>,
}

/// The full `GET /routines` snapshot (counts/vault are served but unused).
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct RoutinesSnapshot {
    #[serde(default)]
    pub routines: Vec<RoutineRow>,
}

// ── AMBER LAW + fire gating (pure — unit-tested) ──

/// Whether the amber Consent affordance must show: an AMBER-verdict hack
/// whose consent is missing or stale. GREEN needs none; RED is banned.
pub(super) fn consent_required(row: &RoutineRow) -> bool {
    row.hack
        .as_ref()
        .is_some_and(|h| h.verdict == "amber" && !h.consent_ok)
}

/// Whether Fire-now may render AND dispatch (defense in depth — the action
/// re-checks this). Invalid rows have no actions; banned never fires; an
/// unconsented AMBER hack is BLOCKED client-side until consent lands.
pub(super) fn fire_allowed(row: &RoutineRow) -> bool {
    row.valid && row.health != "banned" && !consent_required(row)
}

/// Whether the Enable/Disable toggle renders. Banned rows are never
/// toggleable (the bridge 409s); invalid rows get no actions at all.
pub(super) fn toggle_allowed(row: &RoutineRow) -> bool {
    row.valid && row.health != "banned"
}

/// Fold a `POST run-now` 200 body into the row note — HONEST: a skip shows
/// its reason verbatim ("fail closed" and friends), a fire says fired.
pub(super) fn fire_result_note(raw: &str) -> String {
    #[derive(Default, Deserialize)]
    struct FireResult {
        #[serde(default)]
        fired: bool,
        #[serde(default)]
        skipped: Option<String>,
    }
    match serde_json::from_str::<FireResult>(raw) {
        Ok(result) if result.fired => "fired".to_string(),
        Ok(result) => match result.skipped {
            Some(reason) => format!("skipped — {reason}"),
            None => "skipped".to_string(),
        },
        // Undecodable 200 — show the raw body rather than invent a state.
        Err(_) => raw.trim().to_string(),
    }
}

/// Fold a non-2xx response body (`{"error": …}` per serve.py) into the note.
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

/// The three row POSTs (one path builder, no format-string drift).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RoutineAction {
    RunNow,
    Toggle,
    Consent,
}

impl RoutineAction {
    pub(super) fn endpoint(self) -> &'static str {
        match self {
            RoutineAction::RunNow => "run-now",
            RoutineAction::Toggle => "toggle",
            RoutineAction::Consent => "consent",
        }
    }
}

/// Routines-view state riding the panel — the axioms lifecycle shape: last
/// good snapshot survives a failed refetch; per-row in-flight marks stop
/// double-POSTs; per-row notes carry the last action outcome verbatim.
#[derive(Default)]
pub struct RoutinesState {
    /// `None` until the first fetch lands (loading / unavailable states).
    pub(super) snapshot: Option<RoutinesSnapshot>,
    /// The LAST fetch failed or the bridge dropped mid-action — stale note.
    pub(super) stale: bool,
    /// A fetch is in flight (first open shows "Loading…", not a blank).
    pub(super) loading: bool,
    /// Routine ids with a POST in flight (actions collapse to "working…").
    pub(super) in_flight: HashSet<String>,
    /// Last action outcome per routine id (fired / skipped — … / refused …).
    pub(super) notes: HashMap<String, String>,
    /// The in-flight fetch (kept so it isn't dropped/cancelled). Written by
    /// `routines_actions.rs` (the fetch lives with the POST path).
    pub(super) _task: Option<Task<()>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Trimmed from the live 2026-07-10 probe: a consented GREEN ping hack.
    const LIVE_PING_ROW: &str = r#"{
        "id": "01KWNZX8V692W7XDHPZZGKKBSS",
        "title": "Gemini liveness ping (zero-spend relay probe)",
        "project": null, "path": "global/routines/console/h20.md",
        "status": "active", "valid": true, "problems": [], "warnings": [],
        "when": "every 30m", "crons": ["*/30 * * * *"], "on_missed": "skip",
        "api": null, "do": "LIVENESS PROBE", "next_fire": 1783686600.0,
        "next_fire_in": "in 10m",
        "last": {"at": 1783684804.8, "reason": "schedule", "error": null,
                 "skipped": 1783661400.0,
                 "skip_reason": "headroom_gt: pool unreadable (fail closed)",
                 "conv": null, "count": 166},
        "hack": {"pool": "gemini", "verdict": "green", "tos": "…",
                 "benefit": "…", "mode": "scheduled", "params": {},
                 "verify_first": false, "banked": false, "consent_ok": true,
                 "definition_hash": "c6c33d8b5fc7a13c", "guards": [],
                 "auto_disabled": null,
                 "ping": {"vendor": "gemini", "verified_by": null,
                          "last": {"vendor": "gemini", "verified_by": null,
                                   "status": "ok", "http_code": null,
                                   "latency_ms": 6051.3, "at": 1783684804.8}}},
        "health": "ok"}"#;

    #[test]
    fn parses_the_live_row_shape() {
        let row: RoutineRow = serde_json::from_str(LIVE_PING_ROW).unwrap();
        assert_eq!(row.id, "01KWNZX8V692W7XDHPZZGKKBSS");
        assert_eq!(row.status, "active");
        assert_eq!(row.when, "every 30m");
        assert_eq!(row.next_fire_in.as_deref(), Some("in 10m"));
        assert_eq!(row.last.count, 166);
        assert_eq!(
            row.last.skip_reason.as_deref(),
            Some("headroom_gt: pool unreadable (fail closed)")
        );
        let hack = row.hack.as_ref().unwrap();
        assert!(hack.consent_ok);
        let ping = hack.ping.as_ref().unwrap();
        assert_eq!(ping.vendor, "gemini");
        assert_eq!(ping.last.as_ref().unwrap().status, "ok");
        assert_eq!(ping.last.as_ref().unwrap().latency_ms, Some(6051.3));
        assert_eq!(row.health, "ok");
    }

    #[test]
    fn parses_the_empty_snapshot_and_plain_rows() {
        let snapshot: RoutinesSnapshot = serde_json::from_str(
            r#"{"routines": [], "counts": {"active": 0, "invalid": 0, "firing": 0},
                "vault": "L:\\Projects\\atlas-vault"}"#,
        )
        .unwrap();
        assert!(snapshot.routines.is_empty());
        // A plain routine: hack null, never fired.
        let row: RoutineRow = serde_json::from_str(
            r#"{"id": "r1", "title": "Morning brief", "status": "disabled",
                "valid": true, "problems": [], "when": "daily 07:00",
                "next_fire_in": null,
                "last": {"at": null, "reason": null, "error": null,
                         "skipped": null, "skip_reason": null, "conv": null,
                         "count": 0},
                "hack": null, "health": "disabled"}"#,
        )
        .unwrap();
        assert!(row.hack.is_none());
        assert_eq!(row.last.at, None);
        assert!(fire_allowed(&row), "a plain valid routine is fireable");
    }

    fn amber_row(consent_ok: bool) -> RoutineRow {
        RoutineRow {
            id: "a1".into(),
            valid: true,
            status: "disabled".into(),
            health: "disabled".into(),
            hack: Some(HackBlock {
                verdict: "amber".into(),
                consent_ok,
                ..HackBlock::default()
            }),
            ..RoutineRow::default()
        }
    }

    #[test]
    fn amber_law_blocks_fire_until_consent() {
        let unconsented = amber_row(false);
        assert!(consent_required(&unconsented));
        assert!(!fire_allowed(&unconsented), "AMBER LAW: no consent, no fire");
        assert!(toggle_allowed(&unconsented), "toggle stays (bridge 409s enable)");

        let consented = amber_row(true);
        assert!(!consent_required(&consented));
        assert!(fire_allowed(&consented));

        // consent_ok MISSING from the wire → defaults false → still blocked.
        let hack: HackBlock = serde_json::from_str(r#"{"verdict": "amber"}"#).unwrap();
        assert!(!hack.consent_ok, "missing consent key fails CLOSED");
    }

    #[test]
    fn green_and_red_verdicts_gate_correctly() {
        let mut green = amber_row(false);
        green.hack.as_mut().unwrap().verdict = "green".into();
        assert!(!consent_required(&green), "green needs no consent");
        assert!(fire_allowed(&green));

        let mut banned = amber_row(true);
        banned.health = "banned".into();
        assert!(!fire_allowed(&banned), "banned never fires");
        assert!(!toggle_allowed(&banned), "banned is never toggleable");

        let invalid = RoutineRow {
            valid: false,
            problems: vec!["cron: bad field".into()],
            health: "invalid".into(),
            ..RoutineRow::default()
        };
        assert!(!fire_allowed(&invalid));
        assert!(!toggle_allowed(&invalid), "invalid rows carry no actions");
    }

    #[test]
    fn fire_result_note_is_verbatim() {
        assert_eq!(
            fire_result_note(r#"{"fired": true, "conv": "c1", "reason": "run-now"}"#),
            "fired"
        );
        // The live fail-closed skip shape — reason shown VERBATIM.
        assert_eq!(
            fire_result_note(
                r#"{"fired": false,
                    "skipped": "headroom_gt: pool 'gemini_free_rpd' unreadable (fail closed)",
                    "reason": "run-now"}"#
            ),
            "skipped — headroom_gt: pool 'gemini_free_rpd' unreadable (fail closed)"
        );
        assert_eq!(fire_result_note(r#"{"fired": false}"#), "skipped");
        // Undecodable 200 → raw body, never an invented state.
        assert_eq!(fire_result_note("weird"), "weird");
    }

    #[test]
    fn refusal_note_surfaces_the_bridge_error() {
        assert_eq!(
            refusal_note(
                409,
                r#"{"error": "amber hack — consent required before enable (POST /routines/<id>/consent)"}"#
            ),
            "refused (409) — amber hack — consent required before enable (POST /routines/<id>/consent)"
        );
        assert_eq!(
            refusal_note(404, r#"{"error": "unknown routine"}"#),
            "refused (404) — unknown routine"
        );
        assert_eq!(refusal_note(500, "not json"), "refused (500)");
    }
}
