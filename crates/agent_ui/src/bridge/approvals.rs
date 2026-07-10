//! Pending-approval transport (Phase-2 permissions §5.2, wave 3a) — the
//! store-side surface the approval toast (3c) and permissions panel (3b)
//! consume. Transport ONLY: nothing here is user-visible yet.
//!
//! Two feeds, never disagreeing: the SSE `permission` frame lands directly in
//! [`BridgeStore::apply_event`], and the `GET /approvals` poll here covers the
//! polling-fallback window. The poll is watch-gated exactly like
//! [`super::store::TranscriptWatch`]: it runs at 1.5s ONLY while a toast is
//! held or the panel is visible (an [`ApprovalWatch`] is alive), checks the
//! refcount on every wake, and dies at zero — never a free timer.
//!
//! Decisions ride [`post_approval_allow`] / [`post_approval_deny`], the
//! `client.rs` idioms: the bridge's `{"error"}` refusal body (404 unknown id,
//! 409 `already resolved: …` / `expired`) surfaces VERBATIM through
//! [`post_json`] — a refusal reads as a refusal, not as offline.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use anyhow::Result;
use gpui::{AppContext as _, AsyncApp, WeakEntity};
use http_client::HttpClient;

use super::client::{BRIDGE_BASE_URL, fetch_json, post_json};
use super::protocol::ApprovalRow;
use super::store::BridgeStore;

/// `GET /approvals` cadence while watched (Phase-2 design §5.2: 1.5s, only
/// while a toast is held or the panel visible).
const APPROVALS_POLL_INTERVAL: Duration = Duration::from_millis(1500);

/// RAII registration of a live approvals consumer (a held toast, the
/// permissions panel). Dropping it decrements the watcher count; the poll
/// loop notices on its next wake and exits — cx-free teardown, the
/// [`super::store::TranscriptWatch`] pattern.
pub struct ApprovalWatch {
    watchers: Arc<AtomicUsize>,
}

impl Drop for ApprovalWatch {
    fn drop(&mut self) {
        self.watchers.fetch_sub(1, Ordering::SeqCst);
    }
}

impl BridgeStore {
    /// Registers an approvals consumer and (re)starts the `/approvals` poll
    /// on the 0→1 transition. SSE feeds [`BridgeStore::pending_approvals`]
    /// directly when connected; this poll covers the polling-fallback window
    /// so a held toast retires either way.
    pub fn watch_approvals(&mut self, cx: &mut gpui::Context<Self>) -> ApprovalWatch {
        let watchers = self.approval_watchers.clone();
        if watchers.fetch_add(1, Ordering::SeqCst) == 0 {
            let http_client = cx.http_client();
            let loop_watchers = watchers.clone();
            // Replacing the slot drops (cancels) any old loop mid-sleep, so
            // a fast unwatch→watch can never run two loops at once.
            self.approval_task = Some(cx.spawn(async move |this, cx| {
                approvals_poll_loop(http_client, loop_watchers, this, cx).await
            }));
        }
        ApprovalWatch { watchers }
    }

    /// Apply a `GET /approvals` poll result — replace-on-frame + change-gated,
    /// the same semantics as the SSE `permission` arm so the poll and the
    /// push never disagree.
    pub(super) fn apply_approvals(
        &mut self,
        pending: Vec<ApprovalRow>,
        cx: &mut gpui::Context<Self>,
    ) {
        if self.pending_approvals != pending {
            self.pending_approvals = pending;
            cx.notify();
        }
    }
}

/// The `/approvals` poll task — spawned by the store ONLY while at least one
/// [`ApprovalWatch`] is held (Lightness: consumer-presence-gated, never
/// free-running). Fetches immediately on spawn (a fresh toast/panel sees the
/// pending set at once), then at the 1.5s cadence; exits when the watcher
/// count hits zero or the store is gone. Fetch failures keep the last
/// representation and retry next tick — the web's silent catch.
async fn approvals_poll_loop(
    http_client: Arc<dyn HttpClient>,
    watchers: Arc<AtomicUsize>,
    this: WeakEntity<BridgeStore>,
    cx: &mut AsyncApp,
) {
    loop {
        if watchers.load(Ordering::SeqCst) == 0 {
            return; // toast retired / panel hidden — the next watch respawns
        }
        let client = http_client.clone();
        let fetched = cx
            .background_spawn(async move {
                let raw =
                    fetch_json(client.as_ref(), &format!("{BRIDGE_BASE_URL}/approvals")).await?;
                parse_approvals(&raw)
            })
            .await;
        if let Ok(pending) = fetched
            && this
                .update(cx, |store, cx| store.apply_approvals(pending, cx))
                .is_err()
        {
            return; // store dropped
        }
        cx.background_executor()
            .timer(APPROVALS_POLL_INTERVAL)
            .await;
    }
}

/// The `GET /approvals` body: `{"pending": [rows]}`.
fn parse_approvals(raw: &str) -> Result<Vec<ApprovalRow>> {
    #[derive(Default, serde::Deserialize)]
    struct ApprovalsResponse {
        #[serde(default)]
        pending: Vec<ApprovalRow>,
    }
    let response: ApprovalsResponse = serde_json::from_str(raw)?;
    Ok(response.pending)
}

/// `POST /approvals/<id>/allow` — approve the gated spawn. `note` rides the
/// body when present; on refusal the bridge's `{"error"}` message surfaces
/// VERBATIM in the `Err` (the [`post_json`] law).
pub async fn post_approval_allow(
    client: &dyn HttpClient,
    id: &str,
    note: Option<&str>,
) -> Result<String> {
    post_approval_decision(client, id, "allow", note).await
}

/// `POST /approvals/<id>/deny` — refuse the gated spawn. The note lands
/// verbatim in the run's failure text bridge-side (reject-with-message).
pub async fn post_approval_deny(
    client: &dyn HttpClient,
    id: &str,
    note: Option<&str>,
) -> Result<String> {
    post_approval_decision(client, id, "deny", note).await
}

/// The shared decision POST: body is `{}` bare, `{"note": …}` with a note
/// (serde-encoded — never hand-spliced into JSON).
async fn post_approval_decision(
    client: &dyn HttpClient,
    id: &str,
    decision: &str,
    note: Option<&str>,
) -> Result<String> {
    let body = match note {
        Some(note) => serde_json::json!({ "note": note }).to_string(),
        None => "{}".to_string(),
    };
    let url = format!("{BRIDGE_BASE_URL}/approvals/{id}/{decision}");
    post_json(client, &url, body).await
}

#[cfg(test)]
#[path = "approvals_tests.rs"]
mod tests;
