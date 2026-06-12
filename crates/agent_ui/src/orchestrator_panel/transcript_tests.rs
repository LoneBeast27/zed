//! Keyed-reconciliation + live-tick tests for [`super::TranscriptView`]
//! (sibling file per the 500-line ceiling; a child module so private state
//! stays reachable).

use super::*;


fn message(role: &str, text: &str) -> TranscriptMessage {
    TranscriptMessage {
        role: role.into(),
        text: text.into(),
        ..Default::default()
    }
}

fn snapshot(id: &str, busy: bool, messages: Vec<TranscriptMessage>) -> TranscriptSnapshot {
    TranscriptSnapshot {
        id: id.into(),
        title: "t".into(),
        busy,
        brain: None,
        messages,
    }
}

#[gpui::test]
fn growth_appends_and_keeps_existing_view_identity(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        let mut transcript = TranscriptView::new();
        transcript.sync(
            &snapshot("c-1", false, vec![message("user", "hi")]),
            cx,
        );
        assert_eq!(transcript.message_count(), 1);
        assert_eq!(transcript.list_state.item_count(), 1);
        let first_id = transcript.message(0).unwrap().markdown.entity_id();

        // Growth: the existing view (its markdown entity) is untouched.
        transcript.sync(
            &snapshot(
                "c-1",
                false,
                vec![message("user", "hi"), message("orchestrator", "reply")],
            ),
            cx,
        );
        assert_eq!(transcript.message_count(), 2);
        assert_eq!(transcript.list_state.item_count(), 2);
        assert_eq!(
            transcript.message(0).unwrap().markdown.entity_id(),
            first_id,
            "growth must never rebuild on-screen views (§5.6)"
        );
        assert!(!transcript.message(1).unwrap().user);
    });
}

#[gpui::test]
fn conversation_switch_rebuilds(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        let mut transcript = TranscriptView::new();
        transcript.sync(
            &snapshot("c-1", false, vec![message("user", "hi")]),
            cx,
        );
        let first_id = transcript.message(0).unwrap().markdown.entity_id();
        transcript.sync(
            &snapshot("c-2", false, vec![message("user", "other")]),
            cx,
        );
        assert_eq!(transcript.message_count(), 1);
        assert_ne!(
            transcript.message(0).unwrap().markdown.entity_id(),
            first_id,
            "a conversation switch re-keys everything"
        );
    });
}

#[gpui::test]
fn shimmer_item_tracks_busy(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        let mut transcript = TranscriptView::new();
        // Busy without growth: the shimmer item is listed past the
        // messages.
        transcript.sync(&snapshot("c-1", true, vec![message("user", "go")]), cx);
        assert_eq!(transcript.message_count(), 1);
        assert_eq!(transcript.list_state.item_count(), 2, "messages + shimmer");

        // Reply lands, busy drops: the shimmer slot becomes the reply.
        transcript.sync(
            &snapshot(
                "c-1",
                false,
                vec![message("user", "go"), message("orchestrator", "done")],
            ),
            cx,
        );
        assert_eq!(transcript.message_count(), 2);
        assert_eq!(transcript.list_state.item_count(), 2);

        // Busy flips again without growth: shimmer returns.
        transcript.sync(
            &snapshot(
                "c-1",
                true,
                vec![message("user", "go"), message("orchestrator", "done")],
            ),
            cx,
        );
        assert_eq!(transcript.list_state.item_count(), 3);
    });
}

#[gpui::test]
fn transcript_is_bounded_to_60(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        let mut transcript = TranscriptView::new();
        let messages: Vec<_> = (0..70)
            .map(|i| message("user", &format!("m{i}")))
            .collect();
        transcript.sync(&snapshot("c-1", false, messages), cx);
        assert_eq!(transcript.message_count(), 60, "defensive 60-cap (§8.4)");
        assert_eq!(transcript.message(0).unwrap().text.as_ref(), "m10");
    });
}

#[gpui::test]
fn live_tick_only_rides_a_trailing_agent_reply(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        let mut transcript = TranscriptView::new();
        assert_eq!(transcript.live_agent_ix(true), None);

        // Typical busy state: the user message is last — the previous
        // turn's frozen counter must NOT roll.
        transcript.sync(
            &snapshot(
                "c-1",
                true,
                vec![
                    message("user", "q1"),
                    message("orchestrator", "a1"),
                    message("user", "q2"),
                ],
            ),
            cx,
        );
        assert_eq!(transcript.live_agent_ix(true), None);

        // A progressively-streamed reply IS last — that one rolls.
        transcript.sync(
            &snapshot(
                "c-1",
                true,
                vec![
                    message("user", "q1"),
                    message("orchestrator", "a1"),
                    message("user", "q2"),
                    message("orchestrator", "a2 (streaming)"),
                ],
            ),
            cx,
        );
        assert_eq!(transcript.live_agent_ix(true), Some(3));
    });
}

#[gpui::test]
fn settled_trailing_reply_never_ticks(cx: &mut gpui::TestAppContext) {
    // The busy gate (previously only panel.rs's untested
    // `busy.then(...)`): a settled steady state — agent reply last,
    // busy == false — must NOT roll, or every settled conversation's
    // final counter would inflate off its seen_at clock (the
    // Z3_REPORT fix-#1 counter-inflation class).
    cx.update(|cx| {
        let mut transcript = TranscriptView::new();
        transcript.sync(
            &snapshot(
                "c-1",
                false,
                vec![message("user", "q"), message("orchestrator", "a")],
            ),
            cx,
        );
        assert_eq!(
            transcript.live_agent_ix(false),
            None,
            "settled trailing reply must not tick"
        );
        assert_eq!(
            transcript.live_agent_ix(true),
            Some(1),
            "the same shape rolls while busy (progressive reply)"
        );
    });
}

#[gpui::test]
fn shrink_rebuilds_views_and_list(cx: &mut gpui::TestAppContext) {
    // A same-conversation shrink (bridge restart serving a truncated
    // transcript) takes the rebuild path: views and the spliced list
    // both land at the smaller count.
    cx.update(|cx| {
        let mut transcript = TranscriptView::new();
        transcript.sync(
            &snapshot(
                "c-1",
                false,
                vec![
                    message("user", "m0"),
                    message("orchestrator", "m1"),
                    message("user", "m2"),
                ],
            ),
            cx,
        );
        assert_eq!(transcript.list_state.item_count(), 3);

        transcript.sync(
            &snapshot("c-1", false, vec![message("user", "m0")]),
            cx,
        );
        assert_eq!(transcript.message_count(), 1, "views rebuilt to the shrunk set");
        assert_eq!(
            transcript.list_state.item_count(),
            1,
            "list re-splices to the shrunk count"
        );
        assert_eq!(transcript.message(0).unwrap().text.as_ref(), "m0");
    });
}

#[gpui::test]
fn growth_while_busy_keeps_the_shimmer_last(cx: &mut gpui::TestAppContext) {
    // Growth with busy held true: the splice past the stable prefix
    // must keep on-screen views untouched and the shimmer item in the
    // final slot (messages + 1).
    cx.update(|cx| {
        let mut transcript = TranscriptView::new();
        transcript.sync(&snapshot("c-1", true, vec![message("user", "q1")]), cx);
        assert_eq!(transcript.list_state.item_count(), 2, "message + shimmer");
        let first_id = transcript.message(0).unwrap().markdown.entity_id();

        // A progressive reply lands while STILL busy.
        transcript.sync(
            &snapshot(
                "c-1",
                true,
                vec![message("user", "q1"), message("orchestrator", "a1 (streaming)")],
            ),
            cx,
        );
        assert_eq!(transcript.message_count(), 2);
        assert_eq!(
            transcript.list_state.item_count(),
            3,
            "both messages + the shimmer still listed past them"
        );
        assert_eq!(
            transcript.message(0).unwrap().markdown.entity_id(),
            first_id,
            "the stable prefix is never re-spliced (§5.6)"
        );
    });
}
