//! The corner-island chrome (PARITY_SPEC §4.8/§4.9): the usage island as the
//! corner-cluster HEAD pinned top-right of the content area, with the
//! notification stack beneath it — one corner system, alive on every
//! workspace mode.
//!
//! ## Host decision (Task Z2.2)
//!
//! Three in-tree hosts were evaluated for the persistent overlay:
//!
//! 1. **`Workspace::show_notification` / the notification stack**
//!    (`workspace/src/notifications.rs` + `render_notifications`,
//!    workspace.rs) — REJECTED for the cluster: position is hardcoded
//!    bottom-right (`.right_3().bottom_3().w_112().justify_end()`), entries
//!    interleave with Zed's own notifications (an LSP prompt would displace
//!    the island), and the dismiss/suppress lifecycle fights an element
//!    that must NEVER unmount.
//! 2. **`StatusItemView`** (`workspace/src/status_bar.rs`) — named a
//!    usage-pill host candidate in COMPLEMENTARY_SOLUTIONS §5, but the
//!    2026-06-12 user directive moved the island to the top-right corner
//!    head whose card grows DOWN out of the corner anchor; a status-bar
//!    item is bottom-edge chrome and cannot host the §4.9
//!    emergence-from-corner geometry.
//! 3. **A fork-added workspace overlay slot** (`corner_cluster_item`,
//!    mirroring the existing fork addition `activity_bar_item`) — CHOSEN.
//!    COMPLEMENTARY_SOLUTIONS §4 itself blesses "a sibling absolute div at
//!    the workspace.rs render_notifications layering point" for non-default
//!    corners. The slot renders the cluster as a child of the workspace's
//!    relative content div (after docks/center/zoomed, before the status
//!    bar + toast layer), which delivers all three requirements: persistent
//!    across every mode (mode switches only touch docks — the workspace
//!    entity lives on), anchored top-right of the content area at 14px
//!    insets (the cluster owns its own absolute geometry), and never
//!    unmounted (the `AnyView` is held by the `Workspace` struct itself).
//!
//! Concern split (CLAUDE.md modularity):
//! - [`state`] — the pure rest/notify/held/expanded machine + 75/90
//!   crossing queue (CPU-testable, no gpui).
//! - [`island_faces`] — the four representations as data + their builders
//!   and deterministic geometry measurement.
//! - [`usage_island`] — the island entity: store ingest, timers, morphs.
//! - [`notif_stack`] — the toast stack entity: store diffing, surfacing,
//!   expiry/retract lifecycle.
//! - [`notif_card`] — the toast element: card anatomy, slot, and the §4.9
//!   emerge/retract motion.
//! - [`corner_cluster`] — the workspace-mounted positioner that composes
//!   the island + stack into the one corner system.

pub mod corner_cluster;
pub mod island_faces;
mod notif_card;
pub mod notif_logic;
pub mod notif_stack;
pub mod state;
pub mod usage_island;

pub use corner_cluster::CornerCluster;
pub use notif_stack::NotifStack;
pub use usage_island::UsageIsland;
