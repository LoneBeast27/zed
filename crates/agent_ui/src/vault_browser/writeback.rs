//! The WRITEBACK contract — wire types + the pure fold, pinned to the live
//! bridge (`bridge/features/writeback/{routes,engine}.py`, read 2026-07-10):
//!
//! - `GET /writeback` → `{candidates: [...], counts: {pending, conflicts}}`,
//!   newest first. Two candidate kinds ride the one list:
//!   - `kind: "run"` — a completed run's digest/result staged as a note
//!     candidate: `{id: "run-<run_id>", run_id, conv, agent, project, title,
//!     via: "digest"|"result", preview, ts}`. Accepting writes a NEW note
//!     under `projects/<slug>/knowledge/notes/`.
//!   - `kind: "conflict"` — an import hand-edit conflict (the target note
//!     drifted since the vendor copy was cut): `{id: "conf-<sha12>", project,
//!     path, incoming, title, source, reason, actions: ["keep-mine",
//!     "take-theirs"], ts}`. The bodies are NOT on the wire (staged text
//!     stays server-side) — the modal reads both sides from the local vault.
//! - `POST /writeback/<id>/accept` `{mode?}` — run: mode ignored →
//!   `{verdict: "accepted", note}`; conflict: mode REQUIRED (`keep-mine` |
//!   `take-theirs`) → 400 without one. 404 = unknown OR gone-stale candidate
//!   (a re-import raced the decision — the next fetch re-stages the fresh
//!   body under its own id). Errors carry `{"error": msg}`.
//! - `POST /writeback/<id>/dismiss` → `{verdict: "dismissed"}`.
//!
//! CONFLICT LAW: a conflict candidate NEVER accepts without an explicit mode
//! — the UI's only path to `take-theirs` (overwrite) or `keep-mine` (keep
//! current) is the conflict modal's buttons. No silent overwrite exists.

use serde::Deserialize;

/// One candidate as served (all fields defaulted — a partial row renders
/// what it carries instead of failing the whole snapshot, the axioms idiom).
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct WireCandidate {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub project: Option<String>,
    /// Epoch seconds (run: ended; conflict: staged-at).
    #[serde(default)]
    pub ts: Option<f64>,
    // ── run-kind fields ──
    #[serde(default)]
    pub run_id: Option<String>,
    #[serde(default)]
    pub agent: Option<String>,
    /// `"digest"` (Tier-1 receipt) or `"result"` (raw result fallback).
    #[serde(default)]
    pub via: Option<String>,
    /// First 240 chars of the staged text — the only body on the wire.
    #[serde(default)]
    pub preview: Option<String>,
    // ── conflict-kind fields ──
    /// Vault-relative posix path of the CANONICAL (hand-edited) note.
    #[serde(default)]
    pub path: Option<String>,
    /// Vault-relative posix path of the `.incoming.md` vendor copy.
    #[serde(default)]
    pub incoming: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
    /// The modes the server advertises — the fold maps ONLY these (a mode
    /// the bridge withholds renders disabled, never guessed).
    #[serde(default)]
    pub actions: Option<Vec<String>>,
}

/// The `counts` object riding the listing.
#[derive(Debug, Clone, Copy, PartialEq, Default, Deserialize)]
pub struct WireCounts {
    #[serde(default)]
    pub pending: u64,
    #[serde(default)]
    pub conflicts: u64,
}

/// The full `GET /writeback` snapshot as served.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct WireSnapshot {
    #[serde(default)]
    pub candidates: Vec<WireCandidate>,
    #[serde(default)]
    pub counts: WireCounts,
}

/// A folded candidate — the row model the view renders.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub id: String,
    pub title: String,
    pub project: Option<String>,
    pub ts: Option<f64>,
    pub kind: CandidateKind,
}

/// The per-kind shape after the fold.
#[derive(Debug, Clone, PartialEq)]
pub enum CandidateKind {
    /// Run → note candidate: Accept writes a new note, Dismiss drops.
    Run {
        run_id: String,
        agent: String,
        via: String,
        preview: String,
    },
    /// Import conflict: the target note drifted — resolution goes through
    /// the conflict modal ONLY (keep current / overwrite), never a bare
    /// accept (the bridge 400s one).
    Conflict {
        /// Canonical (hand-edited) note, vault-relative posix.
        path: String,
        /// The staged vendor copy, vault-relative posix.
        incoming: String,
        reason: String,
        /// `keep-mine` advertised by the bridge.
        keep_current: bool,
        /// `take-theirs` advertised by the bridge.
        overwrite: bool,
    },
    /// A kind this build doesn't know — rendered honestly (title + kind
    /// chip + Dismiss only), never guessed into an accept.
    Unknown(String),
}

/// The folded snapshot the state holds.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Snapshot {
    pub candidates: Vec<Candidate>,
    pub pending: u64,
    pub conflicts: u64,
}

/// An explicit conflict resolution — the ONLY two verbs the modal offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    /// `keep-mine`: the hand-edited note stays; the incoming body is stamped
    /// adjudicated so the same vendor content never re-raises.
    KeepCurrent,
    /// `take-theirs`: the vendor copy replaces the note; the hand edit is
    /// preserved as a `.mine` sibling (the bridge never destroys either side).
    Overwrite,
}

impl Resolution {
    /// The wire `mode` string for `POST /writeback/<id>/accept`.
    pub fn mode(self) -> &'static str {
        match self {
            Resolution::KeepCurrent => "keep-mine",
            Resolution::Overwrite => "take-theirs",
        }
    }
}

/// Fold one wire candidate into the row model. Pure — unit-tested below.
pub fn fold_candidate(wire: WireCandidate) -> Candidate {
    let kind = match wire.kind.as_str() {
        "run" => CandidateKind::Run {
            run_id: wire.run_id.unwrap_or_default(),
            agent: wire.agent.unwrap_or_default(),
            via: wire.via.unwrap_or_default(),
            preview: wire.preview.unwrap_or_default(),
        },
        "conflict" => {
            let actions = wire.actions.unwrap_or_default();
            CandidateKind::Conflict {
                path: wire.path.unwrap_or_default(),
                incoming: wire.incoming.unwrap_or_default(),
                reason: wire
                    .reason
                    .unwrap_or_else(|| "hand-edited · vendor updated".to_string()),
                keep_current: actions.iter().any(|a| a == "keep-mine"),
                overwrite: actions.iter().any(|a| a == "take-theirs"),
            }
        }
        other => CandidateKind::Unknown(other.to_string()),
    };
    Candidate {
        id: wire.id,
        title: wire.title,
        project: wire.project,
        ts: wire.ts,
        kind,
    }
}

/// Fold the whole snapshot. Order is preserved (the bridge serves newest
/// first); candidates without an id are dropped — no id means no accept or
/// dismiss URL, so a row would carry dead buttons.
pub fn fold_snapshot(wire: WireSnapshot) -> Snapshot {
    Snapshot {
        pending: wire.counts.pending,
        conflicts: wire.counts.conflicts,
        candidates: wire
            .candidates
            .into_iter()
            .filter(|c| !c.id.is_empty())
            .map(fold_candidate)
            .collect(),
    }
}

/// Extract the bridge's `{"error": msg}` body; fall back to the status line.
/// Pure — the POST helper feeds it non-2xx bodies.
pub fn error_message(status: u16, body: &str) -> String {
    #[derive(Deserialize)]
    struct ErrBody {
        error: String,
    }
    match serde_json::from_str::<ErrBody>(body) {
        Ok(parsed) if !parsed.error.is_empty() => parsed.error,
        _ => format!("bridge returned {status}"),
    }
}

/// Cap a note body for the conflict modal's side panes (first ~24 lines /
/// 1600 chars) — a preview, not the file.
pub fn side_preview(body: &str) -> String {
    // Truncation judged by COUNTS, never byte lengths — `lines()` strips
    // \r\n, so a CRLF vault file always measured "shorter than the body" and
    // every complete preview grew a spurious trailing … (2026-07-10 review
    // nit; Windows vaults are CRLF-likely).
    let total_lines = body.lines().count();
    let mut preview: String = body.lines().take(24).collect::<Vec<_>>().join("\n");
    let mut truncated = total_lines > 24;
    if preview.chars().count() > 1600 {
        preview = preview.chars().take(1600).collect();
        truncated = true;
    }
    if truncated {
        preview.push('…');
    }
    preview
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact populated shape the routes test pins (run + conflict).
    const LIVE_SHAPE: &str = r#"{
        "candidates": [
            {"id": "conf-ab12cd34ef56", "kind": "conflict",
             "project": "agentic-ide",
             "path": "projects/agentic-ide/knowledge/imported/vendor/note.md",
             "incoming": "projects/agentic-ide/knowledge/imported/vendor/note.incoming.md",
             "title": "note", "source": "vendor-docs",
             "reason": "hand-edited · vendor updated — review",
             "actions": ["keep-mine", "take-theirs"], "ts": 1789.0},
            {"id": "run-r1", "kind": "run", "run_id": "r1", "conv": "c-1",
             "agent": "claude", "project": "agentic-ide",
             "title": "fix the parser", "via": "digest",
             "preview": "Fixed the frontmatter parser fence-skip.",
             "ts": 1000.0}
        ],
        "counts": {"pending": 2, "conflicts": 1}
    }"#;

    #[test]
    fn parses_and_folds_the_live_shape() {
        let wire: WireSnapshot = serde_json::from_str(LIVE_SHAPE).unwrap();
        let snapshot = fold_snapshot(wire);
        assert_eq!(snapshot.pending, 2);
        assert_eq!(snapshot.conflicts, 1);
        assert_eq!(snapshot.candidates.len(), 2);
        // Order preserved (bridge serves newest first).
        assert_eq!(snapshot.candidates[0].id, "conf-ab12cd34ef56");
        match &snapshot.candidates[0].kind {
            CandidateKind::Conflict {
                path,
                incoming,
                reason,
                keep_current,
                overwrite,
            } => {
                assert!(path.ends_with("note.md"));
                assert!(incoming.ends_with("note.incoming.md"));
                assert!(reason.contains("vendor updated"));
                assert!(*keep_current && *overwrite);
            }
            other => panic!("expected conflict, got {other:?}"),
        }
        match &snapshot.candidates[1].kind {
            CandidateKind::Run {
                run_id,
                agent,
                via,
                preview,
            } => {
                assert_eq!(run_id, "r1");
                assert_eq!(agent, "claude");
                assert_eq!(via, "digest");
                assert!(preview.starts_with("Fixed"));
            }
            other => panic!("expected run, got {other:?}"),
        }
    }

    #[test]
    fn parses_the_empty_shape() {
        let wire: WireSnapshot =
            serde_json::from_str(r#"{"candidates": [], "counts": {"pending": 0, "conflicts": 0}}"#)
                .unwrap();
        let snapshot = fold_snapshot(wire);
        assert!(snapshot.candidates.is_empty());
        assert_eq!((snapshot.pending, snapshot.conflicts), (0, 0));
    }

    #[test]
    fn conflict_maps_only_advertised_modes() {
        // A bridge that withholds a mode → that button renders disabled,
        // never guessed (the honest-disabled law).
        let wire = WireCandidate {
            id: "conf-x".into(),
            kind: "conflict".into(),
            actions: Some(vec!["keep-mine".into()]),
            ..Default::default()
        };
        match fold_candidate(wire).kind {
            CandidateKind::Conflict {
                keep_current,
                overwrite,
                ..
            } => {
                assert!(keep_current);
                assert!(!overwrite);
            }
            other => panic!("expected conflict, got {other:?}"),
        }
        // No actions key at all → nothing advertised, nothing offered.
        let bare = WireCandidate {
            id: "conf-y".into(),
            kind: "conflict".into(),
            ..Default::default()
        };
        match fold_candidate(bare).kind {
            CandidateKind::Conflict {
                keep_current,
                overwrite,
                ..
            } => assert!(!keep_current && !overwrite),
            other => panic!("expected conflict, got {other:?}"),
        }
    }

    #[test]
    fn unknown_kind_folds_honest() {
        let wire = WireCandidate {
            id: "z-1".into(),
            kind: "distillate".into(),
            title: "??".into(),
            ..Default::default()
        };
        assert_eq!(
            fold_candidate(wire).kind,
            CandidateKind::Unknown("distillate".into())
        );
    }

    #[test]
    fn idless_candidates_are_dropped() {
        let wire = WireSnapshot {
            candidates: vec![WireCandidate::default()],
            counts: WireCounts {
                pending: 1,
                conflicts: 0,
            },
        };
        // The count still reads 1 (server truth) but no dead-button row.
        let snapshot = fold_snapshot(wire);
        assert!(snapshot.candidates.is_empty());
        assert_eq!(snapshot.pending, 1);
    }

    #[test]
    fn resolution_modes_match_the_wire() {
        assert_eq!(Resolution::KeepCurrent.mode(), "keep-mine");
        assert_eq!(Resolution::Overwrite.mode(), "take-theirs");
    }

    #[test]
    fn error_message_prefers_the_bridge_body() {
        assert_eq!(
            error_message(
                400,
                r#"{"error": "conflict accept needs {\"mode\": \"keep-mine\" | \"take-theirs\"}"}"#
            ),
            "conflict accept needs {\"mode\": \"keep-mine\" | \"take-theirs\"}"
        );
        assert_eq!(error_message(404, r#"{"error": "unknown candidate"}"#), "unknown candidate");
        assert_eq!(error_message(503, "not json"), "bridge returned 503");
        assert_eq!(error_message(500, r#"{"error": ""}"#), "bridge returned 500");
    }

    #[test]
    fn side_preview_caps_lines_and_chars() {
        let long = (0..40).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
        let preview = side_preview(&long);
        assert_eq!(preview.lines().count(), 24);
        assert!(preview.ends_with('…'));
        assert_eq!(side_preview("short"), "short");
        let wide = "x".repeat(4000);
        assert!(side_preview(&wide).chars().count() <= 1601);
    }
}
