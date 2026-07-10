//! Orchestrator-bridge connection (`http://localhost:4530`).
//!
//! Z1+ panels consume [`BridgeStore`] as an `Entity` and never care which
//! transport fed it: the client task attaches to the bridge's `/sse` stream
//! (push-driven, RUST_PORT_NOTES general principle 1) and falls back to
//! `/board` polling whenever `/sse` is unreachable, reconnecting with
//! 1s→5s-capped backoff.
//!
//! The transcript (Z3) is the one POLLED feed: SSE carries only board/usage
//! today (transcript-over-SSE is bridge-side work, deferred). The poll runs
//! at the web cadences (900ms busy / 2.5s idle) and ONLY while a chat panel
//! holds a [`TranscriptWatch`] — refcount-gated, never free-running.
//!
//! Concern split (CLAUDE.md modularity):
//! - [`protocol`] — wire types (`RunRow`, `PoolRow`, tagged [`BridgeEvent`]).
//! - [`sse`] — incremental SSE line-parser (in-tree Pattern B idiom).
//! - [`store`] — the store entity + the 1s elapsed ticker.
//! - [`client`] — the connection/fallback loop.
//! - [`adversary`] — broadcast jobs (`POST /adversary` + the watch-gated
//!   job poll, Z4).
//! - [`approvals`] — pending-approval transport (Phase-2 permissions §5.2:
//!   the watch-gated `/approvals` poll + allow/deny POSTs).

pub mod adversary;
pub mod approvals;
pub mod client;
pub mod demo;
pub mod protocol;
pub mod sse;
pub mod store;
#[cfg(test)]
mod watch_tests;

pub use adversary::{
    AdversaryJobs, AdversaryPhase, AdversaryResult, AdversaryWatch, SynthesisSections,
    VENDOR_COLUMNS, parse_synthesis_sections,
};
pub use approvals::{ApprovalWatch, post_approval_allow, post_approval_deny};
pub use client::{BRIDGE_BASE_URL, error_message, fetch_json, post_json, post_json_status};
pub use demo::is_agentic_demo;
pub use protocol::{
    ApprovalRow, BridgeEvent, ChannelEventRow, ChannelRow, ConversationRow, OverlapRow,
    PlanSnapshot, PlanSubtask, PoolRow, ProjectRow, RunRow, RunTokens, ScrapeMeta,
    TranscriptMessage, TranscriptRun, TranscriptSnapshot, UnlinkedRun, UsageMeta, VendorLiveness,
};
pub use store::{BridgeStore, PlanWatch, Transport, TranscriptWatch, global_store, init};
