//! The bridge connection task.
//!
//! RUST_PORT_NOTES general principles 1–2 govern this file: state lives in an
//! `Entity<BridgeStore>` updated via `cx.notify()`, all I/O + parsing runs on
//! background executors, and the UI thread only ever applies parsed updates.
//!
//! Transport: `/sse` push stream first (read on a background task, typed
//! events fanned to the foreground over a channel); when `/sse` is
//! unreachable the task FALLS BACK to `/board`+`/usage` polling for the
//! duration of a 1s→5s-capped backoff window, then retries `/sse`. Consumers
//! never see the difference.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::Result;
use futures::{AsyncReadExt as _, StreamExt as _, channel::mpsc};
use gpui::{AppContext as _, AsyncApp, WeakEntity};
use http_client::{AsyncBody, HttpClient, Response};

use super::protocol::{
    BridgeEvent, PoolRow, RunRow, UsageMeta, pools_from_object, usage_meta_from_object,
};
use super::sse::SseParser;
use super::store::BridgeStore;

pub const BRIDGE_BASE_URL: &str = "http://localhost:4530";

/// `/board` cadence while in polling fallback (matches the web bridge poll).
const BOARD_POLL_INTERVAL: Duration = Duration::from_millis(1500);
/// `/usage` cadence (PARITY_SPEC §4.8: the usage island polls at 12s). The
/// schedule is elapsed-based and lives in `connection_loop`, so it rides
/// across backoff windows instead of resetting with each one.
const USAGE_POLL_INTERVAL: Duration = Duration::from_secs(12);
/// When the bridge is fully offline, back off to slow poll retries.
const OFFLINE_RETRY_INTERVAL: Duration = Duration::from_secs(5);
/// SSE reconnect backoff: start here…
const SSE_BACKOFF_START: Duration = Duration::from_secs(1);
/// …grow by this much per failed attach…
const SSE_BACKOFF_STEP: Duration = Duration::from_secs(1);
/// …capped here.
const SSE_BACKOFF_CAP: Duration = Duration::from_secs(5);

pub(super) async fn connection_loop(
    http_client: Arc<dyn HttpClient>,
    this: WeakEntity<BridgeStore>,
    cx: &mut AsyncApp,
) {
    let mut backoff = SSE_BACKOFF_START;
    // The 12s usage schedule (None = due immediately) — owned here so it
    // spans backoff windows instead of resetting with each one.
    let mut last_usage_fetch: Option<Instant> = None;
    loop {
        let client = http_client.clone();
        let attached = cx
            .background_spawn(async move { connect_sse(client.as_ref()).await })
            .await;
        match attached {
            Ok(response) => {
                backoff = SSE_BACKOFF_START;
                if consume_sse(response, &this, cx).await.is_err() {
                    return; // store dropped
                }
                // Stream ended/erred — mark offline and reconnect promptly.
                if this
                    .update(cx, |store, cx| store.set_connected(false, cx))
                    .is_err()
                {
                    return;
                }
            }
            Err(_) => {
                // `/sse` unreachable (bridge down, or an older bridge without
                // the endpoint) — poll `/board` for one backoff window.
                if poll_window(&http_client, &this, cx, backoff, &mut last_usage_fetch)
                    .await
                    .is_err()
                {
                    return; // store dropped
                }
                backoff = (backoff + SSE_BACKOFF_STEP).min(SSE_BACKOFF_CAP);
            }
        }
    }
}

/// Whether the 12s usage schedule is due (`None` = fetch immediately — a
/// fresh fallback session, or the previous fetch failed).
fn usage_due(last_fetch: Option<Instant>, now: Instant) -> bool {
    last_fetch.is_none_or(|at| now.duration_since(at) >= USAGE_POLL_INTERVAL)
}

/// Streaming GET to `/sse`; resolves once headers arrive. Errors on connect
/// failure or non-2xx (e.g. 404 from a bridge predating the endpoint).
async fn connect_sse(client: &dyn HttpClient) -> Result<Response<AsyncBody>> {
    let response = client
        .get(&format!("{BRIDGE_BASE_URL}/sse"), AsyncBody::default(), true)
        .await?;
    anyhow::ensure!(
        response.status().is_success(),
        "bridge returned {} for /sse",
        response.status().as_u16()
    );
    Ok(response)
}

/// Reads the attached SSE stream on a background task, applying typed events
/// to the store as they arrive. Returns `Ok(())` when the stream ends and
/// `Err` only when the store entity is gone.
async fn consume_sse(
    mut response: Response<AsyncBody>,
    this: &WeakEntity<BridgeStore>,
    cx: &mut AsyncApp,
) -> Result<(), ()> {
    // Attached: the bridge pushes a full board+usage snapshot on connect, so
    // flipping `connected` here never shows stale data for long.
    this.update(cx, |store, cx| store.set_connected(true, cx))
        .map_err(|_| ())?;

    let (tx, mut rx) = mpsc::unbounded::<BridgeEvent>();
    let reader = cx.background_spawn(async move {
        let mut parser = SseParser::default();
        let mut buf = [0u8; 8192];
        loop {
            match response.body_mut().read(&mut buf).await {
                Ok(0) | Err(_) => return, // stream over (bridge restart) — outer loop reconnects
                Ok(n) => {
                    for payload in parser.push(&buf[..n]) {
                        match serde_json::from_str::<BridgeEvent>(&payload) {
                            Ok(event) => {
                                if tx.unbounded_send(event).is_err() {
                                    return; // receiver dropped
                                }
                            }
                            Err(error) => {
                                log::warn!("bridge sse: undecodable event: {error}")
                            }
                        }
                    }
                }
            }
        }
    });

    while let Some(event) = rx.next().await {
        this.update(cx, |store, cx| store.apply_event(event, cx))
            .map_err(|_| ())?;
    }
    drop(reader);
    Ok(())
}

/// The polling fallback: fetch `/board` (and `/usage` when the cross-window
/// 12s schedule is due) at the poll cadence until `window` elapses, then
/// return so the caller retries `/sse`. The window check runs BEFORE each
/// fetch, and the final sleep is never truncated to the window edge, so
/// window boundaries can't issue back-to-back `/board` fetches (the window
/// overshoots by at most one poll interval instead). Returns `Err` only when
/// the store entity is gone.
async fn poll_window(
    http_client: &Arc<dyn HttpClient>,
    this: &WeakEntity<BridgeStore>,
    cx: &mut AsyncApp,
    window: Duration,
    last_usage_fetch: &mut Option<Instant>,
) -> Result<(), ()> {
    let mut elapsed = Duration::ZERO;
    loop {
        if elapsed >= window {
            return Ok(());
        }
        let fetch_usage = usage_due(*last_usage_fetch, Instant::now());
        let client = http_client.clone();
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
        this.update(cx, |store, cx| match fetched {
            Ok((board, usage)) => {
                store.apply_event(BridgeEvent::Board { board }, cx);
                if let Some((usage, meta)) = usage {
                    let mut changed = false;
                    if store.usage != usage {
                        store.usage = usage;
                        changed = true;
                    }
                    if store.usage_meta != meta {
                        store.usage_meta = meta;
                        changed = true;
                    }
                    if changed {
                        cx.notify();
                    }
                }
            }
            // Bridge offline: keep the last snapshot, flip the flag.
            Err(_) => store.set_connected(false, cx),
        })
        .map_err(|_| ())?;

        // Bank the 12s schedule on success; reset it on failure so a
        // reconnect refetches usage at once.
        if !ok {
            *last_usage_fetch = None;
        } else if fetch_usage {
            *last_usage_fetch = Some(Instant::now());
        }
        let delay = if ok {
            BOARD_POLL_INTERVAL
        } else {
            OFFLINE_RETRY_INTERVAL
        };
        cx.background_executor().timer(delay).await;
        elapsed += delay;
    }
}

pub async fn fetch_json(client: &dyn HttpClient, url: &str) -> Result<String> {
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
    #[derive(Default, serde::Deserialize)]
    struct BoardResponse {
        #[serde(default)]
        board: Vec<RunRow>,
    }
    let response: BoardResponse = serde_json::from_str(raw)?;
    Ok(response.board)
}

fn parse_usage(raw: &str) -> Result<(Vec<PoolRow>, UsageMeta)> {
    let value: serde_json::Value = serde_json::from_str(raw)?;
    let object = value
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("usage: expected a JSON object"))?;
    Ok((pools_from_object(object), usage_meta_from_object(object)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_schedule_is_12s_and_resets_on_none() {
        let now = Instant::now();
        assert!(usage_due(None, now), "no prior fetch (or a failure) = due");
        assert!(!usage_due(Some(now), now));
        assert!(
            !usage_due(Some(now), now + Duration::from_secs(11)),
            "11s in: not due — the old per-window tick reset fired here"
        );
        assert!(usage_due(Some(now), now + Duration::from_secs(12)));
    }

    #[test]
    fn board_endpoint_fixture_parses() {
        let board = parse_board(r#"{"board": [{"run_id": "r-1", "agent": "claude"}]}"#).unwrap();
        assert_eq!(board.len(), 1);
        assert_eq!(board[0].run_id, "r-1");
        assert_eq!(parse_board("{}").unwrap(), Vec::new());
    }

    #[test]
    fn usage_endpoint_fixture_parses() {
        let (usage, meta) = parse_usage(
            r#"{
                "_meta": { "updated_at": "2026-06-12T03:14:00Z" },
                "_source": "self-metering",
                "claude": { "headroom_pct": 62.5, "window": "5h" },
                "codex": { "headroom_pct": null }
            }"#,
        )
        .unwrap();
        assert_eq!(usage.len(), 2);
        assert_eq!(usage[0].name, "claude");
        assert_eq!(usage[0].headroom_pct, Some(62.5));
        assert_eq!(usage[1].headroom_pct, None);
        assert_eq!(meta.source.as_deref(), Some("self-metering"));
        assert!(parse_usage("[1, 2, 3]").is_err());
    }
}
