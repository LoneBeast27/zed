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
//! axioms view, the routines view, the writeback review, and the new-note
//! POST (all degrade honestly when it is down).
//!
//! Concern split (CLAUDE.md modularity, 500-line ceiling per file):
//! - [`panel`]   — the `Panel` impl, view state, and the render skeleton.
//! - `panel_body` — the body dispatch (which view renders, index gate,
//!                 honest empty states), split from `panel` for the ceiling.
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
//!                 approve/reject), a bridge-fed body in this panel.
//! - [`routines`] — the ROUTINES contract: `GET /routines` wire types + the
//!                 AMBER-LAW/fire gating folds + the view state.
//! - `routines_actions` — the row buttons + their POSTs (run-now / toggle /
//!                 the AMBER consent commit) and the honest outcome notes.
//! - `routines_view` — the ROUTINES render half (cards, chips, consent
//!                 affordance, honest empty/offline states).
//! - [`writeback`] — the WRITEBACK contract: `GET /writeback` wire types +
//!                 the pure candidate fold (run→note / import conflict).
//! - `writeback_state` — writeback fetch/accept/dismiss/resolve actions +
//!                 the conflict modal state (both sides read from the vault).
//! - `writeback_view` — the WRITEBACK render half (candidate cards, honest
//!                 empty/offline states).
//! - `writeback_modal` — the conflict modal (both sides + the explicit
//!                 keep-current / overwrite resolutions — never a silent
//!                 overwrite).
//! - [`importers`] — the IMPORT contract: the discovered two-family importer
//!                 surface (session stores → staging = CLI-only, documented
//!                 honestly; vendor exports → knowledge = live bridge HTTP)
//!                 + the pure mapping (counts/progress/result lines).
//! - `importers_ops` — import fetch/start/cancel + the running-job watch
//!                 (polls the same jobs registry the board's import rows
//!                 merge from).
//! - `importers_view` — the IMPORT render half for the LIVE export wire
//!                 (vendor cards, path+project compose, the watched job's
//!                 progress row, honest empty/offline states).
//! - `importers_view_session` — the IMPORT render half for the CLI-only
//!                 session stores (documented cards, copyable `import run`
//!                 lines, the "view staged" handoff) + shared builders.
//! - [`style`]   — vault chips + age formatting (re-uses task_board tokens).

pub mod axioms;
mod docmeta;
pub mod field;
pub mod graph;
mod graph_render;
pub mod importers;
mod importers_ops;
mod importers_view;
mod importers_view_session;
pub mod index;
pub mod list;
mod new_note;
mod open_doc;
pub mod panel;
mod panel_body;
mod panel_graph;
mod panel_header;
pub mod parser;
pub mod promote;
pub mod routines;
mod routines_actions;
mod routines_view;
mod row;
mod style;
pub mod tree;
pub mod writeback;
mod writeback_modal;
mod writeback_state;
mod writeback_view;

pub use panel::{ToggleFocus, VaultBrowserPanel};
