//! Task board (PARITY_SPEC §4.2) — the Antigravity inbox + our spawn-tree
//! graph + run drawer, ported 1:1 from the approved web reference render
//! (`bridge/ui/board.js`, `graph.js`, `board.css`, `drawer.js`).
//!
//! Concern split mirrors the web file shape:
//! - [`panel`] — workspace `Panel` impl: header, seg-toggle, body dispatch.
//! - [`inbox`] — virtualized run list (grid view).
//! - [`style`] — shared board vocabulary (tokens, pills, chips, phrases).

pub mod inbox;
pub mod panel;
pub mod style;

pub use panel::{TaskBoardEvent, TaskBoardPanel, ToggleFocus};
