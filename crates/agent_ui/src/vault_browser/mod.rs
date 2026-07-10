//! The OKF vault browser — a stock-style LEFT-DOCK workspace `Panel`
//! (PARITY_SPEC Amendment 2026-07-04 (4) item 4): the atlas-vault + the import
//! staging as a first-class filesystem surface. LIST view (task-board list
//! idiom) + GRAPH view (constellation/graph idiom) over OKF documents,
//! including imported session bundles, with browse / open-in-editor /
//! promote-from-staging.
//!
//! User-toggleable, NOT mode-driven (modes never touch the left dock); its own
//! activation priority (21, the fork's next free slot above the 14-20 band).
//! Reads the local vault filesystem directly — no bridge hop.
//!
//! Concern split (CLAUDE.md modularity, 500-line ceiling per file):
//! - [`panel`]   — the `Panel` impl, header/root-switcher, view toggle, filter
//!                 editor, and the deferred interaction handlers (§11).
//! - `panel_header` — the chrome row (root switcher + view toggle + filter),
//!                 split from `panel` to hold the 500-line ceiling.
//! - [`index`]   — the data model + background filesystem walker (streamed
//!                 reads, truncation flag).
//! - [`parser`]  — the hand-rolled frontmatter + link extractor (fence-skip).
//! - [`list`]    — the virtualized grouped LIST view.
//! - [`field`]   — the shared-physics GRAPH field (Obsidian relaxation over
//!                 project clusters; the v2 upgrade over the static fallback).
//! - [`graph`]   — edge resolution + the GRAPH renderer (field or static).
//! - [`promote`] — staging→vault bundle move (collision-safe) + its UI state.
//! - [`axioms`]  — the AXIOMS view (bridge `GET /axioms` + user-gated
//!                 approve/reject), the one bridge-fed body in this panel.
//! - [`style`]   — vault chips + age formatting (re-uses task_board tokens).

pub mod axioms;
pub mod field;
pub mod graph;
mod graph_render;
pub mod index;
pub mod list;
pub mod panel;
mod panel_header;
pub mod parser;
pub mod promote;
mod style;

pub use panel::{ToggleFocus, VaultBrowserPanel};
