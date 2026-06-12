//! The corner-cluster positioner — the one workspace-mounted view composing
//! the §4.8/§4.9 corner system: the usage island as the HEAD at 14px
//! top/right insets. Mounted ONCE into the workspace's `corner_cluster_item`
//! slot (see `islands/mod.rs` for the host decision) and never unmounted —
//! the cross-route persistence that makes the cluster read built-in.
//!
//! The cluster's root is a full-area absolute layer with no listeners of
//! its own, so it is hit-test transparent everywhere except its children;
//! while the island is expanded it interposes an occluding backdrop that
//! routes any outside click to the island's contract (the web's
//! document-level click-away listener).

use gpui::{Context, Entity};
use ui::prelude::*;

use super::notif_stack::{NotifStack, STACK_W};
use super::usage_island::UsageIsland;

/// Corner inset for the cluster head (web `#usage-island { top: 14px;
/// right: 14px }`).
const CORNER_INSET: f32 = 14.;
/// Where the toast stack starts: below the island head (14px inset + 28px
/// rest pill + 14px gap — web `#notif-stack { top: 56px }`).
const STACK_TOP: f32 = 56.;

pub struct CornerCluster {
    island: Entity<UsageIsland>,
    stack: Entity<NotifStack>,
}

impl CornerCluster {
    pub fn new(
        island: Entity<UsageIsland>,
        stack: Entity<NotifStack>,
        cx: &mut Context<Self>,
    ) -> Self {
        // The backdrop renders off the island's state — repaint with it.
        cx.observe(&island, |_, _, cx| cx.notify()).detach();
        Self { island, stack }
    }
}

impl Render for CornerCluster {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let expanded = self.island.read(cx).is_expanded();
        div()
            .absolute()
            .inset_0()
            .when(expanded, |this| {
                this.child(
                    div()
                        .id("island-click-away")
                        .absolute()
                        .inset_0()
                        .occlude()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.island.update(cx, |island, cx| island.click_away(cx));
                        })),
                )
            })
            // Toasts stack beneath the head; the island paints last so the
            // expanded card grows DOWN over them (one corner system).
            .child(
                div()
                    .absolute()
                    .top(px(STACK_TOP))
                    .right(px(CORNER_INSET))
                    .w(px(STACK_W))
                    .child(self.stack.clone()),
            )
            .child(
                div()
                    .absolute()
                    .top(px(CORNER_INSET))
                    .right(px(CORNER_INSET))
                    .child(self.island.clone()),
            )
    }
}
