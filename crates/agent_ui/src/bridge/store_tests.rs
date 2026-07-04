//! Tests for the bridge store: the local elapsed re-base math, the
//! refcount-gated transcript poll lifetime, change-gated transcript apply,
//! conversation→project resolution, and the run-gated 1s ticker. Extracted
//! to a sibling (the `#[path]` idiom) to keep `store.rs` under the 500-line
//! ceiling — zero behavior change.

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

fn transcript(id: &str, busy: bool, texts: &[&str]) -> TranscriptSnapshot {
    TranscriptSnapshot {
        id: id.to_string(),
        title: texts.first().unwrap_or(&"").to_string(),
        busy,
        brain: None,
        messages: texts
            .iter()
            .map(|text| super::super::protocol::TranscriptMessage {
                role: "user".into(),
                text: text.to_string(),
                ..Default::default()
            })
            .collect(),
    }
}

#[gpui::test]
fn transcript_watch_refcount_gates_the_poll(cx: &mut gpui::TestAppContext) {
    let store = cx.new(|_| BridgeStore::default());
    store.update(cx, |store, cx| {
        assert!(
            store.transcript_task.is_none(),
            "no poll task before any watcher"
        );
        let first = store.watch_transcript(cx);
        assert_eq!(store.transcript_watchers.load(Ordering::SeqCst), 1);
        assert!(store.transcript_task.is_some(), "0→1 spawns the poll");

        let second = store.watch_transcript(cx);
        assert_eq!(store.transcript_watchers.load(Ordering::SeqCst), 2);

        drop(first);
        assert_eq!(store.transcript_watchers.load(Ordering::SeqCst), 1);
        drop(second);
        assert_eq!(
            store.transcript_watchers.load(Ordering::SeqCst),
            0,
            "dropping the last watch pauses the poll (loop exits on next wake)"
        );

        // A fresh watch respawns (replacing any drained task).
        let third = store.watch_transcript(cx);
        assert_eq!(store.transcript_watchers.load(Ordering::SeqCst), 1);
        assert!(store.transcript_task.is_some());
        drop(third);
    });
}

#[gpui::test]
fn apply_transcript_canonicalizes_conv_and_change_gates(cx: &mut gpui::TestAppContext) {
    let store = cx.new(|_| BridgeStore::default());
    let notifies = std::rc::Rc::new(std::cell::Cell::new(0usize));
    let _subscription = cx.update(|cx| {
        cx.observe(&store, {
            let notifies = notifies.clone();
            move |_, _| notifies.set(notifies.get() + 1)
        })
    });

    store.update(cx, |store, cx| {
        store.apply_transcript(transcript("c-1", false, &["hi"]), cx);
        assert_eq!(store.transcript_conv.as_deref(), Some("c-1"));
        assert_eq!(store.transcript.as_ref().unwrap().messages.len(), 1);
    });
    cx.run_until_parked();
    let after_first = notifies.get();
    assert!(after_first >= 1, "a new snapshot notifies");

    // An identical snapshot is a no-op (poll ticks while idle).
    store.update(cx, |store, cx| {
        store.apply_transcript(transcript("c-1", false, &["hi"]), cx);
    });
    cx.run_until_parked();
    assert_eq!(notifies.get(), after_first, "unchanged snapshot must not notify");

    // Growth notifies again.
    store.update(cx, |store, cx| {
        store.apply_transcript(transcript("c-1", true, &["hi", "more"]), cx);
    });
    cx.run_until_parked();
    assert!(notifies.get() > after_first);
}

#[gpui::test]
fn project_name_resolves_via_conversation_membership(cx: &mut gpui::TestAppContext) {
    let store = cx.new(|_| BridgeStore::default());
    store.update(cx, |store, cx| {
        assert_eq!(store.project_name_for_conv("c-1"), None);
        store.apply_projects(
            vec![ProjectRow {
                id: "p-1".into(),
                name: "default".into(),
                conversations: vec![super::super::protocol::ConversationRow {
                    id: "c-1".into(),
                    ..Default::default()
                }],
            }],
            cx,
        );
        assert_eq!(store.project_name_for_conv("c-1"), Some("default"));
        assert_eq!(store.project_name_for_conv("c-404"), None);
        assert_eq!(
            store.project_name_for_conv(""),
            None,
            "empty conv id never matches"
        );
    });
}

fn plan_event(conv: &str, plan_id: &str) -> BridgeEvent {
    BridgeEvent::Plan {
        plan: super::super::protocol::PlanSnapshot {
            plan_id: Some(plan_id.to_string()),
            conv: conv.to_string(),
            waves: vec![vec![super::super::protocol::PlanSubtask {
                subtask_id: "t1".into(),
                status: "running".into(),
                ..Default::default()
            }]],
            ..Default::default()
        },
    }
}

#[gpui::test]
fn sse_plan_frame_for_another_conv_is_dropped(cx: &mut gpui::TestAppContext) {
    // P5: the SSE plan frame is global-newest across all conversations. While
    // the panel follows conv A, a frame describing conv B must NOT replace A's
    // plan (the /plan?conv=A poll scopes by conv; the SSE apply must too).
    let store = cx.new(|_| BridgeStore::default());
    store.update(cx, |store, cx| {
        store.transcript_conv = Some("conv-a".to_string());

        // A frame for the followed conv A applies.
        store.apply_event(plan_event("conv-a", "plan-a"), cx);
        assert_eq!(
            store.plan.as_ref().and_then(|p| p.plan_id.clone()),
            Some("plan-a".into())
        );

        // A frame for conv B is dropped — A's plan is untouched (no flicker).
        store.apply_event(plan_event("conv-b", "plan-b"), cx);
        assert_eq!(
            store.plan.as_ref().and_then(|p| p.plan_id.clone()),
            Some("plan-a".into()),
            "a different conv's SSE plan frame must not replace the followed plan"
        );

        // An empty-conv frame (single-conv usage / older bridge) still applies.
        store.apply_event(plan_event("", "plan-default"), cx);
        assert_eq!(
            store.plan.as_ref().and_then(|p| p.plan_id.clone()),
            Some("plan-default".into()),
            "an empty conv is never filtered"
        );
    });
}

#[test]
fn accepts_plan_follow_default_fold() {
    // Nothing followed (a fresh app) → adopt ANY plan; the bridge's
    // global-newest IS the most-recent-conversation-with-a-plan default.
    assert!(accepts_plan(None, "conv-a"));
    assert!(accepts_plan(None, ""));
    // Following a conv → adopt only that conv's plan.
    assert!(accepts_plan(Some("conv-a"), "conv-a"));
    assert!(!accepts_plan(Some("conv-a"), "conv-b"));
    // An empty incoming conv (single-conv / older bridge) is always accepted —
    // there is no conv to disambiguate against.
    assert!(accepts_plan(Some("conv-a"), ""));
}

#[gpui::test]
fn unfollowed_app_defaults_to_the_bridge_newest_plan(cx: &mut gpui::TestAppContext) {
    // Finding 2: a fresh app follows nothing (transcript_conv == None). The
    // bridge's global-newest SSE plan frame must be ADOPTED — the panel
    // defaults to it instead of "No plans yet" — and its conv recorded so the
    // header can label it.
    let store = cx.new(|_| BridgeStore::default());
    store.update(cx, |store, cx| {
        assert!(store.transcript_conv.is_none(), "fresh app follows nothing");
        assert!(store.plan.is_none());

        store.apply_event(plan_event("conv-live", "plan-live"), cx);
        assert_eq!(
            store.plan.as_ref().and_then(|p| p.plan_id.clone()),
            Some("plan-live".into()),
            "the global-newest plan is adopted when nothing is followed"
        );
        assert_eq!(
            store.plan_conv, "conv-live",
            "the plan's conv is recorded for the header label"
        );

        // A newer global frame (another conv) still wins while unfollowed —
        // "most recent" tracks the newest push.
        store.apply_event(plan_event("conv-newer", "plan-newer"), cx);
        assert_eq!(
            store.plan.as_ref().and_then(|p| p.plan_id.clone()),
            Some("plan-newer".into())
        );
        assert_eq!(store.plan_conv, "conv-newer");
    });
}

#[gpui::test]
fn poll_path_shares_the_accept_fold_and_records_conv(cx: &mut gpui::TestAppContext) {
    // apply_plan (the poll / on-connect default-fetch path) uses the same fold:
    // a followed conv scopes it, and the adopted plan's conv is recorded.
    let store = cx.new(|_| BridgeStore::default());
    store.update(cx, |store, cx| {
        store.transcript_conv = Some("conv-a".into());
        let a = super::super::protocol::PlanSnapshot {
            plan_id: Some("plan-a".into()),
            conv: "conv-a".into(),
            waves: vec![vec![Default::default()]],
            ..Default::default()
        };
        store.apply_plan(a, cx);
        assert_eq!(store.plan_conv, "conv-a");

        // A poll result for a different conv is rejected by the fold.
        let b = super::super::protocol::PlanSnapshot {
            plan_id: Some("plan-b".into()),
            conv: "conv-b".into(),
            waves: vec![vec![Default::default()]],
            ..Default::default()
        };
        store.apply_plan(b, cx);
        assert_eq!(
            store.plan.as_ref().and_then(|p| p.plan_id.clone()),
            Some("plan-a".into()),
            "the poll path scopes by the followed conv, like the SSE path"
        );
        assert_eq!(store.plan_conv, "conv-a");
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
