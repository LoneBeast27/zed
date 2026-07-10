//! The vault-browser panel header (a `VaultBrowserPanel` impl split out to
//! keep `panel.rs` under the 500-line ceiling — single concern: the chrome
//! row). Title · build note · "+ new note" · refresh · root switcher
//! (Vault ⇄ Staging) · List/Graph/Axioms toggle · filter box · the inline
//! new-note compose row. The board seg-toggle idiom (hairline, flat, no
//! boxes — PARITY_SPEC §4.2).
//!
//! The old Kind/Recent sort toggle is GONE (vault-UX pass 2026-07-10): the
//! LIST is a folder tree with a permanent Recent section on top, so the
//! toggle had nothing left to say.

use gpui::{Context, ElementId, FontWeight, Hsla, SharedString};
use ui::prelude::*;

use super::index::VaultRoot;
use super::panel::{EdgeMode, VaultBrowserPanel, VaultView};
use super::style::SURFACE_1;

/// A segment target — root / view / graph-edge-filter (so one `segmented`
/// builder handles every header control).
#[derive(Clone, Copy)]
enum RootOrView {
    Root(VaultRoot),
    View(VaultView),
    Edges(EdgeMode),
}

impl VaultBrowserPanel {
    /// The header: title · root switcher (Vault ⇄ Staging) · view toggle ·
    /// new-note · refresh, then the filter box + compose rows.
    pub(super) fn render_header(&self, cx: &mut Context<Self>) -> gpui::Div {
        // The bridge writes notes; with it down the + disables honestly (the
        // tree itself is fs-fed and keeps rendering regardless). Read BEFORE
        // borrowing the theme (global_store wants the &mut App).
        let connected = crate::bridge::global_store(cx).read(cx).connected;
        let colors = cx.theme().colors();
        let build_note = self.index().map(|index| {
            let n = index.docs_for(self.root()).len();
            let ms = index.build_ms;
            SharedString::from(if self.is_indexing() {
                "indexing…".to_string()
            } else {
                format!("{n} docs · {ms}ms")
            })
        });

        v_flex()
            .flex_none()
            .gap(px(8.))
            .px(px(12.))
            .pt(px(12.))
            .pb(px(10.))
            .border_b_1()
            .border_color(colors.border)
            .child(
                h_flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .text_size(px(14.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(colors.text)
                            .child("Vault"),
                    )
                    .children(build_note.map(|note| {
                        div()
                            .text_size(px(11.))
                            .text_color(colors.text_placeholder)
                            .child(note)
                    }))
                    .child(div().flex_1())
                    .child(
                        ui::IconButton::new("vault-new-note", IconName::Plus)
                            .icon_size(ui::IconSize::Small)
                            .disabled(!connected)
                            .tooltip(ui::Tooltip::text(if connected {
                                "New note"
                            } else {
                                "Bridge offline — note creation unavailable"
                            }))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.toggle_compose(window, cx)
                            })),
                    )
                    .child(
                        ui::IconButton::new("vault-refresh", IconName::RotateCw)
                            .icon_size(ui::IconSize::Small)
                            .tooltip(ui::Tooltip::text("Re-index the vault"))
                            .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
                    ),
            )
            .child(
                // WRAPPING row (sweep find 2026-07-08: multiple segmented
                // controls overflowed the dock width and clipped the Graph
                // segment to a sliver) — at narrow dock widths the trailing
                // controls flow onto the next line instead of vanishing.
                h_flex()
                    .items_center()
                    .flex_wrap()
                    .gap(px(8.))
                    .child(self.render_root_switcher(cx))
                    // Contextual control: the GRAPH view filters edges; LIST
                    // (the folder tree) and AXIOMS carry none.
                    .child(match self.view() {
                        VaultView::Graph => self.render_edges_toggle(cx),
                        VaultView::List | VaultView::Axioms => div(),
                    })
                    .child(self.render_view_toggle(cx)),
            )
            // The filter box searches the INDEXED VAULT (title/tag/path) — it
            // does not apply to the bridge axioms list, so it hides there
            // rather than sit dead (honest chrome).
            .when(self.view() != VaultView::Axioms, |this| {
                this.child(self.render_filter(cx))
            })
            // The inline "+ new note" compose row (new_note.rs).
            .children(self.render_new_note(connected, cx))
    }

    /// GRAPH edge filter: everything ⇄ semantic links only ⇄ supersedes only.
    fn render_edges_toggle(&self, cx: &Context<Self>) -> gpui::Div {
        let edges = self.edges();
        self.segmented(
            "edges",
            &[
                ("All", edges == EdgeMode::All, RootOrView::Edges(EdgeMode::All)),
                (
                    "Links",
                    edges == EdgeMode::Links,
                    RootOrView::Edges(EdgeMode::Links),
                ),
                (
                    "Chain",
                    edges == EdgeMode::Supersedes,
                    RootOrView::Edges(EdgeMode::Supersedes),
                ),
            ],
            cx,
        )
    }

    /// The two-root switcher segmented control (Vault ⇄ Staging).
    fn render_root_switcher(&self, cx: &mut Context<Self>) -> gpui::Div {
        let root = self.root();
        self.segmented(
            "root",
            &[
                ("Vault", root == VaultRoot::Vault, RootOrView::Root(VaultRoot::Vault)),
                (
                    "Staging",
                    root == VaultRoot::Staging,
                    RootOrView::Root(VaultRoot::Staging),
                ),
            ],
            cx,
        )
    }

    /// The List/Graph/Axioms view toggle (mirrors the board seg-toggle).
    fn render_view_toggle(&self, cx: &mut Context<Self>) -> gpui::Div {
        let view = self.view();
        self.segmented(
            "view",
            &[
                ("List", view == VaultView::List, RootOrView::View(VaultView::List)),
                ("Graph", view == VaultView::Graph, RootOrView::View(VaultView::Graph)),
                (
                    "Axioms",
                    view == VaultView::Axioms,
                    RootOrView::View(VaultView::Axioms),
                ),
            ],
            cx,
        )
    }

    /// Shared flat segmented-control builder (hairline, no boxes — board idiom).
    fn segmented(
        &self,
        id: &'static str,
        segments: &[(&'static str, bool, RootOrView)],
        cx: &Context<Self>,
    ) -> gpui::Div {
        let colors = cx.theme().colors();
        let on_bg: Hsla = SURFACE_1.into();
        let mut row = h_flex()
            .flex_none()
            .border_1()
            .border_color(colors.border)
            .rounded(px(8.))
            .overflow_hidden();
        for (label, on, target) in segments.iter().copied() {
            let seg = div()
                .id(ElementId::Name(format!("vseg-{id}-{label}").into()))
                .px(px(10.))
                .py(px(4.))
                .text_size(px(12.))
                .font_weight(FontWeight::MEDIUM)
                .cursor_pointer()
                .when(on, |s| s.bg(on_bg).text_color(colors.text))
                .when(!on, |s| s.text_color(colors.text_placeholder))
                .on_click(cx.listener(move |this, _, _, cx| match target {
                    RootOrView::Root(r) => this.set_root(r, cx),
                    RootOrView::View(v) => this.set_view(v, cx),
                    RootOrView::Edges(e) => this.set_edges(e, cx),
                }))
                .child(label);
            row = row.child(seg);
        }
        row
    }

    /// The filter box — the single-line editor on a surface-1 fill. Filters
    /// across EVERY tree section live (title, tags, path segments, run id).
    fn render_filter(&self, cx: &Context<Self>) -> gpui::Div {
        let colors = cx.theme().colors();
        h_flex()
            .w_full()
            .items_center()
            .gap(px(6.))
            .px(px(8.))
            .py(px(4.))
            .rounded(px(8.))
            .bg(SURFACE_1)
            .child(
                Icon::new(IconName::MagnifyingGlass)
                    .size(ui::IconSize::Small)
                    .color(Color::Custom(colors.text_placeholder)),
            )
            .child(div().flex_1().child(self.filter_editor()))
    }
}
