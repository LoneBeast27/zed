//! The PERMISSIONS actions — the fetch and the POSTs behind the buttons
//! (design §5.1 `actions.rs`; render lives in `view.rs`, types/predicates in
//! `data.rs`).
//!
//! Laws carried from the routines/writeback recipe: the fetch is ONE-SHOT and
//! visibility-gated (surface activation + post-action refetch — never a free
//! poll; live pending rows ride the store's watch-gated `/approvals` poll
//! instead); every POST re-guards client-side before I/O, marks in-flight so
//! a double-click can't double-POST, and on 2xx REFETCHES server truth
//! (never optimistic). A non-2xx renders the bridge's `{"error"}` verbatim
//! (a refusal is a refusal, not offline); a transport failure reads "bridge
//! unreachable — action not delivered" under the stale flag.

use std::path::PathBuf;

use editor::Editor;
use gpui::{AppContext as _, Context, Focusable as _, TaskExt as _, Window};
use workspace::{OpenOptions, OpenVisible};

use crate::bridge::{
    self, BRIDGE_BASE_URL, fetch_json, post_approval_allow, post_approval_deny, post_json_status,
};

use super::data::{self, PermissionsSnapshot};
use super::{MODE_KEY, PermissionsPanel};

/// An inbox decision (the wave-3a endpoints).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Decision {
    Allow,
    /// The optional note lands VERBATIM in the denied run's failure text
    /// (OpenCode reject-with-message parity).
    Deny { note: Option<String> },
}

/// The user-scope policy file (`<bridge_root>/.permissions.json`, design §2)
/// — the same `AGENTIC_BRIDGE_ROOT` resolution the bridge-autostart uses
/// (client.rs), so the jump opens the file the engine actually reads on the
/// standard single-box install.
pub(super) fn user_permissions_file() -> PathBuf {
    let root = std::env::var("AGENTIC_BRIDGE_ROOT")
        .unwrap_or_else(|_| r"L:\Projects\agentic-ide".to_string());
    PathBuf::from(root).join(".permissions.json")
}

impl PermissionsPanel {
    /// The conversation the mode POST scopes to: the store's followed
    /// (canonicalized) conversation — `Some` ⇒ session override, `None` ⇒
    /// the user-file default. The card states which before any click.
    pub(super) fn followed_conv(&self, cx: &gpui::App) -> Option<String> {
        self.store.read(cx).transcript_conv.clone()
    }

    /// One-shot `GET /permissions?conv=` — visibility-gated (surface
    /// activation + post-action), never polled. A conv the bridge no longer
    /// knows (restart wiped it) falls back ONCE to the global chain rather
    /// than reading as offline; a real transport failure keeps the last
    /// snapshot under the stale flag.
    pub(super) fn fetch_permissions(&mut self, cx: &mut Context<Self>) {
        if bridge::is_agentic_demo() {
            return; // the staged snapshot is the demo's truth
        }
        self.loading = true;
        let client = cx.http_client();
        let conv = self.followed_conv(cx);
        self.fetch_task = Some(cx.spawn(async move |this, cx| {
            let fetched = cx
                .background_spawn(async move {
                    let base = format!("{BRIDGE_BASE_URL}/permissions");
                    let url = match &conv {
                        Some(conv) => format!("{base}?conv={conv}"),
                        None => base.clone(),
                    };
                    match fetch_json(client.as_ref(), &url).await {
                        Ok(raw) => anyhow::Ok(serde_json::from_str::<PermissionsSnapshot>(&raw)?),
                        // 404 unknown conv (strict lookup bridge-side) — the
                        // global chain is the honest fallback; only a second
                        // failure reads as unreachable.
                        Err(err) if conv.is_some() => {
                            let raw = fetch_json(client.as_ref(), &base).await.map_err(|_| err)?;
                            anyhow::Ok(serde_json::from_str::<PermissionsSnapshot>(&raw)?)
                        }
                        Err(err) => Err(err),
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                this.loading = false;
                match fetched {
                    Ok(snapshot) => {
                        this.snapshot = Some(snapshot);
                        this.stale = false;
                    }
                    Err(_) => this.stale = true,
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// USER-gated mode click — `POST /permissions/mode`. Session scope when
    /// a conversation is followed (`{"mode", "conv"}`), user scope bare.
    /// Re-guards with [`data::mode_change_allowed`] before any I/O.
    pub(super) fn set_mode(&mut self, target: &'static str, cx: &mut Context<Self>) {
        if bridge::is_agentic_demo() {
            return; // POSTs are not offered in demo (and none render)
        }
        let Some(snapshot) = &self.snapshot else {
            return; // nothing fetched yet — buttons aren't rendered either
        };
        let conv = self.followed_conv(cx);
        if !data::mode_change_allowed(&snapshot.mode, conv.is_some(), target) {
            return; // that scope already holds this value — no redundant write
        }
        if !self.in_flight.insert(MODE_KEY.to_string()) {
            return; // a mode POST is already in flight
        }
        cx.notify();
        let client = cx.http_client();
        cx.spawn(async move |this, cx| {
            let outcome = cx
                .background_spawn(async move {
                    let body = match &conv {
                        Some(conv) => {
                            serde_json::json!({ "mode": target, "conv": conv }).to_string()
                        }
                        None => serde_json::json!({ "mode": target }).to_string(),
                    };
                    let url = format!("{BRIDGE_BASE_URL}/permissions/mode");
                    post_json_status(client.as_ref(), &url, body).await
                })
                .await;
            this.update(cx, |this, cx| {
                this.in_flight.remove(MODE_KEY);
                match outcome {
                    Ok((status, _)) if (200..300).contains(&status) => {
                        this.notes.remove(MODE_KEY);
                        // Refetch so the chain/enforcement reflect SERVER
                        // truth, never an optimistic flip.
                        this.fetch_permissions(cx);
                    }
                    // Refusal (400 unknown mode, 409 malformed user file,
                    // 404 conv gone): the bridge is ALIVE — its reason
                    // verbatim, no stale flag.
                    Ok((status, raw)) => {
                        this.notes
                            .insert(MODE_KEY.to_string(), data::refusal_note(status, &raw));
                    }
                    Err(_) => {
                        this.notes.insert(
                            MODE_KEY.to_string(),
                            "bridge unreachable — action not delivered".to_string(),
                        );
                        this.stale = true;
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// USER-gated Allow/Deny — the wave-3a decision helpers
    /// (`POST /approvals/<id>/allow|deny`). Re-guards with
    /// [`data::decision_allowed`] before any I/O; the post-decision refetch
    /// decides stale honestly (a refusal `Err` carries the bridge's
    /// `{"error"}` verbatim per the post_json law — rendered as the note —
    /// while the refetch outcome distinguishes alive-and-refusing from
    /// actually unreachable).
    pub(super) fn approval_decision(
        &mut self,
        id: String,
        decision: Decision,
        cx: &mut Context<Self>,
    ) {
        if bridge::is_agentic_demo() {
            return; // demo rows have no bridge to land on
        }
        let pending = self.store.read(cx).pending_approvals.clone();
        if !data::decision_allowed(&id, &pending, &self.in_flight) {
            return; // resolved elsewhere / already in flight — no I/O
        }
        self.in_flight.insert(id.clone());
        cx.notify();
        let client = cx.http_client();
        let post_id = id.clone();
        cx.spawn(async move |this, cx| {
            let outcome = cx
                .background_spawn(async move {
                    match &decision {
                        Decision::Allow => {
                            post_approval_allow(client.as_ref(), &post_id, None).await
                        }
                        Decision::Deny { note } => {
                            post_approval_deny(client.as_ref(), &post_id, note.as_deref()).await
                        }
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                this.in_flight.remove(&id);
                match outcome {
                    Ok(_) => {
                        this.notes.remove(&id);
                    }
                    // 404 unknown / 409 expired / already-resolved surface
                    // VERBATIM through the post_json error law; a transport
                    // failure reads as the client's error. Either way the
                    // refetch below settles the stale flag honestly.
                    Err(error) => {
                        this.notes.insert(id.clone(), error.to_string());
                    }
                }
                // Server truth: the audit tail + pending set + chain move on
                // every decision (the store's 1.5s poll converges the inbox
                // too; this lands the audit line immediately).
                this.fetch_permissions(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Open the deny-note input row for `id` (the house lazy single-line
    /// editor idiom, new_note.rs) — cleared and focused so typing starts
    /// immediately. Pure state flip + focus; the POST happens on submit.
    pub(super) fn open_deny_prompt(
        &mut self,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.note_editor.is_none() {
            self.note_editor = Some(cx.new(|cx| {
                let mut editor = Editor::single_line(window, cx);
                editor.set_placeholder_text(
                    "optional note — lands in the run's failure text…",
                    window,
                    cx,
                );
                editor
            }));
        }
        if let Some(editor) = &self.note_editor {
            editor.update(cx, |editor, cx| editor.set_text("", window, cx));
            editor.read(cx).focus_handle(cx).focus(window, cx);
        }
        self.deny_prompt = Some(id);
        cx.notify();
    }

    /// Close the deny-note row without posting (the candidate stays pending).
    pub(super) fn cancel_deny(&mut self, cx: &mut Context<Self>) {
        self.deny_prompt = None;
        cx.notify();
    }

    /// Submit the deny with the typed note (trimmed; empty ⇒ no note).
    pub(super) fn submit_deny(&mut self, id: String, cx: &mut Context<Self>) {
        let note = self
            .note_editor
            .as_ref()
            .map(|editor| editor.read(cx).text(cx).trim().to_string())
            .filter(|text| !text.is_empty());
        self.deny_prompt = None;
        self.approval_decision(id, Decision::Deny { note }, cx);
    }

    /// Jump-to-json: open a policy file in a center editor tab. DEFERRED out
    /// of the click listener (§11 / the open_doc idiom: `open_abs_path` is a
    /// layout mutation reached from a listener inside this panel's update).
    pub(super) fn jump_to_path(&mut self, path: PathBuf, window: &Window, cx: &mut Context<Self>) {
        let workspace = self.workspace.clone();
        cx.defer_in(window, move |_, window, cx| {
            workspace
                .update(cx, |workspace, cx| {
                    workspace
                        .open_abs_path(
                            path,
                            OpenOptions {
                                visible: Some(OpenVisible::None),
                                ..Default::default()
                            },
                            window,
                            cx,
                        )
                        .detach_and_log_err(cx);
                })
                .ok();
        });
    }
}
