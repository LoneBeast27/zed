//! Tests for the adversary job data path: parse fixtures, the
//! `parseSections` port, and the watch-gated poll lifetime driven
//! against the REAL loop (fake bridge + fake clock — the
//! `watch_tests.rs` idiom).

use super::*;
use gpui::TestAppContext;
use http_client::{FakeHttpClient, Method, Response};

// ── parse fixtures ──

#[test]
fn job_snapshot_fixtures_parse() {
    // Fixture truth: bridge/adversary_jobs.py job dicts.
    let running: AdversaryJobSnapshot = serde_json::from_str(
        r#"{"id": "ab12", "status": "running", "prompt": "q",
            "started": 1781110195.6, "result": null, "error": null}"#,
    )
    .unwrap();
    assert_eq!(running.status, "running");
    assert_eq!(running.result, None);
    assert_eq!(running.error, None);

    let done: AdversaryJobSnapshot = serde_json::from_str(
        r#"{"status": "done", "result": {
            "prompt": "q",
            "answers": {"claude": "A", "codex": "B", "gemini": "(timed out)"},
            "synthesis": "AGREEMENTS: all three agree.",
            "elapsed": 42.0}, "error": null}"#,
    )
    .unwrap();
    let result = done.result.unwrap();
    assert_eq!(result.answers.len(), 3);
    assert_eq!(result.answers["claude"], "A");
    assert!(result.synthesis.unwrap().starts_with("AGREEMENTS"));

    let errored: AdversaryJobSnapshot =
        serde_json::from_str(r#"{"status": "error", "error": "broadcast exploded"}"#).unwrap();
    assert_eq!(errored.status, "error");
    assert_eq!(errored.error.as_deref(), Some("broadcast exploded"));

    // Liberal: bare object defaults instead of erroring.
    let bare: AdversaryJobSnapshot = serde_json::from_str("{}").unwrap();
    assert_eq!(bare, AdversaryJobSnapshot::default());
}

// ── parseSections port ──

#[test]
fn synthesis_sections_split_like_the_web() {
    let sections = parse_synthesis_sections(
        "AGREEMENTS: all three named the GIL.\n\
         DISAGREEMENTS:\nclaude went deeper on async.\n\
         SYNTHESIS: use processes for CPU work.",
    );
    assert_eq!(sections.agreements, "all three named the GIL.");
    assert_eq!(sections.disagreements, "claude went deeper on async.");
    assert_eq!(sections.synthesis, "use processes for CPU work.");
}

#[test]
fn synthesis_sections_are_case_insensitive_with_loose_colons() {
    let sections = parse_synthesis_sections(
        "agreements : a\nDisagreements\t: d\nsynthesis: s",
    );
    assert_eq!(sections.agreements, "a");
    assert_eq!(sections.disagreements, "d");
    assert_eq!(sections.synthesis, "s");
}

#[test]
fn no_markers_puts_everything_in_synthesis() {
    let sections = parse_synthesis_sections("just a flat judge reply");
    assert_eq!(sections.synthesis, "just a flat judge reply");
    assert_eq!(sections.agreements, "");
    assert_eq!(sections.disagreements, "");
}

#[test]
fn preamble_before_the_first_marker_is_dropped() {
    // JS: the first mark's content starts after its colon; text before
    // `marks[0].index` lands in no bucket.
    let sections = parse_synthesis_sections("Here is my verdict.\nAGREEMENTS: a");
    assert_eq!(sections.agreements, "a");
    assert_eq!(sections.synthesis, "");
}

#[test]
fn disagreements_never_double_matches_its_inner_agreements() {
    // "DISAGREEMENTS" contains "AGREEMENTS" — the global-regex scan
    // resumes past each match, so the inner keyword must not re-match.
    let sections = parse_synthesis_sections("DISAGREEMENTS: only this");
    assert_eq!(sections.disagreements, "only this");
    assert_eq!(sections.agreements, "");
}

#[test]
fn keyword_without_colon_is_not_a_marker() {
    let sections =
        parse_synthesis_sections("The agreements were thin.\nSYNTHESIS: fine.");
    assert_eq!(sections.synthesis, "fine.");
    assert_eq!(sections.agreements, "");
}

// ── job-state ingest + watch lifetime (the watch_tests.rs idiom: a fake
//    bridge + fake clock drive the REAL poll loop) ──

/// A fake bridge: `POST /adversary` mints job `j-1`; `GET /adversary/j-1`
/// serves `running` until `running_polls` hits are consumed, then the
/// given terminal payload. Returns (post hits, poll hits).
fn fake_adversary_bridge(
    cx: &mut TestAppContext,
    running_polls: usize,
    terminal: &'static str,
) -> (Arc<AtomicUsize>, Arc<AtomicUsize>) {
    let post_hits = Arc::new(AtomicUsize::new(0));
    let poll_hits = Arc::new(AtomicUsize::new(0));
    let (posts, polls) = (post_hits.clone(), poll_hits.clone());
    let http_client = FakeHttpClient::create(move |request| {
        let (posts, polls) = (posts.clone(), polls.clone());
        async move {
            let body = if request.method() == Method::POST
                && request.uri().path() == "/adversary"
            {
                posts.fetch_add(1, Ordering::SeqCst);
                r#"{"job": "j-1"}"#.to_string()
            } else if request.uri().path() == "/adversary/j-1" {
                let hit = polls.fetch_add(1, Ordering::SeqCst);
                if hit < running_polls {
                    r#"{"status": "running"}"#.to_string()
                } else {
                    terminal.to_string()
                }
            } else {
                r#"{"error": "not found"}"#.to_string()
            };
            Ok(Response::builder().status(200).body(body.into()).unwrap())
        }
    });
    cx.update(|cx| cx.set_http_client(http_client));
    (post_hits, poll_hits)
}

const DONE: &str = r#"{"status": "done", "result": {
    "answers": {"claude": "A", "codex": "B", "gemini": "C"},
    "synthesis": "SYNTHESIS: merged."}}"#;

#[gpui::test]
async fn job_ingest_runs_running_to_done(cx: &mut TestAppContext) {
    let (post_hits, poll_hits) = fake_adversary_bridge(cx, 1, DONE);
    let jobs = cx.new(|_| AdversaryJobs::default());
    let watch = jobs.update(cx, |jobs, cx| {
        let watch = jobs.watch(cx);
        jobs.start("compare the GILs".into(), cx);
        watch
    });
    cx.run_until_parked();
    assert_eq!(post_hits.load(Ordering::SeqCst), 1, "one broadcast POST");
    jobs.read_with(cx, |jobs, _| {
        assert_eq!(jobs.phase, AdversaryPhase::Pending, "pending while running")
    });

    // First poll tick: still running → stays pending, keeps polling.
    cx.executor().advance_clock(Duration::from_millis(2600));
    cx.run_until_parked();
    assert_eq!(poll_hits.load(Ordering::SeqCst), 1);
    jobs.read_with(cx, |jobs, _| {
        assert_eq!(jobs.phase, AdversaryPhase::Pending)
    });

    // Second tick: terminal `done` lands the answers + synthesis.
    cx.executor().advance_clock(Duration::from_millis(2600));
    cx.run_until_parked();
    jobs.read_with(cx, |jobs, _| {
        let AdversaryPhase::Done(result) = &jobs.phase else {
            panic!("expected Done, got {:?}", jobs.phase);
        };
        assert_eq!(result.answers["claude"], "A");
        assert_eq!(result.synthesis.as_deref(), Some("SYNTHESIS: merged."));
    });

    // Terminal job: the poll is dead even while still watched.
    let settled = poll_hits.load(Ordering::SeqCst);
    cx.executor().advance_clock(Duration::from_secs(60));
    cx.run_until_parked();
    assert_eq!(
        poll_hits.load(Ordering::SeqCst),
        settled,
        "a settled job must never keep polling"
    );
    drop(watch);
}

#[gpui::test]
async fn job_error_status_fails_with_the_bridge_message(cx: &mut TestAppContext) {
    let (_, _) =
        fake_adversary_bridge(cx, 0, r#"{"status": "error", "error": "broadcast exploded"}"#);
    let jobs = cx.new(|_| AdversaryJobs::default());
    let _watch = jobs.update(cx, |jobs, cx| {
        let watch = jobs.watch(cx);
        jobs.start("q".into(), cx);
        watch
    });
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_millis(2600));
    cx.run_until_parked();
    jobs.read_with(cx, |jobs, _| {
        assert_eq!(
            jobs.phase,
            AdversaryPhase::Failed("broadcast exploded".to_string())
        );
    });
}

/// An aborted broadcast: the bridge job lands `status == "cancelled"` with the
/// partial "(aborted)" answers (orchestrator/adversary.py).
const CANCELLED: &str = r#"{"status": "cancelled", "result": {
    "answers": {"claude": "(aborted)", "codex": "(aborted)", "gemini": "(aborted)"},
    "synthesis": "(aborted)"}}"#;

#[gpui::test]
async fn job_cancelled_status_lands_distinct_aborted_phase(cx: &mut TestAppContext) {
    // P4: a cancelled job must land AdversaryPhase::Cancelled, NOT Done — so
    // the panel renders the distinct aborted banner instead of passing the
    // "(aborted)" legs off as a normal completed broadcast.
    let (_, _) = fake_adversary_bridge(cx, 0, CANCELLED);
    let jobs = cx.new(|_| AdversaryJobs::default());
    let _watch = jobs.update(cx, |jobs, cx| {
        let watch = jobs.watch(cx);
        jobs.start("q".into(), cx);
        watch
    });
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_millis(2600));
    cx.run_until_parked();
    jobs.read_with(cx, |jobs, _| {
        let AdversaryPhase::Cancelled(result) = &jobs.phase else {
            panic!("expected Cancelled (distinct from Done), got {:?}", jobs.phase);
        };
        // The partial result is carried so the columns still render under the
        // aborted banner.
        assert_eq!(result.answers["claude"], "(aborted)");
        assert_eq!(result.synthesis.as_deref(), Some("(aborted)"));
    });
}

#[gpui::test]
async fn poll_dies_when_unwatched_and_resumes_on_rewatch(cx: &mut TestAppContext) {
    let (_, poll_hits) = fake_adversary_bridge(cx, 0, DONE);
    let jobs = cx.new(|_| AdversaryJobs::default());
    let watch = jobs.update(cx, |jobs, cx| {
        let watch = jobs.watch(cx);
        jobs.start("q".into(), cx);
        watch
    });
    cx.run_until_parked(); // POST lands, poll armed

    // The panel hides BEFORE the first tick: the loop's wake sees zero
    // watchers and exits without ever fetching — dead, not idling.
    drop(watch);
    cx.executor().advance_clock(Duration::from_secs(60));
    cx.run_until_parked();
    assert_eq!(
        poll_hits.load(Ordering::SeqCst),
        0,
        "no `/adversary/<job>` fetches while the panel is hidden"
    );
    jobs.read_with(cx, |jobs, _| {
        assert_eq!(jobs.phase, AdversaryPhase::Pending, "the job itself survives")
    });

    // Re-show: the 0→1 watch respawns the poll and the job completes.
    let rewatch = jobs.update(cx, |jobs, cx| jobs.watch(cx));
    cx.executor().advance_clock(Duration::from_millis(2600));
    cx.run_until_parked();
    assert_eq!(poll_hits.load(Ordering::SeqCst), 1, "re-watch resumes the poll");
    jobs.read_with(cx, |jobs, _| {
        assert!(matches!(jobs.phase, AdversaryPhase::Done(_)));
    });
    drop(rewatch);
}

#[gpui::test]
async fn rebroadcast_replaces_the_pending_job(cx: &mut TestAppContext) {
    let (post_hits, _) = fake_adversary_bridge(cx, usize::MAX, DONE);
    let jobs = cx.new(|_| AdversaryJobs::default());
    let _watch = jobs.update(cx, |jobs, cx| {
        let watch = jobs.watch(cx);
        jobs.start("first".into(), cx);
        watch
    });
    cx.run_until_parked();
    let first_generation = jobs.read_with(cx, |jobs, _| jobs.generation);

    jobs.update(cx, |jobs, cx| jobs.start("second".into(), cx));
    cx.run_until_parked();
    jobs.read_with(cx, |jobs, _| {
        assert_eq!(jobs.phase, AdversaryPhase::Pending);
        assert_eq!(
            jobs.generation,
            first_generation + 1,
            "each broadcast bumps the entrance-animation generation"
        );
    });
    assert_eq!(post_hits.load(Ordering::SeqCst), 2);
}
