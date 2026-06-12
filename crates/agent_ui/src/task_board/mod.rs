//! Task board (PARITY_SPEC §4.2) — the Antigravity inbox + our spawn-tree
//! graph + run drawer, ported 1:1 from the approved web reference render
//! (`bridge/ui/board.js`, `graph.js`, `board.css`, `drawer.js`).
//!
//! Concern split mirrors the web file shape:
//! - [`panel`] — workspace `Panel` impl: header, seg-toggle, body dispatch.
//! - [`inbox`] — virtualized run list (grid view).
//! - [`graph`] — the spawn-tree graph (layout, edges, spawn motion).
//! - [`node`] — graph node visuals (status dot, spinner, root ring, label).
//! - [`paint_cache`] — cached static paint geometry (settled edges, rings).
//! - [`run_detail`] — the run drawer entity (fetch/poll, slide-over).
//! - [`run_detail_head`] — the drawer chrome (head, close, worked, tabs).
//! - [`run_detail_body`] — the drawer's Summary/Result/Logs tab bodies.
//! - [`motion`] — exact CSS cubic-bezier motion tokens (§4.9).
//! - [`style`] — shared board vocabulary (tokens, pills, chips, phrases).

pub mod graph;
pub mod inbox;
pub mod motion;
mod node;
mod paint_cache;
pub mod panel;
pub mod run_detail;
mod run_detail_body;
mod run_detail_head;
pub mod style;

pub use panel::{TaskBoardEvent, TaskBoardPanel, ToggleFocus};
