//! The bridge store entity: the snapshot Z1+ panels render from, plus the 1s
//! elapsed ticker that keeps running runs' counters live between server
//! frames (serve.py's board digest ignores `elapsed_s` — clients tick
//! elapsed locally; RUST_PORT_NOTES §5's worked-for ticker).

use std::time::{Duration, Instant};

use gpui::{App, AppContext as _, Entity, Task};

use super::client::connection_loop;
use super::protocol::{BridgeEvent, PoolRow, RunRow, pools_from_object};

/// Local elapsed-tick cadence while any run is `running`.
const ELAPSED_TICK: Duration = Duration::from_secs(1);

/// The bridge snapshot Z1+ panels render from. `connected == false` means the
/// bridge is offline — the last snapshot is kept so panels can grey-out rather
/// than blank.
pub struct BridgeStore {
    pub board: Vec<RunRow>,
    pub usage: Vec<PoolRow>,
    pub connected: bool,
    /// When the current board snapshot arrived. The server value re-bases the
    /// local tick offset on every board frame; running runs render
    /// `elapsed_s + (now − board_received_at)`.
    board_received_at: Instant,
    /// The 1s ticker task — `Some` only while any run is `running`
    /// (Lightness Mandate: no idle timers).
    ticker: Option<Task<()>>,
}

impl Default for BridgeStore {
    fn default() -> Self {
        Self {
            board: Vec::new(),
            usage: Vec::new(),
            connected: false,
            board_received_at: Instant::now(),
            ticker: None,
        }
    }
}

/// Adds the local tick offset to every `running` run's `elapsed_s` — the
/// re-base math of the 1s ticker (settled runs keep the server value).
fn tick_elapsed(board: &mut [RunRow], offset_s: f64) {
    for run in board {
        if run.status == "running" {
            run.elapsed_s += offset_s;
        }
    }
}

impl BridgeStore {
    /// The board with locally-ticked elapsed: the server pushes a frame only
    /// when the run/status digest changes, so between frames running runs
    /// tick `elapsed_s` forward from the snapshot's arrival time.
    pub fn ticked_board(&self) -> Vec<RunRow> {
        let mut board = self.board.clone();
        tick_elapsed(&mut board, self.board_received_at.elapsed().as_secs_f64());
        board
    }

    pub(super) fn apply_event(&mut self, event: BridgeEvent, cx: &mut gpui::Context<Self>) {
        let mut changed = !self.connected;
        self.connected = true;
        match event {
            BridgeEvent::Board { board } => {
                // The server's elapsed_s is authoritative at receive time —
                // re-base the local tick even when the rows are unchanged.
                self.board_received_at = Instant::now();
                if self.board != board {
                    self.board = board;
                    changed = true;
                }
                self.update_ticker(cx);
            }
            BridgeEvent::Usage { fields } => {
                let usage = pools_from_object(&fields);
                if self.usage != usage {
                    self.usage = usage;
                    changed = true;
                }
            }
            BridgeEvent::Unknown => {}
        }
        if changed {
            cx.notify();
        }
    }

    pub(super) fn set_connected(&mut self, connected: bool, cx: &mut gpui::Context<Self>) {
        if self.connected != connected {
            self.connected = connected;
            cx.notify();
        }
    }

    /// Starts the 1s ticker when a run is live, drops it when none is. Every
    /// board mutation funnels through [`Self::apply_event`], so this is the
    /// single start/stop point; the task also self-clears as a backstop.
    fn update_ticker(&mut self, cx: &mut gpui::Context<Self>) {
        let any_running = self.board.iter().any(|run| run.status == "running");
        if !any_running {
            self.ticker = None;
        } else if self.ticker.is_none() {
            self.ticker = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(ELAPSED_TICK).await;
                    let live = this.update(cx, |store, cx| {
                        let live = store.board.iter().any(|run| run.status == "running");
                        if live {
                            cx.notify();
                        } else {
                            store.ticker = None;
                        }
                        live
                    });
                    if !matches!(live, Ok(true)) {
                        return;
                    }
                }
            }));
        }
    }
}

struct GlobalBridgeStore(Entity<BridgeStore>);

impl gpui::Global for GlobalBridgeStore {}

/// The app-wide shared store. Created lazily on first access — the bridge
/// connection task starts non-blocking when the first consumer (a Z1+ panel)
/// builds, per the Lightness Mandate's lazy-startup rule.
pub fn global_store(cx: &mut App) -> Entity<BridgeStore> {
    if let Some(global) = cx.try_global::<GlobalBridgeStore>() {
        return global.0.clone();
    }
    let store = init(cx);
    cx.set_global(GlobalBridgeStore(store.clone()));
    store
}

/// Creates the store entity and detaches the connection loop. The loop exits
/// on its own once the entity is dropped (weak-handle update failure).
pub fn init(cx: &mut App) -> Entity<BridgeStore> {
    let http_client = cx.http_client();
    cx.new(|cx| {
        cx.spawn(async move |this, cx| connection_loop(http_client, this, cx).await)
            .detach();
        BridgeStore::default()
    })
}

#[cfg(test)]
mod tests {
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
}
