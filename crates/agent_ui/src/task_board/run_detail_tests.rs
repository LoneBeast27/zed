//! Tests for the run drawer: the sheet-width slide geometry, liberal
//! `RunDetail` deserialization, the abort affordance's failure re-arm (P2),
//! the bounded first-fetch failure path (never an eternal "Loading…"), the
//! local (demo-store) feed path, and the honest title chain. Extracted to a
//! sibling (the `#[path]` idiom) to keep `run_detail.rs` under the 500-line
//! ceiling.

use super::*;
use crate::task_board::run_detail_head::drawer_title;

#[test]
fn slide_travel_is_sheet_width_terms_at_every_panel_width() {
    // Narrow panel (Z1 default 420px): sheet resolves to 92% = 386.4px,
    // travel = 1.02 × that — the geometry where the old panel-relative
    // math coincidentally agreed (within the 2% border clearance).
    assert!((slide_offset(420., 1.0) - (-394.128)).abs() < 1e-3);
    // Wide panel (1200px): sheet caps at 560px, travel = 571.2px —
    // NOT the old 0.92 × panel = 1104px over-travel (§8.7(b) dead zone).
    assert!((slide_offset(1200., 1.0) - (-571.2)).abs() < 1e-3);
    // Crossover panel width (560/0.92 ≈ 608.7): both formulas agree.
    assert!((slide_offset(608.7, 1.0) - (-571.2)).abs() < 0.1);
    // Settled (out = 0) is exactly in place.
    assert_eq!(slide_offset(1200., 0.0), 0.0);
    // Travel scales linearly with the animator's `out`.
    assert!((slide_offset(1200., 0.5) - (-285.6)).abs() < 1e-3);
}

#[test]
fn run_detail_deserializes_liberally() {
    // Fixture truth: the live `GET /run/<id>` carries `session_id`
    // (2026-07-04, curl http://127.0.0.1:4530/run/<id>).
    let detail: RunDetail = serde_json::from_str(
        r#"{
            "run_id": "r-9", "agent": "claude", "status": "running",
            "elapsed_s": 12.5, "task": "do the thing",
            "chip": "claude · code-heavy · 92%",
            "usage": {"tokens": 1234, "cost": null},
            "session_id": "6b363c45-6fad-4b24-bb33-7e32b94969a1",
            "events": [
                {"kind": "spawn", "payload": {"a": 1}},
                {"kind": "log", "payload": "line"}
            ],
            "unknown_field": true
        }"#,
    )
    .unwrap();
    assert_eq!(detail.run_id, "r-9");
    assert_eq!(detail.status, "running");
    assert_eq!(detail.events.len(), 2);
    assert_eq!(detail.events[0].kind, "spawn");
    assert_eq!(detail.text, None);
    assert_eq!(detail.error, None);
    assert_eq!(
        detail.session_id.as_deref(),
        Some("6b363c45-6fad-4b24-bb33-7e32b94969a1")
    );

    // Minimal payload also parses — session_id absent → None.
    let minimal: RunDetail = serde_json::from_str("{}").unwrap();
    assert_eq!(minimal.events.len(), 0);
    assert_eq!(minimal.session_id, None);
}

#[test]
fn drawer_title_prefers_task_then_archetype_then_run_id() {
    // Task title wins (first non-empty line only).
    let full = RunDetail {
        run_id: "r-1".into(),
        agent: "claude".into(),
        task: Some("\nImplementation: absorption choreography\ndetails…".into()),
        archetype: Some("implementation".into()),
        ..Default::default()
    };
    assert_eq!(drawer_title(&full), "Implementation: absorption choreography");
    // No task → archetype (capitalized).
    let archetype_only = RunDetail {
        run_id: "r-2".into(),
        archetype: Some("researcher".into()),
        ..Default::default()
    };
    assert_eq!(drawer_title(&archetype_only), "Researcher");
    // Nothing but the id → the id IS the honest label. NEVER "agent run".
    let bare = RunDetail {
        run_id: "r-3".into(),
        agent: "codex".into(),
        ..Default::default()
    };
    assert_eq!(drawer_title(&bare), "r-3");
}

#[gpui::test]
async fn first_fetch_404_resolves_to_not_found_not_eternal_loading(
    cx: &mut gpui::TestAppContext,
) {
    use http_client::{FakeHttpClient, Response};
    // The constellation-demo bug class: the run doesn't exist on the bridge.
    let http_client = FakeHttpClient::create(move |_request| async move {
        Ok(Response::builder().status(404).body("nope".into()).unwrap())
    });
    cx.update(|cx| cx.set_http_client(http_client));

    let drawer = cx.new(|cx| RunDrawer::new("ghost-run".into(), cx));
    cx.run_until_parked();
    drawer.read_with(cx, |d, _| {
        assert!(d.detail.is_none());
        assert_eq!(
            d.load_failed.as_deref(),
            Some("Run not found on bridge."),
            "a 404 must fail fast into the honest error state"
        );
    });
}

#[gpui::test]
async fn first_fetch_gives_up_after_bounded_retries(cx: &mut gpui::TestAppContext) {
    use http_client::{FakeHttpClient, Response};
    // Bridge down (500s): the drawer retries with backoff, then resolves to
    // an error state after FIRST_FETCH_ATTEMPTS — bounded, never infinite.
    let http_client = FakeHttpClient::create(move |_request| async move {
        Ok(Response::builder().status(500).body("boom".into()).unwrap())
    });
    cx.update(|cx| cx.set_http_client(http_client));

    let drawer = cx.new(|cx| RunDrawer::new("r-1".into(), cx));
    cx.run_until_parked();
    // Failure 1 landed; the loop sleeps 2s, fails again (2), sleeps 3s,
    // fails again (3) → gives up.
    for _ in 0..FIRST_FETCH_ATTEMPTS {
        cx.executor().advance_clock(LOG_POLL_ERROR_CAP);
        cx.run_until_parked();
    }
    drawer.read_with(cx, |d, _| {
        assert!(d.detail.is_none());
        assert_eq!(
            d.load_failed.as_deref(),
            Some("Bridge unreachable — run detail unavailable."),
            "unreachable bridge must resolve, not spin on Loading…"
        );
    });
}

#[gpui::test]
fn local_drawer_feeds_from_the_demo_store_without_bridge_io(cx: &mut gpui::TestAppContext) {
    // No FakeHttpClient installed on purpose: a local drawer must never
    // touch HTTP — construction with the default (panicking) test client
    // proves the path is I/O-free.
    let detail = RunDetail {
        run_id: "demo-designer".into(),
        agent: "gemini".into(),
        status: "running".into(),
        elapsed_s: 4.0,
        task: Some("Designer: keyframe spec from hero recording".into()),
        archetype: Some("designer".into()),
        ..Default::default()
    };
    let drawer = cx.new(|cx| RunDrawer::local(detail, cx));
    drawer.read_with(cx, |d, _| {
        let held = d.detail.as_ref().expect("local drawer holds detail immediately");
        assert_eq!(held.run_id, "demo-designer");
        assert_eq!(held.status, "running");
        assert!(d.load_failed.is_none());
        assert!(d.local, "local drawers hide the abort affordance");
    });
    // The owner's frame-pump push updates elapsed/status in place.
    drawer.update(cx, |d, cx| {
        d.push_local_detail(
            RunDetail {
                run_id: "demo-designer".into(),
                agent: "gemini".into(),
                status: "running".into(),
                elapsed_s: 5.0,
                task: Some("Designer: keyframe spec from hero recording".into()),
                archetype: Some("designer".into()),
                ..Default::default()
            },
            cx,
        );
    });
    drawer.read_with(cx, |d, _| {
        assert_eq!(d.detail.as_ref().unwrap().elapsed_s, 5.0);
    });
}

#[gpui::test]
async fn abort_rearms_button_on_post_failure(cx: &mut gpui::TestAppContext) {
    use http_client::{FakeHttpClient, Method, Response};
    // Bridge: GET /run/r-1 always says "running"; POST .../abort 500s.
    let http_client = FakeHttpClient::create(move |request| async move {
        if request.method() == Method::POST && request.uri().path().ends_with("/abort") {
            // The abort never lands — exactly the P2 dead-end scenario.
            return Ok(Response::builder().status(500).body("boom".into()).unwrap());
        }
        Ok(Response::builder()
            .status(200)
            .body(r#"{"run_id": "r-1", "status": "running"}"#.into())
            .unwrap())
    });
    cx.update(|cx| cx.set_http_client(http_client));

    let drawer = cx.new(|cx| RunDrawer::new("r-1".into(), cx));
    cx.run_until_parked(); // first GET lands the running detail
    drawer.read_with(cx, |d, _| {
        assert_eq!(
            d.detail.as_ref().map(|x| x.status.clone()),
            Some("running".into())
        );
        assert!(!d.aborting, "not aborting before the click");
    });

    // Click abort → aborting flips true immediately…
    drawer.update(cx, |d, cx| d.abort(cx));
    drawer.read_with(cx, |d, _| assert!(d.aborting, "aborting set on POST dispatch"));

    // …and the 500 re-arms it (P2: not stuck on "Aborting…" forever).
    cx.run_until_parked();
    drawer.read_with(cx, |d, _| {
        assert!(!d.aborting, "failed abort POST must re-arm the button");
    });
}

#[gpui::test]
async fn abort_stays_armed_while_post_succeeds(cx: &mut gpui::TestAppContext) {
    use http_client::{FakeHttpClient, Method, Response};
    // A successful abort POST: aborting stays true (the tail-poll, not the
    // POST, clears it once the run reaches a non-running status).
    let http_client = FakeHttpClient::create(move |request| async move {
        if request.method() == Method::POST && request.uri().path().ends_with("/abort") {
            return Ok(Response::builder()
                .status(200)
                .body(r#"{"run_id": "r-1", "status": "killed"}"#.into())
                .unwrap());
        }
        Ok(Response::builder()
            .status(200)
            .body(r#"{"run_id": "r-1", "status": "running"}"#.into())
            .unwrap())
    });
    cx.update(|cx| cx.set_http_client(http_client));

    let drawer = cx.new(|cx| RunDrawer::new("r-1".into(), cx));
    cx.run_until_parked();
    drawer.update(cx, |d, cx| d.abort(cx));
    cx.run_until_parked();
    drawer.read_with(cx, |d, _| {
        assert!(d.aborting, "a successful abort POST does not itself re-arm");
    });
}
