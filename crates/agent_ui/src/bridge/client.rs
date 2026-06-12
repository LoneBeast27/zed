//! The bridge store entity + its connection task.
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
use gpui::{App, AppContext as _, AsyncApp, Entity, Task, WeakEntity};
use http_client::{AsyncBody, HttpClient, Response};

use super::protocol::{BridgeEvent, PoolRow, RunRow, pools_from_object};
use super::sse::SseParser;

pub const BRIDGE_BASE_URL: &str = "http://localhost:4530";

/// `/board` cadence while in polling fallback (matches the web bridge poll).
const BOARD_POLL_INTERVAL: Duration = Duration::from_millis(1500);
/// `/usage` rides every Nth fallback board tick: 8 × 1500ms = 12s.
const USAGE_POLL_TICKS: u64 = 8;
/// When the bridge is fully offline, back off to slow poll retries.
const OFFLINE_RETRY_INTERVAL: Duration = Duration::from_secs(5);
/// SSE reconnect backoff: start here…
const SSE_BACKOFF_START: Duration = Duration::from_secs(1);
/// …grow by this much per failed attach…
const SSE_BACKOFF_STEP: Duration = Duration::from_secs(1);
/// …capped here.
const SSE_BACKOFF_CAP: Duration = Duration::from_secs(5);
/// Local elapsed-tick cadence while any run is `running`. serve.py's board
/// digest ignores `elapsed_s` (clients tick elapsed locally) — this is the 1s
/// worked-for ticker RUST_PORT_NOTES §5 explicitly authorizes.
const ELAPSED_TICK: Duration = Duration::from_secs(1);

/// The bridge snapshot Z1+ panels render from. `connected == false` means the
/// bridge is offline — the last snapshot is kept so panels can grey-out rather
/// than blank.
pub struct BridgeStore {
    pub board: Vec<RunRow>,
    pub usage: Vec<PoolRow>,
    pub connected: bool,
    /// When the current board snapshot arrived. The server value re-bases the
    /// local tick offset on every board frame; running runs render
    /// `elapsed_s + (now − board_received_at)`.
    board_received_at: Instant,
    /// The 1s ticker task — `Some` only while any run is `running`
    /// (Lightness Mandate: no idle timers).
    ticker: Option<Task<()>>,
}

impl Default for BridgeStore {
    fn default() -> Self {
        Self {
            board: Vec::new(),
            usage: Vec::new(),
            connected: false,
            board_received_at: Instant::now(),
            ticker: None,
        }
    }
}

/// Adds the local tick offset to every `running` run's `elapsed_s` — the
/// re-base math of the 1s ticker (settled runs keep the server value).
fn tick_elapsed(board: &mut [RunRow], offset_s: f64) {
    for run in board {
        if run.status == "running" {
            run.elapsed_s += offset_s;
        }
    }
}

impl BridgeStore {
    /// The board with locally-ticked elapsed: the server pushes a frame only
    /// when the run/status digest changes, so between frames running runs
    /// tick `elapsed_s` forward from the snapshot's arrival time.
    pub fn ticked_board(&self) -> Vec<RunRow> {
        let mut board = self.board.clone();
        tick_elapsed(&mut board, self.board_received_at.elapsed().as_secs_f64());
        board
    }

    fn apply_event(&mut self, event: BridgeEvent, cx: &mut gpui::Context<Self>) {
        let mut changed = !self.connected;
        self.connected = true;
        match event {
            BridgeEvent::Board { board } => {
                // The server's elapsed_s is authoritative at receive time —
                // re-base the local tick even when the rows are unchanged.
                self.board_received_at = Instant::now();
                if self.board != board {
                    self.board = board;
                    changed = true;
                }
                self.update_ticker(cx);
            }
            BridgeEvent::Usage { fields } => {
                let usage = pools_from_object(&fields);
                if self.usage != usage {
                    self.usage = usage;
                    changed = true;
                }
            }
            BridgeEvent::Unknown => {}
        }
        if changed {
            cx.notify();
        }
    }

    /// Starts the 1s ticker when a run is live, drops it when none is. Every
    /// board mutation funnels through [`Self::apply_event`], so this is the
    /// single start/stop point; the task also self-clears as a backstop.
    fn update_ticker(&mut self, cx: &mut gpui::Context<Self>) {
        let any_running = self.board.iter().any(|run| run.status == "running");
        if !any_running {
            self.ticker = None;
        } else if self.ticker.is_none() {
            self.ticker = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(ELAPSED_TICK).await;
                    let live = this.update(cx, |store, cx| {
                        let live = store.board.iter().any(|run| run.status == "running");
                        if live {
                            cx.notify();
                        } else {
                            store.ticker = None;
                        }
                        live
                    });
                    if !matches!(live, Ok(true)) {
                        return;
                    }
                }
            }));
        }
    }

    fn set_connected(&mut self, connected: bool, cx: &mut gpui::Context<Self>) {
        if self.connected != connected {
            self.connected = connected;
            cx.notify();
        }
    }
}

struct GlobalBridgeStore(Entity<BridgeStore>);

impl gpui::Global for GlobalBridgeStore {}

/// The app-wide shared store. Created lazily on first access — the bridge
/// connection task starts non-blocking when the first consumer (a Z1+ panel)
/// builds, per the Lightness Mandate's lazy-startup rule.
pub fn global_store(cx: &mut App) -> Entity<BridgeStore> {
    if let Some(global) = cx.try_global::<GlobalBridgeStore>() {
        return global.0.clone();
    }
    let store = init(cx);
    cx.set_global(GlobalBridgeStore(store.clone()));
    store
}

/// Creates the store entity and detaches the connection loop. The loop exits
/// on its own once the entity is dropped (weak-handle update failure).
pub fn init(cx: &mut App) -> Entity<BridgeStore> {
    let http_client = cx.http_client();
    cx.new(|cx| {
        cx.spawn(async move |this, cx| connection_loop(http_client, this, cx).await)
            .detach();
        BridgeStore::default()
    })
}

async fn connection_loop(
    http_client: Arc<dyn HttpClient>,
    this: WeakEntity<BridgeStore>,
    cx: &mut AsyncApp,
) {
    let mut backoff = SSE_BACKOFF_START;
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
                if poll_window(&http_client, &this, cx, backoff).await.is_err() {
                    return; // store dropped
                }
                backoff = (backoff + SSE_BACKOFF_STEP).min(SSE_BACKOFF_CAP);
            }
        }
    }
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

/// The polling fallback: fetch `/board` (and `/usage` every Nth tick) at the
/// poll cadence until `window` elapses, then return so the caller retries
/// `/sse`. Returns `Err` only when the store entity is gone.
async fn poll_window(
    http_client: &Arc<dyn HttpClient>,
    this: &WeakEntity<BridgeStore>,
    cx: &mut AsyncApp,
    window: Duration,
) -> Result<(), ()> {
    let mut elapsed = Duration::ZERO;
    let mut tick: u64 = 0;
    loop {
        let fetch_usage = tick % USAGE_POLL_TICKS == 0;
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
                if let Some(usage) = usage
                    && store.usage != usage
                {
                    store.usage = usage;
                    cx.notify();
                }
            }
            // Bridge offline: keep the last snapshot, flip the flag.
            Err(_) => store.set_connected(false, cx),
        })
        .map_err(|_| ())?;

        // Reset the usage schedule on failure so reconnect refetches at once.
        tick = if ok { tick.wrapping_add(1) } else { 0 };
        let delay = if ok {
            BOARD_POLL_INTERVAL
        } else {
            OFFLINE_RETRY_INTERVAL
        };
        if elapsed >= window {
            return Ok(());
        }
        let delay = delay.min(window - elapsed);
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

fn parse_usage(raw: &str) -> Result<Vec<PoolRow>> {
    let value: serde_json::Value = serde_json::from_str(raw)?;
    let object = value
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("usage: expected a JSON object"))?;
    Ok(pools_from_object(object))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(id: &str, status: &str, elapsed_s: f64) -> RunRow {
        RunRow {
            run_id: id.to_string(),
            status: status.to_string(),
            elapsed_s,
            ..Default::default()
        }
    }

    #[test]
    fn tick_elapsed_offsets_running_runs_only() {
        let mut board = vec![
            run("r-1", "running", 30.0),
            run("r-2", "completed", 300.0),
            run("r-3", "pending", 0.0),
        ];
        tick_elapsed(&mut board, 4.5);
        assert_eq!(board[0].elapsed_s, 34.5);
        assert_eq!(board[1].elapsed_s, 300.0); // settled runs keep server truth
        assert_eq!(board[2].elapsed_s, 0.0);
    }

    #[gpui::test]
    fn board_frame_rebases_local_tick(cx: &mut gpui::TestAppContext) {
        let store = cx.new(|_| BridgeStore::default());
        store.update(cx, |store, cx| {
            // Pretend the last snapshot is 100s old…
            store.board_received_at = Instant::now()
                .checked_sub(Duration::from_secs(100))
                .unwrap_or_else(Instant::now);
            // …then a server frame arrives: the offset re-bases to ~0, so the
            // ticked board shows the server value, not server + stale offset.
            store.apply_event(
                BridgeEvent::Board {
                    board: vec![run("r-1", "running", 5.0)],
                },
                cx,
            );
            let ticked = store.ticked_board();
            assert!(
                (5.0..6.0).contains(&ticked[0].elapsed_s),
                "expected re-based elapsed ≈ 5s, got {}",
                ticked[0].elapsed_s
            );

            // An identical frame (digest unchanged) still re-bases.
            store.board_received_at = Instant::now()
                .checked_sub(Duration::from_secs(100))
                .unwrap_or_else(Instant::now);
            store.apply_event(
                BridgeEvent::Board {
                    board: vec![run("r-1", "running", 5.0)],
                },
                cx,
            );
            let ticked = store.ticked_board();
            assert!(
                (5.0..6.0).contains(&ticked[0].elapsed_s),
                "identical frame must still re-base, got {}",
                ticked[0].elapsed_s
            );
        });
    }

    #[gpui::test]
    fn ticker_lives_only_while_a_run_is_running(cx: &mut gpui::TestAppContext) {
        let store = cx.new(|_| BridgeStore::default());
        store.update(cx, |store, cx| {
            assert!(store.ticker.is_none(), "no idle timers before any board");
            store.apply_event(
                BridgeEvent::Board {
                    board: vec![run("r-1", "running", 1.0)],
                },
                cx,
            );
            assert!(store.ticker.is_some(), "ticker starts with a live run");
            store.apply_event(
                BridgeEvent::Board {
                    board: vec![run("r-1", "completed", 9.0)],
                },
                cx,
            );
            assert!(store.ticker.is_none(), "ticker pauses when no run is live");
            store.apply_event(
                BridgeEvent::Board {
                    board: vec![run("r-1", "completed", 9.0), run("r-2", "running", 0.0)],
                },
                cx,
            );
            assert!(store.ticker.is_some(), "ticker restarts on a new live run");
        });
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
        let usage = parse_usage(
            r#"{
                "_meta": { "updated_at": "2026-06-12T03:14:00Z" },
                "claude": { "headroom_pct": 62.5, "window": "5h" },
                "codex": { "headroom_pct": null }
            }"#,
        )
        .unwrap();
        assert_eq!(usage.len(), 2);
        assert_eq!(usage[0].name, "claude");
        assert_eq!(usage[0].headroom_pct, Some(62.5));
        assert_eq!(usage[1].headroom_pct, None);
        assert!(parse_usage("[1, 2, 3]").is_err());
    }
}
