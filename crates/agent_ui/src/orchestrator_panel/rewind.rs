//! Per-turn rewind (FEATURE_SENTIMENT §C) — the dual-axis, non-destructive,
//! untracked-honest rewind affordance on a user message row.
//!
//! Four laws from §C, each load-bearing:
//! - **C-T1 dual-axis independence** — code / conversation / both are three
//!   selectable axes, never fused. A user restores *code only* (keep the
//!   discussion) or *conversation only* (keep the code on disk) or *both*.
//! - **C-T3 non-destructive** — after a rewind the bridge returns a `redo_sha`;
//!   the UI surfaces a REDO affordance so "you can always undo this later" is
//!   LITERALLY true. Forward history survives.
//! - **C-T4 untracked honesty** — the bridge may return an `untracked_warning`
//!   (bash `rm`/`mv`, hand-edits it can't restore). It is rendered VERBATIM,
//!   never swallowed (the "permanent surprise" anti-pattern).
//! - **409 sibling-active** — a rewind refused because a sibling worker is live
//!   surfaces the bridge's message honestly, never a silent no-op.
//!
//! The POST wiring lives on [`OrchestratorPanel`]; the axis payload + response
//! parse + note synthesis are PURE (unit-tested here).

use gpui::{AnyElement, AppContext as _, SharedString, Window};
use serde::Deserialize;
use ui::prelude::*;

use crate::bridge::{BRIDGE_BASE_URL, error_message, post_json_status};

use super::panel::OrchestratorPanel;

/// The dual-axis rewind selection (C-T1). Independent, never fused: the three
/// variants map to the three real failure modes from §C — execution failed but
/// intent was right (**Code**), context polluted with failed attempts
/// (**Conversation**), total rollback (**Both**).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RewindAxis {
    Code,
    Conversation,
    Both,
}

impl RewindAxis {
    /// The `axis` wire value the bridge expects
    /// (`POST /conv/<id>/rewind {turn_n, axis}`).
    pub(super) fn wire(self) -> &'static str {
        match self {
            RewindAxis::Code => "code",
            RewindAxis::Conversation => "conversation",
            RewindAxis::Both => "both",
        }
    }

    /// The menu row label.
    pub(super) fn label(self) -> &'static str {
        match self {
            RewindAxis::Code => "Code only",
            RewindAxis::Conversation => "Conversation only",
            RewindAxis::Both => "Code + conversation",
        }
    }
}

/// The `turn_n` for a rewind targeting transcript view index `ix`: the ordinal
/// of that USER message among user messages (1-based), i.e. "rewind to before
/// this prompt" enumerates every prompt as a restore target (C-T2). PURE over
/// the (user?, index) view flags so it is unit-tested without a panel.
///
/// Returns `None` when `ix` is not a user row (rewind attaches to user prompts,
/// not agent replies) or is out of range.
pub(super) fn turn_for_index(user_flags: &[bool], ix: usize) -> Option<u32> {
    if !user_flags.get(ix).copied().unwrap_or(false) {
        return None;
    }
    // 1-based ordinal among user messages up to and including `ix`.
    Some(user_flags[..=ix].iter().filter(|u| **u).count() as u32)
}

/// The bridge's `POST /conv/<id>/rewind` reply (Wave C):
/// `{redo_sha, restored_paths, untracked_warning?}`. Every field liberal —
/// bridge drift never breaks the affordance.
#[derive(Debug, Clone, Default, Deserialize)]
pub(super) struct RewindResponse {
    /// The redo handle (C-T3): a rewind is non-destructive — this is the sha to
    /// pass to `POST /conv/<id>/redo` to move FORWARD again. Empty = the bridge
    /// gave no redo point (the affordance then honestly reads no-redo).
    #[serde(default)]
    pub redo_sha: String,
    #[serde(default)]
    pub restored_paths: Vec<String>,
    /// The untracked-change honesty field (C-T4): bash/manual edits the rewind
    /// could NOT restore. Rendered VERBATIM when present, never swallowed.
    #[serde(default)]
    pub untracked_warning: Option<String>,
}

/// The result state a landed rewind leaves for render — the redo handle (so the
/// "undo this later" affordance is literally true, C-T3) plus the verbatim
/// untracked warning (C-T4). Kept together so a redo consumes them as a unit.
#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct RewindOutcome {
    /// `Some(redo_sha)` while a redo is possible (non-empty sha) — the REDO
    /// affordance is shown ONLY while this is Some (honesty: no dead "undo"
    /// button when the bridge offered no redo point).
    pub redo_sha: Option<String>,
    /// The bridge's untracked-change warning VERBATIM (C-T4), shown until the
    /// next rewind/redo clears the outcome.
    pub untracked_warning: Option<String>,
    /// A short human note (paths restored) for the composer note surface.
    pub note: String,
}

/// Synthesize the [`RewindOutcome`] from a parsed response. PURE: the redo
/// affordance is armed iff `redo_sha` is non-empty (C-T3 honesty), and the
/// untracked warning rides through untouched (C-T4).
pub(super) fn outcome_from_response(axis: RewindAxis, response: RewindResponse) -> RewindOutcome {
    let n = response.restored_paths.len();
    let redo_sha = (!response.redo_sha.is_empty()).then_some(response.redo_sha);
    // The note names the axis + coverage; the redo/untracked lines render as
    // their own affordance rows so they can't be lost in prose.
    let note = match axis {
        RewindAxis::Conversation => "Rewound the conversation.".to_string(),
        _ => format!(
            "Rewound {} — {n} file{} restored.",
            axis.label().to_ascii_lowercase(),
            if n == 1 { "" } else { "s" }
        ),
    };
    RewindOutcome {
        redo_sha,
        untracked_warning: response.untracked_warning.filter(|w| !w.is_empty()),
        note,
    }
}

impl OrchestratorPanel {
    /// Toggle the per-turn rewind axis menu for view index `ix`. Opening a
    /// different row closes the previous menu (one open at a time).
    pub(super) fn toggle_rewind_menu(&mut self, ix: usize, cx: &mut gpui::Context<Self>) {
        self.rewind_menu_ix = if self.rewind_menu_ix == Some(ix) {
            None
        } else {
            Some(ix)
        };
        cx.notify();
    }

    /// POST `/conv/<id>/rewind {turn_n, axis}` for the user message at view
    /// index `ix`. Closes the menu, then lands the HONEST outcome:
    /// - 2xx → the redo affordance (C-T3) + any untracked_warning VERBATIM
    ///   (C-T4) on the [`Self::rewind_outcome`] state, plus a note;
    /// - 409 (sibling worker active) / 404 / any refusal → the bridge's
    ///   `{"error"}` VERBATIM on the note surface, never a silent no-op.
    /// Fire-and-forget on the background executor (never steals focus).
    pub(super) fn rewind_to(
        &mut self,
        ix: usize,
        axis: RewindAxis,
        _window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        self.rewind_menu_ix = None;
        let Some(turn_n) = turn_for_index(&self.transcript.user_flags(), ix) else {
            return;
        };
        let conv = {
            let store = self.store.read(cx);
            store
                .transcript
                .as_ref()
                .map(|snapshot| snapshot.id.clone())
                .or_else(|| store.transcript_conv.clone())
        };
        let Some(conv) = conv else { return };
        self.wave_note = None;
        cx.notify();

        let client = cx.http_client();
        let store = self.store.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let url = format!("{BRIDGE_BASE_URL}/conv/{conv}/rewind");
                    let body =
                        serde_json::json!({ "turn_n": turn_n, "axis": axis.wire() }).to_string();
                    post_json_status(client.as_ref(), &url, body).await
                })
                .await;
            this.update(cx, |this, cx| {
                match result {
                    Ok((status, raw)) if (200..300).contains(&status) => {
                        let response: RewindResponse =
                            serde_json::from_str(&raw).unwrap_or_default();
                        let outcome = outcome_from_response(axis, response);
                        this.wave_note = Some(outcome.note.clone().into());
                        this.rewind_outcome = Some(outcome);
                    }
                    // 409 sibling-active / 404 / refusal — the bridge's error
                    // VERBATIM, never swallowed.
                    Ok((status, raw)) => {
                        this.wave_note = Some(error_message(status, &raw).into());
                    }
                    Err(error) => {
                        this.wave_note = Some(format!("rewind failed: {error}").into());
                    }
                }
                cx.notify();
            })
            .ok();
            // The restored code/conversation lands via the transcript refetch.
            let _ = store.update(cx, |store, cx| store.refetch_transcript_soon(cx));
        })
        .detach();
    }

    /// POST `/conv/<id>/redo {redo_sha}` — the non-destructive forward move
    /// (C-T3: "you can always undo this later" made literally true). Consumes
    /// the [`Self::rewind_outcome`] redo handle; a refusal lands verbatim.
    pub(super) fn redo_rewind(&mut self, cx: &mut gpui::Context<Self>) {
        let Some(redo_sha) = self
            .rewind_outcome
            .as_ref()
            .and_then(|o| o.redo_sha.clone())
        else {
            return;
        };
        // Clear the outcome optimistically — the redo affordance is single-use
        // until the next rewind re-arms it.
        self.rewind_outcome = None;
        self.wave_note = None;
        cx.notify();
        let conv = {
            let store = self.store.read(cx);
            store
                .transcript
                .as_ref()
                .map(|snapshot| snapshot.id.clone())
                .or_else(|| store.transcript_conv.clone())
        };
        let Some(conv) = conv else { return };
        let client = cx.http_client();
        let store = self.store.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let url = format!("{BRIDGE_BASE_URL}/conv/{conv}/redo");
                    let body = serde_json::json!({ "redo_sha": redo_sha }).to_string();
                    post_json_status(client.as_ref(), &url, body).await
                })
                .await;
            this.update(cx, |this, cx| {
                let note = match result {
                    Ok((status, _)) if (200..300).contains(&status) => {
                        "Redone — moved forward to the later state.".to_string()
                    }
                    Ok((status, raw)) => error_message(status, &raw),
                    Err(error) => format!("redo failed: {error}"),
                };
                this.wave_note = Some(note.into());
                cx.notify();
            })
            .ok();
            let _ = store.update(cx, |store, cx| store.refetch_transcript_soon(cx));
        })
        .detach();
    }

    /// The rewind outcome affordance rows above the composer (§C): the REDO
    /// button (C-T3, shown ONLY while a `redo_sha` is armed) + the verbatim
    /// untracked-change warning (C-T4). `None` until a rewind lands; cleared on
    /// redo or the next rewind. Rendered in flow above the deck.
    pub(super) fn render_rewind_outcome(&self, cx: &mut gpui::Context<Self>) -> Option<AnyElement> {
        let outcome = self.rewind_outcome.as_ref()?;
        let colors = cx.theme().colors();
        let mut col = gpui::div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .px(px(14.))
            .pb(px(6.));

        // C-T4: the untracked-change warning VERBATIM — a plain honest line in
        // the "blocked" tone, never swallowed, never rewritten.
        if let Some(warning) = &outcome.untracked_warning {
            col = col.child(
                gpui::div()
                    .text_size(px(12.))
                    .text_color(gpui::Hsla::from(crate::agent_accents::color_for_status(
                        "blocked",
                    )))
                    .child(SharedString::from(warning.clone())),
            );
        }

        // C-T3: the REDO affordance — literally "you can always undo this
        // later". Present iff a redo_sha is armed (no dead button otherwise).
        if outcome.redo_sha.is_some() {
            col = col.child(
                h_flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        gpui::div()
                            .text_size(px(12.))
                            .text_color(colors.text_muted)
                            .child("Rewound — non-destructive."),
                    )
                    .child(
                        h_flex()
                            .id("rewind-redo")
                            .items_center()
                            .gap(px(5.))
                            .px(px(9.))
                            .py(px(3.))
                            .rounded(px(7.))
                            .border_1()
                            .border_color(colors.border)
                            .text_size(px(12.))
                            .text_color(colors.text)
                            .cursor_pointer()
                            .hover(|s| s.bg(colors.element_hover))
                            .on_click(cx.listener(|this, _, _, cx| this.redo_rewind(cx)))
                            .child(
                                Icon::new(IconName::RotateCw)
                                    .size(IconSize::Small)
                                    .color(Color::Custom(colors.text)),
                            )
                            .child("Redo"),
                    ),
            );
        }

        Some(col.into_any_element())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn axis_wire_and_labels_are_the_three_independent_axes() {
        // C-T1: the three axes are distinct wire values — never fused.
        assert_eq!(RewindAxis::Code.wire(), "code");
        assert_eq!(RewindAxis::Conversation.wire(), "conversation");
        assert_eq!(RewindAxis::Both.wire(), "both");
        // All three distinct — the independence guard.
        let wires = [
            RewindAxis::Code.wire(),
            RewindAxis::Conversation.wire(),
            RewindAxis::Both.wire(),
        ];
        assert_eq!(
            wires.iter().collect::<std::collections::HashSet<_>>().len(),
            3,
            "the three axes must be distinct"
        );
    }

    #[test]
    fn turn_for_index_enumerates_every_user_prompt() {
        // Transcript: user, agent, user, agent, user  → prompts 1,2,3 at the
        // user rows (C-T2 "rewind to any point" — each prompt is a target).
        let flags = [true, false, true, false, true];
        assert_eq!(turn_for_index(&flags, 0), Some(1));
        assert_eq!(turn_for_index(&flags, 2), Some(2));
        assert_eq!(turn_for_index(&flags, 4), Some(3));
        // Agent rows are NOT rewind targets — the affordance attaches to user
        // prompts only.
        assert_eq!(turn_for_index(&flags, 1), None);
        assert_eq!(turn_for_index(&flags, 3), None);
        // Out of range.
        assert_eq!(turn_for_index(&flags, 9), None);
    }

    #[test]
    fn outcome_arms_redo_only_when_sha_present() {
        // C-T3: the REDO affordance exists iff the bridge returned a redo_sha —
        // never a dead "undo this later" button.
        let armed = outcome_from_response(
            RewindAxis::Both,
            RewindResponse {
                redo_sha: "abc123".into(),
                restored_paths: vec!["a.rs".into(), "b.rs".into()],
                untracked_warning: None,
            },
        );
        assert_eq!(armed.redo_sha.as_deref(), Some("abc123"));
        assert!(armed.note.contains("2 files"), "note names the coverage: {}", armed.note);

        // No redo_sha → no redo affordance (honest, not a phantom).
        let no_redo = outcome_from_response(
            RewindAxis::Code,
            RewindResponse {
                redo_sha: String::new(),
                restored_paths: vec!["only.rs".into()],
                untracked_warning: None,
            },
        );
        assert_eq!(no_redo.redo_sha, None);
        assert!(no_redo.note.contains("1 file "), "singular file: {}", no_redo.note);
    }

    #[test]
    fn untracked_warning_passes_through_verbatim() {
        // C-T4: the bridge's untracked_warning is rendered VERBATIM, never
        // swallowed — the exact string survives the parse→outcome pipeline.
        let warning = "2 files changed via bash (rm/mv) are NOT restored — permanent.";
        let outcome = outcome_from_response(
            RewindAxis::Both,
            RewindResponse {
                redo_sha: "sha".into(),
                restored_paths: vec![],
                untracked_warning: Some(warning.to_string()),
            },
        );
        assert_eq!(
            outcome.untracked_warning.as_deref(),
            Some(warning),
            "the untracked warning must survive verbatim (no swallow, no rewrite)"
        );
        // An EMPTY warning string is treated as absent (no empty honesty row).
        let empty = outcome_from_response(
            RewindAxis::Both,
            RewindResponse {
                redo_sha: "sha".into(),
                restored_paths: vec![],
                untracked_warning: Some(String::new()),
            },
        );
        assert_eq!(empty.untracked_warning, None);
    }

    #[test]
    fn conversation_axis_note_omits_file_count() {
        // A conversation-only rewind restores no files — the note must not
        // claim "0 files restored" (a code-axis phrase leaking into the
        // conversation axis).
        let outcome = outcome_from_response(
            RewindAxis::Conversation,
            RewindResponse::default(),
        );
        assert!(!outcome.note.to_lowercase().contains("file"), "conversation note: {}", outcome.note);
        assert!(outcome.note.to_lowercase().contains("conversation"));
    }
}
