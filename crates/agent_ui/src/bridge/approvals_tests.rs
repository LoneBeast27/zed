//! Tests for the pending-approval transport (wave 3a): the watch-gated
//! `/approvals` poll driven against the REAL loop (fake bridge + fake clock,
//! the `watch_tests.rs` idiom), the replace-semantics apply, and the
//! allow/deny POST helpers' exact bodies + error-body pass-through.

use std::time::Duration;

use futures::AsyncReadExt as _;
use gpui::TestAppContext;
use http_client::{FakeHttpClient, Method, Response};

use super::*;

/// A fake bridge that serves one pending approval and counts `/approvals`
/// hits.
fn fake_approvals_bridge(cx: &mut TestAppContext) -> Arc<AtomicUsize> {
    let approvals_hits = Arc::new(AtomicUsize::new(0));
    let hits = approvals_hits.clone();
    let http_client = FakeHttpClient::create(move |request| {
        let hits = hits.clone();
        async move {
            let body = if request.uri().path() == "/approvals" {
                hits.fetch_add(1, Ordering::SeqCst);
                r#"{"pending": [{"id": "ap_1", "run_id": "codex-9", "agent": "codex",
                    "task_head": "deploy", "rule": "ask: agent=*"}]}"#
            } else {
                "{}"
            };
            Ok(Response::builder().status(200).body(body.into()).unwrap())
        }
    });
    cx.update(|cx| cx.set_http_client(http_client));
    approvals_hits
}

#[gpui::test]
fn approval_watch_refcount_gates_the_poll(cx: &mut TestAppContext) {
    let _hits = fake_approvals_bridge(cx);
    let store = cx.new(|_| BridgeStore::default());
    store.update(cx, |store, cx| {
        assert!(
            store.approval_task.is_none(),
            "no poll task before any watcher"
        );
        let first = store.watch_approvals(cx);
        assert_eq!(store.approval_watchers.load(Ordering::SeqCst), 1);
        assert!(store.approval_task.is_some(), "0→1 spawns the poll");

        let second = store.watch_approvals(cx);
        assert_eq!(store.approval_watchers.load(Ordering::SeqCst), 2);

        drop(first);
        assert_eq!(store.approval_watchers.load(Ordering::SeqCst), 1);
        drop(second);
        assert_eq!(
            store.approval_watchers.load(Ordering::SeqCst),
            0,
            "dropping the last watch pauses the poll (loop exits on next wake)"
        );

        // A fresh watch respawns (replacing any drained task).
        let third = store.watch_approvals(cx);
        assert_eq!(store.approval_watchers.load(Ordering::SeqCst), 1);
        assert!(store.approval_task.is_some());
        drop(third);
    });
}

#[gpui::test]
async fn approvals_poll_runs_while_watched_and_dies_after_the_last_drop(cx: &mut TestAppContext) {
    let hits = fake_approvals_bridge(cx);
    let store = cx.new(|_| BridgeStore::default());

    // 0→1: the watch spawns the poll loop, whose first iteration fetches
    // immediately and lands the pending row on the store.
    let watch = store.update(cx, |store, cx| store.watch_approvals(cx));
    cx.run_until_parked();
    assert_eq!(
        hits.load(Ordering::SeqCst),
        1,
        "acquiring the watch fetches at once"
    );
    store.read_with(cx, |store, _| {
        assert_eq!(store.pending_approvals.len(), 1);
        assert_eq!(store.pending_approvals[0].id, "ap_1");
        assert_eq!(store.pending_approvals[0].agent, "codex");
        assert_eq!(store.pending_approvals[0].rule, "ask: agent=*");
    });

    // The 1.5s cadence keeps ticking while the watch is held.
    cx.executor().advance_clock(Duration::from_millis(1600));
    cx.run_until_parked();
    let while_watched = hits.load(Ordering::SeqCst);
    assert!(
        while_watched >= 2,
        "the 1.5s cadence must keep fetching while watched (got {while_watched})"
    );

    // The last watch drops: the loop's next wake sees zero watchers and
    // exits WITHOUT fetching — the timer is dead, not just quiet.
    drop(watch);
    let at_drop = hits.load(Ordering::SeqCst);
    cx.executor().advance_clock(Duration::from_secs(60));
    cx.run_until_parked();
    assert_eq!(
        hits.load(Ordering::SeqCst),
        at_drop,
        "no `/approvals` fetches after the last watch drops — the poll \
         task must be dead, not idling"
    );

    // Re-watch (toast re-held) respawns the loop and fetches immediately.
    let rewatch = store.update(cx, |store, cx| store.watch_approvals(cx));
    cx.run_until_parked();
    assert_eq!(
        hits.load(Ordering::SeqCst),
        at_drop + 1,
        "a fresh watch must respawn the loop and fetch immediately"
    );
    drop(rewatch);
}

#[test]
fn approvals_endpoint_fixture_parses() {
    let pending = parse_approvals(
        r#"{"pending": [{"id": "ap_1", "agent": "claude"},
                        {"id": "ap_2", "agent": "codex"}]}"#,
    )
    .unwrap();
    assert_eq!(pending.len(), 2);
    assert_eq!(pending[0].id, "ap_1");
    assert_eq!(pending[1].agent, "codex");
    // Liberal: a bare object is the empty set; a scalar is genuinely invalid.
    assert_eq!(parse_approvals("{}").unwrap(), Vec::new());
    assert!(parse_approvals("42").is_err());
}

#[gpui::test]
async fn approval_decision_posts_the_exact_body(_cx: &mut TestAppContext) {
    // Captured (method, path, body) per request — the POST-body pin.
    let seen: Arc<std::sync::Mutex<Vec<(Method, String, String)>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let log = seen.clone();
    let client = FakeHttpClient::create(move |mut request| {
        let log = log.clone();
        async move {
            let mut body = String::new();
            request.body_mut().read_to_string(&mut body).await.unwrap();
            log.lock().unwrap().push((
                request.method().clone(),
                request.uri().path().to_string(),
                body,
            ));
            Ok(Response::builder()
                .status(200)
                .body(r#"{"id": "ap_1", "decision": "allow"}"#.into())
                .unwrap())
        }
    });

    post_approval_allow(client.as_ref(), "ap_1", None)
        .await
        .unwrap();
    post_approval_allow(client.as_ref(), "ap_1", Some("go"))
        .await
        .unwrap();
    post_approval_deny(client.as_ref(), "ap_2", Some("not in this cwd"))
        .await
        .unwrap();

    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 3);
    assert_eq!(seen[0].0, Method::POST);
    assert_eq!(seen[0].1, "/approvals/ap_1/allow");
    assert_eq!(seen[0].2, "{}", "a bare allow posts an empty object");
    assert_eq!(seen[1].1, "/approvals/ap_1/allow");
    assert_eq!(seen[1].2, r#"{"note":"go"}"#);
    assert_eq!(seen[2].0, Method::POST);
    assert_eq!(seen[2].1, "/approvals/ap_2/deny");
    assert_eq!(seen[2].2, r#"{"note":"not in this cwd"}"#);
}

#[gpui::test]
async fn approval_decision_surfaces_the_error_body_verbatim(_cx: &mut TestAppContext) {
    // The bridge's 409 refusal must read as a refusal, not offline — the
    // {"error"} body VERBATIM (the post_json law).
    let client = FakeHttpClient::create(|_| async {
        Ok(Response::builder()
            .status(409)
            .body(r#"{"error": "already resolved: deny"}"#.into())
            .unwrap())
    });
    let error = post_approval_allow(client.as_ref(), "ap_1", None)
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "already resolved: deny");

    // A body-less refusal degrades to the honest status line.
    let client = FakeHttpClient::create(|_| async {
        Ok(Response::builder().status(404).body("".into()).unwrap())
    });
    let error = post_approval_deny(client.as_ref(), "ap_404", Some("n"))
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "bridge returned 404");
}
