//! Pure toast lifecycle logic for the corner notification stack
//! (PARITY_SPEC §4.9) — the diff/dedupe/timer bookkeeping of
//! `notifications.js` + `island.js createNotificationQueue`, with no gpui so
//! the whole lifecycle is CPU-testable. The entity (notif_stack.rs) owns the
//! real tasks and rendering.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use crate::agent_accents::{STATUS_BLOCKED, STATUS_DONE, STATUS_ERROR};
use crate::bridge::{ApprovalRow, RunRow, ScrapeMeta};

/// Toast auto-retract timeout (Caelestia notifsconfig: 5000ms, hover pauses).
pub const TOAST_TIMEOUT: Duration = Duration::from_secs(5);
/// On-screen cap (§8 bounded DOM) — the oldest beyond it retracts.
pub const MAX_VISIBLE: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Done,
    Error,
    Stale,
    /// A pending runtime approval (Phase-2 permissions §5.2): HELD — no
    /// expiry clock; retired only by a decision (the pending frame's replace
    /// semantics) or the connection's fresh→stale latch.
    Approval,
}

impl ToastKind {
    /// Status-dot color (the TONE map in notifications.js: done/error/warn).
    pub fn color(self) -> gpui::Hsla {
        match self {
            ToastKind::Done => STATUS_DONE.into(),
            ToastKind::Error => STATUS_ERROR.into(),
            // Approval = blocked-on-YOU: the amber safety hue (MONO ruling).
            ToastKind::Stale | ToastKind::Approval => STATUS_BLOCKED.into(),
        }
    }

    /// Held toasts never arm the auto-retract clock — a pending approval
    /// stays up until decided/expired bridge-side (design §5.2: no
    /// `ExpireTimer`).
    pub fn held(self) -> bool {
        matches!(self, ToastKind::Approval)
    }
}

/// The approval-specific payload riding a [`ToastKind::Approval`] toast —
/// what the inline Allow/Deny + countdown render from.
#[derive(Debug, Clone, PartialEq)]
pub struct ApprovalMeta {
    /// The bridge approval id (`"ap_9f2c1e"`) — the decision endpoints key.
    pub approval_id: String,
    /// The matched rule, rendered (`"ask: agent=* cwd_outside_roots"`).
    pub rule: String,
    pub cwd: String,
    /// TTL deadline (unix seconds) — the countdown's anchor.
    pub expires_ts: f64,
}

/// The one payload shape every source produces.
#[derive(Debug, Clone, PartialEq)]
pub struct ToastPayload {
    /// Stable dedupe id (`run:<id>:<status>` / `stale:<age>` /
    /// `approval:<id>`): the same event never double-fires.
    pub id: String,
    pub kind: ToastKind,
    pub title: String,
    pub agent: String,
    pub run_id: Option<String>,
    /// Present only on [`ToastKind::Approval`] toasts.
    pub approval: Option<ApprovalMeta>,
}

/// `terminalKind()` — which run-status transitions toast.
pub fn terminal_kind(status: &str) -> Option<ToastKind> {
    match status {
        "completed" => Some(ToastKind::Done),
        "failed" | "killed" => Some(ToastKind::Error),
        // Explicitly NON-terminal (Phase-2 §5.2): an awaiting run is held,
        // not settled — it resumes on allow or finishes FAILED on deny, and
        // THAT transition toasts. Never a Done/Error toast here.
        "awaiting_approval" => None,
        _ => None,
    }
}

/// `boardPayload()`.
pub fn board_payload(run: &RunRow, kind: ToastKind) -> ToastPayload {
    let task = run
        .task
        .clone()
        .filter(|task| !task.is_empty())
        .unwrap_or_else(|| "untitled task".to_string());
    let suffix = match kind {
        ToastKind::Error => " — blocked",
        _ => " — done",
    };
    ToastPayload {
        id: format!("run:{}:{}", run.run_id, run.status),
        kind,
        title: format!("{task}{suffix}"),
        agent: run.agent.clone(),
        run_id: Some(run.run_id.clone()),
        approval: None,
    }
}

/// `stalePayload()`.
pub fn stale_payload(scrape: Option<&ScrapeMeta>) -> ToastPayload {
    let age = scrape.and_then(|scrape| scrape.age_h);
    let age_label = age.map(|age| format!(" ({age}h old)")).unwrap_or_default();
    ToastPayload {
        id: format!(
            "stale:{}",
            age.map(|age| age.to_string()).unwrap_or_else(|| "?".into())
        ),
        kind: ToastKind::Stale,
        title: format!("Usage scrape went stale{age_label}"),
        agent: String::new(),
        run_id: None,
        approval: None,
    }
}

/// The stable toast id for a pending approval — ONE id per approval (not
/// per frame), so the dedupe set is also the "user dismissed it, don't
/// pester" memory while the approval stays pending.
pub fn approval_toast_id(approval_id: &str) -> String {
    format!("approval:{approval_id}")
}

/// `approvalPayload()` — a held [`ToastKind::Approval`] toast from one
/// pending row (§5.2 body: agent + task_head + matched rule).
pub fn approval_payload(row: &ApprovalRow) -> ToastPayload {
    let title = if row.task_head.trim().is_empty() {
        "Subagent spawn awaiting approval".to_string()
    } else {
        row.task_head.clone()
    };
    ToastPayload {
        id: approval_toast_id(&row.id),
        kind: ToastKind::Approval,
        title,
        agent: row.agent.clone(),
        run_id: (!row.run_id.is_empty()).then(|| row.run_id.clone()),
        approval: Some(ApprovalMeta {
            approval_id: row.id.clone(),
            rule: row.rule.clone(),
            cwd: row.cwd.clone(),
            expires_ts: row.expires_ts,
        }),
    }
}

/// One reconcile of the held approval toasts against the live pending set
/// (replace-on-frame semantics — the frame IS the truth):
///
/// - **surface**: pending rows not yet surfaced (`seen` is the dedupe AND
///   the dismissed-but-still-pending memory: a toast the user retracted by
///   hand is NOT re-raised while its approval stays pending).
/// - **retire**: held toast ids whose approval left the pending set — a
///   decision or TTL expiry landed; the empty frame carries the signal, no
///   tombstone needed. Their `seen` slots are pruned so the ids are free.
/// - **disconnect** (`connected == false`): every held approval toast
///   retires (the pending registry is process-lifetime bridge-side — a
///   stale connection can't vouch for it) and every approval `seen` slot is
///   cleared, so a reconnect re-raises whatever is genuinely still pending.
///   This is the fresh→stale retirement latch of §6 "bridge restart".
pub fn sync_approvals(
    connected: bool,
    pending: &[ApprovalRow],
    held: &[String],
    seen: &mut HashSet<String>,
) -> (Vec<ToastPayload>, Vec<String>) {
    if !connected {
        seen.retain(|id| !id.starts_with("approval:"));
        return (Vec::new(), held.to_vec());
    }
    let live: HashSet<String> = pending
        .iter()
        .map(|row| approval_toast_id(&row.id))
        .collect();
    let surface = pending
        .iter()
        .filter(|row| !seen.contains(&approval_toast_id(&row.id)))
        .map(approval_payload)
        .collect();
    let retire = held
        .iter()
        .filter(|id| !live.contains(*id))
        .cloned()
        .collect();
    // Prune resolved approvals out of the dedupe set (their ids never
    // recur bridge-side, but the set must not grow unbounded).
    seen.retain(|id| !id.starts_with("approval:") || live.contains(id));
    (surface, retire)
}

/// Unix-seconds "now" (the `ago_now` idiom) — the approval countdown's
/// clock, shared by the toast card and the run drawer.
pub fn unix_now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|epoch| epoch.as_secs_f64())
        .unwrap_or(0.0)
}

/// Compact countdown text for the approval TTL ("34s" / "4m32s"); an
/// elapsed deadline reads "expired" (the bridge denies fail-closed — the
/// next frame retires the toast).
pub fn expiry_countdown(expires_ts: f64, now_unix: f64) -> String {
    let left = expires_ts - now_unix;
    if left <= 0.0 {
        return "expired".to_string();
    }
    let left = left.ceil() as i64;
    if left < 60 {
        format!("{left}s")
    } else {
        format!("{}m{:02}s", left / 60, left % 60)
    }
}

/// `pollBoard()`'s diff: a transition INTO a terminal status fires exactly
/// once. The first snapshot only seeds `prev` — no toast storm on connect
/// (the `primed` latch); after priming, a run that first appears already
/// terminal DOES toast (it completed between frames).
pub fn diff_board(
    prev: &mut HashMap<String, String>,
    primed: &mut bool,
    board: &[RunRow],
) -> Vec<ToastPayload> {
    let mut out = Vec::new();
    let mut next = HashMap::new();
    for run in board {
        next.insert(run.run_id.clone(), run.status.clone());
        let before = prev.get(&run.run_id);
        if !*primed && before.is_none() {
            continue;
        }
        if before.is_some_and(|before| before == &run.status) {
            continue;
        }
        if let Some(kind) = terminal_kind(&run.status) {
            out.push(board_payload(run, kind));
        }
    }
    *prev = next;
    *primed = true;
    out
}

/// `pollUsage()`'s staleness latch: only a fresh→stale flip fires (a feed
/// that CONNECTS stale stays quiet; stale→fresh re-arms the latch).
#[derive(Debug, Default)]
pub struct StaleLatch {
    prev: Option<bool>,
}

impl StaleLatch {
    pub fn flip(&mut self, stale: bool) -> bool {
        let fired = self.prev == Some(false) && stale;
        self.prev = Some(stale);
        fired
    }
}

/// The hover-pausable expiry clock (`createNotificationQueue` arm/pause/
/// resume): pausing banks the remaining time; resuming spends what's left.
/// The entity pairs this with a real timer task using
/// [`remaining`](Self::remaining).
#[derive(Debug)]
pub struct ExpireTimer {
    remaining: Duration,
    running_since: Option<Instant>,
}

impl ExpireTimer {
    pub fn armed(timeout: Duration) -> Self {
        Self {
            remaining: timeout,
            running_since: Some(Instant::now()),
        }
    }

    pub fn pause(&mut self) {
        if let Some(since) = self.running_since.take() {
            self.remaining = self.remaining.saturating_sub(since.elapsed());
        }
    }

    pub fn resume(&mut self) {
        if self.running_since.is_none() {
            self.running_since = Some(Instant::now());
        }
    }

    /// Time left on the clock right now.
    pub fn remaining(&self) -> Duration {
        match self.running_since {
            Some(since) => self.remaining.saturating_sub(since.elapsed()),
            None => self.remaining,
        }
    }

    pub fn paused(&self) -> bool {
        self.running_since.is_none()
    }
}

#[cfg(test)]
#[path = "notif_logic_tests.rs"]
mod tests;
