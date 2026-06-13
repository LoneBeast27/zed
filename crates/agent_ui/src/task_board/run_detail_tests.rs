//! Tests for the run drawer: the sheet-width slide geometry, liberal
//! `RunDetail` deserialization, and the abort affordance's failure re-arm
//! (P2). Extracted to a sibling (the `#[path]` idiom) to keep `run_detail.rs`
//! under the 500-line ceiling — zero behavior change.

use super::*;

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
    let detail: RunDetail = serde_json::from_str(
        r#"{
            "run_id": "r-9", "agent": "claude", "status": "running",
            "elapsed_s": 12.5, "task": "do the thing",
            "chip": "claude · code-heavy · 92%",
            "usage": {"tokens": 1234, "cost": null},
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

    // Minimal payload also parses.
    let minimal: RunDetail = serde_json::from_str("{}").unwrap();
    assert_eq!(minimal.events.len(), 0);
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
