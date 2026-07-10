//! The LIST view — an Obsidian-style folder tree (`uniform_list`, virtualized)
//! over OKF documents: a Recent section on top, then the vault's real
//! hierarchy as collapsible sections with counts (see [`super::tree`]).
//! Machine dirs never reach the tree (the walker skips them). Row click →
//! the RENDERED reading view; the pencil → raw source (see [`super::row`]).
//!
//! Everything is flattened into a single row vec (header rows + doc rows) so
//! one `uniform_list` virtualizes the whole tree — rows stay fixed-height,
//! render cost is bounded by the viewport (RUST_PORT_NOTES §8 / principle 4).

use std::collections::HashSet;
use std::sync::Arc;

use gpui::{AnyElement, ElementId, FontWeight, SharedString, WeakEntity, uniform_list};
use ui::prelude::*;

use super::index::{VaultDoc, VaultRoot};
use super::panel::VaultBrowserPanel;
use super::promote::PromoteState;
use super::row::{INDENT, ROW_HEIGHT, doc_row};
use super::tree::{Row, flatten_rows};

/// Build the virtualized document tree for `root`, filtered by `filter`
/// (lowercased substring over the precomputed search key — title, tags, path
/// segments, run id — so the one box filters across EVERY section live).
/// `collapsed` is the panel's session-local folder state; a live filter
/// auto-expands and hides Recent (matches show in place).
pub fn list_view(
    docs: &[&VaultDoc],
    root: VaultRoot,
    filter: &str,
    collapsed: &HashSet<SharedString>,
    promote: &PromoteState,
    reveal: Option<&str>,
    panel: WeakEntity<VaultBrowserPanel>,
    cx: &App,
) -> AnyElement {
    let filter = filter.trim().to_lowercase();
    let filtering = !filter.is_empty();
    let filtered: Vec<&VaultDoc> = docs
        .iter()
        .copied()
        .filter(|d| !filtering || d.search_key.contains(&filter))
        .collect();

    if filtered.is_empty() {
        let (headline, copy) = if filtering {
            ("No matches", "No documents match the filter.")
        } else {
            ("No documents", "This root has no OKF notes to list.")
        };
        return super::style::empty_state(IconName::Filter, headline, copy, cx).into_any_element();
    }

    let rows = Arc::new(flatten_rows(&filtered, root, filtering, collapsed));
    let promote = promote.clone();
    let reveal: Option<SharedString> = reveal.map(SharedString::from);

    uniform_list(
        "vault-list",
        rows.len(),
        move |range, _window, cx| {
            range
                .map(|ix| match &rows[ix] {
                    Row::Header {
                        key,
                        label,
                        count,
                        depth,
                        collapsed,
                    } => header_row(
                        key.clone(),
                        label.clone(),
                        *count,
                        *depth,
                        *collapsed,
                        panel.clone(),
                        cx,
                    ),
                    Row::Doc(doc, depth) => {
                        let revealed = reveal.as_deref() == Some(doc.id.as_str());
                        doc_row(doc, *depth, root, &promote, revealed, panel.clone(), cx)
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

/// A collapsible section header: disclosure chevron · label · count. Click
/// toggles the fold (a pure entity-state flip on the panel — §11-safe from
/// the listener; the tree re-flattens on the notify). Bottom-anchored inside
/// the uniform slot (the single-row-height law — see [`super::row`]).
fn header_row(
    key: SharedString,
    label: SharedString,
    count: usize,
    depth: usize,
    collapsed: bool,
    panel: WeakEntity<VaultBrowserPanel>,
    cx: &mut App,
) -> AnyElement {
    let colors = cx.theme().colors();
    let hover_color = colors.text_muted;
    let chevron = if collapsed {
        IconName::ChevronRight
    } else {
        IconName::ChevronDown
    };
    let toggle_key = key.clone();
    h_flex()
        .id(ElementId::Name(format!("vault-section-{key}").into()))
        .h(px(ROW_HEIGHT))
        .w_full()
        .items_end()
        .gap(px(5.))
        .pl(px(depth as f32 * INDENT))
        .pb(px(8.))
        .cursor_pointer()
        .text_color(colors.text_placeholder)
        .hover(move |s| s.text_color(hover_color))
        .on_click(move |_, _, cx| {
            let k = toggle_key.clone();
            panel
                .update(cx, |panel, cx| panel.toggle_section(k, cx))
                .ok();
        })
        .child(
            Icon::new(chevron)
                .size(ui::IconSize::XSmall)
                .color(Color::Custom(colors.text_placeholder)),
        )
        .child(
            div()
                .text_size(px(11.))
                .font_weight(FontWeight::MEDIUM)
                .child(label),
        )
        .child(
            div()
                .text_size(px(11.))
                .child(SharedString::from(format!("({count})"))),
        )
        .into_any_element()
}
