//! The Constellation — the agent-fleet lifecycle board (PARITY_SPEC §4.9
//! "Subagent-lifecycle panel", RUST_PORT_NOTES §10), ported from the web
//! reference render (`bridge/ui/{graph.js, graph-channels.js, board.css}`).
//! The DESIGN transfers; the code is a GPUI rewrite: physics stepped in
//! native Rust off the element-build path, positions folded into sim
//! outputs once per frame, paint closures replaying small snapshots.
//!
//! As-built law (user-locked 2026-07-03): star topology · Atlas
//! checkpoint-dot grammar (root = same language one rank bigger, accent
//! ring) · billiards-aim fan · Obsidian physics (shifted-inverse-square
//! repel, collide floor, anchor springs, sleep gate) · drag re-pin +
//! auto-arrange tween + spawn-with-autosort · Agar.io gobble (linger →
//! ease-in reel, edge follows the flight, root gulps + gains the receipt) ·
//! Kiali boost edges (glow tint by state, one particle per batch, k/max
//! badge, terminal retract) · token-mass sizing (area ∝ tokens).
//!
//! Concern split (CLAUDE.md modularity):
//! - [`panel`] — the workspace `Panel` (right dock), frame loop, drag.
//! - [`sim`] / advance — event→state folding + the per-frame step.
//! - [`physics`] — the pure Obsidian field stepper.
//! - [`layout`] — billiards fan + weighted arrange seriation.
//! - [`mass`] — token-mass mapping + root ctx/bonus/exhale state.
//! - [`edges`] — boost-channel runtime (particles, retract).
//! - draw — element/canvas builders (pure read of sim outputs).
//! - feed — the supplemental visibility-gated poll (SSE digest gaps).
//! - demo — the staged design scenario (shared agentic-demo gate; also the
//!   board's demo-board source, so it is `pub`).

mod advance;
pub mod demo;
mod draw;
mod draw_node;
pub mod edges;
mod feed;
pub mod layout;
pub mod mass;
pub mod panel;
pub mod physics;
pub mod sim;

pub use panel::{ConstellationPanel, ToggleFocus};
