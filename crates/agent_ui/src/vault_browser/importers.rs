//! Importers — wire types + pure mapping for the IMPORT view (Phase-1
//! backlog: "Importers panel"). The discovered contract (probed live
//! 2026-07-10) covers TWO importer families:
//!
//! 1. SESSION STORES → STAGING (`core/importers`, the backlog's "4 vendor
//!    parsers server-ready"): claude_code / codex / vscode / antigravity
//!    session stores normalize into OKF bundles under the staging root the
//!    vault browser already renders (`VaultRoot::Staging`). **The bridge
//!    exposes NO trigger endpoint for these** — the importer is a standalone
//!    CLI by documented choice ("no runtime coupling to the bridge",
//!    `core/importers/src/bin/import.rs`): `import run [--vendor V]
//!    [--since YYYY-MM-DD] [--staging DIR]`. The panel therefore renders
//!    these as DOCUMENTED cards (store hint + copyable CLI, actions
//!    disabled with an honest tooltip) — no faked wire.
//!
//! 2. VENDOR EXPORTS → KNOWLEDGE (`bridge/features/importers`, live HTTP):
//!    official export files (chatgpt / claude / gemini) imported into
//!    `projects/<slug>/knowledge/imported/<vendor>/`.
//!      GET  /importers                  → [`ImportersSnapshot`]
//!      POST /importers/<vendor>/import  `{path, project}` → `{"job": id}`
//!                                       (400 bad path · 404 vendor ·
//!                                        409 per-(vendor, project) mutex)
//!      GET  /importers/job/<id>         → [`ImportJob`] (stage + counts)
//!      POST /importers/job/<id>/cancel  cooperative cancel
//!    Import jobs also ride `GET /board` as run-shaped rows
//!    (`run_id: "import-<id>"`, agent `importer`) — `/importers/job/<id>`
//!    reads the SAME `bridge/jobs.py` registry the board merges, with the
//!    per-stage counts the board rows drop, so the job poll here IS the
//!    board's source, one hop closer.

use serde::Deserialize;

/// One of the four `core/importers` session-store parsers (server-ready,
/// CLI-triggered — see the module header). `vendor` ids match
/// `core/importers/src/stores.rs::VENDORS` exactly.
pub(super) struct SessionVendor {
    pub vendor: &'static str,
    pub label: &'static str,
    /// Where the parser reads sessions from (env-overridable defaults).
    pub store_hint: &'static str,
}

pub(super) const SESSION_VENDORS: [SessionVendor; 4] = [
    SessionVendor {
        vendor: "claude_code",
        label: "Claude Code",
        store_hint: "~/.claude/projects/<slug>/*.jsonl",
    },
    SessionVendor {
        vendor: "codex",
        label: "Codex",
        store_hint: "~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl",
    },
    SessionVendor {
        vendor: "vscode",
        label: "VSCode chat",
        store_hint: "%APPDATA%/Code/User/workspaceStorage",
    },
    SessionVendor {
        vendor: "antigravity",
        label: "Antigravity",
        store_hint: "~/.gemini/antigravity",
    },
];

/// The per-vendor CLI invocation (`core/importers` bin — Wave 1.5).
pub(super) fn session_cli(vendor: &str) -> String {
    format!("import run --vendor {vendor}")
}

/// "Import all" = the bare run (every vendor store, one pass).
pub(super) const IMPORT_ALL_CLI: &str = "import run";

// ── wire types (bridge/features/importers, pinned to the live bridge) ──

/// The engine's per-stage counters (`engine.py::_zero()` — every key always
/// present on the wire, defaulted here anyway: formats drift).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub(super) struct ImportCounts {
    #[serde(default)]
    pub found: u64,
    #[serde(default)]
    pub conversations: u64,
    #[serde(default)]
    pub memories: u64,
    #[serde(default)]
    pub written: u64,
    #[serde(default)]
    pub updated: u64,
    #[serde(default)]
    pub skipped: u64,
    #[serde(default)]
    pub conflicts: u64,
    #[serde(default)]
    pub errors: u64,
}

/// The `last_import` sidecar summary (`engine.py::_save_state` — `at` is
/// epoch seconds, the rest is the import summary).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub(super) struct LastImport {
    #[serde(default)]
    pub at: f64,
    #[serde(default)]
    pub project: String,
    #[serde(default)]
    pub counts: ImportCounts,
}

/// One `GET /importers` vendor card.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub(super) struct VendorStatus {
    #[serde(default)]
    pub vendor: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub last_import: Option<LastImport>,
    #[serde(default)]
    pub running_job: Option<String>,
}

/// The full `GET /importers` response.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub(super) struct ImportersSnapshot {
    #[serde(default)]
    pub vendors: Vec<VendorStatus>,
    /// The bridge's own note that only export-file ingestion is served (the
    /// live-scraper seam) — rendered verbatim as the section footnote.
    #[serde(default)]
    pub scraper_seam: String,
}

/// `GET /importers/job/<id>` — the `bridge/jobs.py` public dict + the
/// importer meta (`vendor`/`project`/`stage`/`counts`/`errors`).
/// Status vocabulary: running | done | error | cancelled.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub(super) struct ImportJob {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub status: String,
    /// locating | parsing | writing | done
    #[serde(default)]
    pub stage: String,
    #[serde(default)]
    pub vendor: String,
    #[serde(default)]
    pub project: String,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub counts: ImportCounts,
    /// Per-record failures (capped at 100 server-side) — rendered verbatim.
    #[serde(default)]
    pub errors: Vec<serde_json::Value>,
    #[serde(default)]
    pub started: f64,
    #[serde(default)]
    pub ended: Option<f64>,
}

/// `POST /importers/<vendor>/import` response.
#[derive(Debug, Default, Deserialize)]
pub(super) struct StartedImport {
    #[serde(default)]
    pub job: String,
}

// ── pure mapping (unit-tested below) ──

/// Job status → the board pill vocabulary (mirrors
/// `engine.py::STATUS_MAP` so `status_pill` renders the same words/colors
/// the board gives the same job).
pub(super) fn board_status(status: &str) -> &str {
    match status {
        "running" => "running",
        "done" => "completed",
        "error" => "failed",
        "cancelled" => "killed",
        other => other,
    }
}

/// Compact counts summary — zero components are skipped so the line reads
/// only what happened ("17 written · 21 skipped · 3 errors"). All-zero
/// (an empty export) reads "nothing found".
pub(super) fn counts_line(c: &ImportCounts) -> String {
    let mut parts = Vec::new();
    for (n, word) in [
        (c.written, "written"),
        (c.updated, "updated"),
        (c.skipped, "skipped"),
        (c.conflicts, "conflicts"),
        (c.errors, "errors"),
    ] {
        if n > 0 {
            parts.push(format!("{n} {word}"));
        }
    }
    if parts.is_empty() {
        "nothing found".to_string()
    } else {
        parts.join(" · ")
    }
}

/// The live progress line while a job runs: stage first, then the rolling
/// counters ("writing · 42 found · 17 written").
pub(super) fn progress_line(job: &ImportJob) -> String {
    let mut parts = vec![if job.stage.is_empty() {
        job.status.clone()
    } else {
        job.stage.clone()
    }];
    parts.push(format!("{} found", job.counts.found));
    for (n, word) in [
        (job.counts.written, "written"),
        (job.counts.updated, "updated"),
        (job.counts.skipped, "skipped"),
        (job.counts.errors, "errors"),
    ] {
        if n > 0 {
            parts.push(format!("{n} {word}"));
        }
    }
    parts.join(" · ")
}

/// The terminal result line: `None` while running; the error VERBATIM on
/// failure; the counts summary on done/cancelled.
pub(super) fn result_line(job: &ImportJob) -> Option<String> {
    match job.status.as_str() {
        "running" => None,
        "error" => Some(
            job.error
                .clone()
                .filter(|e| !e.is_empty())
                .unwrap_or_else(|| "failed (no error reported)".to_string()),
        ),
        "cancelled" => Some(format!("cancelled · {}", counts_line(&job.counts))),
        _ => Some(counts_line(&job.counts)),
    }
}

/// A vendor card's last-import summary ("3h · 17 written · 21 skipped →
/// myproj"), `now` in unix seconds (injected for testability).
pub(super) fn last_import_line(last: &LastImport, now: f64) -> String {
    let age = crate::task_board::style::ago(last.at, now);
    let mut line = format!("{age} · {}", counts_line(&last.counts));
    if !last.project.is_empty() {
        line.push_str(&format!(" → {}", last.project));
    }
    line
}

/// One per-record failure, verbatim: "<title|source_id|vendor>: <error>"
/// (`parsers/common.py::error` shape — every field defensive).
pub(super) fn record_error_line(value: &serde_json::Value) -> String {
    let field = |k: &str| value.get(k).and_then(|v| v.as_str()).unwrap_or("");
    let who = [field("title"), field("source_id"), field("vendor")]
        .into_iter()
        .find(|s| !s.is_empty())
        .unwrap_or("record");
    let error = field("error");
    if error.is_empty() {
        who.to_string()
    } else {
        format!("{who}: {error}")
    }
}

/// Verbatim bridge error from a non-2xx body — the bridge answers
/// `{"error": msg}` (serve.py); fall back to the bare status code when the
/// body isn't that shape.
pub(super) fn error_from_body(status: u16, body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(String::from))
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| format!("bridge returned {status}"))
}

/// Elapsed seconds for a job row (ended-anchored once terminal).
pub(super) fn elapsed_s(job: &ImportJob, now: f64) -> f64 {
    (job.ended.unwrap_or(now) - job.started).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_live_importers_shape() {
        // Verbatim live response (probed 2026-07-10, bridge uptime 6688s).
        let snapshot: ImportersSnapshot = serde_json::from_str(
            r#"{"vendors": [
                {"vendor": "chatgpt", "label": "ChatGPT", "last_import": null, "running_job": null},
                {"vendor": "claude", "label": "claude.ai", "last_import": null, "running_job": null},
                {"vendor": "gemini", "label": "Gemini", "last_import": null, "running_job": null}],
                "scraper_seam": "export-file ingestion only; the live vendor-page scraper is user-driven"}"#,
        )
        .unwrap();
        assert_eq!(snapshot.vendors.len(), 3);
        assert_eq!(snapshot.vendors[0].vendor, "chatgpt");
        assert_eq!(snapshot.vendors[1].label, "claude.ai");
        assert!(snapshot.vendors[0].last_import.is_none());
        assert!(snapshot.scraper_seam.starts_with("export-file"));
        // Degenerate shapes stay parseable (formats drift law).
        assert_eq!(
            serde_json::from_str::<ImportersSnapshot>("{}").unwrap(),
            ImportersSnapshot::default()
        );
    }

    #[test]
    fn parses_a_running_job() {
        // jobs.py public dict + the engine's start_import meta.
        let job: ImportJob = serde_json::from_str(
            r#"{"id": "a1b2c3d4e5f6", "kind": "import", "label": "Import ChatGPT → myproj",
                "status": "running", "started": 1783600000.0, "ended": null,
                "result": null, "error": null, "vendor": "chatgpt", "project": "myproj",
                "path": "C:\\exports\\conversations.json", "stage": "writing",
                "counts": {"found": 42, "conversations": 40, "memories": 2, "written": 17,
                           "updated": 0, "skipped": 21, "conflicts": 1, "errors": 3},
                "errors": [{"vendor": "chatgpt", "error": "ValueError: bad node", "title": "Weird convo"}],
                "conflicts": []}"#,
        )
        .unwrap();
        assert_eq!(job.status, "running");
        assert_eq!(job.stage, "writing");
        assert_eq!(job.counts.found, 42);
        assert_eq!(job.counts.conflicts, 1);
        assert!(job.ended.is_none());
        assert_eq!(
            record_error_line(&job.errors[0]),
            "Weird convo: ValueError: bad node"
        );
        assert_eq!(
            progress_line(&job),
            "writing · 42 found · 17 written · 21 skipped · 3 errors"
        );
        assert_eq!(result_line(&job), None, "running jobs have no result line");
    }

    #[test]
    fn result_lines_are_verbatim_and_status_shaped() {
        let mut job = ImportJob {
            status: "error".into(),
            error: Some("FileNotFoundError: export path not found".into()),
            ..Default::default()
        };
        assert_eq!(
            result_line(&job).unwrap(),
            "FileNotFoundError: export path not found",
            "errors surface VERBATIM"
        );
        job.status = "done".into();
        job.counts.written = 12;
        job.counts.skipped = 3;
        assert_eq!(result_line(&job).unwrap(), "12 written · 3 skipped");
        job.status = "cancelled".into();
        assert_eq!(result_line(&job).unwrap(), "cancelled · 12 written · 3 skipped");
        job.counts = ImportCounts::default();
        job.status = "done".into();
        assert_eq!(result_line(&job).unwrap(), "nothing found");
    }

    #[test]
    fn board_status_mirrors_the_engine_map() {
        // engine.py STATUS_MAP — the board and this view must agree.
        assert_eq!(board_status("running"), "running");
        assert_eq!(board_status("done"), "completed");
        assert_eq!(board_status("error"), "failed");
        assert_eq!(board_status("cancelled"), "killed");
        assert_eq!(board_status("weird"), "weird");
    }

    #[test]
    fn session_catalog_matches_the_core_importers() {
        // core/importers/src/stores.rs VENDORS, same ids, same order.
        let ids: Vec<&str> = SESSION_VENDORS.iter().map(|v| v.vendor).collect();
        assert_eq!(ids, ["claude_code", "codex", "vscode", "antigravity"]);
        assert_eq!(session_cli("claude_code"), "import run --vendor claude_code");
        assert_eq!(IMPORT_ALL_CLI, "import run");
    }

    #[test]
    fn bridge_errors_surface_verbatim_with_status_fallback() {
        assert_eq!(
            error_from_body(409, r#"{"error": "an import for chatgpt → myproj is already running (job abc)"}"#),
            "an import for chatgpt → myproj is already running (job abc)"
        );
        assert_eq!(error_from_body(500, "<html>boom</html>"), "bridge returned 500");
        assert_eq!(error_from_body(400, r#"{"error": ""}"#), "bridge returned 400");
    }

    #[test]
    fn last_import_line_reads_age_counts_project() {
        let last = LastImport {
            at: 1_783_600_000.0,
            project: "myproj".into(),
            counts: ImportCounts {
                written: 17,
                skipped: 21,
                ..Default::default()
            },
        };
        // 3h later.
        let line = last_import_line(&last, 1_783_600_000.0 + 3.0 * 3600.0);
        assert_eq!(line, "3h · 17 written · 21 skipped → myproj");
    }

    #[test]
    fn elapsed_anchors_on_ended_once_terminal() {
        let mut job = ImportJob {
            started: 100.0,
            ended: None,
            ..Default::default()
        };
        assert_eq!(elapsed_s(&job, 160.0), 60.0);
        job.ended = Some(130.0);
        assert_eq!(elapsed_s(&job, 9999.0), 30.0);
        job.ended = Some(90.0); // clock skew never reads negative
        assert_eq!(elapsed_s(&job, 9999.0), 0.0);
    }

    #[test]
    fn start_response_parses() {
        let started: StartedImport = serde_json::from_str(r#"{"job": "a1b2c3d4e5f6"}"#).unwrap();
        assert_eq!(started.job, "a1b2c3d4e5f6");
        assert_eq!(serde_json::from_str::<StartedImport>("{}").unwrap().job, "");
    }
}
