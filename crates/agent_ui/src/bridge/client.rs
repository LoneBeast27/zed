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
    BridgeEvent, PlanSnapshot, PoolRow, ProjectRow, RunRow, TranscriptSnapshot, UsageMeta,
    pools_from_object, usage_meta_from_object,
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
/// `/transcript` cadence while the orchestrator is busy (the web's 900ms
/// busy poll — SSE carries only board/usage today; transcript-over-SSE is
/// bridge-side work, deferred).
const TRANSCRIPT_POLL_BUSY: Duration = Duration::from_millis(900);
/// `/transcript` cadence while idle (web 2.5s).
const TRANSCRIPT_POLL_IDLE: Duration = Duration::from_millis(2500);
/// `/plan` poll cadence — the symphony check-off fallback when SSE isn't
/// feeding the plan event (matches the web symphony `/events` 2s loop).
const PLAN_POLL_INTERVAL: Duration = Duration::from_millis(2000);
/// SSE reconnect backoff: start here…
const SSE_BACKOFF_START: Duration = Duration::from_secs(1);
/// …grow by this much per failed attach…
const SSE_BACKOFF_STEP: Duration = Duration::from_secs(1);
/// …capped here.
const SSE_BACKOFF_CAP: Duration = Duration::from_secs(5);

/// Boot the bridge if nothing answers on 4530 (2026-07-10: the app was
/// CONNECT-ONLY — every session silently depended on a manually-started
/// `python -m bridge.serve`, and the day one wasn't running every surface
/// sat offline). One spawn attempt per app run: detached, fire-and-forget —
/// the connection loop's normal backoff picks it up as it comes alive. The
/// bridge repo comes from `AGENTIC_BRIDGE_ROOT` (dev-box default compiled
/// in); a missing python/repo degrades to today's behavior (offline poll +
/// honest offline UI), logged.
fn ensure_bridge_process() {
    let root = std::env::var("AGENTIC_BRIDGE_ROOT")
        .unwrap_or_else(|_| r"L:\Projects\agentic-ide".to_string());
    if !std::path::Path::new(&root).join("bridge").is_dir() {
        log::warn!("bridge autostart: no bridge/ under {root} — skipping spawn");
        return;
    }
    let mut cmd = std::process::Command::new("python");
    cmd.args(["-m", "bridge.serve"])
        .current_dir(&root)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt as _;
        // DETACHED_PROCESS | CREATE_NO_WINDOW: outlives the app, no console.
        cmd.creation_flags(0x0000_0008 | 0x0800_0000);
    }
    match cmd.spawn() {
        Ok(child) => log::info!("bridge autostart: spawned pid {} in {root}", child.id()),
        Err(err) => log::warn!("bridge autostart failed ({root}): {err}"),
    }
}

pub(super) async fn connection_loop(
    http_client: Arc<dyn HttpClient>,
    this: WeakEntity<BridgeStore>,
    cx: &mut AsyncApp,
) {
    let mut backoff = SSE_BACKOFF_START;
    // The 12s usage schedule (None = due immediately) — owned here so it
    // spans backoff windows instead of resetting with each one.
    let mut last_usage_fetch: Option<Instant> = None;
    // Bridge autostart: one probe, one spawn attempt, before the loop. A
    // healthy bridge answers and nothing is spawned.
    {
        let client = http_client.clone();
        let alive = cx
            .background_spawn(async move { connect_sse(client.as_ref()).await })
            .await
            .is_ok();
        if !alive {
            ensure_bridge_process();
        }
    }
    loop {
        let client = http_client.clone();
        let attached = cx
            .background_spawn(async move { connect_sse(client.as_ref()).await })
            .await;
        match attached {
            Ok(response) => {
                backoff = SSE_BACKOFF_START;
                if consume_sse(&http_client, response, &this, cx)
                    .await
                    .is_err()
                {
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
    http_client: &Arc<dyn HttpClient>,
    mut response: Response<AsyncBody>,
    this: &WeakEntity<BridgeStore>,
    cx: &mut AsyncApp,
) -> Result<(), ()> {
    // Attached: the bridge pushes a full board+usage snapshot on connect, so
    // flipping `connected` here never shows stale data for long.
    this.update(cx, |store, cx| {
        store.set_connected(true, cx);
        store.set_transport(super::store::Transport::Sse, cx);
    })
    .map_err(|_| ())?;

    // Finding 2: SSE pushes the `plan` frame only on a CHANGE, so a fresh
    // connect replays no plan — the Symphony panel would sit on "No plans yet"
    // while a live plan already exists bridge-wide. Fetch the bridge default
    // `/plan` ONCE on connect (empty conv = the bridge's most-recent
    // conversation with a plan) so the panel defaults to it immediately; the
    // store's `accepts_plan` fold scopes it (adopted only when nothing is
    // explicitly followed). This is a one-shot on-connect fetch, NOT a poll.
    fetch_and_apply_default_plan(http_client, &this, cx).await.ok();

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
                store.set_transport(super::store::Transport::Polling, cx);
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

/// The transcript poll cadence for a busy flag (PARITY_SPEC §5: 900ms while
/// the orchestrator is busy, 2.5s idle).
pub(super) fn transcript_cadence(busy: bool) -> Duration {
    if busy {
        TRANSCRIPT_POLL_BUSY
    } else {
        TRANSCRIPT_POLL_IDLE
    }
}

/// The `/transcript` poll task — spawned by the store ONLY while at least
/// one chat panel holds a [`super::store::TranscriptWatch`] (Lightness: the
/// poll is refcount-gated by panel presence, never free-running). Exits as
/// soon as the watcher count drops to zero or the store is gone; the next
/// watcher respawns it.
pub(super) async fn transcript_poll_loop(
    http_client: Arc<dyn HttpClient>,
    watchers: Arc<std::sync::atomic::AtomicUsize>,
    this: WeakEntity<BridgeStore>,
    cx: &mut AsyncApp,
) {
    loop {
        if watchers.load(std::sync::atomic::Ordering::SeqCst) == 0 {
            return; // no chat panel alive — pause until the next watch
        }
        let busy = match fetch_and_apply_transcript(&http_client, &this, cx).await {
            Err(()) => return, // store dropped
            Ok(busy) => busy,
        };
        // Fetch failure (bridge restarting) keeps the last representation
        // and retries at the idle cadence — the web's silent catch.
        cx.background_executor()
            .timer(transcript_cadence(busy.unwrap_or(false)))
            .await;
    }
}

/// One `/transcript` fetch applied to the store, fetching `/projects`
/// alongside when the snapshot's conversation can't be resolved to a
/// project name yet (the crumb's `project / title`). Returns the snapshot's
/// busy flag, `Ok(None)` on a fetch failure, and `Err` only when the store
/// entity is gone. Also the one-shot refetch body (post-send, conv switch).
pub(super) async fn fetch_and_apply_transcript(
    http_client: &Arc<dyn HttpClient>,
    this: &WeakEntity<BridgeStore>,
    cx: &mut AsyncApp,
) -> Result<Option<bool>, ()> {
    let conv = this
        .read_with(cx, |store, _| store.transcript_conv.clone())
        .map_err(|_| ())?;
    let client = http_client.clone();
    let fetched = cx
        .background_spawn(async move {
            let url = format!(
                "{BRIDGE_BASE_URL}/transcript?conv={}",
                conv.unwrap_or_default()
            );
            let raw = fetch_json(client.as_ref(), &url).await?;
            parse_transcript(&raw)
        })
        .await;
    let Ok(snapshot) = fetched else {
        return Ok(None);
    };
    let needs_projects = this
        .read_with(cx, |store, _| {
            store.project_name_for_conv(&snapshot.id).is_none()
        })
        .map_err(|_| ())?;
    let projects = if needs_projects {
        let client = http_client.clone();
        cx.background_spawn(async move {
            let raw = fetch_json(client.as_ref(), &format!("{BRIDGE_BASE_URL}/projects")).await?;
            parse_projects(&raw)
        })
        .await
        .ok()
    } else {
        None
    };
    let busy = snapshot.busy;
    this.update(cx, |store, cx| {
        if let Some(projects) = projects {
            store.apply_projects(projects, cx);
        }
        store.apply_transcript(snapshot, cx);
    })
    .map_err(|_| ())?;
    Ok(Some(busy))
}

/// One-shot `/plan` fetch for the bridge DEFAULT plan (empty conv = the
/// bridge's most-recent conversation with a plan), applied through the store's
/// `accepts_plan` fold. Called once on SSE connect (Finding 2) so a fresh app
/// defaults to the live plan instead of "No plans yet" — SSE only pushes plan
/// frames on change, so without this the current plan never arrives until it
/// next mutates. Returns `Err` only when the store entity is gone; a fetch
/// failure is swallowed (the empty state is the correct fallback then).
pub(super) async fn fetch_and_apply_default_plan(
    http_client: &Arc<dyn HttpClient>,
    this: &WeakEntity<BridgeStore>,
    cx: &mut AsyncApp,
) -> Result<(), ()> {
    // Only worth fetching when nothing is explicitly followed — a followed
    // conv gets its plan from the watch-gated poll / the scoped SSE frame.
    let unfollowed = this
        .read_with(cx, |store, _| store.transcript_conv.is_none())
        .map_err(|_| ())?;
    if !unfollowed {
        return Ok(());
    }
    let client = http_client.clone();
    let fetched = cx
        .background_spawn(async move {
            // Empty conv → the bridge default (latest plan across conversations).
            let url = format!("{BRIDGE_BASE_URL}/plan?conv=");
            let raw = fetch_json(client.as_ref(), &url).await?;
            anyhow::Ok(serde_json::from_str::<PlanSnapshot>(&raw)?)
        })
        .await;
    if let Ok(plan) = fetched {
        this.update(cx, |store, cx| store.apply_plan(plan, cx))
            .map_err(|_| ())?;
    }
    Ok(())
}

/// The `/plan` poll task — spawned by the store ONLY while the Symphony panel
/// holds a [`super::store::PlanWatch`] (Lightness: panel-presence-gated, never
/// free-running). SSE feeds the plan event directly when connected; this poll
/// is the polling-fallback path and a belt-and-suspenders refresh. Follows the
/// store's active conversation each cycle; exits when the watcher count hits
/// zero or the store is gone.
pub(super) async fn plan_poll_loop(
    http_client: Arc<dyn HttpClient>,
    watchers: Arc<std::sync::atomic::AtomicUsize>,
    this: WeakEntity<BridgeStore>,
    cx: &mut AsyncApp,
) {
    loop {
        if watchers.load(std::sync::atomic::Ordering::SeqCst) == 0 {
            return; // panel hidden — the next watch respawns the loop
        }
        let conv = match this.read_with(cx, |store, _| store.transcript_conv.clone()) {
            Ok(conv) => conv.unwrap_or_default(),
            Err(_) => return, // store dropped
        };
        let client = http_client.clone();
        let fetched = cx
            .background_spawn(async move {
                let url = format!("{BRIDGE_BASE_URL}/plan?conv={conv}");
                let raw = fetch_json(client.as_ref(), &url).await?;
                anyhow::Ok(serde_json::from_str::<PlanSnapshot>(&raw)?)
            })
            .await;
        // Fetch failure = the web's silent idle catch — keep the last plan and
        // retry next tick.
        if let Ok(plan) = fetched
            && this.update(cx, |store, cx| store.apply_plan(plan, cx)).is_err()
        {
            return; // store dropped
        }
        cx.background_executor().timer(PLAN_POLL_INTERVAL).await;
    }
}

/// POST a JSON body to a bridge endpoint, returning the raw response body
/// (the web's `post()` helper from app.js). Background-executor only, like
/// [`fetch_json`].
pub async fn post_json(client: &dyn HttpClient, url: &str, body: String) -> Result<String> {
    let mut response = client.post_json(url, AsyncBody::from(body)).await?;
    let mut raw = String::new();
    response.body_mut().read_to_string(&mut raw).await?;
    anyhow::ensure!(
        response.status().is_success(),
        "bridge returned {} for {url}",
        response.status().as_u16()
    );
    Ok(raw)
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

fn parse_transcript(raw: &str) -> Result<TranscriptSnapshot> {
    Ok(serde_json::from_str(raw)?)
}

fn parse_projects(raw: &str) -> Result<Vec<ProjectRow>> {
    #[derive(Default, serde::Deserialize)]
    struct ProjectsResponse {
        #[serde(default)]
        projects: Vec<ProjectRow>,
    }
    let response: ProjectsResponse = serde_json::from_str(raw)?;
    Ok(response.projects)
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
    fn transcript_cadence_is_busy_aware() {
        assert_eq!(transcript_cadence(true), TRANSCRIPT_POLL_BUSY);
        assert_eq!(transcript_cadence(false), TRANSCRIPT_POLL_IDLE);
        assert!(transcript_cadence(true) < transcript_cadence(false));
    }

    #[test]
    fn transcript_endpoint_fixture_parses() {
        let snapshot =
            parse_transcript(r#"{"id": "c-1", "title": "t", "transcript": [], "busy": true}"#)
                .unwrap();
        assert_eq!(snapshot.id, "c-1");
        assert!(snapshot.busy);
        // (serde structs also accept JSON sequences, so `[]` parses to all-
        // defaults — a scalar is the genuinely-invalid shape.)
        assert!(parse_transcript("42").is_err());
    }

    #[test]
    fn projects_endpoint_fixture_parses() {
        let projects = parse_projects(
            r#"{"projects": [{"id": "p-1", "name": "default",
                "conversations": [{"id": "c-1", "title": "t"}]}]}"#,
        )
        .unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].name, "default");
        assert_eq!(projects[0].conversations[0].id, "c-1");
        assert_eq!(parse_projects("{}").unwrap(), Vec::new());
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
