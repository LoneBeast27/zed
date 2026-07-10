//! The run drawer's pending-approval section (Phase-2 permissions §5.2,
//! wave 3c): a run held in `awaiting_*` shows the pending request — matched
//! rule · mode · cwd · TTL countdown — with inline Allow / Deny in the
//! thread_view `render_permission_buttons` grammar at its
//! `PermissionOptions::Flat` shape, kinds AllowOnce | RejectOnce ONLY (the
//! AllowAlways/dropdown grammar is the reserved Wave-3+ era), plus the
//! optional deny note (OpenCode reject-with-message parity: the note lands
//! verbatim in the run's failure text bridge-side).
//!
//! Decisions ride the 3a transport helpers. A 404/409 refusal renders the
//! bridge's `{"error"}` VERBATIM on the drawer's action-error line (a
//! refusal reads as a refusal, not offline). On 2xx nothing flips locally —
//! server truth only: the 1s tail poll + the pending frame's replace
//! semantics move the drawer on. Nothing in this section ever takes focus
//! on its own (the no-focus-theft law) — the deny-note editor focuses only
//! on the user's own toggle click.

use std::sync::Arc;

use gpui::{AnyElement, Context, Focusable as _, FontWeight, SharedString, Window};
use http_client::HttpClient;
use settings::Settings as _;
use theme_settings::ThemeSettings;
use ui::prelude::*;

use crate::agent_accents::{ACCENT_FILL, STATUS_BLOCKED};
use crate::bridge::{ApprovalRow, post_approval_allow, post_approval_deny};
use crate::islands::notif_logic::{expiry_countdown, unix_now};

use super::run_detail::RunDrawer;
use super::style::tabular_nums;

/// The pending approval gating `run_id`, if any — the §5.2 store-row →
/// drawer match (pure).
pub(super) fn pending_for_run<'a>(
    pending: &'a [ApprovalRow],
    run_id: &str,
) -> Option<&'a ApprovalRow> {
    pending.iter().find(|row| row.run_id == run_id)
}

impl RunDrawer {
    /// Toggle the optional deny-note input row (the steer-editor idiom).
    /// Focus moves only here — on the user's own click, never on surfacing.
    pub(super) fn toggle_deny_note(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.deny_editor.is_some() {
            self.deny_editor = None;
        } else {
            let editor = cx.new(|cx| {
                let mut editor = editor::Editor::single_line(window, cx);
                editor.set_placeholder_text(
                    "Why deny? (optional — lands in the run's failure text)",
                    window,
                    cx,
                );
                editor
            });
            window.focus(&editor.read(cx).focus_handle(cx), cx);
            self.deny_editor = Some(editor);
        }
        cx.notify();
    }

    /// POST the decision (`/approvals/<id>/allow` / `/deny`, 3a helpers).
    /// In-flight collapse: one POST at a time. Success leaves the section
    /// collapsed until server truth retires it; failure re-arms the buttons
    /// with the refusal VERBATIM.
    pub(super) fn decide_approval(
        &mut self,
        approval_id: String,
        allow: bool,
        cx: &mut Context<Self>,
    ) {
        if self.local || self.deciding_approval.is_some() {
            return;
        }
        let note = if allow {
            None
        } else {
            self.deny_editor
                .as_ref()
                .map(|editor| editor.read(cx).text(cx).trim().to_string())
                .filter(|note| !note.is_empty())
        };
        self.deciding_approval = Some(approval_id.clone());
        self.action_error = None;
        cx.notify();
        let http_client: Arc<dyn HttpClient> = cx.http_client();
        self._action = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    if allow {
                        post_approval_allow(http_client.as_ref(), &approval_id, None).await
                    } else {
                        post_approval_deny(http_client.as_ref(), &approval_id, note.as_deref())
                            .await
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(_) => {
                        // Landed: stay collapsed — the tail poll + the
                        // pending frame carry the run forward (server truth,
                        // never optimistic).
                        this.deny_editor = None;
                    }
                    Err(error) => {
                        // 404 unknown / 409 already-resolved / expired /
                        // transport — the `{"error"}` message VERBATIM; the
                        // buttons re-arm for a retry.
                        this.deciding_approval = None;
                        this.action_error = Some(format!("{error}").into());
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// The §5.2 drawer section, present only while the bridge holds a
    /// pending approval for THIS run: amber held card under the head with
    /// the request meta and the flat Allow / Deny pair.
    pub(super) fn render_approval_row(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.local {
            return None;
        }
        let store = self.store.as_ref()?;
        let row = pending_for_run(&store.read(cx).pending_approvals, &self.run_id)?.clone();
        let deciding = self.deciding_approval.is_some();
        // Copy the Hsla tokens out (the render_foot idiom) — the button
        // closures below take listeners while these stay plain values.
        let (border, text_muted, hover_bg, placeholder, element_bg) = {
            let colors = cx.theme().colors();
            (
                colors.border,
                colors.text_muted,
                colors.element_hover,
                colors.text_placeholder,
                colors.element_background,
            )
        };
        let mono = ThemeSettings::get_global(cx).buffer_font.family.clone();
        let amber: gpui::Hsla = STATUS_BLOCKED.into();
        let countdown: SharedString =
            format!("expires {}", expiry_countdown(row.expires_ts, unix_now())).into();

        // Meta line: the matched rule + effective mode, honest and compact.
        let mut meta = row.rule.clone();
        if !row.mode.is_empty() {
            if !meta.is_empty() {
                meta.push_str(" · ");
            }
            meta.push_str("mode ");
            meta.push_str(&row.mode);
        }

        let header = h_flex()
            .items_center()
            .gap(px(7.))
            .child(div().flex_none().size(px(7.)).rounded_full().bg(amber))
            .child(
                div()
                    .text_size(px(13.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(amber)
                    .child("Awaiting your approval"),
            )
            .child(div().flex_1())
            .child(
                div()
                    .font_family(mono.clone())
                    .text_size(px(11.5))
                    .font_features(tabular_nums())
                    .text_color(amber.opacity(0.9))
                    .child(countdown),
            );

        // The flat decision pair (AllowOnce | RejectOnce): Allow filled
        // (the send-circle ACCENT_FILL grammar), Deny outlined, plus the
        // deny-note toggle. In flight, both collapse to "deciding…".
        let allow_id = row.id.clone();
        let deny_id = row.id.clone();
        let deny_label: SharedString = if self.deny_editor.is_some() {
            "Deny with note".into()
        } else {
            "Deny".into()
        };
        let note_label: SharedString = if self.deny_editor.is_some() {
            "drop the note".into()
        } else {
            "add a deny note…".into()
        };
        let decisions = h_flex()
            .items_center()
            .gap(px(8.))
            .when(!deciding, |row| {
                row.child(
                    div()
                        .id("drawer-approval-allow")
                        .px(px(12.))
                        .py(px(4.))
                        .rounded_full()
                        .bg(gpui::Hsla::from(ACCENT_FILL))
                        .text_size(px(12.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(gpui::black())
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.decide_approval(allow_id.clone(), true, cx);
                        }))
                        .child("Allow"),
                )
                .child(
                    div()
                        .id("drawer-approval-deny")
                        .px(px(12.))
                        .py(px(4.))
                        .rounded_full()
                        .border_1()
                        .border_color(border)
                        .text_size(px(12.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(text_muted)
                        .cursor_pointer()
                        .hover(move |x| x.bg(hover_bg))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.decide_approval(deny_id.clone(), false, cx);
                        }))
                        .child(deny_label),
                )
                .child(
                    div()
                        .id("drawer-approval-note")
                        .px(px(6.))
                        .py(px(4.))
                        .text_size(px(12.))
                        .text_color(placeholder)
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.toggle_deny_note(window, cx);
                        }))
                        .child(note_label),
                )
            })
            .when(deciding, |row| {
                row.child(
                    div()
                        .text_size(px(12.))
                        .text_color(placeholder)
                        .child("deciding…"),
                )
            });

        let note_editor = (!deciding)
            .then(|| self.deny_editor.clone())
            .flatten()
            .map(|editor| {
                div()
                    .px(px(10.))
                    .py(px(5.))
                    .rounded(px(8.))
                    .border_1()
                    .border_color(border)
                    .bg(element_bg)
                    .child(editor)
            });

        let mut card = v_flex()
            .flex_none()
            .mx(px(22.))
            .mt(px(12.))
            .p(px(12.))
            .rounded(px(12.))
            .border_1()
            .border_color(amber.opacity(0.30))
            .bg(amber.opacity(0.06))
            .gap(px(8.))
            .child(header);
        if !meta.is_empty() {
            card = card.child(
                div()
                    .text_size(px(12.))
                    .text_color(text_muted)
                    .line_clamp(1)
                    .child(SharedString::from(meta)),
            );
        }
        if !row.cwd.is_empty() {
            card = card.child(
                div()
                    .font_family(mono)
                    .text_size(px(11.5))
                    .text_color(placeholder)
                    .truncate()
                    .child(SharedString::from(row.cwd.clone())),
            );
        }
        card = card.child(decisions);
        if let Some(note_editor) = note_editor {
            card = card.child(note_editor);
        }
        Some(card.into_any_element())
    }
}
