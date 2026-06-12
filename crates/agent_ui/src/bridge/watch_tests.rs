//! End-to-end tests for the watch-gated `/transcript` poll: a fake bridge +
//! fake clock drive the REAL [`super::client::transcript_poll_loop`] through
//! [`BridgeStore::watch_transcript`], proving the timer genuinely runs while
//! a watch is held and genuinely DIES once the last watch drops (Z3 fix #2 —
//! the previous refcount test never drove the loop to its zero-watcher
//! exit, so "poll task dead" was only ever verified by code reading).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use gpui::{AppContext as _, TestAppContext};
use http_client::{FakeHttpClient, Response};

use super::store::BridgeStore;

/// A fake bridge that serves an idle transcript and counts `/transcript`
/// hits.
fn fake_bridge(cx: &mut TestAppContext) -> Arc<AtomicUsize> {
    let transcript_hits = Arc::new(AtomicUsize::new(0));
    let hits = transcript_hits.clone();
    let http_client = FakeHttpClient::create(move |request| {
        let hits = hits.clone();
        async move {
            if request.uri().path() == "/transcript" {
                hits.fetch_add(1, Ordering::SeqCst);
            }
            Ok(Response::builder()
                .status(200)
                .body(r#"{"id":"c-1","title":"t","transcript":[],"busy":false}"#.into())
                .unwrap())
        }
    });
    cx.update(|cx| cx.set_http_client(http_client));
    transcript_hits
}

#[gpui::test]
async fn transcript_poll_timer_runs_while_watched_and_dies_after_the_last_drop(
    cx: &mut TestAppContext,
) {
    let transcript_hits = fake_bridge(cx);
    let store = cx.new(|_| BridgeStore::default());

    // 0→1: the watch spawns the poll loop, whose first iteration fetches
    // immediately.
    let watch = store.update(cx, |store, cx| store.watch_transcript(cx));
    cx.run_until_parked();
    assert_eq!(
        transcript_hits.load(Ordering::SeqCst),
        1,
        "acquiring the watch fetches at once"
    );

    // The idle cadence (2.5s) keeps ticking while the watch is held.
    cx.executor().advance_clock(Duration::from_millis(2600));
    cx.run_until_parked();
    let while_watched = transcript_hits.load(Ordering::SeqCst);
    assert!(
        while_watched >= 2,
        "the idle cadence must keep fetching while watched (got {while_watched})"
    );

    // The last watch drops: the loop's next wake sees zero watchers and
    // exits WITHOUT fetching — the timer is dead, not just quiet.
    drop(watch);
    let at_drop = transcript_hits.load(Ordering::SeqCst);
    cx.executor().advance_clock(Duration::from_secs(60));
    cx.run_until_parked();
    assert_eq!(
        transcript_hits.load(Ordering::SeqCst),
        at_drop,
        "no `/transcript` fetches after the last watch drops — the poll \
         task must be dead, not idling"
    );
}

#[gpui::test]
async fn rewatching_respawns_the_drained_poll(cx: &mut TestAppContext) {
    let transcript_hits = fake_bridge(cx);
    let store = cx.new(|_| BridgeStore::default());

    let watch = store.update(cx, |store, cx| store.watch_transcript(cx));
    cx.run_until_parked();
    drop(watch);
    cx.executor().advance_clock(Duration::from_secs(10));
    cx.run_until_parked();
    let drained = transcript_hits.load(Ordering::SeqCst);

    // A fresh watch (panel re-activated) restarts the poll: re-activation
    // doubles as the fresh fetch.
    let rewatch = store.update(cx, |store, cx| store.watch_transcript(cx));
    cx.run_until_parked();
    assert_eq!(
        transcript_hits.load(Ordering::SeqCst),
        drained + 1,
        "a fresh watch must respawn the loop and fetch immediately"
    );
    drop(rewatch);
}
