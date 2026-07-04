//! The LIST view — the task-board list idiom (`uniform_list`, virtualized)
//! over OKF documents grouped by kind (sessions by vendor, notes by type,
//! runs). Each row: title · type chip · vendor chip · updated-age ·
//! redactions count (sessions) · promote action (staged sessions). Click →
//! open the md in a center editor tab (deferred by the panel per §11).
//!
//! Groups are flattened into a single row vec (section-header rows + doc rows)
//! so one `uniform_list` virtualizes the whole thing — rows stay fixed-height,
//! render cost is bounded by the viewport (RUST_PORT_NOTES §8 / principle 4).

use std::sync::Arc;

use gpui::{AnyElement, ElementId, FontWeight, SharedString, WeakEntity, uniform_list};
use ui::prelude::*;

use super::index::{DocKind, VaultDoc, VaultRoot, group_by_kind};
use super::panel::VaultBrowserPanel;
use super::promote::PromoteState;
use super::style::{redactions_badge, type_chip, updated_age, vendor_chip};

/// Fixed doc-row height (title + meta sub-line, matching the inbox 72px feel
/// but tighter for a dense file list).
const ROW_HEIGHT: f32 = 56.;
/// Fixed section-header row height.
const HEADER_HEIGHT: f32 = 30.;

/// A flattened list row: a group header or a document.
#[derive(Clone)]
enum Row {
    Header { label: SharedString, count: usize },
    Doc(Arc<VaultDoc>),
}

/// Build the virtualized document list for `root`, filtered by `filter`
/// (lowercased substring over the precomputed search key). `promote` reflects
/// per-bundle arm/promoted state for the staged-session row action.
pub fn list_view(
    docs: &[&VaultDoc],
    root: VaultRoot,
    filter: &str,
    promote: &PromoteState,
    reveal: Option<&str>,
    panel: WeakEntity<VaultBrowserPanel>,
    cx: &App,
) -> AnyElement {
    let filter = filter.trim().to_lowercase();
    let filtered: Vec<&VaultDoc> = docs
        .iter()
        .copied()
        .filter(|d| filter.is_empty() || d.search_key.contains(&filter))
        .collect();

    if filtered.is_empty() {
        let (headline, copy) = if filter.is_empty() {
            ("No documents", "This root has no OKF notes to list.")
        } else {
            ("No matches", "No documents match the filter.")
        };
        return super::style::empty_state(IconName::Filter, headline, copy, cx).into_any_element();
    }

    // Flatten grouped docs into header + doc rows (stable section order).
    let groups = group_by_kind(&filtered);
    let mut rows: Vec<Row> = Vec::with_capacity(filtered.len() + groups.len());
    for (kind, group_docs) in &groups {
        rows.push(Row::Header {
            label: kind.group_label().into(),
            count: group_docs.len(),
        });
        for doc in group_docs {
            rows.push(Row::Doc(Arc::new((*doc).clone())));
        }
    }
    let rows = Arc::new(rows);
    let promote = promote.clone();
    let reveal: Option<SharedString> = reveal.map(SharedString::from);

    uniform_list(
        "vault-list",
        rows.len(),
        move |range, _window, cx| {
            range
                .map(|ix| match &rows[ix] {
                    Row::Header { label, count } => {
                        header_row(label.clone(), *count, cx).into_any_element()
                    }
                    Row::Doc(doc) => {
                        let revealed = reveal.as_deref() == Some(doc.id.as_str());
                        doc_row(doc, root, &promote, revealed, panel.clone(), cx)
                    }
                })
                .collect()
        },
    )
    .size_full()
    .px(px(12.))
    .pt(px(4.))
    .pb(px(20.))
    .into_any_element()
}

/// A section header (`DocKind::group_label` · count), muted uppercase-ish.
fn header_row(label: SharedString, count: usize, cx: &mut App) -> Div {
    let colors = cx.theme().colors();
    h_flex()
        .h(px(HEADER_HEIGHT))
        .w_full()
        .items_center()
        .gap(px(6.))
        .pt(px(10.))
        .child(
            div()
                .text_size(px(11.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(colors.text_placeholder)
                .child(label),
        )
        .child(
            div()
                .text_size(px(11.))
                .text_color(colors.text_placeholder)
                .child(SharedString::from(format!("({count})"))),
        )
}

/// One document row: title + meta line (type/vendor chips, age, redactions),
/// plus a promote affordance for staged sessions.
fn doc_row(
    doc: &VaultDoc,
    root: VaultRoot,
    promote: &PromoteState,
    revealed: bool,
    panel: WeakEntity<VaultBrowserPanel>,
    cx: &mut App,
) -> AnyElement {
    let colors = cx.theme().colors();
    let hover_bg = colors.element_hover;
    let title: SharedString = doc.title.clone().into();
    let age = updated_age(&doc.updated);
    let abs_path = doc.abs_path.clone();

    let mut meta = h_flex()
        .items_center()
        .gap(px(8.))
        .text_size(px(13.))
        .text_color(colors.text_muted);
    meta = meta.children(type_chip(&doc.doc_type, cx));
    meta = meta.children(vendor_chip(&doc.vendor, cx));
    if doc.kind == DocKind::Session {
        meta = meta.children(redactions_badge(doc.redactions, cx));
    }
    if !age.is_empty() {
        meta = meta.child(
            div()
                .text_color(colors.text_placeholder)
                .child(SharedString::from(age)),
        );
    }
    // Link-scan truncation is an honest per-node flag (spec §5): note it.
    if doc.link_scan_truncated {
        meta = meta.child(
            div()
                .text_color(colors.text_placeholder)
                .child(SharedString::from("· links partial")),
        );
    }

    let open_path = abs_path;
    let open_panel = panel.clone();
    let mut row = h_flex()
        .id(ElementId::Name(SharedString::from(doc.id.clone())))
        .w_full()
        .h(px(ROW_HEIGHT))
        .items_center()
        .gap(px(12.))
        .px(px(10.))
        .border_b_1()
        .border_color(colors.border)
        .cursor_pointer()
        // A reveal-from-graph highlights the row once (a soft accent wash).
        .when(revealed, |r| r.bg(colors.element_selected))
        .hover(move |s| s.bg(hover_bg))
        .on_click(move |_, window, cx| {
            let path = open_path.clone();
            open_panel
                .update(cx, |panel, cx| panel.request_open(path, window, cx))
                .ok();
        })
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap(px(4.))
                .child(
                    div()
                        .text_size(px(14.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(colors.text)
                        .truncate()
                        .child(title),
                )
                .child(meta),
        );

    // "Show in graph" affordance (§3): a small icon button that flips to the
    // graph view (the node is already present in the field).
    let graph_panel = panel.clone();
    row = row.child(
        div()
            .id(ElementId::Name(format!("show-in-graph-{}", doc.id).into()))
            .flex_none()
            .p(px(4.))
            .rounded(px(6.))
            .cursor_pointer()
            .text_color(colors.text_placeholder)
            .hover(|s| s.text_color(colors.text_muted))
            .on_click(move |_, _, cx| {
                graph_panel
                    .update(cx, |panel, cx| panel.show_in_graph(cx))
                    .ok();
            })
            .child(
                Icon::new(IconName::GitBranch)
                    .size(ui::IconSize::XSmall)
                    .color(Color::Custom(colors.text_placeholder)),
            ),
    );

    // Staged sessions get the promote affordance on the right rail.
    let row = if root == VaultRoot::Staging && doc.bundle_dir.is_some() {
        row.child(promote_affordance(doc, promote, panel, cx))
    } else {
        row
    };

    row.into_any_element()
}

/// The two-click arm/confirm promote affordance (no modal — the spec's inline
/// confirm). Rest = "Promote"; armed = "Confirm" (accent) + a dismiss;
/// promoted = a done chip; in-flight = "Promoting…".
fn promote_affordance(
    doc: &VaultDoc,
    promote: &PromoteState,
    panel: WeakEntity<VaultBrowserPanel>,
    cx: &App,
) -> AnyElement {
    use super::promote::BundleState;
    let colors = cx.theme().colors();
    let Some(bundle_dir) = doc.bundle_dir.clone() else {
        return div().into_any_element();
    };
    let key = doc.id.clone();

    match promote.state_for(&key) {
        BundleState::Promoted { collided } => {
            let label = if collided { "Promoted (renamed)" } else { "Promoted" };
            div()
                .flex_none()
                .px(px(9.))
                .py(px(4.))
                .rounded(px(8.))
                .text_size(px(12.))
                .text_color(crate::agent_accents::STATUS_DONE)
                .child(label)
                .into_any_element()
        }
        BundleState::InFlight => div()
            .flex_none()
            .px(px(9.))
            .py(px(4.))
            .text_size(px(12.))
            .text_color(colors.text_muted)
            .child("Promoting…")
            .into_any_element(),
        BundleState::Failed(msg) => div()
            .flex_none()
            .px(px(9.))
            .py(px(4.))
            .text_size(px(12.))
            .text_color(crate::agent_accents::STATUS_ERROR)
            .child(SharedString::from(format!("Failed: {msg}")))
            .into_any_element(),
        BundleState::Armed => {
            let confirm_key = key.clone();
            let confirm_dir = bundle_dir;
            let confirm_panel = panel.clone();
            let cancel_key = key.clone();
            let cancel_panel = panel;
            h_flex()
                .flex_none()
                .items_center()
                .gap(px(6.))
                .child(
                    div()
                        .id(ElementId::Name(format!("promote-confirm-{key}").into()))
                        .px(px(9.))
                        .py(px(4.))
                        .rounded(px(8.))
                        .bg(crate::agent_accents::ACCENT_FILL)
                        .text_size(px(12.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(gpui::white())
                        .cursor_pointer()
                        .on_click(move |_, _, cx| {
                            let (k, d) = (confirm_key.clone(), confirm_dir.clone());
                            confirm_panel
                                .update(cx, |panel, cx| panel.confirm_promote(k, d, cx))
                                .ok();
                        })
                        .child("Confirm"),
                )
                .child(
                    div()
                        .id(ElementId::Name(format!("promote-cancel-{key}").into()))
                        .px(px(6.))
                        .py(px(4.))
                        .text_size(px(12.))
                        .text_color(colors.text_placeholder)
                        .cursor_pointer()
                        .on_click(move |_, _, cx| {
                            let k = cancel_key.clone();
                            cancel_panel
                                .update(cx, |panel, cx| panel.cancel_promote(k, cx))
                                .ok();
                        })
                        .child("Cancel"),
                )
                .into_any_element()
        }
        BundleState::Rest => {
            let arm_key = key.clone();
            div()
                .id(ElementId::Name(format!("promote-arm-{key}").into()))
                .flex_none()
                .px(px(9.))
                .py(px(4.))
                .rounded(px(8.))
                .border_1()
                .border_color(colors.border)
                .text_size(px(12.))
                .text_color(colors.text_muted)
                .cursor_pointer()
                .hover(|s| s.border_color(colors.border_variant))
                .on_click(move |_, _, cx| {
                    let k = arm_key.clone();
                    panel.update(cx, |panel, cx| panel.arm_promote(k, cx)).ok();
                })
                .child("Promote")
                .into_any_element()
        }
    }
}
