//! Tests for the pure toast lifecycle logic (notif_logic.rs) — board
//! diffing, the stale latch, the pausable expiry clock, and the Phase-2
//! §5.2 held-approval reconcile (surface-once, retire-on-resolve, the
//! disconnect latch, the countdown). Sibling file (the `#[path]` idiom) to
//! keep notif_logic.rs under the line ceiling.

use super::*;

use super::*;

fn run(id: &str, status: &str, task: &str) -> RunRow {
    RunRow {
        run_id: id.to_string(),
        status: status.to_string(),
        task: Some(task.to_string()),
        agent: "claude".to_string(),
        ..Default::default()
    }
}

fn approval(id: &str, run_id: &str) -> ApprovalRow {
    ApprovalRow {
        id: id.to_string(),
        run_id: run_id.to_string(),
        agent: "codex".to_string(),
        task_head: "deploy the thing".to_string(),
        rule: "ask: agent=* cwd_outside_roots".to_string(),
        cwd: "L:\\Projects\\other".to_string(),
        expires_ts: 1_000_300.0,
        ..Default::default()
    }
}

#[test]
fn terminal_kind_maps_statuses() {
    assert_eq!(terminal_kind("completed"), Some(ToastKind::Done));
    assert_eq!(terminal_kind("failed"), Some(ToastKind::Error));
    assert_eq!(terminal_kind("killed"), Some(ToastKind::Error));
    assert_eq!(terminal_kind("running"), None);
    assert_eq!(terminal_kind("pending"), None);
    // Phase-2 §5.2: awaiting is HELD, explicitly non-terminal — the
    // allow/deny transition toasts, never the wait itself.
    assert_eq!(terminal_kind("awaiting_approval"), None);
}

#[test]
fn approval_toasts_are_held_and_amber() {
    assert!(ToastKind::Approval.held(), "no ExpireTimer on approvals");
    assert!(!ToastKind::Done.held());
    assert!(!ToastKind::Error.held());
    assert!(!ToastKind::Stale.held());
    assert_eq!(ToastKind::Approval.color(), gpui::Hsla::from(STATUS_BLOCKED));
}

#[test]
fn approval_payload_carries_the_decision_anatomy() {
    let payload = approval_payload(&approval("ap_1", "r-7"));
    assert_eq!(payload.id, "approval:ap_1");
    assert_eq!(payload.kind, ToastKind::Approval);
    assert_eq!(payload.title, "deploy the thing");
    assert_eq!(payload.agent, "codex");
    assert_eq!(payload.run_id.as_deref(), Some("r-7"));
    let meta = payload.approval.expect("approval meta rides the payload");
    assert_eq!(meta.approval_id, "ap_1");
    assert_eq!(meta.rule, "ask: agent=* cwd_outside_roots");
    assert_eq!(meta.cwd, "L:\\Projects\\other");
    assert_eq!(meta.expires_ts, 1_000_300.0);

    // Empty task_head → an honest fallback title, never a blank card.
    let mut bare = approval("ap_2", "");
    bare.task_head = String::new();
    let payload = approval_payload(&bare);
    assert_eq!(payload.title, "Subagent spawn awaiting approval");
    assert_eq!(payload.run_id, None);
}

#[test]
fn sync_surfaces_pending_once_and_retires_on_resolve() {
    let mut seen = HashSet::new();
    // Frame 1: one pending approval surfaces.
    let (surface, retire) =
        sync_approvals(true, &[approval("ap_1", "r-1")], &[], &mut seen);
    assert_eq!(surface.len(), 1);
    assert_eq!(surface[0].id, "approval:ap_1");
    assert!(retire.is_empty());
    seen.insert(surface[0].id.clone()); // the stack's surface() does this

    // Frame 2 (same set, toast held): nothing new, nothing retired.
    let held = vec!["approval:ap_1".to_string()];
    let (surface, retire) =
        sync_approvals(true, &[approval("ap_1", "r-1")], &held, &mut seen);
    assert!(surface.is_empty(), "dedupe: the same approval never re-fires");
    assert!(retire.is_empty());

    // Frame 3 (empty frame = the decision landed): the held toast
    // retires — replace semantics carry the signal, no tombstone.
    let (surface, retire) = sync_approvals(true, &[], &held, &mut seen);
    assert!(surface.is_empty());
    assert_eq!(retire, vec!["approval:ap_1".to_string()]);
    assert!(seen.is_empty(), "resolved ids are pruned from the dedupe set");
}

#[test]
fn sync_never_pesters_a_dismissed_but_pending_approval() {
    // The user retracted the toast by hand (×) while the approval is
    // still pending: seen holds the id, held does not — no re-raise.
    let mut seen = HashSet::from(["approval:ap_1".to_string()]);
    let (surface, retire) =
        sync_approvals(true, &[approval("ap_1", "r-1")], &[], &mut seen);
    assert!(surface.is_empty(), "dismissed-but-pending must not re-raise");
    assert!(retire.is_empty());
    assert!(seen.contains("approval:ap_1"), "memory kept while pending");
}

#[test]
fn sync_disconnect_retires_all_and_rearms_for_reconnect() {
    // Bridge restart (§6): pending registry is process-lifetime — a
    // stale connection can't vouch for the held toast. Retire it and
    // clear the approval seen slots so a reconnect re-raises what's
    // genuinely still pending.
    let mut seen = HashSet::from([
        "approval:ap_1".to_string(),
        "run:r-9:completed".to_string(),
    ]);
    let held = vec!["approval:ap_1".to_string()];
    let (surface, retire) =
        sync_approvals(false, &[approval("ap_1", "r-1")], &held, &mut seen);
    assert!(surface.is_empty(), "a stale frame surfaces nothing");
    assert_eq!(retire, held, "fresh→stale latch retires the held toast");
    assert!(!seen.contains("approval:ap_1"), "approval slots cleared");
    assert!(
        seen.contains("run:r-9:completed"),
        "run-toast dedupe memory untouched"
    );

    // Reconnect with the approval still pending: it re-raises.
    let (surface, _) = sync_approvals(true, &[approval("ap_1", "r-1")], &[], &mut seen);
    assert_eq!(surface.len(), 1, "still-blocking approvals re-raise on reconnect");
}

#[test]
fn expiry_countdown_formats_and_fails_closed() {
    assert_eq!(expiry_countdown(1_000_300.0, 1_000_000.0), "5m00s");
    assert_eq!(expiry_countdown(1_000_034.5, 1_000_000.0), "35s");
    assert_eq!(expiry_countdown(1_000_090.0, 1_000_000.0), "1m30s");
    assert_eq!(expiry_countdown(1_000_000.0, 1_000_000.0), "expired");
    assert_eq!(expiry_countdown(999_000.0, 1_000_000.0), "expired");
}

#[test]
fn first_snapshot_seeds_without_toasting() {
    let mut prev = HashMap::new();
    let mut primed = false;
    // Pre-existing completed runs on connect must NOT storm.
    let toasts = diff_board(
        &mut prev,
        &mut primed,
        &[run("r-1", "completed", "old work"), run("r-2", "running", "live")],
    );
    assert!(toasts.is_empty());
    assert!(primed);

    // The live run completing IS a transition.
    let toasts = diff_board(
        &mut prev,
        &mut primed,
        &[run("r-1", "completed", "old work"), run("r-2", "completed", "live")],
    );
    assert_eq!(toasts.len(), 1);
    assert_eq!(toasts[0].id, "run:r-2:completed");
    assert_eq!(toasts[0].kind, ToastKind::Done);
    assert_eq!(toasts[0].title, "live — done");
    assert_eq!(toasts[0].run_id.as_deref(), Some("r-2"));

    // Staying completed never re-toasts.
    let toasts = diff_board(
        &mut prev,
        &mut primed,
        &[run("r-2", "completed", "live")],
    );
    assert!(toasts.is_empty());
}

#[test]
fn post_prime_appearing_terminal_run_toasts() {
    let mut prev = HashMap::new();
    let mut primed = false;
    diff_board(&mut prev, &mut primed, &[]);
    // A run that appears already failed completed between frames.
    let toasts = diff_board(&mut prev, &mut primed, &[run("r-9", "failed", "broke")]);
    assert_eq!(toasts.len(), 1);
    assert_eq!(toasts[0].kind, ToastKind::Error);
    assert_eq!(toasts[0].title, "broke — blocked");
}

#[test]
fn untitled_runs_get_the_fallback_title() {
    let mut row = run("r-1", "completed", "");
    row.task = None;
    assert_eq!(
        board_payload(&row, ToastKind::Done).title,
        "untitled task — done"
    );
}

#[test]
fn stale_latch_fires_only_on_fresh_to_stale() {
    let mut latch = StaleLatch::default();
    assert!(!latch.flip(true), "first observation stale: seed only");
    assert!(!latch.flip(true));
    assert!(!latch.flip(false), "recovery never toasts");
    assert!(latch.flip(true), "fresh→stale fires");
    assert!(!latch.flip(true), "…once");
}

#[test]
fn stale_payload_carries_age() {
    let scrape = ScrapeMeta {
        stale: true,
        age_h: Some(36.2),
        ..Default::default()
    };
    let payload = stale_payload(Some(&scrape));
    assert_eq!(payload.id, "stale:36.2");
    assert_eq!(payload.title, "Usage scrape went stale (36.2h old)");
    assert_eq!(payload.kind, ToastKind::Stale);
    assert_eq!(stale_payload(None).id, "stale:?");
}

#[test]
fn expire_timer_banks_remaining_across_pauses() {
    let mut timer = ExpireTimer::armed(Duration::from_secs(5));
    assert!(!timer.paused());
    assert!(timer.remaining() <= Duration::from_secs(5));

    timer.pause();
    assert!(timer.paused());
    let banked = timer.remaining();
    std::thread::sleep(Duration::from_millis(15));
    // Paused clocks don't tick.
    assert_eq!(timer.remaining(), banked);

    timer.resume();
    assert!(!timer.paused());
    assert!(timer.remaining() <= banked);
    // Double-resume is a no-op (web queue.resume guards rec.t).
    timer.resume();
    assert!(timer.remaining() <= banked);
}
