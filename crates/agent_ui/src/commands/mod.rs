//! The `/` command-surface layer (S0–S4 of the build ladder,
//! `COMMAND_SURFACE_PARITY.md`).
//!
//! - [`types`]       — the [`CommandEntry`] data model + enums.
//! - [`static_seed`] — the curated static fold of the four vendor inventories.
//! - [`discovery`]   — background walk of the user's LOCAL commands + skills.
//! - [`registry`]    — the runtime registry entity + the pure scoping/filter
//!                     fold both the `/` typeahead and `/help` render from.
//! - [`help`]        — the `/help [vendor]` transient-buffer markdown builder.
//!
//! The composer typeahead (S1) and the dispatch/routing (S2/S4) live in
//! `orchestrator_panel::typeahead` / `orchestrator_panel::dispatch`, reading
//! this registry.

pub mod discovery;
pub mod help;
pub mod registry;
pub mod static_seed;
pub mod types;

pub use registry::{ClassCounts, CommandRegistry, class_counts, visible};
pub use types::{
    Classification, CommandEntry, CommandKind, Mechanism, OrchTarget, Vendor,
};
