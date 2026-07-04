//! The corner-cluster positioner — the one workspace-mounted view composing
//! the §4.8/§4.9 corner system: the usage island as the HEAD anchored
//! top/right of the content area. Mounted ONCE into the workspace's
//! `corner_cluster_item` slot (see `islands/mod.rs` for the host decision)
//! and never unmounted — the cross-route persistence that makes the cluster
//! read built-in.
//!
//! ## Anchor choice (Finding 1, 2026-07-04) — respecting the island law
//!
//! PARITY_SPEC §4.8/§4.9: islands are ANCHORED, not positioned — the head is
//! pinned to the top-right of the CONTENT AREA and the card grows DOWN out of
//! that anchor. The slot (`corner_cluster_item`) mounts the cluster as a child
//! of the workspace `#workspace` div (workspace.rs), whose TOP EDGE is now the
//! top of the center pane — and since mode surfaces became center-pane items,
//! that pane carries an editor tab bar whose right-corner `+` / split buttons
//! sit exactly where a 14px top anchor put the island pill. "pill and zed
//! intersect wrong."
//!
//! The law-correct fix is NOT to reposition the pill by hand (that would be
//! "positioned, not anchored") but to move the ANCHOR ELEMENT down: the head
//! now anchors below the tab-bar row (top inset = one tab-bar row height +
//! the 14px corner inset). The pill stays right-pinned and grows down out of
//! the corner exactly as §4.9 mandates; it simply emerges from an anchor that
//! clears the chrome the new center-pane layout introduced. The offset is the
//! tab-bar's own height (`DynamicSpacing::Base32`, the value
//! `ui::Tab::container_height` returns), so it tracks the UI font scale — the
//! pill never intersects tab-bar chrome at any window width or zoom (the
//! geometry is width-independent: both are right-anchored, and the vertical
//! clearance is a full tab row). When the tab bar is hidden the pill simply
//! sits one row lower in empty pane space — harmless, and still never over
//! chrome. Documented against §4.9: the emergence origin is the head anchor;
//! this only relocates that origin, the growth idiom is unchanged.
//!
//! The cluster's root is a full-area absolute layer with no listeners of
//! its own, so it is hit-test transparent everywhere except its children.
//! Click-away for the expanded island is PASSIVE and lives on the island
//! itself (`on_mouse_down_out` — the web's document-level capture listener
//! with no stopPropagation): outside clicks contract the card AND still
//! land on their target; hover/scroll beneath are never occluded.

use gpui::{Context, Entity, px};
use ui::prelude::*;

use super::layout::{ClusterAnchor, cluster_anchor};
use super::notif_stack::{NotifStack, clamped_width};
use super::usage_island::UsageIsland;

/// Corner inset for the cluster head (web `#usage-island { top: 14px;
/// right: 14px }`). The TOP inset is measured from below the tab-bar row (see
/// [`cluster_anchor`]); the RIGHT inset is unchanged.
pub(super) const CORNER_INSET: f32 = 14.;
/// Gap between the island head's foot and the top of the toast stack (web
/// `#notif-stack { top: 56px }` = 14px inset + 30px rest pill + 12px gap; the
/// 42px here is the pill-plus-gap contribution, added to the head's top).
pub(super) const STACK_GAP: f32 = 42.;

/// One tab-bar row's height, matching `ui::Tab::container_height`
/// (`DynamicSpacing::Base32`). Computed here (not by importing `Tab`) so the
/// cluster depends on the spacing scale, not the tab component; the anchor
/// tracks the UI-font zoom exactly as the tab bar does.
pub(super) fn tab_bar_row_height(cx: &App) -> f32 {
    f32::from(DynamicSpacing::Base32.px(cx))
}

pub struct CornerCluster {
    island: Entity<UsageIsland>,
    stack: Entity<NotifStack>,
}

impl CornerCluster {
    pub fn new(
        island: Entity<UsageIsland>,
        stack: Entity<NotifStack>,
        _cx: &mut Context<Self>,
    ) -> Self {
        Self { island, stack }
    }
}

impl Render for CornerCluster {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Web `#notif-stack { max-width: calc(100vw - 28px) }`.
        let stack_w = clamped_width(f32::from(window.viewport_size().width));
        // The head anchor drops below the tab-bar row so the pill never
        // intersects the center pane's tab-bar corner buttons (Finding 1). The
        // geometry is pure + width-independent — see [`cluster_anchor`].
        let ClusterAnchor {
            head_top,
            stack_top,
            right,
        } = cluster_anchor(tab_bar_row_height(cx), CORNER_INSET, STACK_GAP);
        div()
            .absolute()
            .inset_0()
            // Toasts stack beneath the head positionally but paint ABOVE
            // it (web: `#notif-stack` z-index 60 over the island's 40) —
            // while the card is expanded past the stack's top, live toasts
            // overlay it and stay clickable.
            .child(
                div()
                    .absolute()
                    .top(px(head_top))
                    .right(px(right))
                    .child(self.island.clone()),
            )
            .child(
                div()
                    .absolute()
                    .top(px(stack_top))
                    .right(px(right))
                    .w(px(stack_w))
                    .child(self.stack.clone()),
            )
    }
}
