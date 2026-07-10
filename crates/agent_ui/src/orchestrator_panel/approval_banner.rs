//! The above-composer approval banner (Phase-2 permissions §5.2, layer 3):
//! while the ACTIVE conversation has pending runtime approvals, ONE slim
//! amber strip sits in flow directly above the composer deck linking
//! attention to them — the thread_view "Subagents Awaiting Permission"
//! banner idiom (thread_view.rs:2663): click-to-open, non-modal, and it
//! NEVER takes focus (surfacing is pure render; the click routes via the
//! panel's deferred `open_run`, the user's own gesture).

use gpui::{AnyElement, SharedString};
use ui::prelude::*;

use crate::agent_accents::STATUS_BLOCKED;
use crate::bridge::ApprovalRow;

use super::panel::OrchestratorPanel;

/// Pending approvals scoped to the active conversation (pure). A row
/// matches when its `conv` IS the active one, or when it carries no conv
/// (single-conv bridge — nothing to mismatch against). No active
/// conversation (a fresh app before the transcript canonicalizes) → no
/// banner; the held toast still covers the global set.
pub(super) fn approvals_for_conv<'a>(
    pending: &'a [ApprovalRow],
    active_conv: Option<&str>,
) -> Vec<&'a ApprovalRow> {
    let Some(conv) = active_conv.filter(|conv| !conv.is_empty()) else {
        return Vec::new();
    };
    pending
        .iter()
        .filter(|row| row.conv.is_empty() || row.conv == conv)
        .collect()
}

/// The banner's one line: the single request reads its agent + task head;
/// several collapse to an honest count (pure).
pub(super) fn banner_label(rows: &[&ApprovalRow]) -> String {
    match rows {
        [] => String::new(),
        [row] => {
            let head = if row.task_head.trim().is_empty() {
                "subagent spawn"
            } else {
                row.task_head.trim()
            };
            let agent = if row.agent.is_empty() { "agent" } else { &row.agent };
            format!("Awaiting your approval — {agent}: {head}")
        }
        many => format!("{} subagents awaiting your approval", many.len()),
    }
}

impl OrchestratorPanel {
    /// The §5.2 composer banner, present only while the active conversation
    /// holds pending approvals. In flow (displaces the deck downward like
    /// the typeahead) — never an overlay, never a modal, never focused.
    pub(super) fn render_approval_banner(
        &self,
        cx: &mut gpui::Context<Self>,
    ) -> Option<AnyElement> {
        let (label, first_run): (String, Option<SharedString>) = {
            let store = self.store.read(cx);
            let active = store
                .transcript
                .as_ref()
                .map(|snapshot| snapshot.id.clone())
                .or_else(|| store.transcript_conv.clone());
            let rows = approvals_for_conv(&store.pending_approvals, active.as_deref());
            if rows.is_empty() {
                return None;
            }
            let first_run = rows
                .iter()
                .map(|row| row.run_id.clone())
                .find(|run_id| !run_id.is_empty())
                .map(SharedString::from);
            (banner_label(&rows), first_run)
        };
        let amber: gpui::Hsla = STATUS_BLOCKED.into();
        let placeholder = cx.theme().colors().text_placeholder;
        Some(
            h_flex()
                .id("composer-approval-banner")
                .mb(px(6.))
                .px(px(12.))
                .py(px(6.))
                .rounded(px(10.))
                .border_1()
                .border_color(amber.opacity(0.30))
                .bg(amber.opacity(0.06))
                .items_center()
                .gap(px(8.))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, window, cx| {
                    // Attention link: route to the board and open the first
                    // awaiting run's drawer (deferred — the panel's open_run
                    // idiom). The user's own click, never an ambient focus
                    // move.
                    if let Some(run_id) = first_run.clone() {
                        this.open_run(run_id, window, cx);
                    }
                }))
                .child(div().flex_none().size(px(7.)).rounded_full().bg(amber))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(12.5))
                        .text_color(amber)
                        .truncate()
                        .child(SharedString::from(label)),
                )
                .child(
                    div()
                        .flex_none()
                        .text_size(px(12.))
                        .text_color(placeholder)
                        .child("review →"),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, conv: &str, run: &str, agent: &str, head: &str) -> ApprovalRow {
        ApprovalRow {
            id: id.to_string(),
            conv: conv.to_string(),
            run_id: run.to_string(),
            agent: agent.to_string(),
            task_head: head.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn scopes_to_the_active_conversation() {
        let pending = vec![
            row("ap_1", "c-1", "r-1", "codex", "deploy"),
            row("ap_2", "c-2", "r-2", "claude", "other conv"),
            row("ap_3", "", "r-3", "gemini", "single-conv bridge"),
        ];
        let rows = approvals_for_conv(&pending, Some("c-1"));
        let ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
        // c-1's own row + the conv-less row (nothing to mismatch against);
        // NEVER another conversation's approval.
        assert_eq!(ids, ["ap_1", "ap_3"]);

        // No active conversation (fresh app) → no banner rows; the held
        // toast covers the global set.
        assert!(approvals_for_conv(&pending, None).is_empty());
        assert!(approvals_for_conv(&pending, Some("")).is_empty());
        // An active conv with nothing pending → empty.
        assert_eq!(approvals_for_conv(&pending, Some("c-9")).len(), 1); // ap_3 only
    }

    #[test]
    fn banner_label_reads_one_or_counts_many() {
        let single = row("ap_1", "c-1", "r-1", "codex", "deploy the thing");
        assert_eq!(
            banner_label(&[&single]),
            "Awaiting your approval — codex: deploy the thing"
        );
        // Empty task head → an honest fallback, never a dangling colon.
        let bare = row("ap_2", "c-1", "r-2", "", "  ");
        assert_eq!(
            banner_label(&[&bare]),
            "Awaiting your approval — agent: subagent spawn"
        );
        let a = row("ap_1", "c-1", "r-1", "codex", "x");
        let b = row("ap_2", "c-1", "r-2", "claude", "y");
        assert_eq!(banner_label(&[&a, &b]), "2 subagents awaiting your approval");
        assert_eq!(banner_label(&[]), "");
    }
}
