//! Orchestrator-bridge connection (`http://localhost:4530`).
//!
//! Z1+ panels consume [`BridgeStore`] as an `Entity` and never care which
//! transport fed it: the client task attaches to the bridge's `/sse` stream
//! (push-driven, RUST_PORT_NOTES general principle 1) and falls back to
//! `/board` polling whenever `/sse` is unreachable, reconnecting with
//! 1s→5s-capped backoff.
//!
//! Concern split (CLAUDE.md modularity):
//! - [`protocol`] — wire types (`RunRow`, `PoolRow`, tagged [`BridgeEvent`]).
//! - [`sse`] — incremental SSE line-parser (in-tree Pattern B idiom).
//! - [`client`] — the store entity + connection/fallback loop.

pub mod client;
pub mod protocol;
pub mod sse;

pub use client::{BRIDGE_BASE_URL, BridgeStore, fetch_json, global_store, init};
pub use protocol::{BridgeEvent, PoolRow, RunRow};
