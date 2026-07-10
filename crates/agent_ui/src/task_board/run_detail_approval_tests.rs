//! Tests for the drawer's §5.2 approval slice: the live-status vocabulary
//! (the poll must outlast an `awaiting_*` hold), the pending-row → drawer
//! match, the decision POSTs (URL + note body), the in-flight collapse, and
//! the `{"error"}`-verbatim refusal path (a 409 reads as a refusal, not
//! offline). Sibling file (the `#[path]` idiom) — `run_detail.rs` is at the
//! line ceiling.

use super::*;
use crate::bridge::ApprovalRow;
use crate::task_board::run_detail_approval::pending_for_run;

#[test]
fn live_status_includes_the_awaiting_family() {
    // Phase-2 §5.2: awaiting is non-terminal — the drawer keeps tailing.
    assert!(is_live_status("running"));
    assert!(is_live_status("awaiting_approval"));
    assert!(is_live_status("awaiting_a"), "channel-grammar prefix matches");
    assert!(!is_live_status("completed"));
    assert!(!is_live_status("failed"));
    assert!(!is_live_status("killed"));
    assert!(!is_live_status("pending"));
}

#[test]
fn pending_for_run_matches_on_run_id() {
    let pending = vec![
        ApprovalRow {
            id: "ap_1".into(),
            run_id: "r-1".into(),
            ..Default::default()
        },
        ApprovalRow {
            id: "ap_2".into(),
            run_id: "r-2".into(),
            ..Default::default()
        },
    ];
    assert_eq!(pending_for_run(&pending, "r-2").map(|r| r.id.as_str()), Some("ap_2"));
    assert_eq!(pending_for_run(&pending, "r-9"), None);
    assert_eq!(pending_for_run(&[], "r-1"), None);
}

#[gpui::test]
async fn drawer_keeps_tailing_an_awaiting_run(cx: &mut gpui::TestAppContext) {
    use http_client::{FakeHttpClient, Response};
    use std::sync::atomic::{AtomicUsize, Ordering};

    // The regression this pins: with the old `status == "running"` gate the
    // poll STOPPED at the first awaiting frame — the drawer would freeze on
    // the held state and never land the allow/deny transition.
    let fetches = std::sync::Arc::new(AtomicUsize::new(0));
    let counter = fetches.clone();
    let http_client = FakeHttpClient::create(move |_request| {
        let counter = counter.clone();
        async move {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok(Response::builder()
                .status(200)
                .body(r#"{"run_id": "r-1", "status": "awaiting_approval"}"#.into())
                .unwrap())
        }
    });
    cx.update(|cx| cx.set_http_client(http_client));

    let drawer = cx.new(|cx| RunDrawer::new("r-1".into(), cx));
    cx.run_until_parked();
    drawer.read_with(cx, |d, _| {
        assert_eq!(
            d.detail.as_ref().map(|x| x.status.as_str()),
            Some("awaiting_approval")
        );
    });
    let after_first = fetches.load(Ordering::SeqCst);
    assert!(after_first >= 1);

    // Advance past one poll tick: the tail poll must still be alive.
    cx.executor().advance_clock(LOG_POLL);
    cx.run_until_parked();
    assert!(
        fetches.load(Ordering::SeqCst) > after_first,
        "the tail poll must outlast the awaiting hold (§5.2 non-terminal)"
    );
}

#[gpui::test]
async fn decide_posts_the_allow_endpoint_and_collapses(cx: &mut gpui::TestAppContext) {
    use http_client::{FakeHttpClient, Method, Response};
    use std::sync::{Arc, Mutex};

    let posts: Arc<Mutex<Vec<(String, String)>>> = Arc::default();
    let seen = posts.clone();
    let http_client = FakeHttpClient::create(move |mut request| {
        let seen = seen.clone();
        async move {
            if request.method() == Method::POST {
                let mut body = String::new();
                use futures::AsyncReadExt as _;
                request.body_mut().read_to_string(&mut body).await.ok();
                seen.lock()
                    .unwrap()
                    .push((request.uri().path().to_string(), body));
                return Ok(Response::builder()
                    .status(200)
                    .body(r#"{"id": "ap_1", "decision": "allow"}"#.into())
                    .unwrap());
            }
            Ok(Response::builder()
                .status(200)
                .body(r#"{"run_id": "r-1", "status": "awaiting_approval"}"#.into())
                .unwrap())
        }
    });
    cx.update(|cx| cx.set_http_client(http_client));

    let drawer = cx.new(|cx| RunDrawer::new("r-1".into(), cx));
    cx.run_until_parked();
    drawer.update(cx, |d, cx| d.decide_approval("ap_1".into(), true, cx));
    drawer.read_with(cx, |d, _| {
        assert_eq!(
            d.deciding_approval.as_deref(),
            Some("ap_1"),
            "in-flight collapse marks the id immediately"
        );
    });
    cx.run_until_parked();
    {
        let posts = posts.lock().unwrap();
        assert_eq!(posts.len(), 1);
        assert_eq!(posts[0].0, "/approvals/ap_1/allow");
        assert_eq!(posts[0].1, "{}", "no note on allow");
    }
    drawer.read_with(cx, |d, _| {
        assert_eq!(
            d.deciding_approval.as_deref(),
            Some("ap_1"),
            "success stays collapsed — server truth retires the section"
        );
        assert_eq!(d.action_error, None);
    });

    // A second decide while one is in flight/banked is a no-op (one POST
    // at a time).
    drawer.update(cx, |d, cx| d.decide_approval("ap_1".into(), false, cx));
    cx.run_until_parked();
    assert_eq!(posts.lock().unwrap().len(), 1, "in-flight collapse holds");
}

#[gpui::test]
async fn deny_refusal_lands_the_bridge_error_verbatim(cx: &mut gpui::TestAppContext) {
    use http_client::{FakeHttpClient, Method, Response};

    // The 409 already-resolved race: the refusal body must surface VERBATIM
    // and the buttons must re-arm (deciding cleared) for honest retry.
    let http_client = FakeHttpClient::create(move |request| async move {
        if request.method() == Method::POST {
            return Ok(Response::builder()
                .status(409)
                .body(r#"{"error": "already resolved: deny"}"#.into())
                .unwrap());
        }
        Ok(Response::builder()
            .status(200)
            .body(r#"{"run_id": "r-1", "status": "awaiting_approval"}"#.into())
            .unwrap())
    });
    cx.update(|cx| cx.set_http_client(http_client));

    let drawer = cx.new(|cx| RunDrawer::new("r-1".into(), cx));
    cx.run_until_parked();
    drawer.update(cx, |d, cx| d.decide_approval("ap_9".into(), false, cx));
    cx.run_until_parked();
    drawer.read_with(cx, |d, _| {
        assert_eq!(
            d.action_error.as_deref(),
            Some("already resolved: deny"),
            "the bridge's {{\"error\"}} body VERBATIM — never a bare status code"
        );
        assert_eq!(d.deciding_approval, None, "refusal re-arms the buttons");
    });
}

#[gpui::test]
async fn local_drawers_never_decide(cx: &mut gpui::TestAppContext) {
    // No FakeHttpClient on purpose (the panicking default): a local/demo
    // drawer has no bridge — decide must be a no-op, not an HTTP call.
    let detail = RunDetail {
        run_id: "demo-1".into(),
        agent: "gemini".into(),
        status: "awaiting_approval".into(),
        ..Default::default()
    };
    let drawer = cx.new(|cx| RunDrawer::local(detail, cx));
    drawer.update(cx, |d, cx| d.decide_approval("ap_1".into(), true, cx));
    cx.run_until_parked();
    drawer.read_with(cx, |d, _| {
        assert_eq!(d.deciding_approval, None);
        assert!(d.store.is_none(), "local drawers are store-less");
    });
}
