//! WRITEBACK state + actions — the fetch lifecycle, the accept/dismiss
//! POSTs, and the conflict modal (both sides read from the local vault).
//! Split from `writeback.rs` (the pure contract) and `writeback_view.rs`
//! (the render) per the panel/panel_header concern-split precedent.
//!
//! Fetches are visibility-gated one-shots (view flip / header refresh /
//! post-action — the axioms idiom, never polled). Degrade law: a failed
//! fetch keeps the last-known snapshot under an honest stale note.
//!
//! CONFLICT LAW (enforced here): `resolve_conflict` is the ONLY path that
//! posts a conflict mode, and it is reached ONLY from the modal buttons;
//! `accept_run_candidate` refuses conflict-kind ids outright — a bare accept
//! can never silently overwrite a drifted note.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use futures::AsyncReadExt as _;
use gpui::{AppContext as _, Context, Task};
use http_client::{AsyncBody, HttpClient};

use crate::bridge::{BRIDGE_BASE_URL, fetch_json};

use super::index::vault_root_path;
use super::panel::VaultBrowserPanel;
use super::writeback::{
    Candidate, CandidateKind, Resolution, Snapshot, WireSnapshot, error_message, fold_snapshot,
    side_preview,
};

/// Writeback-view state riding the panel (the axioms shape + the modal).
#[derive(Default)]
pub struct WritebackState {
    /// `None` until the first fetch lands (loading / unavailable states).
    pub(super) snapshot: Option<Snapshot>,
    /// The LAST fetch failed — the snapshot (if any) is stale.
    pub(super) stale: bool,
    /// A fetch is in flight (first open shows "Loading…", not a blank).
    pub(super) loading: bool,
    /// Candidate ids with a POST in flight (buttons collapse to "saving…"
    /// so a double-click can't double-POST).
    pub(super) in_flight: HashSet<String>,
    /// Per-candidate inline error from the last failed action (the bridge's
    /// `{"error"}` body — e.g. "canonical file is gone…" on keep-mine).
    pub(super) row_errors: HashMap<String, String>,
    /// The open conflict modal, if any.
    pub(super) modal: Option<ConflictModal>,
    /// The in-flight fetch (kept so it isn't dropped/cancelled).
    _task: Option<Task<()>>,
}

/// One side of the conflict modal (read from the local vault mirror — the
/// bodies are never on the wire).
pub enum SideLoad {
    Loading,
    Loaded(String),
    /// The read failed (path gone / unreadable) — shown honestly.
    Failed(String),
}

/// The conflict modal: candidate identity + both sides + resolution wiring.
pub struct ConflictModal {
    pub(super) id: String,
    pub(super) title: String,
    /// Canonical (hand-edited) note, vault-relative posix.
    pub(super) path: String,
    /// The staged vendor copy, vault-relative posix.
    pub(super) incoming: String,
    pub(super) reason: String,
    /// `keep-mine` advertised by the bridge (disabled otherwise).
    pub(super) keep_current: bool,
    /// `take-theirs` advertised by the bridge (disabled otherwise).
    pub(super) overwrite: bool,
    /// The CURRENT side — the canonical note body.
    pub(super) current: SideLoad,
    /// The CANDIDATE side — the `.incoming.md` vendor body.
    pub(super) candidate: SideLoad,
    /// The last resolution attempt's error (candidate stays pending).
    pub(super) error: Option<String>,
    /// The side-load task (kept so it isn't dropped).
    _load: Option<Task<()>>,
}

impl VaultBrowserPanel {
    /// One-shot `GET /writeback` (every project — this is the review inbox,
    /// not a per-project strip). Visibility-gated: view flip, header
    /// refresh, panel re-activation, post-action.
    pub(super) fn fetch_writeback(&mut self, cx: &mut Context<Self>) {
        self.writeback.loading = true;
        let client = cx.http_client();
        self.writeback._task = Some(cx.spawn(async move |this, cx| {
            let fetched = cx
                .background_spawn(async move {
                    let url = format!("{BRIDGE_BASE_URL}/writeback");
                    let raw = fetch_json(client.as_ref(), &url).await?;
                    anyhow::Ok(serde_json::from_str::<WireSnapshot>(&raw)?)
                })
                .await;
            this.update(cx, |this, cx| {
                this.writeback.loading = false;
                match fetched {
                    Ok(wire) => {
                        this.writeback.snapshot = Some(fold_snapshot(wire));
                        this.writeback.stale = false;
                    }
                    Err(_) => this.writeback.stale = true,
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// Accept a RUN candidate (writes a new vault note bridge-side). Refuses
    /// non-run ids: a conflict must go through [`Self::resolve_conflict`]'s
    /// modal — the bridge would 400 a bare accept, and this guard makes the
    /// no-silent-overwrite law structural rather than server-enforced.
    pub(super) fn accept_run_candidate(&mut self, id: String, cx: &mut Context<Self>) {
        let is_run = self.writeback_candidate(&id).is_some_and(|candidate| {
            matches!(candidate.kind, CandidateKind::Run { .. })
        });
        if !is_run {
            return;
        }
        self.post_candidate_action(id, "accept", "{}".to_string(), cx);
    }

    /// Dismiss any candidate kind (drops it; conflicts leave their
    /// `.incoming` file in place bridge-side and may re-stage on new vendor
    /// content — the engine's documented dismiss semantics).
    pub(super) fn dismiss_candidate(&mut self, id: String, cx: &mut Context<Self>) {
        self.post_candidate_action(id, "dismiss", "{}".to_string(), cx);
    }

    /// Resolve a conflict with an EXPLICIT mode — reached ONLY from the
    /// conflict modal's buttons (CONFLICT LAW: never programmatic, never a
    /// default).
    pub(super) fn resolve_conflict(
        &mut self,
        id: String,
        resolution: Resolution,
        cx: &mut Context<Self>,
    ) {
        let body = serde_json::json!({ "mode": resolution.mode() }).to_string();
        self.post_candidate_action(id, "accept", body, cx);
    }

    /// Open the conflict modal for a conflict candidate and start reading
    /// both sides from the local vault (the bodies are not on the wire).
    pub(super) fn open_conflict_modal(&mut self, candidate: &Candidate, cx: &mut Context<Self>) {
        let CandidateKind::Conflict {
            path,
            incoming,
            reason,
            keep_current,
            overwrite,
        } = &candidate.kind
        else {
            return;
        };
        let mut modal = ConflictModal {
            id: candidate.id.clone(),
            title: candidate.title.clone(),
            path: path.clone(),
            incoming: incoming.clone(),
            reason: reason.clone(),
            keep_current: *keep_current,
            overwrite: *overwrite,
            current: SideLoad::Loading,
            candidate: SideLoad::Loading,
            error: None,
            _load: None,
        };
        let id = candidate.id.clone();
        let fs = self.fs.clone();
        let current_abs = vault_rel_to_abs(path);
        let incoming_abs = vault_rel_to_abs(incoming);
        modal._load = Some(cx.spawn(async move |this, cx| {
            let loaded = cx
                .background_spawn(async move {
                    let current = fs.load(&current_abs).await;
                    let incoming = fs.load(&incoming_abs).await;
                    (current, incoming)
                })
                .await;
            this.update(cx, |this, cx| {
                // The modal may have been closed or replaced mid-read.
                if let Some(modal) = this.writeback.modal.as_mut().filter(|m| m.id == id) {
                    modal.current = side_load(loaded.0, "current note");
                    modal.candidate = side_load(loaded.1, "incoming copy");
                    cx.notify();
                }
            })
            .ok();
        }));
        self.writeback.modal = Some(modal);
        cx.notify();
    }

    /// Close the modal (Cancel — the candidate stays pending). Pure flip.
    pub(super) fn close_conflict_modal(&mut self, cx: &mut Context<Self>) {
        self.writeback.modal = None;
        cx.notify();
    }

    /// Look up a folded candidate by id.
    pub(super) fn writeback_candidate(&self, id: &str) -> Option<&Candidate> {
        self.writeback
            .snapshot
            .as_ref()?
            .candidates
            .iter()
            .find(|candidate| candidate.id == id)
    }

    /// Shared POST path for accept/dismiss. §11: the in-flight mark is a
    /// pure state flip (immediate feedback); the POST rides `cx.spawn`; the
    /// outcome lands via `this.update` — the axioms `resolve_axiom` shape.
    /// Success refetches (the row leaves pending) and closes a same-id
    /// modal; failure surfaces the bridge's error inline and the candidate
    /// STAYS pending — never a silent drop.
    fn post_candidate_action(
        &mut self,
        id: String,
        action: &'static str,
        body: String,
        cx: &mut Context<Self>,
    ) {
        if !self.writeback.in_flight.insert(id.clone()) {
            return; // already in flight — a double-click must not double-POST
        }
        self.writeback.row_errors.remove(&id);
        if let Some(modal) = self.writeback.modal.as_mut().filter(|m| m.id == id) {
            modal.error = None;
        }
        cx.notify();
        let client = cx.http_client();
        let url = format!("{BRIDGE_BASE_URL}/writeback/{id}/{action}");
        cx.spawn(async move |this, cx| {
            let outcome = cx
                .background_spawn(async move { post_writeback(client, url, body).await })
                .await;
            this.update(cx, |this, cx| {
                this.writeback.in_flight.remove(&id);
                match outcome {
                    Ok(_) => {
                        if this.writeback.modal.as_ref().is_some_and(|m| m.id == id) {
                            this.writeback.modal = None;
                        }
                        // Resolved server-side — refetch so the row leaves
                        // pending and the counts advance.
                        this.fetch_writeback(cx);
                    }
                    Err(message) => {
                        if let Some(modal) =
                            this.writeback.modal.as_mut().filter(|m| m.id == id)
                        {
                            modal.error = Some(message);
                        } else {
                            this.writeback.row_errors.insert(id, message);
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

/// POST returning the bridge's `{"error"}` body on non-2xx (the shared
/// [`crate::bridge::post_json`] drops error bodies — here the message IS the
/// product: "canonical file is gone — nothing to keep; use take-theirs…").
async fn post_writeback(
    client: Arc<dyn HttpClient>,
    url: String,
    body: String,
) -> Result<String, String> {
    let mut response = client
        .post_json(&url, AsyncBody::from(body))
        .await
        .map_err(|_| "bridge unreachable — nothing changed".to_string())?;
    let mut raw = String::new();
    response
        .body_mut()
        .read_to_string(&mut raw)
        .await
        .map_err(|_| "bridge response unreadable — refresh to see state".to_string())?;
    if response.status().is_success() {
        Ok(raw)
    } else {
        Err(error_message(response.status().as_u16(), &raw))
    }
}

/// A vault-relative posix path → the absolute path under the local vault.
fn vault_rel_to_abs(rel: &str) -> std::path::PathBuf {
    vault_root_path().join(rel.replace('/', std::path::MAIN_SEPARATOR_STR))
}

/// Fold an fs read into a modal side (preview-capped; honest on failure).
fn side_load(read: anyhow::Result<String>, what: &str) -> SideLoad {
    match read {
        Ok(body) => SideLoad::Loaded(side_preview(&body)),
        Err(_) => SideLoad::Failed(format!("could not read the {what} from the vault")),
    }
}
