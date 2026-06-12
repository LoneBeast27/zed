//! Task board (PARITY_SPEC §4.2) — the Antigravity inbox + our spawn-tree
//! graph + run drawer, ported 1:1 from the approved web reference render
//! (`bridge/ui/board.js`, `graph.js`, `board.css`, `drawer.js`).
//!
//! Concern split mirrors the web file shape:
//! - [`panel`] — workspace `Panel` impl: header, seg-toggle, body dispatch.
//! - [`inbox`] — virtualized run list (grid view).
//! - [`graph`] — the spawn-tree graph (graph view).
//! - [`motion`] — exact CSS cubic-bezier motion tokens (§4.9).
//! - [`style`] — shared board vocabulary (tokens, pills, chips, phrases).

pub mod graph;
pub mod inbox;
pub mod motion;
pub mod panel;
pub mod style;

pub use panel::{TaskBoardEvent, TaskBoardPanel, ToggleFocus};
