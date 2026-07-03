//! The constellation's supplemental data poll.
//!
//! SSE remains the push transport for run/status transitions and channel
//! state (the store's connection loop consumes both), but the bridge's SSE
//! board digest is `[run_id, status]` ONLY — token-mass growth (T4) and the
//! root's `ctx_tokens` ramp never push a frame. Until the bridge digests
//! grow token buckets + a conv/ctx event (reported as a server-side gap;
//! native must not add Python endpoints), this loop polls `/board` +
//! `/channels` + `/projects` at 2s — visibility-gated: it fetches only
//! while the panel rendered recently, and everything funnels through the
//! same change-gated store/sim folds, so a poll frame and an SSE frame are
//! indistinguishable downstream. Decode happens ONCE, on the background
//! executor, into the typed protocol structs.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use gpui::{AppContext as _, AsyncApp, WeakEntity};
use http_client::HttpClient;
use serde::Deserialize;

use crate::bridge::{
    BRIDGE_BASE_URL, BridgeEvent, ChannelRow, OverlapRow, ProjectRow, RunRow, fetch_json,
};

use super::panel::ConstellationPanel;

/// Poll cadence while the panel is visible.
const FEED_INTERVAL: Duration = Duration::from_secs(2);
/// The panel counts as visible if it rendered within this window.
pub(super) const VISIBLE_WINDOW: Duration = Duration::from_secs(3);

/// `GET /channels` response: channel rows + overlap venn rows (the arrange
/// weights). `events` is served too but unused here.
#[derive(Debug, Default, Deserialize)]
struct ChannelsResponse {
    #[serde(default)]
    channels: Vec<ChannelRow>,
    #[serde(default)]
    overlap: Vec<OverlapRow>,
}

#[derive(Debug, Default, Deserialize)]
struct BoardResponse {
    #[serde(default)]
    board: Vec<RunRow>,
}

#[derive(Debug, Default, Deserialize)]
struct ProjectsResponse {
    #[serde(default)]
    projects: Vec<ProjectRow>,
}

struct FeedFrame {
    board: Vec<RunRow>,
    channels: Vec<ChannelRow>,
    overlap: Vec<OverlapRow>,
    projects: Vec<ProjectRow>,
}

async fn fetch_frame(client: &dyn HttpClient) -> Result<FeedFrame> {
    let board: BoardResponse =
        serde_json::from_str(&fetch_json(client, &format!("{BRIDGE_BASE_URL}/board")).await?)?;
    let channels: ChannelsResponse =
        serde_json::from_str(&fetch_json(client, &format!("{BRIDGE_BASE_URL}/channels")).await?)
            .unwrap_or_default(); // older bridge without /channels
    let projects: ProjectsResponse =
        serde_json::from_str(&fetch_json(client, &format!("{BRIDGE_BASE_URL}/projects")).await?)?;
    Ok(FeedFrame {
        board: board.board,
        channels: channels.channels,
        overlap: channels.overlap,
        projects: projects.projects,
    })
}

/// The poll loop — spawned once per panel, exits when the panel drops.
/// Skips fetching entirely while the panel isn't rendering (hidden dock),
/// so an idle workspace costs nothing.
pub(super) async fn feed_loop(
    http_client: Arc<dyn HttpClient>,
    this: WeakEntity<ConstellationPanel>,
    cx: &mut AsyncApp,
) {
    loop {
        let visible = match this.read_with(cx, |panel, _| panel.rendered_recently()) {
            Ok(visible) => visible,
            Err(_) => return, // panel dropped
        };
        if visible {
            let client = http_client.clone();
            let fetched = cx
                .background_spawn(async move { fetch_frame(client.as_ref()).await })
                .await;
            let applied = this.update(cx, |panel, cx| {
                if let Ok(frame) = fetched {
                    // Board + channels + projects flow through the shared
                    // store (change-gated: unchanged frames don't notify);
                    // overlap stays panel-local for arrange time.
                    panel.store().update(cx, |store, cx| {
                        store.apply_event(
                            BridgeEvent::Board { board: frame.board },
                            cx,
                        );
                        store.apply_event(
                            BridgeEvent::Channels {
                                channels: frame.channels,
                            },
                            cx,
                        );
                        store.apply_projects(frame.projects, cx);
                    });
                    panel.set_overlap(frame.overlap);
                }
                // Fetch failure = the web's silent catch: keep the last
                // representation, retry next tick.
            });
            if applied.is_err() {
                return; // panel dropped
            }
        }
        cx.background_executor().timer(FEED_INTERVAL).await;
    }
}
