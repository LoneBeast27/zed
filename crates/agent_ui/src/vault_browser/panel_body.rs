//! The body dispatch (a `VaultBrowserPanel` impl split out of `panel.rs` to
//! hold the 500-line ceiling — single concern: WHICH body renders). The
//! bridge-fed views (Axioms / Routines / Writeback / Import) render before
//! the index gate — an unindexed vault must not blank a bridge review
//! surface; List/Graph run the fs index with honest empty states (spec §6).

use gpui::Context;
use ui::prelude::*;

use super::index::{VaultIndex, VaultRoot};
use super::panel::{VaultBrowserPanel, VaultView};

impl VaultBrowserPanel {
    /// Build the body element (returns the element, whether a frame pump is
    /// needed, and whether the graph is the active surface — for drag wiring).
    /// The indexing placeholder shows until the first walk lands.
    pub(super) fn render_body_element(
        &mut self,
        cx: &mut Context<Self>,
    ) -> (gpui::AnyElement, bool, bool) {
        // Bridge-fed views render regardless of the index state.
        if self.view() == VaultView::Axioms {
            return (self.axioms_view(cx), false, false);
        }
        if self.view() == VaultView::Routines {
            return (self.routines_view(cx), false, false);
        }
        if self.view() == VaultView::Writeback {
            // The shared offline state gates the accept/dismiss/resolve
            // actions (read BEFORE borrowing the theme, header idiom).
            let connected = crate::bridge::global_store(cx).read(cx).connected;
            return (self.writeback_view(connected, cx), false, false);
        }
        if self.view() == VaultView::Import {
            return (self.importers_view(cx), false, false);
        }
        let editor = self.filter_editor();
        let filter = editor.read(cx).text(cx);
        // Take the index out to sever the `&self.index` borrow across the
        // `&mut self` body-building call, then put it back (a cheap swap).
        let Some(index) = self.index.take() else {
            let el = super::style::empty_state(
                IconName::Sparkle,
                "Indexing the vault…",
                "Walking the live vault and import staging.",
                cx,
            )
            .into_any_element();
            return (el, false, false);
        };
        let out = self.render_body(&index, &filter, cx);
        self.index = Some(index);
        out
    }

    /// The list/graph body for the active root, plus the honest empty states
    /// when a root's directory is missing (spec §6). Returns `(element,
    /// animating, graph_active)`.
    fn render_body(
        &mut self,
        index: &VaultIndex,
        filter: &str,
        cx: &mut Context<Self>,
    ) -> (gpui::AnyElement, bool, bool) {
        let present = match self.root() {
            VaultRoot::Vault => index.vault_present,
            VaultRoot::Staging => index.staging_present,
        };
        if !present {
            let (headline, copy) = match self.root() {
                VaultRoot::Vault => (
                    "Vault not found",
                    "Looked for the live vault at L:\\Projects\\atlas-vault.",
                ),
                VaultRoot::Staging => (
                    "No staged sessions",
                    "Looked in vault-import-staging — stage sessions from the Import view (`import run`).",
                ),
            };
            let el = super::style::empty_state(IconName::FolderOpen, headline, copy, cx)
                .into_any_element();
            return (el, false, false);
        }

        let docs = index.docs_for(self.root());
        // Staging with zero docs → the import hint (spec §6) — the Import
        // view is the surface that hint points at.
        if docs.is_empty() && self.root() == VaultRoot::Staging {
            let el = super::style::empty_state(
                IconName::Envelope,
                "No staged sessions",
                "Stage sessions from the Import view (`import run` per vendor) — they land here for review.",
                cx,
            )
            .into_any_element();
            return (el, false, false);
        }

        match self.view() {
            // Handled before the index gate in `render_body_element` (bridge
            // data, not vault fs) — kept here for match exhaustiveness.
            VaultView::Axioms => (self.axioms_view(cx), false, false),
            VaultView::Routines => (self.routines_view(cx), false, false),
            VaultView::Writeback => {
                let connected = crate::bridge::global_store(cx).read(cx).connected;
                (self.writeback_view(connected, cx), false, false)
            }
            VaultView::Import => (self.importers_view(cx), false, false),
            VaultView::List => {
                let reveal = self.take_graph_reveal();
                let weak = cx.weak_entity();
                let el = super::list::list_view(
                    &docs,
                    self.root(),
                    filter,
                    &self.collapsed,
                    &self.promote,
                    reveal.as_deref(),
                    weak,
                    cx,
                );
                (el, false, false)
            }
            VaultView::Graph => {
                // Fold: (re)build the field when the doc set (identity) changed
                // — a promote re-indexes and the fold picks it up with no jump
                // (§3); pins + live displacement carry across. Below the field
                // ceiling only (the static fallback needs no field).
                let now = self.field_now();
                let degraded = docs.len() > super::field::FIELD_MAX_NODES;
                let animating = if degraded {
                    false
                } else {
                    if !self.graph_field.matches(&docs) {
                        self.graph_field
                            .rebuild(&docs, super::graph::node_size, now);
                    }
                    self.graph_field.advance(now)
                };
                // Fit-to-viewport base scale × the user's zoom (user ask
                // 2026-07-10: the field is bounded by the window at zoom 1,
                // ctrl+wheel / caption-clicks dive into clusters). The
                // measured viewport lags one frame (ScrollHandle bounds —
                // the constellation width idiom); 0 on the very first frame
                // falls back to unscaled.
                let bounds = self.graph_scroll.bounds().size;
                let (vw, vh) = (bounds.width.as_f32(), bounds.height.as_f32());
                let fit_s = if vw > 0. && vh > 0. {
                    (vw / self.graph_field.width.max(1.))
                        .min(vh / self.graph_field.height.max(1.))
                        .min(1.)
                } else {
                    1.
                };
                let scale = fit_s * self.graph_zoom;
                self.graph_scale = scale;
                let hovered = self.graph_hovered.clone();
                let weak = cx.weak_entity();
                let el = super::graph::graph_view(
                    &self.graph_field,
                    &docs,
                    &self.graph_scroll,
                    hovered.as_deref(),
                    self.edges(),
                    super::graph_render::GraphNav {
                        scale,
                        zoomed: self.graph_zoom > 1.001,
                        viewport: (vw, vh),
                        filter: filter.to_string(),
                    },
                    weak,
                    cx,
                );
                (el, animating, true)
            }
        }
    }
}
