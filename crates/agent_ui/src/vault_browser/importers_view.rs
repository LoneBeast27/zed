//! The IMPORT view — the vault browser's importers body: the trigger +
//! progress surface the staging empty-state hint points at. Two honest
//! sections mirroring the discovered contract (`importers.rs` header):
//!
//! - SESSION STORES → STAGING: the four `core/importers` parsers as
//!   documented cards — rendered by `importers_view_session.rs` (split for
//!   the 500-line ceiling): store hint + copyable CLI, Import actions
//!   DISABLED with the truth in the tooltip (the bridge exposes no
//!   session-import endpoint). "View staged →" hands off to the staging
//!   LIST the browser already renders.
//! - VENDOR EXPORTS → KNOWLEDGE (this file): the live bridge surface —
//!   per-vendor cards (last-import summary from the sidecar), a
//!   path+project compose that POSTs the import, and the watched job's
//!   live progress row (stage + rolling counts), result line (counts /
//!   error VERBATIM), cancel, and a "View in vault →" jump once done.

use gpui::{AnyElement, Context, Div, ElementId, FontWeight, SharedString};
use settings::Settings as _;
use theme_settings::ThemeSettings;
use ui::prelude::*;

use crate::agent_accents::{ACCENT_FILL, STATUS_BLOCKED, STATUS_ERROR, accent_for_agent};
use crate::task_board::style::{rel, status_pill, tabular_nums};

use super::importers::{
    ImportJob, VendorStatus, board_status, elapsed_s, last_import_line, progress_line,
    record_error_line, result_line,
};
use super::importers_view_session::{card, link_action, section_label};
use super::index::VaultRoot;
use super::panel::VaultBrowserPanel;
use super::style::{SURFACE_1, empty_state};

fn now_unix() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

impl VaultBrowserPanel {
    /// The IMPORT body. Bridge-fed (plus the fs index for the staged count),
    /// so the panel renders it before the index gate.
    pub(super) fn importers_view(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let connected = crate::bridge::global_store(cx).read(cx).connected;
        let colors = cx.theme().colors();
        let mut column = v_flex().w_full().gap(px(10.));

        if self.importers.stale {
            column = column.child(
                div()
                    .text_size(px(11.))
                    .text_color(STATUS_BLOCKED)
                    .child("bridge unreachable — showing last-known importer status"),
            );
        }

        column = column.child(self.session_stores_section(cx));
        column = column.child(
            div()
                .w_full()
                .mt(px(4.))
                .border_t_1()
                .border_color(colors.border),
        );
        column = column.child(self.export_section(connected, cx));

        div()
            .id("vault-importers")
            .size_full()
            .overflow_y_scroll()
            .p(px(12.))
            .child(column)
            .into_any_element()
    }

    // ── vendor exports → knowledge (the live wire) ──

    fn export_section(&mut self, connected: bool, cx: &mut Context<Self>) -> Div {
        // Copy the one color used after the `&mut cx` card builders — the
        // `colors` borrow must end before them (E0502, the panel idiom).
        let text_placeholder = cx.theme().colors().text_placeholder;
        let mut section = v_flex().w_full().gap(px(8.));
        section = section.child(section_label("Vendor exports → knowledge", cx));

        let Some(snapshot) = self.importers.snapshot.clone() else {
            let (icon, headline, copy) = if self.importers.loading {
                (
                    IconName::ArrowCircle,
                    "Loading importer status…",
                    "Fetching vendor import status from the bridge.",
                )
            } else {
                (
                    IconName::XCircle,
                    "Export importers unavailable",
                    "Could not reach the bridge at 127.0.0.1:4530 — session-store CLI imports above still work.",
                )
            };
            return section.child(div().w_full().h(px(180.)).child(empty_state(icon, headline, copy, cx)));
        };

        section = section.child(
            div()
                .text_size(px(11.))
                .text_color(text_placeholder)
                .child("Official export files import into projects/<project>/knowledge/imported/ via the bridge."),
        );
        for (ix, vendor) in snapshot.vendors.iter().enumerate() {
            section = section.child(self.export_vendor_card(ix, vendor, connected, cx));
        }
        if let Some(job) = self.importers.job.clone() {
            section = section.child(self.job_card(&job, cx));
        }
        if !snapshot.scraper_seam.is_empty() {
            section = section.child(
                div()
                    .text_size(px(10.))
                    .italic()
                    .text_color(text_placeholder)
                    .child(SharedString::from(snapshot.scraper_seam.clone())),
            );
        }
        section
    }

    fn export_vendor_card(
        &self,
        ix: usize,
        vendor: &VendorStatus,
        connected: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let colors = cx.theme().colors();
        let mono = ThemeSettings::get_global(cx).buffer_font.family.clone();
        let composing = self.importers.composing.as_deref() == Some(vendor.vendor.as_str());
        let running = vendor.running_job.is_some();

        let last_line = match &vendor.last_import {
            Some(last) => last_import_line(last, now_unix()),
            None => "never imported".to_string(),
        };

        let action: AnyElement = if running {
            div()
                .text_size(px(11.))
                .text_color(crate::agent_accents::STATUS_RUNNING)
                .child("importing…")
                .into_any_element()
        } else if connected {
            let open_vendor = vendor.vendor.clone();
            div()
                .id(ElementId::Name(format!("import-open-{ix}").into()))
                .px(px(9.))
                .py(px(3.))
                .rounded(px(8.))
                .border_1()
                .border_color(colors.border)
                .text_size(px(12.))
                .text_color(colors.text_muted)
                .cursor_pointer()
                .hover(|s| s.border_color(colors.border_variant).text_color(colors.text))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.toggle_import_compose(open_vendor.clone(), window, cx)
                }))
                .child(if composing { "Close" } else { "Import…" })
                .into_any_element()
        } else {
            div()
                .id(ElementId::Name(format!("import-open-{ix}").into()))
                .px(px(9.))
                .py(px(3.))
                .rounded(px(8.))
                .border_1()
                .border_color(colors.border)
                .text_size(px(12.))
                .text_color(colors.text_placeholder)
                .tooltip(ui::Tooltip::text("Bridge offline — export imports run on the bridge"))
                .child("Import…")
                .into_any_element()
        };

        let mut body = card(cx)
            .child(
                h_flex()
                    .items_center()
                    .gap(px(6.))
                    .child(div().size(px(6.)).rounded_full().bg(accent_for_agent(&vendor.vendor)))
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(colors.text)
                            .child(SharedString::from(vendor.label.clone())),
                    )
                    .child(div().flex_1())
                    .child(action),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .font_family(mono)
                    .font_features(tabular_nums())
                    .text_color(colors.text_placeholder)
                    .child(SharedString::from(last_line)),
            );
        if composing {
            body = body.child(self.import_compose_row(vendor.vendor.clone(), connected, cx));
        }
        body
    }

    /// The compose row: project chips · export path box · Start / inline
    /// error (`POST /importers/<vendor>/import` needs `{path, project}`).
    fn import_compose_row(&self, vendor: String, connected: bool, cx: &mut Context<Self>) -> Div {
        let colors = cx.theme().colors();
        let Some(editor) = self.importers.path_editor.clone() else {
            return div();
        };
        let picked = self.picked_import_project();
        let path_empty = editor.read(cx).text(cx).trim().is_empty();

        let mut chips = h_flex().items_center().flex_wrap().gap(px(4.));
        for slug in self.project_slugs() {
            let on = slug == picked;
            let pick = slug.clone();
            chips = chips.child(
                div()
                    .id(ElementId::Name(format!("import-project-{slug}").into()))
                    .px(px(8.))
                    .py(px(2.))
                    .rounded(px(6.))
                    .text_size(px(11.))
                    .cursor_pointer()
                    .when(on, |s| s.bg(SURFACE_1).text_color(colors.text))
                    .when(!on, |s| s.text_color(colors.text_placeholder))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.set_import_project(pick.clone(), cx)
                    }))
                    .child(SharedString::from(slug)),
            );
        }

        let start_enabled = connected && !path_empty && !self.importers.start_in_flight;
        let start_label = if self.importers.start_in_flight { "Starting…" } else { "Start" };
        let mut row = v_flex()
            .w_full()
            .gap(px(6.))
            .child(chips)
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        div()
                            .flex_1()
                            .px(px(8.))
                            .py(px(4.))
                            .rounded(px(8.))
                            .bg(SURFACE_1)
                            .child(editor),
                    )
                    .child(
                        div()
                            .id("import-start")
                            .px(px(9.))
                            .py(px(4.))
                            .rounded(px(8.))
                            .text_size(px(12.))
                            .font_weight(FontWeight::MEDIUM)
                            .when(start_enabled, |s| {
                                s.bg(ACCENT_FILL).text_color(gpui::white()).cursor_pointer()
                            })
                            .when(!start_enabled, |s| {
                                s.border_1()
                                    .border_color(colors.border)
                                    .text_color(colors.text_placeholder)
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.start_export_import(vendor.clone(), cx)
                            }))
                            .child(start_label),
                    ),
            );
        if let Some(error) = &self.importers.error {
            row = row.child(
                div()
                    .text_size(px(11.))
                    .text_color(STATUS_ERROR)
                    .child(SharedString::from(error.clone())),
            );
        }
        row
    }

    /// The watched job's row: label · status pill · elapsed, the live
    /// stage/counts progress, per-record errors + the result line VERBATIM,
    /// cancel while running, "View in vault →" once done.
    fn job_card(&self, job: &ImportJob, cx: &mut Context<Self>) -> Div {
        let colors = cx.theme().colors();
        let mono = ThemeSettings::get_global(cx).buffer_font.family.clone();
        let running = job.status == "running";
        let mut body = card(cx).child(
            h_flex()
                .items_center()
                .gap(px(8.))
                .child(
                    div()
                        .flex_1()
                        .text_size(px(12.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(colors.text)
                        .child(SharedString::from(job.label.clone())),
                )
                .child(status_pill("import-job-pill", board_status(&job.status), None, cx))
                .child(
                    div()
                        .text_size(px(11.))
                        .font_features(tabular_nums())
                        .text_color(colors.text_placeholder)
                        .child(SharedString::from(rel(elapsed_s(job, now_unix())))),
                ),
        );
        if running {
            body = body.child(
                div()
                    .text_size(px(11.))
                    .font_family(mono.clone())
                    .font_features(tabular_nums())
                    .text_color(colors.text_muted)
                    .child(SharedString::from(progress_line(job))),
            );
        }
        for record_error in job.errors.iter().take(3) {
            body = body.child(
                div()
                    .text_size(px(11.))
                    .font_family(mono.clone())
                    .text_color(STATUS_ERROR)
                    .child(SharedString::from(record_error_line(record_error))),
            );
        }
        if let Some(result) = result_line(job) {
            let tone = if job.status == "error" { STATUS_ERROR.into() } else { colors.text_muted };
            body = body.child(
                div()
                    .text_size(px(11.))
                    .font_family(mono)
                    .font_features(tabular_nums())
                    .text_color(tone)
                    .child(SharedString::from(result)),
            );
        }
        let mut actions = h_flex().items_center().gap(px(8.));
        if running {
            let cancel_label = if self.importers.cancel_in_flight { "Cancelling…" } else { "Cancel" };
            actions = actions.child(link_action(
                "import-job-cancel",
                cancel_label,
                cx.listener(|this, _, _, cx| this.cancel_import_job(cx)),
                cx,
            ));
        } else if job.status == "done" {
            actions = actions.child(link_action(
                "import-view-vault",
                "View in vault →",
                cx.listener(|this, _, _, cx| this.jump_to_root_list(VaultRoot::Vault, cx)),
                cx,
            ));
        }
        body.child(actions)
    }
}
