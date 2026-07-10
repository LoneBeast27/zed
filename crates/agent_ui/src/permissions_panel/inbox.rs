//! The PERMISSIONS runtime cards (design §5.1, split from `view.rs` at the
//! 500-line ceiling — single concern: how LIVE decisions render): the
//! pending-approvals inbox (store-fed rows, TTL countdown, Allow / Deny…
//! with the optional-note input) and the audit tail (the last 20 ledger
//! decisions, newest first). POSTs are never offered in demo.

use gpui::{AnyElement, Context, ElementId, FontWeight, SharedString};
use ui::prelude::*;

use crate::agent_accents::{STATUS_BLOCKED, STATUS_ERROR, accent_for_agent};
use crate::bridge::{self, ApprovalRow};
use crate::task_board::style::{SURFACE_1, ago};

use super::actions::Decision;
use super::data::{expires_line, verdict_color};
use super::view::now_unix;
use super::PermissionsPanel;

impl PermissionsPanel {
    // ── card 4: pending approvals inbox ──

    pub(super) fn render_pending_card(&self, mono: &SharedString, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors();
        let demo = bridge::is_agentic_demo();
        let connected = self.store.read(cx).connected;
        let pending = self.pending_rows(cx);
        let now = now_unix();
        let mut rows: Vec<AnyElement> = Vec::new();
        if !demo && !connected {
            rows.push(
                div()
                    .text_size(px(11.))
                    .text_color(STATUS_BLOCKED)
                    .child("bridge unreachable — pending approvals may be stale")
                    .into_any_element(),
            );
        }
        if pending.is_empty() {
            rows.push(
                div()
                    .text_size(px(11.))
                    .text_color(colors.text_placeholder)
                    .child(
                        "No pending approvals — a spawn matching an ask rule queues \
                         here (and denies itself after the TTL, fail closed).",
                    )
                    .into_any_element(),
            );
        }
        for (ix, row) in pending.iter().enumerate() {
            rows.push(self.render_approval_row(ix, row, demo, now, mono, cx));
        }
        self.card("Pending approvals", rows, None, cx)
    }

    /// One inbox row: identity chips + task/cwd/rule, TTL countdown, and the
    /// Allow / Deny… actions (deny opens the optional-note input row).
    fn render_approval_row(
        &self,
        ix: usize,
        row: &ApprovalRow,
        demo: bool,
        now: f64,
        mono: &SharedString,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = cx.theme().colors();
        let ttl = expires_line(row.expires_ts, now);
        let expired = ttl.starts_with("expired");

        let mut card = v_flex()
            .w_full()
            .gap(px(6.))
            .p(px(10.))
            .rounded(px(10.))
            .bg(SURFACE_1)
            .border_1()
            .border_color(colors.border)
            .mb(px(6.))
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        h_flex()
                            .flex_none()
                            .items_center()
                            .gap(px(6.))
                            .child(
                                div()
                                    .size(px(7.))
                                    .rounded_full()
                                    .bg(accent_for_agent(&row.agent)),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .font_family(mono.clone())
                                    .text_color(colors.text_muted)
                                    .child(SharedString::from(row.agent.clone())),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(11.))
                            .font_family(mono.clone())
                            .text_color(colors.text_placeholder)
                            .truncate()
                            .child(SharedString::from(format!("{} · {}", row.run_id, row.mode))),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(11.))
                            .font_family(mono.clone())
                            .text_color(if expired { STATUS_ERROR } else { STATUS_BLOCKED })
                            .child(SharedString::from(ttl)),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .text_size(px(13.))
                    .text_color(colors.text)
                    .child(SharedString::from(row.task_head.clone())),
            )
            .child(
                div()
                    .w_full()
                    .text_size(px(11.))
                    .font_family(mono.clone())
                    .text_color(colors.text_muted)
                    .truncate()
                    .child(SharedString::from(row.cwd.clone())),
            )
            .child(
                div()
                    .w_full()
                    .text_size(px(11.))
                    .font_family(mono.clone())
                    .text_color(colors.text_placeholder)
                    .child(SharedString::from(row.rule.clone())),
            );

        if !demo {
            card = card.child(self.render_approval_actions(ix, row, mono, cx));
        }
        if let Some(note) = self.note_line(&row.id, mono, cx) {
            card = card.child(note);
        }
        card.into_any_element()
    }

    /// The Allow / Deny… action row (or the open deny-note input, or the
    /// in-flight collapse). POSTs are never offered in demo.
    fn render_approval_actions(
        &self,
        ix: usize,
        row: &ApprovalRow,
        mono: &SharedString,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = cx.theme().colors();
        if self.in_flight.contains(&row.id) {
            return self.working_note(mono, cx);
        }
        if self.deny_prompt.as_deref() == Some(row.id.as_str()) {
            // The optional-note input row (the new_note compose idiom).
            let mut input = h_flex().w_full().items_center().gap(px(6.));
            if let Some(editor) = self.note_editor.clone() {
                input = input.child(
                    div()
                        .flex_1()
                        .px(px(8.))
                        .py(px(4.))
                        .rounded(px(8.))
                        .bg(SURFACE_1)
                        .border_1()
                        .border_color(colors.border)
                        .child(editor),
                );
            }
            let deny_id = row.id.clone();
            input = input
                .child(self.button(
                    ElementId::Name(format!("approval-deny-send-{ix}").into()),
                    "Deny".into(),
                    true,
                    true,
                    cx,
                    move |this, _, cx| this.submit_deny(deny_id.clone(), cx),
                ))
                .child(self.button(
                    ElementId::Name(format!("approval-deny-cancel-{ix}").into()),
                    "Cancel".into(),
                    false,
                    true,
                    cx,
                    move |this, _, cx| this.cancel_deny(cx),
                ));
            return input.into_any_element();
        }
        let allow_id = row.id.clone();
        let deny_id = row.id.clone();
        h_flex()
            .items_center()
            .gap(px(6.))
            .child(self.button(
                ElementId::Name(format!("approval-allow-{ix}").into()),
                "Allow".into(),
                true,
                true,
                cx,
                move |this, _, cx| this.approval_decision(allow_id.clone(), Decision::Allow, cx),
            ))
            .child(self.button(
                ElementId::Name(format!("approval-deny-{ix}").into()),
                "Deny…".into(),
                false,
                true,
                cx,
                move |this, window, cx| this.open_deny_prompt(deny_id.clone(), window, cx),
            ))
            .into_any_element()
    }

    // ── card 5: audit tail ──

    pub(super) fn render_audit_card(&self, mono: &SharedString, cx: &mut Context<Self>) -> AnyElement {
        let Some(snapshot) = &self.snapshot else {
            return div().into_any_element();
        };
        let colors = cx.theme().colors();
        let now = now_unix();
        let mut rows: Vec<AnyElement> = Vec::new();
        if snapshot.audit_tail.is_empty() {
            rows.push(
                div()
                    .text_size(px(11.))
                    .text_color(colors.text_placeholder)
                    .child("No decisions recorded yet — the ledger appends one line per verdict.")
                    .into_any_element(),
            );
        }
        // Bridge serves oldest-first; the panel reads newest-first.
        for line in snapshot.audit_tail.iter().rev() {
            let who = line.agent.clone().unwrap_or_default();
            let detail = if line.note.is_empty() {
                format!("{who} · {} · by {}", line.rule, line.decided_by)
            } else {
                format!(
                    "{who} · {} · by {} — {}",
                    line.rule, line.decided_by, line.note
                )
            };
            rows.push(
                h_flex()
                    .w_full()
                    .items_baseline()
                    .gap(px(8.))
                    .py(px(3.))
                    .child(
                        div()
                            .flex_none()
                            .w(px(44.))
                            .text_size(px(11.))
                            .font_family(mono.clone())
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(verdict_color(&line.verdict))
                            .child(SharedString::from(line.verdict.clone())),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(11.))
                            .font_family(mono.clone())
                            .text_color(colors.text_muted)
                            .truncate()
                            .child(SharedString::from(detail)),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(11.))
                            .text_color(colors.text_placeholder)
                            .child(SharedString::from(ago(line.ts, now))),
                    )
                    .into_any_element(),
            );
        }
        self.card(
            "Audit tail",
            rows,
            Some("The last 20 ledger decisions, newest first — the append-only JSONL is the full record."),
            cx,
        )
    }
}
