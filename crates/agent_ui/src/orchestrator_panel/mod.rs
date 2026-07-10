//! Orchestrator chat (PARITY_SPEC §4.1) — Antigravity anatomy × Claude
//! dynamism, ported 1:1 from the approved web reference render
//! (`bridge/ui/chat.js` + `chat.css`) minus the RUST_PORT_NOTES idiom
//! rulings (no span-splitting chunk-fade; instant Disclosure expand;
//! pulsate shimmer).
//!
//! Concern split mirrors the web file shape (idiom-copied from the
//! thread_view / message_editor monoliths into <500-line modules — never
//! extending them, per COMPLEMENTARY_SOLUTIONS verdict C):
//! - [`panel`] — workspace `Panel` impl: crumb, body dispatch, busy ticker,
//!   the [`crate::bridge::TranscriptWatch`] that gates the `/transcript`
//!   poll to panel presence.
//! - [`transcript`] — keyed message views over `list(ListState)` + the
//!   shimmer item and empty-state greeting.
//! - [`message`] — per-message anatomy: user tonal cards, bare markdown
//!   prose, worked-for collapsibles, the hover meta trio.
//! - [`composer`] — the two-layer composer deck; its send⇄stop island
//!   morph lives in [`send_circle`].
//! - [`command_note`] — S5 google-lane execution: the bridge POST + the
//!   honest system-row outcome render.

mod command_note;
mod composer;
mod dispatch;
mod message;
mod panel;
mod send_circle;
mod transcript;
mod typeahead;
mod typeahead_menu;
mod usage_strip;

#[cfg(test)]
mod typeahead_dispatch_tests;

pub use panel::{OrchestratorPanel, Send, ToggleFocus};
