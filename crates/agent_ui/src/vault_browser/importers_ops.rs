//! IMPORT-view operations (a `VaultBrowserPanel` impl split out — single
//! concern: the bridge I/O + state flips behind `importers_view.rs`).
//!
//! Idioms: fetches are visibility-gated one-shots (the axioms/briefing
//! pattern); the ONE poll is the running-job watch, gated on the IMPORT
//! view being up and stopping at terminal status / three straight misses
//! (jobs are process-lifetime on the bridge — a restart mid-import 404s the
//! id forever, so unbounded retry would spin). Degrade law: a failed fetch
//! keeps the last-known snapshot under an honest stale note; POST failures
//! surface the bridge's `{"error": …}` message VERBATIM.
//!
//! §11: compose open/close, project picks, copy-CLI, and in-flight marks
//! are pure entity-state flips; every POST/GET rides `cx.spawn` +
//! `background_spawn` and lands via `this.update` — never re-entering the
//! listener's update.

use std::time::Duration;

use editor::Editor;
use futures::AsyncReadExt as _;
use gpui::{AppContext as _, ClipboardItem, Context, Entity, Focusable as _, Task, Window};
use http_client::{AsyncBody, HttpClient};

use crate::bridge::{BRIDGE_BASE_URL, fetch_json};

use super::importers::{
    ImportJob, ImportersSnapshot, StartedImport, error_from_body,
};
use super::index::VaultRoot;
use super::panel::{VaultBrowserPanel, VaultView};

/// Running-job poll cadence (between the board's 1.5s fallback poll and the
/// transcript's 900ms busy poll — progress rolls without hammering).
const JOB_POLL_INTERVAL: Duration = Duration::from_millis(1200);
/// Consecutive failed job fetches before the watch gives up (see header).
const JOB_POLL_MAX_MISSES: u32 = 3;

/// IMPORT-view state riding the panel: snapshot + fetch lifecycle, the
/// compose row (path + project for `POST /importers/<vendor>/import`), and
/// the watched job.
#[derive(Default)]
pub struct ImportersState {
    /// `None` until the first fetch lands (loading / unavailable states).
    pub snapshot: Option<ImportersSnapshot>,
    /// The LAST fetch or POST attempt failed — snapshot (if any) is stale.
    pub stale: bool,
    /// A snapshot fetch is in flight.
    pub loading: bool,
    /// The vendor whose compose row (path + project) is open.
    pub composing: Option<String>,
    /// Lazily created on first compose (needs a window).
    pub path_editor: Option<Entity<Editor>>,
    /// The picked project slug (None → the first known slug).
    pub project: Option<String>,
    /// A start POST is in flight (double-click can't double-POST).
    pub start_in_flight: bool,
    /// A cancel POST is in flight.
    pub cancel_in_flight: bool,
    /// The last start/cancel failure — the bridge's error, VERBATIM.
    pub error: Option<String>,
    /// The watched job's last snapshot (survives view flips + poll stops).
    pub job: Option<ImportJob>,
    /// The job id the poll loop is currently watching.
    pub watching: Option<String>,
    /// Which CLI line was last copied (the "copied" chip).
    pub copied_cli: Option<String>,
    /// In-flight snapshot fetch (kept so it isn't dropped/cancelled).
    pub(super) _fetch_task: Option<Task<()>>,
    /// The job poll loop (replaced on re-watch — the old loop cancels).
    pub(super) _job_task: Option<Task<()>>,
}

/// POST returning the response body, with non-2xx surfacing the bridge's
/// `{"error": …}` VERBATIM (the shared `post_json` drops the body on error,
/// which would eat the 400/404/409 messages this view exists to show).
async fn post_verbatim(client: &dyn HttpClient, url: &str, body: String) -> anyhow::Result<String> {
    let mut response = client.post_json(url, AsyncBody::from(body)).await?;
    let mut raw = String::new();
    response.body_mut().read_to_string(&mut raw).await?;
    if !response.status().is_success() {
        anyhow::bail!(error_from_body(response.status().as_u16(), &raw));
    }
    Ok(raw)
}

impl VaultBrowserPanel {
    /// One-shot `GET /importers` — visibility-gated (view flip, header
    /// refresh, post-action). A failed fetch keeps the last-known snapshot
    /// and flips the stale flag. A snapshot carrying a `running_job` (this
    /// session's or another client's) re-attaches the job watch; a locally
    /// stored job stuck on "running" with no live watch is refreshed the
    /// same way so it can't read stale forever.
    pub(super) fn fetch_importers(&mut self, cx: &mut Context<Self>) {
        self.importers.loading = true;
        let client = cx.http_client();
        self.importers._fetch_task = Some(cx.spawn(async move |this, cx| {
            let fetched = cx
                .background_spawn(async move {
                    let url = format!("{BRIDGE_BASE_URL}/importers");
                    let raw = fetch_json(client.as_ref(), &url).await?;
                    anyhow::Ok(serde_json::from_str::<ImportersSnapshot>(&raw)?)
                })
                .await;
            this.update(cx, |this, cx| {
                this.importers.loading = false;
                match fetched {
                    Ok(snapshot) => {
                        let watch = snapshot
                            .vendors
                            .iter()
                            .find_map(|v| v.running_job.clone())
                            .or_else(|| {
                                this.importers
                                    .job
                                    .as_ref()
                                    .filter(|j| j.status == "running")
                                    .map(|j| j.id.clone())
                            });
                        this.importers.snapshot = Some(snapshot);
                        this.importers.stale = false;
                        if let Some(id) = watch {
                            this.watch_import_job(id, cx);
                        }
                    }
                    Err(_) => this.importers.stale = true,
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// Poll `GET /importers/job/<id>` while the IMPORT view is up and the
    /// job runs — the same `bridge/jobs.py` registry the board's import
    /// rows merge from, with the per-stage counts the board drops. Stops at
    /// terminal status (refreshing the snapshot + the vault index — done
    /// imports landed real files), on a view flip away (re-attached by the
    /// next snapshot fetch), or after three straight misses.
    pub(super) fn watch_import_job(&mut self, job_id: String, cx: &mut Context<Self>) {
        if self.importers.watching.as_deref() == Some(job_id.as_str()) {
            return; // already watching this job
        }
        self.importers.watching = Some(job_id.clone());
        let client = cx.http_client();
        self.importers._job_task = Some(cx.spawn(async move |this, cx| {
            let mut misses = 0u32;
            loop {
                let id = job_id.clone();
                let client = client.clone();
                let fetched = cx
                    .background_spawn(async move {
                        let url = format!("{BRIDGE_BASE_URL}/importers/job/{id}");
                        let raw = fetch_json(client.as_ref(), &url).await?;
                        anyhow::Ok(serde_json::from_str::<ImportJob>(&raw)?)
                    })
                    .await;
                let keep_going = this.update(cx, |this, cx| {
                    let keep = match fetched {
                        Ok(job) => {
                            misses = 0;
                            let terminal = job.status != "running";
                            this.importers.stale = false;
                            this.importers.job = Some(job);
                            if terminal {
                                this.importers.watching = None;
                                // The sidecar last-import + running flags
                                // moved; done imports wrote vault files —
                                // refresh() re-indexes AND (view-gated)
                                // refetches the importers snapshot.
                                this.refresh(cx);
                                false
                            } else {
                                this.view() == VaultView::Import
                            }
                        }
                        Err(_) => {
                            misses += 1;
                            this.importers.stale = true;
                            misses < JOB_POLL_MAX_MISSES && this.view() == VaultView::Import
                        }
                    };
                    if !keep {
                        this.importers.watching = None;
                    }
                    cx.notify();
                    keep
                });
                if !keep_going.unwrap_or(false) {
                    return;
                }
                cx.background_executor().timer(JOB_POLL_INTERVAL).await;
            }
        }));
    }

    /// USER-gated `POST /importers/<vendor>/import {path, project}` — the
    /// compose row's Start button only. On accept the returned job is
    /// watched immediately; the bridge's 400/404/409 messages land VERBATIM.
    pub(super) fn start_export_import(&mut self, vendor: String, cx: &mut Context<Self>) {
        if self.importers.start_in_flight {
            return;
        }
        let Some(editor) = self.importers.path_editor.clone() else {
            return;
        };
        let path = editor.read(cx).text(cx).trim().to_string();
        if path.is_empty() {
            self.importers.error = Some("An export file path is required.".to_string());
            cx.notify();
            return;
        }
        let project = self.picked_import_project();
        self.importers.start_in_flight = true;
        self.importers.error = None;
        cx.notify();
        let client = cx.http_client();
        cx.spawn(async move |this, cx| {
            let outcome = cx
                .background_spawn(async move {
                    let url = format!("{BRIDGE_BASE_URL}/importers/{vendor}/import");
                    let body =
                        serde_json::json!({ "path": path, "project": project }).to_string();
                    let raw = post_verbatim(client.as_ref(), &url, body).await?;
                    anyhow::Ok(serde_json::from_str::<StartedImport>(&raw)?)
                })
                .await;
            this.update(cx, |this, cx| {
                this.importers.start_in_flight = false;
                match outcome {
                    Ok(started) if !started.job.is_empty() => {
                        this.importers.composing = None;
                        this.importers.job = None;
                        this.watch_import_job(started.job, cx);
                        this.fetch_importers(cx); // running_job flags moved
                    }
                    Ok(_) => {
                        this.importers.error =
                            Some("Bridge accepted the import but returned no job id.".to_string());
                    }
                    Err(error) => this.importers.error = Some(error.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// USER-gated `POST /import/run {vendor?}` — the Family-A session-store
    /// staging trigger (bridge 60cc040: the standalone `import` CLI spawned
    /// as a board-riding job). `None` = all four stores. The returned job
    /// polls through the SAME /importers/job/<id> watch as an export import;
    /// the bridge's 400/404/409/501 messages (e.g. the honest "binary not
    /// built" remedy) land VERBATIM.
    pub(super) fn start_session_import(
        &mut self,
        vendor: Option<&'static str>,
        cx: &mut Context<Self>,
    ) {
        if self.importers.start_in_flight {
            return;
        }
        self.importers.start_in_flight = true;
        self.importers.error = None;
        cx.notify();
        let client = cx.http_client();
        cx.spawn(async move |this, cx| {
            let outcome = cx
                .background_spawn(async move {
                    let url = format!("{BRIDGE_BASE_URL}/import/run");
                    let body = match vendor {
                        Some(vendor) => serde_json::json!({ "vendor": vendor }),
                        None => serde_json::json!({}),
                    }
                    .to_string();
                    let raw = post_verbatim(client.as_ref(), &url, body).await?;
                    anyhow::Ok(serde_json::from_str::<StartedImport>(&raw)?)
                })
                .await;
            this.update(cx, |this, cx| {
                this.importers.start_in_flight = false;
                match outcome {
                    Ok(started) if !started.job.is_empty() => {
                        this.importers.job = None;
                        this.watch_import_job(started.job, cx);
                    }
                    Ok(_) => {
                        this.importers.error = Some(
                            "Bridge accepted the import but returned no job id."
                                .to_string(),
                        );
                    }
                    Err(error) => this.importers.error = Some(error.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// USER-gated `POST /importers/job/<id>/cancel` — cooperative
    /// (watch-then-cancel: partial files persist, a re-import heals). The
    /// job stays "running" until its thread polls the event; the watch loop
    /// lands the terminal state.
    pub(super) fn cancel_import_job(&mut self, cx: &mut Context<Self>) {
        if self.importers.cancel_in_flight {
            return;
        }
        let Some(id) = self
            .importers
            .job
            .as_ref()
            .filter(|j| j.status == "running")
            .map(|j| j.id.clone())
        else {
            return;
        };
        self.importers.cancel_in_flight = true;
        cx.notify();
        let client = cx.http_client();
        cx.spawn(async move |this, cx| {
            let outcome = cx
                .background_spawn(async move {
                    let url = format!("{BRIDGE_BASE_URL}/importers/job/{id}/cancel");
                    post_verbatim(client.as_ref(), &url, String::new()).await
                })
                .await;
            this.update(cx, |this, cx| {
                this.importers.cancel_in_flight = false;
                if let Err(error) = outcome {
                    this.importers.error = Some(error.to_string());
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Toggle a vendor's compose row (path + project). Pure state flip; the
    /// path editor is created lazily and focused so typing starts at once.
    pub(super) fn toggle_import_compose(
        &mut self,
        vendor: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.importers.composing.as_deref() == Some(vendor.as_str()) {
            self.importers.composing = None;
            cx.notify();
            return;
        }
        if self.importers.path_editor.is_none() {
            let editor = cx.new(|cx| {
                let mut editor = Editor::single_line(window, cx);
                editor.set_placeholder_text("Path to the vendor export file…", window, cx);
                editor
            });
            // Re-render on typing so Start's enabled state tracks.
            let sub = cx.subscribe(&editor, |_, _, event, cx| {
                if let editor::EditorEvent::BufferEdited = event {
                    cx.notify();
                }
            });
            self.push_subscription(sub);
            self.importers.path_editor = Some(editor);
        }
        self.importers.composing = Some(vendor);
        self.importers.error = None;
        if let Some(editor) = &self.importers.path_editor {
            editor.read(cx).focus_handle(cx).focus(window, cx);
        }
        cx.notify();
    }

    /// Pick the import target project chip. Pure state flip.
    pub(super) fn set_import_project(&mut self, slug: String, cx: &mut Context<Self>) {
        if self.importers.project.as_deref() != Some(slug.as_str()) {
            self.importers.project = Some(slug);
            cx.notify();
        }
    }

    /// The effective import project (picked, else the first known slug —
    /// shares the new-note picker's source of truth).
    pub(super) fn picked_import_project(&self) -> String {
        self.importers
            .project
            .clone()
            .unwrap_or_else(|| self.project_slugs().remove(0))
    }

    /// Copy a session-store CLI invocation (the honest affordance where no
    /// trigger endpoint exists — see `importers.rs` header). Pure state flip
    /// + clipboard write.
    pub(super) fn copy_import_cli(&mut self, key: String, cli: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(cli));
        self.importers.copied_cli = Some(key);
        cx.notify();
    }

    /// "View staged →" / "View in vault →" — flip to the target root's LIST.
    pub(super) fn jump_to_root_list(&mut self, root: VaultRoot, cx: &mut Context<Self>) {
        self.set_root(root, cx);
        self.set_view(VaultView::List, cx);
    }
}
