//! The WRITEBACK view — a bridge-fed vault-browser body (the same hosting as
//! Axioms/Routines): pending run→note candidates + import conflicts from the
//! bridge (`GET /writeback`) with USER-gated Accept / Dismiss. The CONFLICT
//! MODAL lives in `writeback_modal.rs`; the pinned wire contract in
//! `writeback.rs`; the enforced conflict law in `writeback_state.rs`.
//!
//! Offline = the shared bridge state (`BridgeStore::connected`): actions
//! disable with an honest note; the last-known snapshot keeps rendering.

use gpui::{AnyElement, Context, Div, ElementId, FontWeight, Hsla, SharedString, Stateful};
use settings::Settings as _;
use theme_settings::ThemeSettings;
use ui::prelude::*;

use crate::agent_accents::{ACCENT_FILL, STATUS_BLOCKED, STATUS_ERROR, accent_for_agent};

use super::panel::VaultBrowserPanel;
use super::style::{SURFACE_1, ago_now, empty_state, type_chip};
use super::writeback::{Candidate, CandidateKind};

impl VaultBrowserPanel {
    /// The WRITEBACK body. Bridge-fed (not vault fs), so the panel renders
    /// it before the index gate — same hosting as the axioms view.
    pub(super) fn writeback_view(&self, connected: bool, cx: &Context<Self>) -> AnyElement {
        let Some(snapshot) = self.writeback.snapshot.clone() else {
            return if self.writeback.stale || !connected {
                empty_state(
                    IconName::XCircle,
                    "Write-back unavailable",
                    "Could not reach the bridge at 127.0.0.1:4530 — candidates will appear when it is back.",
                    cx,
                )
                .into_any_element()
            } else {
                empty_state(
                    IconName::ArrowCircle,
                    "Loading write-back…",
                    "Fetching pending candidates from the bridge.",
                    cx,
                )
                .into_any_element()
            };
        };

        let colors = cx.theme().colors();
        let mut column = v_flex().w_full().gap(px(10.));

        if !connected {
            column = column.child(
                div()
                    .text_size(px(11.))
                    .text_color(STATUS_BLOCKED)
                    .child("bridge offline — showing last-known candidates; actions return with it"),
            );
        } else if self.writeback.stale {
            column = column.child(
                div()
                    .text_size(px(11.))
                    .text_color(STATUS_BLOCKED)
                    .child("bridge unreachable — showing last-known write-back"),
            );
        }

        if snapshot.candidates.is_empty() {
            column = column.child(div().w_full().h(px(220.)).child(empty_state(
                IconName::Check,
                "No pending write-back",
                "Candidates appear here when a completed run proposes a note update, \
                 or an import conflicts with a hand edit.",
                cx,
            )));
        } else {
            let counts = if snapshot.conflicts > 0 {
                format!(
                    "{} pending · {} conflict{}",
                    snapshot.pending,
                    snapshot.conflicts,
                    if snapshot.conflicts == 1 { "" } else { "s" }
                )
            } else {
                format!("{} pending", snapshot.pending)
            };
            column = column.child(
                div()
                    .text_size(px(11.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(colors.text_placeholder)
                    .child(SharedString::from(counts)),
            );
            for (ix, candidate) in snapshot.candidates.iter().enumerate() {
                column = column.child(self.writeback_row(ix, candidate, connected, cx));
            }
        }

        div()
            .size_full()
            .relative()
            .child(
                div()
                    .id("vault-writeback")
                    .size_full()
                    .overflow_y_scroll()
                    .p(px(12.))
                    .child(column),
            )
            .children(
                self.writeback
                    .modal
                    .as_ref()
                    .map(|modal| self.conflict_modal(modal, connected, cx)),
            )
            .into_any_element()
    }

    /// One candidate card: kind chip · title · age, per-kind meta (source
    /// run chip / target note), the preview snippet the wire carries, and
    /// the actions (run: Accept/Dismiss; conflict: Review…/Dismiss).
    fn writeback_row(
        &self,
        ix: usize,
        candidate: &Candidate,
        connected: bool,
        cx: &Context<Self>,
    ) -> Div {
        let colors = cx.theme().colors();
        let mono = ThemeSettings::get_global(cx).buffer_font.family.clone();
        let in_flight = self.writeback.in_flight.contains(&candidate.id);

        // ── header: kind chip · title · age ──
        let mut head = h_flex().items_center().gap(px(8.));
        head = match &candidate.kind {
            CandidateKind::Conflict { .. } => head.child(conflict_chip(cx)),
            CandidateKind::Run { .. } => head.children(type_chip("run", cx)),
            CandidateKind::Unknown(kind) => head.children(type_chip(kind, cx)),
        };
        head = head.child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(px(13.))
                .text_color(colors.text)
                .truncate()
                .child(SharedString::from(candidate.title.clone())),
        );
        if let Some(ts) = candidate.ts {
            head = head.child(
                div()
                    .flex_none()
                    .text_size(px(11.))
                    .text_color(colors.text_placeholder)
                    .child(SharedString::from(ago_now(ts))),
            );
        }

        let mut card = v_flex()
            .w_full()
            .gap(px(8.))
            .p(px(10.))
            .rounded(px(10.))
            .bg(SURFACE_1)
            .border_1()
            .border_color(colors.border)
            .child(head);

        // ── per-kind meta + preview ──
        match &candidate.kind {
            CandidateKind::Run {
                run_id,
                agent,
                via,
                preview,
            } => {
                let mut meta = h_flex().items_center().flex_wrap().gap(px(6.));
                if !agent.is_empty() || !run_id.is_empty() {
                    meta = meta.child(run_chip(agent, run_id, cx));
                }
                if !via.is_empty() {
                    meta = meta.children(type_chip(via, cx));
                }
                meta = meta.child(
                    div()
                        .text_size(px(11.))
                        .text_color(colors.text_placeholder)
                        .child(SharedString::from(format!(
                            "→ new note · {}",
                            candidate.project.as_deref().unwrap_or("unfiled")
                        ))),
                );
                card = card.child(meta);
                if !preview.is_empty() {
                    card = card.child(
                        div()
                            .w_full()
                            .p(px(8.))
                            .rounded(px(8.))
                            .bg(SURFACE_1)
                            .text_size(px(11.))
                            .font_family(mono.clone())
                            .text_color(colors.text_muted)
                            .child(SharedString::from(preview.clone())),
                    );
                }
            }
            CandidateKind::Conflict { path, reason, .. } => {
                card = card
                    .child(
                        div()
                            .w_full()
                            .text_size(px(11.))
                            .font_family(mono.clone())
                            .text_color(colors.text_placeholder)
                            .truncate()
                            .child(SharedString::from(format!("→ {path}"))),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(colors.text_muted)
                            .child(SharedString::from(reason.clone())),
                    );
            }
            CandidateKind::Unknown(kind) => {
                card = card.child(
                    div()
                        .text_size(px(11.))
                        .text_color(colors.text_placeholder)
                        .child(SharedString::from(format!(
                            "unknown candidate kind \"{kind}\" — this build can only dismiss it"
                        ))),
                );
            }
        }

        // ── actions ──
        let actions: AnyElement = if in_flight {
            saving_note(&mono, cx).into_any_element()
        } else {
            let mut row = h_flex().items_center().gap(px(6.));
            match &candidate.kind {
                CandidateKind::Run { .. } => {
                    let accept_id = candidate.id.clone();
                    row = row.child(wb_button(
                        ElementId::Name(format!("wb-accept-{ix}").into()),
                        "Accept".into(),
                        true,
                        connected,
                        cx,
                        move |this, cx| this.accept_run_candidate(accept_id.clone(), cx),
                    ));
                }
                CandidateKind::Conflict { .. } => {
                    // The ONLY door to a conflict resolution — opens the
                    // modal (local fs reads, so it works offline too).
                    let open = candidate.clone();
                    row = row.child(wb_button(
                        ElementId::Name(format!("wb-review-{ix}").into()),
                        "Review conflict…".into(),
                        true,
                        true,
                        cx,
                        move |this, cx| this.open_conflict_modal(&open, cx),
                    ));
                }
                CandidateKind::Unknown(_) => {}
            }
            let dismiss_id = candidate.id.clone();
            row = row.child(wb_button(
                ElementId::Name(format!("wb-dismiss-{ix}").into()),
                "Dismiss".into(),
                false,
                connected,
                cx,
                move |this, cx| this.dismiss_candidate(dismiss_id.clone(), cx),
            ));
            row.into_any_element()
        };
        card = card.child(h_flex().items_center().child(div().flex_1()).child(actions));

        if let Some(error) = self.writeback.row_errors.get(&candidate.id) {
            card = card.child(
                div()
                    .text_size(px(11.))
                    .text_color(STATUS_ERROR)
                    .child(SharedString::from(error.clone())),
            );
        }
        card
    }

    // The conflict modal itself lives in `writeback_modal.rs` (500-line
    // ceiling split — single concern: the modal).
}

/// The "conflict" chip — the amber (blocked-tone) sibling of `type_chip`.
fn conflict_chip(cx: &Context<VaultBrowserPanel>) -> Div {
    let _ = cx;
    let amber: Hsla = STATUS_BLOCKED.into();
    div()
        .flex_none()
        .px(px(7.))
        .py(px(2.))
        .rounded(px(6.))
        .bg(amber.opacity(0.12))
        .text_size(px(11.))
        .text_color(amber)
        .child("conflict")
}

/// The source-run chip: vendor-accent dot + `agent · run-id` (mono) — the
/// agent-chip idiom carrying the run identity alongside.
fn run_chip(agent: &str, run_id: &str, cx: &Context<VaultBrowserPanel>) -> Div {
    let colors = cx.theme().colors();
    let mono = ThemeSettings::get_global(cx).buffer_font.family.clone();
    let label = match (agent.is_empty(), run_id.is_empty()) {
        (false, false) => format!("{agent} · {run_id}"),
        (false, true) => agent.to_string(),
        _ => run_id.to_string(),
    };
    h_flex()
        .flex_none()
        .items_center()
        .gap(px(5.))
        .px(px(7.))
        .py(px(2.))
        .rounded(px(6.))
        .bg(SURFACE_1)
        .text_size(px(11.))
        .font_family(mono)
        .text_color(colors.text_muted)
        .child(div().size(px(6.)).rounded_full().bg(accent_for_agent(agent)))
        .child(SharedString::from(label))
}

/// The in-flight "saving…" note (buttons collapse so a double-click can't
/// double-POST — the axioms idiom). Shared with `writeback_modal.rs`.
pub(super) fn saving_note(mono: &SharedString, cx: &Context<VaultBrowserPanel>) -> Div {
    let colors = cx.theme().colors();
    div()
        .text_size(px(12.))
        .font_family(mono.clone())
        .italic()
        .text_color(colors.text_placeholder)
        .child("saving…")
}

/// A writeback action button: accent-filled (primary) or hairline-bordered;
/// a disabled one renders the honest inert state (no handler, no pointer).
/// Shared with `writeback_modal.rs`.
pub(super) fn wb_button(
    id: ElementId,
    label: SharedString,
    primary: bool,
    enabled: bool,
    cx: &Context<VaultBrowserPanel>,
    on_click: impl Fn(&mut VaultBrowserPanel, &mut Context<VaultBrowserPanel>) + 'static,
) -> Stateful<Div> {
    let colors = cx.theme().colors();
    let base = div()
        .id(id)
        .px(px(9.))
        .py(px(4.))
        .rounded(px(8.))
        .text_size(px(12.))
        .font_weight(FontWeight::MEDIUM);
    if !enabled {
        return base
            .border_1()
            .border_color(colors.border)
            .text_color(colors.text_placeholder)
            .child(label);
    }
    let styled = if primary {
        base.bg(ACCENT_FILL).text_color(gpui::white())
    } else {
        base.border_1()
            .border_color(colors.border)
            .text_color(colors.text_muted)
            .hover(|s| s.border_color(colors.border_variant))
    };
    styled
        .cursor_pointer()
        .on_click(cx.listener(move |this, _, _, cx| on_click(this, cx)))
        .child(label)
}
