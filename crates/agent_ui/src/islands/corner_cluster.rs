//! The corner-cluster positioner — the one workspace-mounted view composing
//! the §4.8/§4.9 corner system: the usage island as the HEAD at 14px
//! top/right insets. Mounted ONCE into the workspace's `corner_cluster_item`
//! slot (see `islands/mod.rs` for the host decision) and never unmounted —
//! the cross-route persistence that makes the cluster read built-in.
//!
//! The cluster's root is a full-area absolute layer with no listeners of
//! its own, so it is hit-test transparent everywhere except its children.
//! Click-away for the expanded island is PASSIVE and lives on the island
//! itself (`on_mouse_down_out` — the web's document-level capture listener
//! with no stopPropagation): outside clicks contract the card AND still
//! land on their target; hover/scroll beneath are never occluded.

use gpui::{Context, Entity};
use ui::prelude::*;

use super::notif_stack::{NotifStack, clamped_width};
use super::usage_island::UsageIsland;

/// Corner inset for the cluster head (web `#usage-island { top: 14px;
/// right: 14px }`).
const CORNER_INSET: f32 = 14.;
/// Where the toast stack starts: below the island head (14px inset + 30px
/// border-box rest pill + 12px gap — web `#notif-stack { top: 56px }`).
const STACK_TOP: f32 = 56.;

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
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        // Web `#notif-stack { max-width: calc(100vw - 28px) }`.
        let stack_w = clamped_width(f32::from(window.viewport_size().width));
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
                    .top(px(CORNER_INSET))
                    .right(px(CORNER_INSET))
                    .child(self.island.clone()),
            )
            .child(
                div()
                    .absolute()
                    .top(px(STACK_TOP))
                    .right(px(CORNER_INSET))
                    .w(px(stack_w))
                    .child(self.stack.clone()),
            )
    }
}
