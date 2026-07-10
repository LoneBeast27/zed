//! The AXIOMS view — the vault browser's third body (List / Graph / Axioms):
//! pending axiom candidates from the bridge (`GET /axioms`) with USER-gated
//! Approve/Reject actions (`POST /axioms/approve` / `/axioms/reject`, body
//! `{"axiom": "<text>"}`), over the page ledger (approved/rejected counts +
//! body preview).
//!
//! Wire shape pinned to the live bridge (probed 2026-07-10):
//! `{pending: [{axiom, recurrence?}, ...], page: {body, approved, rejected} | null}`.
//!
//! Fetches are visibility-gated one-shots (view flip / header refresh /
//! post-action — the briefing-panel idiom, never polled). Degrade law: a
//! failed fetch keeps the last-known snapshot under an honest stale note; a
//! bridge that was never reachable reads "Axioms unavailable" — the view
//! never blanks silently.
//!
//! LAW: approve/reject fire ONLY from the row buttons — nothing in this
//! module (or anywhere else) calls [`VaultBrowserPanel::resolve_axiom`]
//! programmatically.

use std::collections::HashSet;

use gpui::{
    AnyElement, AppContext as _, Context, Div, ElementId, FontWeight, SharedString, Task,
};
use serde::Deserialize;
use settings::Settings as _;
use theme_settings::ThemeSettings;
use ui::prelude::*;

use crate::agent_accents::{ACCENT_FILL, STATUS_BLOCKED};
use crate::bridge::{BRIDGE_BASE_URL, fetch_json, post_json};

use super::panel::VaultBrowserPanel;
use super::style::{SURFACE_1, empty_state};

/// One pending candidate. `recurrence` stays a raw JSON value (count-shaped
/// on the wire per spec, but unobserved live — the pending list was empty at
/// probe time), so a numeric OR string recurrence renders instead of failing
/// the whole deserialize.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct PendingAxiom {
    #[serde(default)]
    pub axiom: String,
    #[serde(default)]
    pub recurrence: Option<serde_json::Value>,
}

/// The axioms page ledger (`page` — null until the first resolution lands).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct AxiomPage {
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub approved: u64,
    #[serde(default)]
    pub rejected: u64,
}

/// The full `GET /axioms` snapshot.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct AxiomsSnapshot {
    #[serde(default)]
    pub pending: Vec<PendingAxiom>,
    #[serde(default)]
    pub page: Option<AxiomPage>,
}

/// Axioms-view state riding the panel: fetch lifecycle + per-axiom action
/// in-flight marks. The last good snapshot survives a failed refetch.
#[derive(Default)]
pub struct AxiomsState {
    /// `None` until the first fetch lands (loading / unavailable states).
    snapshot: Option<AxiomsSnapshot>,
    /// The LAST fetch or POST attempt failed — snapshot (if any) is stale.
    stale: bool,
    /// A fetch is in flight (first open shows "Loading…", not a blank).
    loading: bool,
    /// Axiom texts with an approve/reject POST in flight (buttons collapse
    /// to "saving…" so a double-click can't double-POST).
    in_flight: HashSet<String>,
    /// The in-flight fetch (kept so it isn't dropped/cancelled).
    _task: Option<Task<()>>,
}

impl VaultBrowserPanel {
    /// One-shot `GET /axioms` — visibility-gated (view flip, header refresh,
    /// post-action), the briefing-panel fetch idiom. A failed fetch keeps the
    /// last-known snapshot and flips the stale flag.
    pub(super) fn fetch_axioms(&mut self, cx: &mut Context<Self>) {
        self.axioms.loading = true;
        let client = cx.http_client();
        self.axioms._task = Some(cx.spawn(async move |this, cx| {
            let fetched = cx
                .background_spawn(async move {
                    let url = format!("{BRIDGE_BASE_URL}/axioms");
                    let raw = fetch_json(client.as_ref(), &url).await?;
                    anyhow::Ok(serde_json::from_str::<AxiomsSnapshot>(&raw)?)
                })
                .await;
            this.update(cx, |this, cx| {
                this.axioms.loading = false;
                match fetched {
                    Ok(snapshot) => {
                        this.axioms.snapshot = Some(snapshot);
                        this.axioms.stale = false;
                    }
                    Err(_) => this.axioms.stale = true,
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// USER-gated approve/reject — reached ONLY from the row buttons (LAW:
    /// never called programmatically). §11: the in-flight mark is a pure
    /// state flip (immediate feedback, synchronous is correct); the POST
    /// rides `cx.spawn` (schedules, does not re-enter this update) and the
    /// entity lands the outcome via `this.update` in the async continuation
    /// — the same shape as `confirm_promote`.
    pub(super) fn resolve_axiom(&mut self, axiom: String, approve: bool, cx: &mut Context<Self>) {
        if !self.axioms.in_flight.insert(axiom.clone()) {
            return; // already in flight — a double-click must not double-POST
        }
        cx.notify();
        let client = cx.http_client();
        let post_axiom = axiom.clone();
        cx.spawn(async move |this, cx| {
            let outcome = cx
                .background_spawn(async move {
                    let endpoint = if approve { "approve" } else { "reject" };
                    let url = format!("{BRIDGE_BASE_URL}/axioms/{endpoint}");
                    let body = serde_json::json!({ "axiom": post_axiom }).to_string();
                    post_json(client.as_ref(), &url, body).await
                })
                .await;
            this.update(cx, |this, cx| {
                this.axioms.in_flight.remove(&axiom);
                match outcome {
                    // Resolved server-side — refetch so the row leaves pending
                    // and the ledger counts advance.
                    Ok(_) => this.fetch_axioms(cx),
                    // POST failed (bridge down mid-click): the row STAYS
                    // pending and the stale note surfaces it — never a
                    // silent drop.
                    Err(_) => this.axioms.stale = true,
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The AXIOMS body. Independent of the fs index (bridge data, not vault
    /// files), so the panel renders it before the index gate.
    pub(super) fn axioms_view(&self, cx: &Context<Self>) -> AnyElement {
        let Some(snapshot) = self.axioms.snapshot.clone() else {
            return if self.axioms.stale {
                empty_state(
                    IconName::XCircle,
                    "Axioms unavailable",
                    "Could not reach the bridge at 127.0.0.1:4530 — candidates will appear when it is back.",
                    cx,
                )
                .into_any_element()
            } else {
                empty_state(
                    IconName::ArrowCircle,
                    "Loading axioms…",
                    "Fetching pending candidates from the bridge.",
                    cx,
                )
                .into_any_element()
            };
        };

        let colors = cx.theme().colors();
        let mut column = v_flex().w_full().gap(px(10.));

        if self.axioms.stale {
            column = column.child(
                div()
                    .text_size(px(11.))
                    .text_color(STATUS_BLOCKED)
                    .child("bridge unreachable — showing last-known axioms"),
            );
        }

        if snapshot.pending.is_empty() {
            column = column.child(
                div().w_full().h(px(220.)).child(empty_state(
                    IconName::Check,
                    "No pending axioms",
                    "Candidates appear here when the distiller stages axioms for review.",
                    cx,
                )),
            );
        } else {
            column = column.child(
                div()
                    .text_size(px(11.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(colors.text_placeholder)
                    .child(SharedString::from(format!(
                        "{} pending",
                        snapshot.pending.len()
                    ))),
            );
            for (ix, pending) in snapshot.pending.iter().enumerate() {
                column = column.child(self.pending_axiom_row(ix, pending, cx));
            }
        }

        column = column.child(render_page_ledger(snapshot.page.as_ref(), cx));

        div()
            .id("vault-axioms")
            .size_full()
            .overflow_y_scroll()
            .p(px(12.))
            .child(column)
            .into_any_element()
    }

    /// One pending candidate card: the axiom text + optional recurrence chip
    /// over the Approve / Reject buttons (list.rs promote-button idiom).
    fn pending_axiom_row(&self, ix: usize, pending: &PendingAxiom, cx: &Context<Self>) -> Div {
        let colors = cx.theme().colors();
        let mono = ThemeSettings::get_global(cx).buffer_font.family.clone();
        let in_flight = self.axioms.in_flight.contains(&pending.axiom);

        let actions: AnyElement = if in_flight {
            div()
                .text_size(px(12.))
                .font_family(mono.clone())
                .italic()
                .text_color(colors.text_placeholder)
                .child("saving…")
                .into_any_element()
        } else {
            let approve_axiom = pending.axiom.clone();
            let reject_axiom = pending.axiom.clone();
            h_flex()
                .items_center()
                .gap(px(6.))
                .child(
                    div()
                        .id(ElementId::Name(format!("axiom-approve-{ix}").into()))
                        .px(px(9.))
                        .py(px(4.))
                        .rounded(px(8.))
                        .bg(ACCENT_FILL)
                        .text_size(px(12.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(gpui::white())
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.resolve_axiom(approve_axiom.clone(), true, cx)
                        }))
                        .child("Approve"),
                )
                .child(
                    div()
                        .id(ElementId::Name(format!("axiom-reject-{ix}").into()))
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
                            this.resolve_axiom(reject_axiom.clone(), false, cx)
                        }))
                        .child("Reject"),
                )
                .into_any_element()
        };

        v_flex()
            .w_full()
            .gap(px(8.))
            .p(px(10.))
            .rounded(px(10.))
            .bg(SURFACE_1)
            .border_1()
            .border_color(colors.border)
            .child(
                div()
                    .w_full()
                    .text_size(px(13.))
                    .text_color(colors.text)
                    .child(SharedString::from(pending.axiom.clone())),
            )
            .child(
                h_flex()
                    .items_center()
                    .gap(px(8.))
                    .children(pending.recurrence.as_ref().and_then(recurrence_label).map(
                        |label| {
                            div()
                                .flex_none()
                                .px(px(7.))
                                .py(px(2.))
                                .rounded(px(6.))
                                .bg(SURFACE_1)
                                .text_size(px(11.))
                                .font_family(mono.clone())
                                .text_color(colors.text_muted)
                                .child(SharedString::from(label))
                        },
                    ))
                    .child(div().flex_1())
                    .child(actions),
            )
    }
}

/// The recurrence chip label ("×3") — number or string recurrences render,
/// anything else (including null) shows no chip.
fn recurrence_label(recurrence: &serde_json::Value) -> Option<String> {
    match recurrence {
        serde_json::Value::Number(n) => Some(format!("×{n}")),
        serde_json::Value::String(s) if !s.is_empty() => Some(format!("×{s}")),
        _ => None,
    }
}

/// The page ledger under the pending list: approved/rejected counts + a body
/// preview. A null page reads the honest "no axioms page yet".
fn render_page_ledger(page: Option<&AxiomPage>, cx: &gpui::App) -> Div {
    let colors = cx.theme().colors();
    let mono = ThemeSettings::get_global(cx).buffer_font.family.clone();
    let mut block = v_flex()
        .w_full()
        .mt(px(4.))
        .pt(px(10.))
        .border_t_1()
        .border_color(colors.border)
        .gap(px(6.))
        .child(
            div()
                .text_size(px(12.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(colors.text_muted)
                .child("Page ledger"),
        );
    match page {
        Some(page) => {
            block = block.child(
                div()
                    .text_size(px(11.))
                    .font_family(mono.clone())
                    .text_color(colors.text_placeholder)
                    .child(SharedString::from(format!(
                        "{} approved · {} rejected",
                        page.approved, page.rejected
                    ))),
            );
            if !page.body.is_empty() {
                block = block.child(
                    div()
                        .w_full()
                        .p(px(8.))
                        .rounded(px(8.))
                        .bg(SURFACE_1)
                        .text_size(px(11.))
                        .font_family(mono)
                        .text_color(colors.text_placeholder)
                        .child(SharedString::from(body_preview(&page.body))),
                );
            }
        }
        None => {
            block = block.child(
                div()
                    .text_size(px(11.))
                    .font_family(mono)
                    .italic()
                    .text_color(colors.text_placeholder)
                    .child("no axioms page yet"),
            );
        }
    }
    block
}

/// First ~6 lines / 360 chars of the page body — a preview, not the page.
fn body_preview(body: &str) -> String {
    let mut preview: String = body.lines().take(6).collect::<Vec<_>>().join("\n");
    if preview.chars().count() > 360 {
        preview = preview.chars().take(360).collect();
    }
    if preview.len() < body.trim_end().len() {
        preview.push('…');
    }
    preview
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_live_empty_shape() {
        // Verbatim live response (probed 2026-07-10).
        let snapshot: AxiomsSnapshot = serde_json::from_str(r#"{"pending": [], "page": null}"#)
            .unwrap();
        assert!(snapshot.pending.is_empty());
        assert!(snapshot.page.is_none());
    }

    #[test]
    fn parses_the_populated_shape() {
        // `r##` — the page body starts with a markdown heading, so the JSON
        // contains the `"#` sequence that would close a plain `r#` string.
        let snapshot: AxiomsSnapshot = serde_json::from_str(
            r##"{"pending": [{"axiom": "prefer usable context", "recurrence": 3},
                             {"axiom": "no recurrence key"}],
                 "page": {"body": "# Axioms\n- one", "approved": 4, "rejected": 1}}"##,
        )
        .unwrap();
        assert_eq!(snapshot.pending.len(), 2);
        assert_eq!(snapshot.pending[0].axiom, "prefer usable context");
        assert_eq!(
            snapshot.pending[0].recurrence.as_ref().and_then(recurrence_label),
            Some("×3".to_string())
        );
        assert_eq!(snapshot.pending[1].recurrence.as_ref().and_then(recurrence_label), None);
        let page = snapshot.page.unwrap();
        assert_eq!(page.approved, 4);
        assert_eq!(page.rejected, 1);
    }

    #[test]
    fn body_preview_truncates() {
        let long = (0..10).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
        let preview = body_preview(&long);
        assert!(preview.ends_with('…'));
        assert_eq!(preview.lines().count(), 6);
        assert_eq!(body_preview("short"), "short");
    }
}
