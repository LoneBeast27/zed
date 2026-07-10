//! The IMPORT view's SESSION-STORES half (split from `importers_view.rs` to
//! hold the 500-line ceiling — single concern: the documented, NOT-wired
//! cards) + the small shared card/label/action builders both halves use.
//!
//! These four parsers (`core/importers`: claude_code / codex / vscode /
//! antigravity) are server-ready but CLI-only — the bridge exposes NO
//! session-import endpoint (documented choice, see `importers.rs`). The
//! honest surface is therefore: store hint + copyable CLI invocation, with
//! the Import actions DISABLED and the reason in the tooltip. No faked wire.

use gpui::{Context, Div, ElementId, FontWeight, SharedString};
use settings::Settings as _;
use theme_settings::ThemeSettings;
use ui::prelude::*;

use crate::agent_accents::accent_for_agent;
use crate::task_board::style::tabular_nums;

use super::importers::{IMPORT_ALL_CLI, SESSION_VENDORS, session_cli};
use super::index::VaultRoot;
use super::panel::VaultBrowserPanel;
use super::style::SURFACE_1;

const NO_ENDPOINT_TOOLTIP: &str =
    "The bridge exposes no session-import endpoint (CLI-only by design) — click the command to copy it";

impl VaultBrowserPanel {
    /// The "Session stores → staging" section: the four documented vendor
    /// cards, the copyable `import run` lines, the staged-count line and the
    /// "View staged →" handoff into the staging LIST.
    pub(super) fn session_stores_section(&self, cx: &Context<Self>) -> Div {
        let colors = cx.theme().colors();
        let staged = self
            .index()
            .map(|index| index.docs_for(VaultRoot::Staging).len());
        let mut section = v_flex().w_full().gap(px(8.));

        section = section.child(
            h_flex()
                .items_center()
                .gap(px(8.))
                .child(section_label("Session stores → staging", cx))
                .child(div().flex_1())
                .child(disabled_action("import-run-all", "Import all", cx)),
        );
        section = section.child(
            div()
                .text_size(px(11.))
                .text_color(colors.text_placeholder)
                .child("Local vendor session stores stage OKF bundles for review — parsers are server-ready, triggered by the `import` CLI (no bridge endpoint yet)."),
        );
        section = section.child(self.cli_row("all", IMPORT_ALL_CLI.to_string(), cx));

        for vendor in &SESSION_VENDORS {
            section = section.child(self.session_vendor_card(
                vendor.vendor,
                vendor.label,
                vendor.store_hint,
                cx,
            ));
        }

        // The staging handoff: honest count from the fs index + the jump the
        // staging empty-state hint points back at.
        let staged_note = match staged {
            Some(n) => format!("{n} staged docs"),
            None => "staging index building…".to_string(),
        };
        section.child(
            h_flex()
                .items_center()
                .gap(px(8.))
                .child(
                    div()
                        .text_size(px(11.))
                        .font_features(tabular_nums())
                        .text_color(colors.text_muted)
                        .child(SharedString::from(staged_note)),
                )
                .child(link_action(
                    "import-view-staged",
                    "View staged →",
                    cx.listener(|this, _, _, cx| {
                        this.jump_to_root_list(VaultRoot::Staging, cx)
                    }),
                    cx,
                )),
        )
    }

    fn session_vendor_card(
        &self,
        vendor: &'static str,
        label: &'static str,
        store_hint: &'static str,
        cx: &Context<Self>,
    ) -> Div {
        let colors = cx.theme().colors();
        let mono = ThemeSettings::get_global(cx).buffer_font.family.clone();
        card(cx)
            .child(
                h_flex()
                    .items_center()
                    .gap(px(6.))
                    .child(div().size(px(6.)).rounded_full().bg(accent_for_agent(vendor)))
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(colors.text)
                            .child(label),
                    )
                    .child(div().flex_1())
                    .child(disabled_action(
                        ElementId::Name(format!("import-session-{vendor}").into()),
                        "Import",
                        cx,
                    )),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .font_family(mono)
                    .text_color(colors.text_placeholder)
                    .child(store_hint),
            )
            .child(self.cli_row(vendor, session_cli(vendor), cx))
    }

    /// A copyable CLI invocation line (the honest trigger where no endpoint
    /// exists). Click copies; the "copied" chip confirms.
    fn cli_row(&self, key: &'static str, cli: String, cx: &Context<Self>) -> Div {
        let colors = cx.theme().colors();
        let mono = ThemeSettings::get_global(cx).buffer_font.family.clone();
        let copied = self.importers.copied_cli.as_deref() == Some(key);
        let display = cli.clone();
        h_flex()
            .items_center()
            .gap(px(6.))
            .child(
                div()
                    .id(ElementId::Name(format!("import-cli-{key}").into()))
                    .px(px(8.))
                    .py(px(3.))
                    .rounded(px(6.))
                    .bg(SURFACE_1)
                    .text_size(px(11.))
                    .font_family(mono)
                    .text_color(colors.text_muted)
                    .cursor_pointer()
                    .hover(|s| s.text_color(colors.text))
                    .tooltip(ui::Tooltip::text("Copy the CLI command"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.copy_import_cli(key.to_string(), cli.clone(), cx)
                    }))
                    .child(SharedString::from(display)),
            )
            .when(copied, |row| {
                row.child(
                    div()
                        .text_size(px(11.))
                        .text_color(colors.text_placeholder)
                        .child("copied"),
                )
            })
    }
}

// ── shared card / label / action builders (both IMPORT halves) ──

pub(super) fn card(cx: &App) -> Div {
    let colors = cx.theme().colors();
    v_flex()
        .w_full()
        .gap(px(6.))
        .p(px(10.))
        .rounded(px(10.))
        .bg(SURFACE_1)
        .border_1()
        .border_color(colors.border)
}

pub(super) fn section_label(text: &'static str, cx: &App) -> Div {
    div()
        .text_size(px(12.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(cx.theme().colors().text_muted)
        .child(text)
}

/// A bordered action that is honestly DISABLED (no endpoint) — the tooltip
/// carries the reason; the cursor never lies.
pub(super) fn disabled_action(
    id: impl Into<ElementId>,
    label: &'static str,
    cx: &App,
) -> impl IntoElement {
    let colors = cx.theme().colors();
    div()
        .id(id)
        .px(px(9.))
        .py(px(3.))
        .rounded(px(8.))
        .border_1()
        .border_color(colors.border)
        .text_size(px(12.))
        .text_color(colors.text_placeholder)
        .tooltip(ui::Tooltip::text(NO_ENDPOINT_TOOLTIP))
        .child(label)
}

/// A small text-link action (cancel / jump affordances).
pub(super) fn link_action(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    listener: impl Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
    cx: &App,
) -> impl IntoElement {
    let colors = cx.theme().colors();
    div()
        .id(id)
        .text_size(px(11.))
        .text_color(colors.text_muted)
        .cursor_pointer()
        .hover(|s| s.text_color(colors.text))
        .on_click(listener)
        .child(label.into())
}
