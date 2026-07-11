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

fn run(agent: &str, chip: Option<&str>) -> TranscriptRun {
    TranscriptRun {
        run_id: "r-1".into(),
        agent: agent.into(),
        chip: chip.map(str::to_string),
    }
}

#[test]
fn forced_turn_meta_credits_the_run_agent_not_the_brain() {
    // The live chip shapes (task_board::style::chip_reason fixtures): the
    // reason segment carries "Forced target" / "user override".
    let forced = [
        run("claude", Some("claude · Forced target · 100%")),
        run("gemini", Some("gemini · user override")),
        // The RAW bridge chip (live shape, caught by the 2026-07-10 playtest:
        // "forced target" as a contiguous substring never matched it).
        run("codex", Some("codex · forced @vendor target (short-circuit) · 100%")),
    ];
    assert_eq!(
        meta_vendor(Some("gemini"), &forced[..1]),
        Some("@claude".into()),
        "a forced turn shows the agent that ran it, never the brain"
    );
    assert_eq!(meta_vendor(None, &forced[1..2]), Some("@gemini".into()));
    assert_eq!(
        meta_vendor(Some("gemini"), &forced[2..]),
        Some("@codex".into()),
        "the raw bridge chip shape must also detect as forced"
    );
}

#[test]
fn routed_turn_meta_keeps_the_brain() {
    let routed = [run("claude", Some("claude · code-heavy task · 92%"))];
    assert_eq!(meta_vendor(Some("qwen"), &routed), Some("qwen".into()));
    // Chip-less and solo-chip runs are not forced either.
    assert_eq!(meta_vendor(Some("qwen"), &[run("claude", None)]), Some("qwen".into()));
    assert_eq!(
        meta_vendor(Some("qwen"), &[run("claude", Some("solo"))]),
        Some("qwen".into())
    );
    // No brain, no forced run — no vendor segment at all (never a blank).
    assert_eq!(meta_vendor(None, &routed), None);
    // A forced chip with an EMPTY agent falls back to the brain (a bare
    // "@" would be worse than the misattribution).
    assert_eq!(
        meta_vendor(Some("qwen"), &[run("", Some("x · forced target"))]),
        Some("qwen".into())
    );
}

fn agent_reply_with_run() -> TranscriptMessage {
    TranscriptMessage {
        role: "orchestrator".into(),
        text: String::new(), // streaming reply — text arrives via deltas
        runs: vec![TranscriptRun {
            run_id: "claude-1".into(),
            agent: "claude".into(),
            chip: None,
        }],
        ..Default::default()
    }
}

#[gpui::test]
fn feed_stream_appends_incrementally_without_rebuilding_the_view(
    cx: &mut gpui::TestAppContext,
) {
    // §A-T1 no-flicker append: consecutive deltas grow the trailing reply's
    // markdown SOURCE while the markdown ENTITY identity is preserved — the
    // view is never rebuilt (no repaint of already-rendered text).
    cx.update(|cx| {
        let mut transcript = TranscriptView::new();
        transcript.sync(
            &snapshot(
                "c-1",
                true,
                vec![message("user", "q"), agent_reply_with_run()],
            ),
            cx,
        );
        let (ix, run_id) = transcript.trailing_run_id().expect("trailing agent run");
        assert_eq!(ix, 1);
        assert_eq!(run_id, "claude-1");
        let entity_id = transcript.message(ix).unwrap().markdown.entity_id();

        // First delta: establishes the buffer as the source (replace-once).
        assert!(transcript.feed_stream(ix, "Hello", cx));
        assert_eq!(
            transcript.message(ix).unwrap().markdown.read(cx).source(),
            "Hello"
        );
        assert_eq!(transcript.message(ix).unwrap().streamed_len, 5);

        // Second delta (buffer grew): only the tail is appended.
        assert!(transcript.feed_stream(ix, "Hello, world", cx));
        assert_eq!(
            transcript.message(ix).unwrap().markdown.read(cx).source(),
            "Hello, world"
        );

        // The markdown ENTITY is the SAME across deltas — no rebuild/flicker.
        assert_eq!(
            transcript.message(ix).unwrap().markdown.entity_id(),
            entity_id,
            "streaming must never rebuild the on-screen markdown entity (§A-T1)"
        );
    });
}

#[gpui::test]
fn feed_stream_is_idempotent_on_a_non_growing_buffer(cx: &mut gpui::TestAppContext) {
    // A repeated feed with the same (or shorter) buffer appends nothing — the
    // seq-idempotency at the store carries through to the render (no double
    // characters, no repaint).
    cx.update(|cx| {
        let mut transcript = TranscriptView::new();
        transcript.sync(
            &snapshot(
                "c-1",
                true,
                vec![message("user", "q"), agent_reply_with_run()],
            ),
            cx,
        );
        let ix = transcript.trailing_agent_ix().unwrap();
        assert!(transcript.feed_stream(ix, "abc", cx));
        assert!(
            !transcript.feed_stream(ix, "abc", cx),
            "the same buffer feeds nothing (idempotent)"
        );
        assert!(
            !transcript.feed_stream(ix, "ab", cx),
            "a shorter buffer is shrink-safe — no repaint"
        );
        assert_eq!(
            transcript.message(ix).unwrap().markdown.read(cx).source(),
            "abc"
        );
    });
}

#[gpui::test]
fn feed_stream_never_paints_a_user_card(cx: &mut gpui::TestAppContext) {
    // A trailing USER message is never a stream target (deltas belong to an
    // agent reply); the feed refuses it.
    cx.update(|cx| {
        let mut transcript = TranscriptView::new();
        transcript.sync(&snapshot("c-1", true, vec![message("user", "q")]), cx);
        assert_eq!(transcript.trailing_agent_ix(), None);
        // Even if called directly on the user index, nothing is painted.
        assert!(!transcript.feed_stream(0, "sneaky", cx));
        assert_eq!(transcript.message(0).unwrap().markdown.read(cx).source(), "q");
    });
}

#[gpui::test]
fn settled_sync_replaces_the_streamed_view_the_reconcile(cx: &mut gpui::TestAppContext) {
    // §A reconciliation at the render: after streaming a draft into the
    // trailing reply, a settled snapshot carrying the terminal whole text
    // REPLACES the draft (source := settled text) and retires the streamed
    // cursor — never appends on top (the double-render guard). The markdown
    // ENTITY is preserved across the reconcile (replace-in-place, not rebuild)
    // so no flicker even at the settle boundary.
    cx.update(|cx| {
        let mut transcript = TranscriptView::new();
        transcript.sync(
            &snapshot(
                "c-1",
                true,
                vec![message("user", "q"), agent_reply_with_run()],
            ),
            cx,
        );
        let ix = transcript.trailing_agent_ix().unwrap();
        let entity_id = transcript.message(ix).unwrap().markdown.entity_id();
        transcript.feed_stream(ix, "partial draft", cx);
        assert_eq!(transcript.message(ix).unwrap().streamed_len, 13);

        // The settled snapshot carries the terminal reply text.
        transcript.sync(
            &snapshot(
                "c-1",
                false,
                vec![
                    message("user", "q"),
                    message("orchestrator", "the settled full reply"),
                ],
            ),
            cx,
        );
        assert_eq!(
            transcript.message(ix).unwrap().markdown.read(cx).source(),
            "the settled full reply",
            "the settled text replaces the streamed draft (no streamed-on-top)"
        );
        assert_eq!(
            transcript.message(ix).unwrap().streamed_len,
            0,
            "the reconciled settled view carries no streamed cursor"
        );
        assert_eq!(
            transcript.message(ix).unwrap().text.as_ref(),
            "the settled full reply",
            "the copy-payload text is reconciled to the settled reply too"
        );
        assert_eq!(
            transcript.message(ix).unwrap().markdown.entity_id(),
            entity_id,
            "reconcile replaces in place — the entity is preserved (no flicker)"
        );
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
