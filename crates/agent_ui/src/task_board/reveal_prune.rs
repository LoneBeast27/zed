//! Reveal-spring housekeeping (P7). The task-board panel keeps one
//! velocity-carrying [`AnimatedValue`] per hovered row's pill-elapsed reveal
//! width. Before P7 the only prune ran on hover/un-hover ([`retarget_reveal`]),
//! so a row that was hover-revealed and then LEFT the board (run completed /
//! filtered out) WITHOUT a mouse-leave kept its (settled) map entry forever —
//! GPUI's `.on_hover` does not synthesise `on_hover(false)` when the row stops
//! being built. A bounded but ever-growing leak over a long session.
//!
//! [`prune_reveal_springs`] is the per-render retain that closes it, mirroring
//! the sibling `usage_panel` meter/roll-map discipline (retain against the live
//! pool set every render). It is a pure function so the keep/drop decision is
//! unit-testable without a `Window`.
//!
//! [`AnimatedValue`]: super::motion::AnimatedValue
//! [`retarget_reveal`]: super::panel::TaskBoardPanel

use std::collections::{HashMap, HashSet};

use gpui::SharedString;

use super::motion::AnimatedValue;

/// Retain only the reveal springs whose row is still on the board OR whose
/// spring is still animating — drop the rest. Keeping the `|| animating()` arm
/// lets an in-flight collapse on a just-vanished row finish its slide rather
/// than snapping closed; a SETTLED entry for a vanished row is the leak, so it
/// goes. `live` is the set of run-ids the current board frame is rendering.
pub(super) fn prune_reveal_springs(
    springs: &mut HashMap<SharedString, AnimatedValue>,
    live: &HashSet<SharedString>,
) {
    springs.retain(|id, spring| live.contains(id) || spring.animating());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task_board::motion::CHROME_OMEGA;

    fn live(ids: &[&str]) -> HashSet<SharedString> {
        ids.iter().map(|s| SharedString::from(s.to_string())).collect()
    }

    fn key(s: &str) -> SharedString {
        SharedString::from(s.to_string())
    }

    #[test]
    fn settled_entry_for_a_vanished_row_is_dropped() {
        // The leak: a row was hover-revealed (spring settled at target=1), then
        // left the board with no mouse-leave. Its settled entry must be pruned.
        let mut springs = HashMap::new();
        springs.insert(key("gone"), AnimatedValue::spring(1., CHROME_OMEGA)); // settled, not animating
        assert!(!springs[&key("gone")].animating(), "precondition: settled");

        prune_reveal_springs(&mut springs, &live(&["still-here"]));
        assert!(
            !springs.contains_key(&key("gone")),
            "a settled reveal for a row no longer on the board must be pruned"
        );
    }

    #[test]
    fn settled_entry_still_on_the_board_is_kept() {
        // A revealed row that is STILL rendering keeps its spring (so the
        // reveal stays open across renders).
        let mut springs = HashMap::new();
        springs.insert(key("hovered"), AnimatedValue::spring(1., CHROME_OMEGA));
        prune_reveal_springs(&mut springs, &live(&["hovered"]));
        assert!(springs.contains_key(&key("hovered")), "live rows are kept");
    }

    #[test]
    fn animating_entry_off_the_board_is_kept_until_it_settles() {
        // A row that vanished WHILE its reveal was still collapsing keeps the
        // spring so the in-flight slide finishes rather than snapping shut.
        let mut springs = HashMap::new();
        let mut spring = AnimatedValue::spring(1., CHROME_OMEGA);
        spring.retarget(0.); // begin a collapse — now animating
        assert!(spring.animating(), "precondition: mid-collapse");
        springs.insert(key("collapsing"), spring);

        prune_reveal_springs(&mut springs, &live(&[])); // board has no rows
        assert!(
            springs.contains_key(&key("collapsing")),
            "an in-flight collapse on a vanished row must complete, not snap"
        );
    }

    #[test]
    fn empty_board_drops_all_settled_entries() {
        let mut springs = HashMap::new();
        springs.insert(key("a"), AnimatedValue::spring(1., CHROME_OMEGA));
        springs.insert(key("b"), AnimatedValue::spring(0., CHROME_OMEGA));
        prune_reveal_springs(&mut springs, &live(&[]));
        assert!(springs.is_empty(), "a board with no rows clears settled reveals");
    }
}
