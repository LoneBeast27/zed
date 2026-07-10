//! The OKF vault browser — a stock-style LEFT-DOCK workspace `Panel`
//! (PARITY_SPEC Amendment 2026-07-04 (4) item 4): the atlas-vault + the import
//! staging as a first-class filesystem surface. LIST view (an Obsidian-style
//! folder tree — vault-UX pass 2026-07-10) + GRAPH view (constellation idiom)
//! over OKF documents, including imported session bundles, with browse /
//! reading-view / edit-source / promote-from-staging / new-note.
//!
//! User-toggleable, NOT mode-driven (modes never touch the left dock); its own
//! activation priority (21, the fork's next free slot above the 14-20 band).
//! Reads the local vault filesystem directly — the bridge feeds only the
//! axioms view and the new-note POST (both degrade honestly when it is down).
//!
//! Concern split (CLAUDE.md modularity, 500-line ceiling per file):
//! - [`panel`]   — the `Panel` impl, view state, and the render skeleton.
//! - `panel_header` — the chrome row (root switcher + view toggle + filter +
//!                 new-note button), split from `panel` to hold the ceiling.
//! - `panel_graph` — the graph drag/hover/reveal handlers (pure state flips).
//! - [`index`]   — the data model + background filesystem walker (streamed
//!                 reads, truncation flag, machine dirs skipped).
//! - `docmeta`   — doc construction: kind classification + the HUMAN title
//!                 law (frontmatter title → run outcome → first heading →
//!                 filename) + run/routine secondary meta.
//! - [`parser`]  — the hand-rolled frontmatter + link extractor (fence-skip).
//! - [`tree`]    — the folder-tree derivation (sections mirroring the vault's
//!                 real hierarchy, Recent on top, collapse state, counts).
//! - [`list`]    — the virtualized tree LIST view (uniform rows).
//! - `row`       — one document row (title-first, per-kind meta, promote).
//! - `open_doc`  — opening docs: the RENDERED reading view (the real
//!                 `markdown_preview` view, deduped) + raw source.
//! - `new_note`  — the "+ new note" compose row (bridge POST, honest degrade).
//! - [`field`]   — the shared-physics GRAPH field (Obsidian relaxation over
//!                 project clusters; the v2 upgrade over the static fallback).
//! - [`graph`]   — edge resolution + the GRAPH renderer (field or static).
//! - [`promote`] — staging→vault bundle move (collision-safe) + its UI state.
//! - [`axioms`]  — the AXIOMS view (bridge `GET /axioms` + user-gated
//!                 approve/reject), the one bridge-fed body in this panel.
//! - [`style`]   — vault chips + age formatting (re-uses task_board tokens).

pub mod axioms;
mod docmeta;
pub mod field;
pub mod graph;
mod graph_render;
pub mod index;
pub mod list;
mod new_note;
mod open_doc;
pub mod panel;
mod panel_graph;
mod panel_header;
pub mod parser;
pub mod promote;
mod row;
mod style;
pub mod tree;

pub use panel::{ToggleFocus, VaultBrowserPanel};
