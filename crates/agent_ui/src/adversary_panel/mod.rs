//! The Adversary panel (PARITY_SPEC §4.5), ported from the approved web
//! reference render (`bridge/ui/adversary.js` + `panels.css`): a composer
//! deck that broadcasts one prompt to all three models (`POST /adversary`),
//! three vendor columns in an `h_flex` that shimmer while the job runs and
//! fill with Markdown when it lands, and the synthesis card
//! (AGREEMENTS / DISAGREEMENTS / SYNTHESIS) beneath.
//!
//! Data path: [`crate::bridge::AdversaryJobs`] — the broadcast POST and the
//! 2.5s job poll run on background executors through the bridge connection
//! idiom (RUST_PORT_NOTES §4.5: never blocking the UI thread), and the poll
//! is watch-gated on `Panel::set_active` exactly like the orchestrator's
//! TranscriptWatch: hidden panel, dead poll.
//!
//! Concern split (CLAUDE.md modularity): [`panel`] owns the panel lifecycle,
//! composer deck, and dock plumbing; [`columns`] owns the result rendering
//! (vendor columns, shimmer, synthesis card).

mod columns;
mod panel;

pub use panel::{AdversaryPanel, Broadcast, ToggleFocus};
