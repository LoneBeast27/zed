//! The ROUTINES actions — the buttons and the POSTs behind them (split from
//! `routines.rs`/`routines_view.rs` at the 500-line ceiling; single concern:
//! how a user acts on a routine row and how the outcome lands honestly).
//!
//! LAW: run-now / toggle / consent fire ONLY from the row buttons built here
//! — nothing calls [`VaultBrowserPanel::routine_action`] programmatically.
//! AMBER LAW rides both ends: an unconsented amber hack renders the amber
//! Consent affordance with NO Fire-now, and `routine_action` re-guards with
//! [`fire_allowed`] before any I/O (defense in depth).

use gpui::{AnyElement, AppContext as _, Context, ElementId, FontWeight, SharedString};
use ui::prelude::*;

use crate::agent_accents::STATUS_BLOCKED;
// post_json_status: `Ok((status, body))` whenever the HTTP round-trip
// completed (2xx or a refusal), `Err` only on transport failure — so a 409
// refusal renders its reason instead of masquerading as "bridge unreachable"
// (upstreamed from this file's local copy, 2026-07-10).
use crate::bridge::{BRIDGE_BASE_URL, fetch_json, post_json_status};

use super::panel::VaultBrowserPanel;
use super::routines::{
    RoutineAction, RoutineRow, RoutinesSnapshot, consent_required, fire_allowed, fire_result_note,
    refusal_note, toggle_allowed,
};

impl VaultBrowserPanel {
    /// One-shot `GET /routines` — visibility-gated (view flip, header
    /// refresh, post-action), the axioms/briefing fetch idiom, never polled.
    pub(super) fn fetch_routines(&mut self, cx: &mut Context<Self>) {
        self.routines.loading = true;
        let client = cx.http_client();
        self.routines._task = Some(cx.spawn(async move |this, cx| {
            let fetched = cx
                .background_spawn(async move {
                    let url = format!("{BRIDGE_BASE_URL}/routines");
                    let raw = fetch_json(client.as_ref(), &url).await?;
                    anyhow::Ok(serde_json::from_str::<RoutinesSnapshot>(&raw)?)
                })
                .await;
            this.update(cx, |this, cx| {
                this.routines.loading = false;
                match fetched {
                    Ok(snapshot) => {
                        this.routines.snapshot = Some(snapshot);
                        this.routines.stale = false;
                    }
                    Err(_) => this.routines.stale = true,
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// USER-gated row action — reached ONLY from the row buttons. §11: the
    /// in-flight mark is a pure state flip; the POST rides `cx.spawn` and
    /// the outcome lands via `this.update` (the `resolve_axiom` shape).
    ///
    /// AMBER LAW enforcement lives HERE too: a run-now on a row that fails
    /// [`fire_allowed`] is refused before any I/O — even if a stale render
    /// somehow offered the button, an unconsented routine never fires.
    pub(super) fn routine_action(
        &mut self,
        id: String,
        action: RoutineAction,
        cx: &mut Context<Self>,
    ) {
        let row = self
            .routines
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.routines.iter().find(|row| row.id == id));
        let Some(row) = row else {
            return; // row left the snapshot — nothing to act on
        };
        if action == RoutineAction::RunNow && !fire_allowed(row) {
            self.routines
                .notes
                .insert(id, "fire blocked — consent required".to_string());
            cx.notify();
            return;
        }
        if !self.routines.in_flight.insert(id.clone()) {
            return; // already in flight — a double-click must not double-POST
        }
        cx.notify();
        let client = cx.http_client();
        let post_id = id.clone();
        cx.spawn(async move |this, cx| {
            let outcome = cx
                .background_spawn(async move {
                    let url = format!(
                        "{BRIDGE_BASE_URL}/routines/{post_id}/{}",
                        action.endpoint()
                    );
                    post_json_status(client.as_ref(), &url, "{}".to_string()).await
                })
                .await;
            this.update(cx, |this, cx| {
                this.routines.in_flight.remove(&id);
                match outcome {
                    Ok((status, raw)) if (200..300).contains(&status) => {
                        match action {
                            // Render the bridge's own fired/skipped verdict.
                            RoutineAction::RunNow => {
                                this.routines.notes.insert(id, fire_result_note(&raw));
                            }
                            // The refetched row IS the outcome — clear stale
                            // notes so they don't read as this action's.
                            RoutineAction::Toggle | RoutineAction::Consent => {
                                this.routines.notes.remove(&id);
                            }
                        }
                        // Refetch so status/consent/last-fire reflect the
                        // server truth, not an optimistic guess.
                        this.fetch_routines(cx);
                    }
                    // Refusal (409 consent/banned, 404 gone…): the bridge is
                    // ALIVE — show its reason verbatim, no stale flag.
                    Ok((status, raw)) => {
                        this.routines.notes.insert(id, refusal_note(status, &raw));
                    }
                    // Transport failure mid-action: honest stale note.
                    Err(_) => {
                        this.routines
                            .notes
                            .insert(id, "bridge unreachable — action not delivered".to_string());
                        this.routines.stale = true;
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The action row for a VALID routine (invalid rows never reach here —
    /// the view returns before actions). AMBER LAW: an unconsented amber
    /// hack renders the amber Consent affordance and NO Fire-now — fire is
    /// blocked client-side until `POST consent` lands.
    pub(super) fn routine_actions(
        &self,
        ix: usize,
        row: &RoutineRow,
        mono: &SharedString,
        cx: &Context<Self>,
    ) -> AnyElement {
        let colors = cx.theme().colors();
        if self.routines.in_flight.contains(&row.id) {
            return div()
                .text_size(px(12.))
                .font_family(mono.clone())
                .italic()
                .text_color(colors.text_placeholder)
                .child("working…")
                .into_any_element();
        }

        let mut actions = h_flex().items_center().gap(px(6.));
        if consent_required(row) {
            let consent_id = row.id.clone();
            actions = actions
                .child(
                    div()
                        .id(ElementId::Name(format!("routine-consent-{ix}").into()))
                        .px(px(9.))
                        .py(px(4.))
                        .rounded(px(8.))
                        .bg(STATUS_BLOCKED)
                        .text_size(px(12.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(gpui::black())
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.routine_action(consent_id.clone(), RoutineAction::Consent, cx)
                        }))
                        .child("Consent"),
                )
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(STATUS_BLOCKED)
                        .child("fire blocked until consent"),
                );
        }
        if fire_allowed(row) {
            let fire_id = row.id.clone();
            actions = actions.child(
                div()
                    .id(ElementId::Name(format!("routine-fire-{ix}").into()))
                    .px(px(9.))
                    .py(px(4.))
                    .rounded(px(8.))
                    .border_1()
                    .border_color(colors.border)
                    .text_size(px(12.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(colors.text)
                    .cursor_pointer()
                    .hover(|s| s.border_color(colors.border_variant))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.routine_action(fire_id.clone(), RoutineAction::RunNow, cx)
                    }))
                    .child("Fire now"),
            );
        }
        if toggle_allowed(row) {
            let toggle_id = row.id.clone();
            let label = if row.status == "active" {
                "Disable"
            } else {
                "Enable"
            };
            actions = actions.child(
                div()
                    .id(ElementId::Name(format!("routine-toggle-{ix}").into()))
                    .px(px(9.))
                    .py(px(4.))
                    .rounded(px(8.))
                    .border_1()
                    .border_color(colors.border)
                    .text_size(px(12.))
                    .text_color(colors.text_muted)
                    .cursor_pointer()
                    .hover(|s| s.border_color(colors.border_variant))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.routine_action(toggle_id.clone(), RoutineAction::Toggle, cx)
                    }))
                    .child(label),
            );
        }
        actions.into_any_element()
    }
}
