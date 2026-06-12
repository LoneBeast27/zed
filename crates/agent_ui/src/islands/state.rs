//! The island lifecycle state machine (PARITY_SPEC §4.8) — a pure-logic port
//! of `island.js createStateMachine` + the threshold semantics of
//! `usage-island.js ingest/detectCrossings/pump`. No gpui: the entity that
//! owns it (usage_island.rs) arms the real timers and repaints; this module
//! decides every transition, so the whole §4.8 lifecycle is CPU-testable.
//!
//! States: rest ↔ notify ↔ held (critical) ↔ expanded. Crossings of the
//! 75/90 bounds queue and drain ONE at a time — never two morphs at once;
//! a ≥90% pool pins `held` until it drops or the user acknowledges.

use std::collections::{HashMap, VecDeque};

/// The four representations of the island (§4.8). `Held` is the pinned
/// critical extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IslandState {
    #[default]
    Rest,
    Notify,
    Held,
    Expanded,
}

/// What the owning entity must do after a machine call (timer effects only —
/// repaint/morph decisions are derived from the state + data in render).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimerCmd {
    /// Entering notify: arm the 6s auto-revert (clearing any pending timer).
    ArmHold,
    /// Entering rest with crossings queued: arm the pump after the morph
    /// settles (the web's `setTimeout(pump, MORPH_MS)`).
    ArmPump,
    /// Any other transition: clear pending timers (createStateMachine
    /// clears the hold timer on every transition).
    Clear,
}

#[derive(Debug, Default)]
pub struct IslandMachine {
    state: IslandState,
    /// pool → used% from the last ingest (threshold diffing).
    prev_used: HashMap<String, Option<f64>>,
    /// Queued threshold-crossing notifications (pool keys; one entry per
    /// bound crossed, like the web).
    queue: VecDeque<String>,
    /// Pool currently shown in notify/held.
    active_pool: Option<String>,
    /// Whether the current held state is a ≥90% critical pin.
    held_crit: bool,
    /// Native divergence from usage-island.js (documented in the module
    /// owner): an acknowledged critical pool is not re-pinned while it
    /// stays ≥90%. The web re-pins on the next 12s poll, which at SSE push
    /// cadence would undo the ack within seconds.
    acked_pool: Option<String>,
}

impl IslandMachine {
    pub fn state(&self) -> IslandState {
        self.state
    }

    pub fn active_pool(&self) -> Option<&str> {
        self.active_pool.as_deref()
    }

    pub fn queued(&self) -> usize {
        self.queue.len()
    }

    fn go(&mut self, next: IslandState) -> TimerCmd {
        self.state = next;
        match next {
            IslandState::Notify => TimerCmd::ArmHold,
            IslandState::Rest if !self.queue.is_empty() => TimerCmd::ArmPump,
            _ => TimerCmd::Clear,
        }
    }

    /// A usage snapshot arrived (`ingest()` from usage-island.js): diff the
    /// 75/90 bounds against the previous snapshot, queue crossings, pin or
    /// release the critical hold. `pools` = (name, used-%) pairs.
    pub fn ingest(&mut self, pools: &[(String, Option<f64>)]) -> TimerCmd {
        // detectCrossings: one queued notify per bound crossed upward.
        for (name, used) in pools {
            let before = self.prev_used.get(name).copied().flatten();
            if let (Some(before), Some(now)) = (before, *used) {
                for bound in [75.0, 90.0] {
                    if before < bound && now >= bound {
                        self.queue.push_back(name.clone());
                    }
                }
            }
        }
        let mut cmd = if !self.queue.is_empty() && self.state == IslandState::Rest {
            self.pump()
        } else {
            TimerCmd::Clear
        };

        // A pool at/above 90% pins HELD until it drops or is acknowledged.
        let crit = pools
            .iter()
            .find(|(_, used)| used.is_some_and(|used| used >= 90.0));
        match crit {
            Some((name, _)) if self.state != IslandState::Expanded => {
                if self.acked_pool.as_deref() != Some(name.as_str()) {
                    self.active_pool = Some(name.clone());
                    self.held_crit = true;
                    self.acked_pool = None;
                    cmd = self.go(IslandState::Held);
                }
            }
            None => {
                self.acked_pool = None;
                if self.state == IslandState::Held && self.held_crit {
                    self.held_crit = false;
                    cmd = self.go(IslandState::Rest);
                }
            }
            Some(_) => {}
        }

        for (name, used) in pools {
            self.prev_used.insert(name.clone(), *used);
        }
        cmd
    }

    /// Island click: held → acknowledge (contract, dot stays red);
    /// rest/notify → expand; expanded → no-op (rows handle their own).
    pub fn click(&mut self) -> TimerCmd {
        match self.state {
            IslandState::Expanded => TimerCmd::Clear,
            IslandState::Held => self.acknowledge(),
            _ => self.go(IslandState::Expanded),
        }
    }

    /// Click-away while expanded contracts back to rest.
    pub fn click_away(&mut self) -> TimerCmd {
        if self.state == IslandState::Expanded {
            self.held_crit = false;
            self.go(IslandState::Rest)
        } else {
            TimerCmd::Clear
        }
    }

    /// An expanded-card row was clicked (routes to the usage mode) —
    /// contract.
    pub fn row_clicked(&mut self) -> TimerCmd {
        self.held_crit = false;
        self.go(IslandState::Rest)
    }

    /// The notify 6s hold expired.
    pub fn hold_expired(&mut self) -> TimerCmd {
        if self.state == IslandState::Notify {
            self.go(IslandState::Rest)
        } else {
            TimerCmd::Clear
        }
    }

    /// Drain the next queued crossing into a notify (only ever from rest —
    /// the "never two morphs at once" guarantee).
    pub fn pump(&mut self) -> TimerCmd {
        if self.state == IslandState::Rest
            && let Some(pool) = self.queue.pop_front()
        {
            self.active_pool = Some(pool);
            return self.go(IslandState::Notify);
        }
        TimerCmd::Clear
    }

    fn acknowledge(&mut self) -> TimerCmd {
        self.held_crit = false;
        self.acked_pool = self.active_pool.clone();
        self.go(IslandState::Rest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pools(values: &[(&str, Option<f64>)]) -> Vec<(String, Option<f64>)> {
        values
            .iter()
            .map(|(name, used)| (name.to_string(), *used))
            .collect()
    }

    #[test]
    fn crossings_queue_one_notify_per_bound() {
        let mut machine = IslandMachine::default();
        machine.ingest(&pools(&[("claude", Some(70.0)), ("codex", Some(10.0))]));
        assert_eq!(machine.state(), IslandState::Rest);

        // 70 → 80 crosses 75: pumps immediately from rest.
        let cmd = machine.ingest(&pools(&[("claude", Some(80.0)), ("codex", Some(10.0))]));
        assert_eq!(machine.state(), IslandState::Notify);
        assert_eq!(machine.active_pool(), Some("claude"));
        assert_eq!(cmd, TimerCmd::ArmHold);

        // Hold expires → rest (queue empty: no pump).
        let cmd = machine.hold_expired();
        assert_eq!(machine.state(), IslandState::Rest);
        assert_eq!(cmd, TimerCmd::Clear);

        // No re-notify without a fresh crossing.
        machine.ingest(&pools(&[("claude", Some(80.0)), ("codex", Some(10.0))]));
        assert_eq!(machine.state(), IslandState::Rest);
    }

    #[test]
    fn double_bound_jump_queues_both_and_drains_one_at_a_time() {
        let mut machine = IslandMachine::default();
        machine.ingest(&pools(&[("claude", Some(70.0)), ("codex", Some(70.0))]));

        // claude jumps past both bounds, codex past one — but ≥90 pins HELD
        // over the pumped notify (held wins; the queue holds).
        machine.ingest(&pools(&[("claude", Some(91.0)), ("codex", Some(80.0))]));
        assert_eq!(machine.state(), IslandState::Held);
        assert_eq!(machine.active_pool(), Some("claude"));
        assert!(machine.queued() >= 1, "remaining crossings stay queued");

        // Pool drops below 90 → held releases to rest, then the queue pumps.
        let cmd = machine.ingest(&pools(&[("claude", Some(50.0)), ("codex", Some(80.0))]));
        assert_eq!(machine.state(), IslandState::Rest);
        assert_eq!(cmd, TimerCmd::ArmPump);
        machine.pump();
        assert_eq!(machine.state(), IslandState::Notify);
    }

    #[test]
    fn notify_queue_never_overlaps_morphs() {
        let mut machine = IslandMachine::default();
        machine.ingest(&pools(&[("a", Some(70.0)), ("b", Some(70.0))]));
        machine.ingest(&pools(&[("a", Some(80.0)), ("b", Some(80.0))]));
        // First crossing notifies, second stays queued behind it.
        assert_eq!(machine.state(), IslandState::Notify);
        assert_eq!(machine.queued(), 1);

        // pump() from notify is a no-op — only rest drains the queue.
        machine.pump();
        assert_eq!(machine.state(), IslandState::Notify);
        assert_eq!(machine.queued(), 1);

        let cmd = machine.hold_expired();
        assert_eq!(cmd, TimerCmd::ArmPump, "rest with a queue arms the pump");
        machine.pump();
        assert_eq!(machine.state(), IslandState::Notify);
        assert_eq!(machine.queued(), 0);
    }

    #[test]
    fn critical_pins_until_drop_or_ack() {
        let mut machine = IslandMachine::default();
        machine.ingest(&pools(&[("claude", Some(50.0))]));
        machine.ingest(&pools(&[("claude", Some(95.0))]));
        assert_eq!(machine.state(), IslandState::Held);

        // Ack contracts; the still-critical pool is NOT re-pinned on the
        // next ingest (native divergence: SSE cadence would undo the ack).
        machine.click();
        assert_eq!(machine.state(), IslandState::Rest);
        machine.ingest(&pools(&[("claude", Some(96.0))]));
        assert_eq!(machine.state(), IslandState::Rest, "acked pool stays acked");

        // Dropping below 90 clears the ack; a fresh ≥90 re-pins.
        machine.ingest(&pools(&[("claude", Some(50.0))]));
        machine.ingest(&pools(&[("claude", Some(95.0))]));
        assert_eq!(machine.state(), IslandState::Held);
    }

    #[test]
    fn expanded_blocks_held_pin_and_contracts_on_click_away() {
        let mut machine = IslandMachine::default();
        machine.ingest(&pools(&[("claude", Some(50.0))]));
        machine.click();
        assert_eq!(machine.state(), IslandState::Expanded);

        // Clicks on the expanded container are row-handled — no transition.
        machine.click();
        assert_eq!(machine.state(), IslandState::Expanded);

        // A critical pool does NOT steal the expanded card (web:
        // `crit && !sm.is("expanded")`).
        machine.ingest(&pools(&[("claude", Some(95.0))]));
        assert_eq!(machine.state(), IslandState::Expanded);

        // Click-away contracts; the next ingest re-pins the critical.
        machine.click_away();
        assert_eq!(machine.state(), IslandState::Rest);
        machine.ingest(&pools(&[("claude", Some(95.0))]));
        assert_eq!(machine.state(), IslandState::Held);
    }

    #[test]
    fn row_click_contracts_from_expanded() {
        let mut machine = IslandMachine::default();
        machine.click();
        assert_eq!(machine.state(), IslandState::Expanded);
        machine.row_clicked();
        assert_eq!(machine.state(), IslandState::Rest);
    }

    #[test]
    fn unknown_pools_never_cross() {
        let mut machine = IslandMachine::default();
        machine.ingest(&pools(&[("claude", None)]));
        machine.ingest(&pools(&[("claude", Some(80.0))]));
        // None → 80 is not a crossing (web: before == null → skip).
        assert_eq!(machine.state(), IslandState::Rest);
        assert_eq!(machine.queued(), 0);
        // …and back to None never pins or crosses.
        machine.ingest(&pools(&[("claude", None)]));
        assert_eq!(machine.state(), IslandState::Rest);
    }
}
