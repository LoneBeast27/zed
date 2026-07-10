//! One LIST-view document row + the staged-session promote affordance (split
//! from `list.rs` to hold the 500-line ceiling — list.rs owns the tree
//! flattening/virtualization, this file owns how a single row looks).
//!
//! Row anatomy (vault-UX pass 2026-07-10, "human titles first"): the HUMAN
//! title leads; the machine-y bits (run id, status, schedule, age) sit small
//! underneath as secondary meta. Click = open the RENDERED reading view;
//! the pencil = raw source; the branch = show in graph.

use gpui::{AnyElement, ElementId, FontWeight, SharedString, WeakEntity};
use ui::prelude::*;

use super::index::{DocKind, VaultDoc, VaultRoot};
use super::panel::VaultBrowserPanel;
use super::promote::PromoteState;
use super::style::{redactions_badge, type_chip, updated_age, vendor_chip};

/// Fixed row height for EVERY row — headers included. `uniform_list`
/// measures one item and gives all items that slot, so mixed heights
/// (the old 30px header / 56px doc split) silently overlap: every doc
/// overflowed its short slot and painted over the next section (dogfood
/// 2026-07-07, the vault dock's overdrawn headers). Headers bottom-anchor
/// their label inside the uniform slot, which reads as section spacing.
pub(super) const ROW_HEIGHT: f32 = 56.;

/// Indent per tree depth (the Obsidian folder inset).
pub(super) const INDENT: f32 = 14.;

/// One document row: human title + per-kind meta line, plus the right-rail
/// affordances (edit source · show in graph · promote for staged sessions).
pub(super) fn doc_row(
    doc: &VaultDoc,
    depth: usize,
    root: VaultRoot,
    promote: &PromoteState,
    revealed: bool,
    panel: WeakEntity<VaultBrowserPanel>,
    cx: &mut App,
) -> AnyElement {
    let colors = cx.theme().colors();
    let hover_bg = colors.element_hover;
    let icon_hover = colors.text_muted;
    let icon_rest = colors.text_placeholder;
    let title: SharedString = doc.title.clone().into();

    let open_path = doc.abs_path.clone();
    let open_panel = panel.clone();
    let mut row = h_flex()
        .id(ElementId::Name(SharedString::from(doc.id.clone())))
        .w_full()
        .h(px(ROW_HEIGHT))
        .items_center()
        .gap(px(10.))
        .pl(px(10. + depth as f32 * INDENT))
        .pr(px(10.))
        .border_b_1()
        .border_color(colors.border)
        .cursor_pointer()
        // A reveal-from-graph highlights the row once (a soft accent wash).
        .when(revealed, |r| r.bg(colors.element_selected))
        .hover(move |s| s.bg(hover_bg))
        // Click → the RENDERED reading view (note-app default). §11: the
        // handler is a state flip + spawn inside the panel; the layout
        // mutation (pane add) happens in the async continuation.
        .on_click(move |_, window, cx| {
            let path = open_path.clone();
            open_panel
                .update(cx, |panel, cx| panel.request_open_preview(path, window, cx))
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
                .child(meta_line(doc, cx)),
        );

    // Edit affordance: the small pencil opens RAW source (the reading view is
    // the default; source is one click away, like Obsidian's edit toggle).
    let edit_panel = panel.clone();
    let edit_path = doc.abs_path.clone();
    row = row.child(
        div()
            .id(ElementId::Name(format!("edit-source-{}", doc.id).into()))
            .flex_none()
            .p(px(4.))
            .rounded(px(6.))
            .cursor_pointer()
            .hover(move |s| s.text_color(icon_hover))
            .on_click(move |_, window, cx| {
                let path = edit_path.clone();
                edit_panel
                    .update(cx, |panel, cx| panel.request_open(path, window, cx))
                    .ok();
            })
            .child(
                Icon::new(IconName::Pencil)
                    .size(ui::IconSize::XSmall)
                    .color(Color::Custom(icon_rest)),
            ),
    );

    // "Show in graph" affordance (§3): flips to the graph view (the node is
    // already present in the field).
    let graph_panel = panel.clone();
    row = row.child(
        div()
            .id(ElementId::Name(format!("show-in-graph-{}", doc.id).into()))
            .flex_none()
            .p(px(4.))
            .rounded(px(6.))
            .cursor_pointer()
            .hover(move |s| s.text_color(icon_hover))
            .on_click(move |_, _, cx| {
                graph_panel
                    .update(cx, |panel, cx| panel.show_in_graph(cx))
                    .ok();
            })
            .child(
                Icon::new(IconName::GitBranch)
                    .size(ui::IconSize::XSmall)
                    .color(Color::Custom(icon_rest)),
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

/// The per-kind secondary meta line — the machine detail, small and muted:
/// runs → run-id · status · age; routines → schedule · disabled-flag · age;
/// sessions → vendor · redactions · age; notes → type chip · age.
fn meta_line(doc: &VaultDoc, cx: &App) -> Div {
    let colors = cx.theme().colors();
    let age = updated_age(&doc.updated);
    let mut meta = h_flex()
        .items_center()
        .gap(px(8.))
        .text_size(px(11.))
        .text_color(colors.text_muted);

    match doc.kind {
        DocKind::Run => {
            if !doc.run_id.is_empty() {
                meta = meta.child(
                    div()
                        .text_color(colors.text_placeholder)
                        .child(SharedString::from(doc.run_id.clone())),
                );
            }
            if !doc.status.is_empty() {
                let status_color: gpui::Hsla = match doc.status.as_str() {
                    "succeeded" => crate::agent_accents::STATUS_DONE.into(),
                    "failed" | "error" => crate::agent_accents::STATUS_ERROR.into(),
                    _ => colors.text_muted,
                };
                meta = meta.child(
                    div()
                        .text_color(status_color)
                        .child(SharedString::from(doc.status.clone())),
                );
            }
        }
        DocKind::Routine => {
            // Title + next-fire; the "hack" chip prominence is gone.
            if !doc.schedule.is_empty() {
                meta = meta
                    .child(
                        Icon::new(IconName::Clock)
                            .size(ui::IconSize::XSmall)
                            .color(Color::Custom(colors.text_placeholder)),
                    )
                    .child(SharedString::from(doc.schedule.clone()));
            }
            if doc.status == "disabled" {
                meta = meta.child(
                    div()
                        .text_color(crate::agent_accents::STATUS_BLOCKED)
                        .child("disabled"),
                );
            }
        }
        DocKind::Session => {
            meta = meta.children(vendor_chip(&doc.vendor, cx));
            meta = meta.children(redactions_badge(doc.redactions, cx));
        }
        _ => {
            meta = meta.children(type_chip(&doc.doc_type, cx));
        }
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
    meta
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
