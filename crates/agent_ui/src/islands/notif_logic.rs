//! Pure toast lifecycle logic for the corner notification stack
//! (PARITY_SPEC §4.9) — the diff/dedupe/timer bookkeeping of
//! `notifications.js` + `island.js createNotificationQueue`, with no gpui so
//! the whole lifecycle is CPU-testable. The entity (notif_stack.rs) owns the
//! real tasks and rendering.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::agent_accents::{STATUS_BLOCKED, STATUS_DONE, STATUS_ERROR};
use crate::bridge::{RunRow, ScrapeMeta};

/// Toast auto-retract timeout (Caelestia notifsconfig: 5000ms, hover pauses).
pub const TOAST_TIMEOUT: Duration = Duration::from_secs(5);
/// On-screen cap (§8 bounded DOM) — the oldest beyond it retracts.
pub const MAX_VISIBLE: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Done,
    Error,
    Stale,
}

impl ToastKind {
    /// Status-dot color (the TONE map in notifications.js: done/error/warn).
    pub fn color(self) -> gpui::Hsla {
        match self {
            ToastKind::Done => STATUS_DONE.into(),
            ToastKind::Error => STATUS_ERROR.into(),
            ToastKind::Stale => STATUS_BLOCKED.into(),
        }
    }
}

/// The one payload shape every source produces.
#[derive(Debug, Clone, PartialEq)]
pub struct ToastPayload {
    /// Stable dedupe id (`run:<id>:<status>` / `stale:<age>`): the same
    /// event never double-fires.
    pub id: String,
    pub kind: ToastKind,
    pub title: String,
    pub agent: String,
    pub run_id: Option<String>,
}

/// `terminalKind()` — which run-status transitions toast.
pub fn terminal_kind(status: &str) -> Option<ToastKind> {
    match status {
        "completed" => Some(ToastKind::Done),
        "failed" | "killed" => Some(ToastKind::Error),
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
mod tests {
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

    #[test]
    fn terminal_kind_maps_statuses() {
        assert_eq!(terminal_kind("completed"), Some(ToastKind::Done));
        assert_eq!(terminal_kind("failed"), Some(ToastKind::Error));
        assert_eq!(terminal_kind("killed"), Some(ToastKind::Error));
        assert_eq!(terminal_kind("running"), None);
        assert_eq!(terminal_kind("pending"), None);
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
}
