//! Background-polling client for the local orchestrator bridge
//! (`http://localhost:4530`). Z1+ panels consume [`BridgeStore`] as an Entity.
//!
//! RUST_PORT_NOTES general principles 1–2 govern this file: state lives in an
//! `Entity<BridgeStore>` updated via `cx.notify()`, all I/O + JSON parsing runs
//! on background executors, and the UI thread only ever applies the parsed
//! snapshot. Polling is a stopgap until the bridge ships its `/events` SSE
//! endpoint (Wave Z0.5) — cadences mirror the web bridge's.

use std::{sync::Arc, time::Duration};

use anyhow::Result;
use futures::AsyncReadExt as _;
use gpui::{App, AppContext as _, AsyncApp, Entity, WeakEntity};
use http_client::{AsyncBody, HttpClient};
use serde::Deserialize;

pub const BRIDGE_BASE_URL: &str = "http://localhost:4530";

/// `/board` cadence (matches the web bridge poll).
const BOARD_POLL_INTERVAL: Duration = Duration::from_millis(1500);
/// `/usage` rides every Nth board tick: 8 × 1500ms = 12s.
const USAGE_POLL_TICKS: u64 = 8;
/// When the bridge is offline, back off to slow retries.
const OFFLINE_RETRY_INTERVAL: Duration = Duration::from_secs(5);

/// One run row from `GET /board`. Liberal in what it accepts: every field
/// defaults so schema drift on the bridge side never breaks the panel.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct RunRow {
    #[serde(default)]
    pub run_id: String,
    #[serde(default)]
    pub conv: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub elapsed_s: f64,
    #[serde(default)]
    pub task: Option<String>,
    #[serde(default)]
    pub agent: String,
    #[serde(default)]
    pub chip: Option<String>,
}

/// One pool row extracted from the `GET /usage` object
/// (`pool-name -> { headroom_pct, window }`; `_`-prefixed keys are metadata).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PoolRow {
    pub name: String,
    pub headroom_pct: Option<f64>,
    pub window: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct BoardResponse {
    #[serde(default)]
    board: Vec<RunRow>,
}

/// The bridge snapshot Z1+ panels render from. `connected == false` means the
/// bridge is offline — the last snapshot is kept so panels can grey-out rather
/// than blank.
#[derive(Default)]
pub struct BridgeStore {
    pub board: Vec<RunRow>,
    pub usage: Vec<PoolRow>,
    pub connected: bool,
}

/// Creates the store entity and detaches the polling loop. The loop exits on
/// its own once the entity is dropped (weak-handle update failure).
pub fn init(cx: &mut App) -> Entity<BridgeStore> {
    let http_client = cx.http_client();
    cx.new(|cx| {
        cx.spawn(async move |this, cx| poll_loop(http_client, this, cx).await)
            .detach();
        BridgeStore::default()
    })
}

async fn poll_loop(
    http_client: Arc<dyn HttpClient>,
    this: WeakEntity<BridgeStore>,
    cx: &mut AsyncApp,
) {
    let mut tick: u64 = 0;
    loop {
        let fetch_usage = tick % USAGE_POLL_TICKS == 0;
        let client = http_client.clone();

        // Fetch + parse entirely off the UI thread.
        let fetched = cx
            .background_spawn(async move {
                let board_raw =
                    fetch_json(client.as_ref(), &format!("{BRIDGE_BASE_URL}/board")).await?;
                let board = parse_board(&board_raw)?;
                let usage = if fetch_usage {
                    let usage_raw =
                        fetch_json(client.as_ref(), &format!("{BRIDGE_BASE_URL}/usage")).await?;
                    Some(parse_usage(&usage_raw)?)
                } else {
                    None
                };
                anyhow::Ok((board, usage))
            })
            .await;

        let ok = fetched.is_ok();
        let alive = this
            .update(cx, |store, cx| {
                let mut changed = false;
                match fetched {
                    Ok((board, usage)) => {
                        if !store.connected {
                            store.connected = true;
                            changed = true;
                        }
                        if store.board != board {
                            store.board = board;
                            changed = true;
                        }
                        if let Some(usage) = usage
                            && store.usage != usage
                        {
                            store.usage = usage;
                            changed = true;
                        }
                    }
                    // Bridge offline: keep the last snapshot, flip the flag.
                    Err(_) => {
                        if store.connected {
                            store.connected = false;
                            changed = true;
                        }
                    }
                }
                if changed {
                    cx.notify();
                }
            })
            .is_ok();
        if !alive {
            return; // store dropped — stop polling
        }

        // Reset the schedule on failure so reconnect refetches usage at once.
        tick = if ok { tick.wrapping_add(1) } else { 0 };
        let delay = if ok {
            BOARD_POLL_INTERVAL
        } else {
            OFFLINE_RETRY_INTERVAL
        };
        cx.background_executor().timer(delay).await;
    }
}

async fn fetch_json(client: &dyn HttpClient, url: &str) -> Result<String> {
    let mut response = client.get(url, AsyncBody::default(), true).await?;
    let mut body = String::new();
    response.body_mut().read_to_string(&mut body).await?;
    anyhow::ensure!(
        response.status().is_success(),
        "bridge returned {} for {url}",
        response.status().as_u16()
    );
    Ok(body)
}

fn parse_board(raw: &str) -> Result<Vec<RunRow>> {
    let response: BoardResponse = serde_json::from_str(raw)?;
    Ok(response.board)
}

fn parse_usage(raw: &str) -> Result<Vec<PoolRow>> {
    let value: serde_json::Value = serde_json::from_str(raw)?;
    let object = value
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("usage: expected a JSON object"))?;

    let mut rows = Vec::new();
    for (name, entry) in object {
        if name.starts_with('_') {
            continue; // bridge metadata keys
        }
        rows.push(PoolRow {
            name: name.clone(),
            headroom_pct: entry.get("headroom_pct").and_then(|v| v.as_f64()),
            window: entry
                .get("window")
                .and_then(|v| v.as_str())
                .map(str::to_string),
        });
    }
    // Deterministic render order regardless of JSON map ordering.
    rows.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOARD_FIXTURE: &str = r#"{
        "board": [
            {
                "run_id": "r-123",
                "conv": "conv-9",
                "status": "running",
                "elapsed_s": 42.5,
                "task": "port wave Z0",
                "agent": "claude",
                "chip": "skc3"
            },
            {
                "run_id": "r-456",
                "conv": "conv-2",
                "status": "done",
                "elapsed_s": 901,
                "agent": "codex",
                "unknown_future_field": true
            }
        ]
    }"#;

    const USAGE_FIXTURE: &str = r#"{
        "_meta": { "updated_at": "2026-06-12T03:14:00Z" },
        "claude": { "headroom_pct": 62.5, "window": "5h" },
        "codex": { "headroom_pct": null },
        "gemini": { "headroom_pct": 12.0, "window": "daily" }
    }"#;

    #[test]
    fn board_fixture_deserializes() {
        let board = parse_board(BOARD_FIXTURE).unwrap();
        assert_eq!(board.len(), 2);

        assert_eq!(board[0].run_id, "r-123");
        assert_eq!(board[0].conv, "conv-9");
        assert_eq!(board[0].status, "running");
        assert_eq!(board[0].elapsed_s, 42.5);
        assert_eq!(board[0].task.as_deref(), Some("port wave Z0"));
        assert_eq!(board[0].agent, "claude");
        assert_eq!(board[0].chip.as_deref(), Some("skc3"));

        // Optional fields absent + unknown fields present are both tolerated.
        assert_eq!(board[1].run_id, "r-456");
        assert_eq!(board[1].elapsed_s, 901.0);
        assert_eq!(board[1].task, None);
        assert_eq!(board[1].chip, None);
    }

    #[test]
    fn board_missing_key_is_empty() {
        assert_eq!(parse_board("{}").unwrap(), Vec::new());
    }

    #[test]
    fn usage_fixture_deserializes() {
        let usage = parse_usage(USAGE_FIXTURE).unwrap();
        // Sorted by name, "_meta" filtered out.
        assert_eq!(usage.len(), 3);
        assert_eq!(usage[0].name, "claude");
        assert_eq!(usage[0].headroom_pct, Some(62.5));
        assert_eq!(usage[0].window.as_deref(), Some("5h"));

        assert_eq!(usage[1].name, "codex");
        assert_eq!(usage[1].headroom_pct, None); // explicit null
        assert_eq!(usage[1].window, None); // missing key

        assert_eq!(usage[2].name, "gemini");
        assert_eq!(usage[2].headroom_pct, Some(12.0));
        assert_eq!(usage[2].window.as_deref(), Some("daily"));
    }

    #[test]
    fn usage_rejects_non_object() {
        assert!(parse_usage("[1, 2, 3]").is_err());
    }
}
