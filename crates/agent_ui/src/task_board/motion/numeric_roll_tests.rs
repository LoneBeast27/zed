//! Behavior tests for the numeric-roll primitive — kept in a sibling file
//! (`#[path]`-included from `numeric_roll.rs`) so the source stays under the
//! 500-line ceiling while retaining private-item (`cells`, `cell_motion`,
//! `enter_dy`, `leave_dy`, `same_shape`) access via `super::*`.

use super::*;

#[test]
fn unchanged_value_is_a_noop() {
    let mut roll = RollValue::new("82%");
    assert!(!roll.animating());
    assert!(!roll.set("82%"), "same value never rolls");
    assert!(!roll.animating());
}

#[test]
fn changed_digit_rolls_and_settles() {
    let mut roll = RollValue::new("82%");
    assert!(roll.set("83%"), "a digit change repaints");
    assert!(roll.animating());
    assert_eq!(roll.value(), "83%");
}

#[test]
fn cells_roll_only_changed_digits_of_same_shape() {
    // "82%" → "83%": the ones digit rolls (8 stays, 3 replaces 2, % is a
    // sep) — the web's per-digit roll.
    let cells = cells("83%", "82%");
    assert_eq!(
        cells,
        vec![
            Cell::Digit { from: None, to: '8' }, // unchanged → no roll
            Cell::Digit { from: Some('2'), to: '3' }, // changed → rolls
            Cell::Sep('%'),
        ]
    );
}

#[test]
fn ones_place_stays_put_as_magnitude_grows() {
    // Shape change ("9s" 2 chars → "10s" 3 chars): the web REBUILDS with
    // no roll. Every cell is settled (`from = None`).
    let cells = cells("10s", "9s");
    assert_eq!(
        cells,
        vec![
            Cell::Digit { from: None, to: '1' },
            Cell::Digit { from: None, to: '0' },
            Cell::Sep('s'),
        ]
    );
}

#[test]
fn separators_never_roll() {
    // "1.2h" → "1.5h": only the tenths digit rolls; ".", "h" are seps.
    let cells = cells("1.5h", "1.2h");
    assert_eq!(
        cells,
        vec![
            Cell::Digit { from: None, to: '1' },
            Cell::Sep('.'),
            Cell::Digit { from: Some('2'), to: '5' },
            Cell::Sep('h'),
        ]
    );
}

#[test]
fn same_shape_requires_matching_digit_sep_layout() {
    assert!(same_shape("82%", "91%"), "both ##%");
    assert!(!same_shape("9s", "10s"), "different length");
    assert!(!same_shape("5 live", "5live"), "space vs none shifts shape");
}

#[test]
fn n_live_label_rolls_only_the_count() {
    // "5 live" → "6 live": only the leading count digit rolls; the space
    // and the word are seps (the board-head "N live" consumer).
    let cells = cells("6 live", "5 live");
    assert_eq!(cells[0], Cell::Digit { from: Some('5'), to: '6' });
    assert!(cells[1..].iter().all(|c| matches!(c, Cell::Sep(_))));
}

// --- P10: paint-time dy interpolation (the "HOW they roll" the cell tests
// don't cover) ----------------------------------------------------------

#[test]
fn enter_and_leave_dy_are_complementary_lerps_of_one_line() {
    let lh = gpui::px(20.);
    // Roll-start (progress 0): the new glyph sits one full line below (entering
    // from +1em), the old glyph rests at the row (leaving from 0).
    assert_eq!(enter_dy(lh, 0.), lh, "new glyph enters from +1em");
    assert_eq!(leave_dy(lh, 0.), gpui::px(0.), "old glyph starts at rest");
    // Rest (progress 1): the new glyph is at the row, the old has slid a full
    // line up and out — the entering/leaving offsets always sum to one line.
    assert_eq!(enter_dy(lh, 1.), gpui::px(0.), "new glyph settles at the row");
    assert_eq!(leave_dy(lh, 1.), lh, "old glyph slid one line up and out");
    // Midway: both are half a line, complementary (no snap — a pure lerp of an
    // already-eased progress).
    assert_eq!(enter_dy(lh, 0.5), lh * 0.5);
    assert_eq!(leave_dy(lh, 0.5), lh * 0.5);
    for &p in &[0.0_f32, 0.25, 0.5, 0.75, 1.0] {
        assert_eq!(
            enter_dy(lh, p) + leave_dy(lh, p),
            lh,
            "entering + leaving offsets span exactly one line at progress {p}"
        );
    }
}

#[test]
fn cell_motion_routes_only_changed_digits_to_rolling_and_settles_at_progress_1() {
    let unchanged = Cell::Digit { from: None, to: '8' };
    let changed = Cell::Digit { from: Some('2'), to: '3' };
    let sep = Cell::Sep('%');
    // Mid-roll: only the changed digit is Rolling; the unchanged digit and the
    // separator paint static — the per-digit law the dy offsets ride on.
    assert_eq!(cell_motion(unchanged, 0.5), CellMotion::Static('8'));
    assert_eq!(cell_motion(changed, 0.5), CellMotion::Rolling { from: '2', to: '3' });
    assert_eq!(cell_motion(sep, 0.5), CellMotion::Static('%'));
    // Settled (progress >= 1): EVERY cell is Static, so a finished roll paints
    // one bare row — the entering glyph rests at dy=0, nothing slides.
    assert_eq!(cell_motion(changed, 1.0), CellMotion::Static('3'));
}

// --- P3: RollValue::set mid-roll continuity contract --------------------
//
// True single-scalar continuity is impractical (a changed digit's in-flight
// NEW glyph is below the row entering; making it the OUTGOING glyph for the
// re-aim has no progress value — outgoing only travels upward from rest, so a
// continued role would teleport, a WORSE 1em jump than the restart). So the
// honest, roll.js-faithful contract is: a value arriving mid-roll re-bases the
// slide to roll-start with the CURRENTLY-DISPLAYED value as the new outgoing
// (`roll.js:79-89` snaps the stack to translateY(0) then re-climbs with the
// prior target as `before`). These pin THAT behavior, not the data shape.

#[test]
fn set_mid_roll_rebases_outgoing_to_the_displayed_value_not_the_original() {
    // "82%" → "83%" begins a roll; before it settles, "84%" arrives. The new
    // roll must roll from "83%" (the value on screen / rolling in) toward
    // "84%" — NOT from the original "82%". This is the minimised-snap restart:
    // the in-flight "3" becomes the outgoing glyph, so the changed column never
    // jumps farther than the half-em a clean restart costs.
    let mut roll = RollValue::new("82%");
    assert!(roll.set("83%"));
    assert!(roll.animating(), "first roll is mid-flight");
    assert!(roll.set("84%"), "a second value mid-roll repaints");
    assert!(roll.animating(), "the re-aimed roll is still animating");
    assert_eq!(roll.value(), "84%", "target follows the latest value");
    // The element pairs `current` against `previous`; after the re-aim
    // `previous` is the displayed "83%", so the ones place rolls '3'→'4' — NOT
    // the stale "82%". (Child module → private fields of the ancestor visible.)
    let el = roll.element("t", gpui::px(13.), FontWeight::NORMAL, gpui::black());
    assert_eq!(el.current, "84%");
    assert_eq!(el.previous, "83%", "outgoing is the displayed value, not 82%");
    let cells = cells(&el.current, &el.previous);
    assert_eq!(cells[1], Cell::Digit { from: Some('3'), to: '4' });
}

#[test]
fn set_mid_roll_restarts_progress_at_zero_matching_roll_js() {
    // roll.js re-climbs from translateY(0) on a chained change — the Rust port
    // restarts progress at 0. After a mid-roll re-aim the new flight starts at
    // the bottom (the new glyph fully entering), never carrying a stale blend.
    let mut roll = RollValue::new("10s");
    assert!(roll.set("11s")); // same shape — rolls
    assert!(roll.set("12s")); // chained mid-roll re-aim
    let el = roll.element("t", gpui::px(13.), FontWeight::NORMAL, gpui::black());
    assert!(
        el.progress < 0.5,
        "a chained set() re-bases progress to roll-start (got {})",
        el.progress
    );
}

#[test]
fn set_shape_change_appears_settled_with_no_entrance_roll() {
    // The needRebuild branch (`roll.js:28-33`): a digit-count change shows the
    // new value SETTLED — no roll-out and (the P2 fix) no slide-in. Even when
    // it arrives mid-roll, progress is settled so the element paints at rest.
    let mut roll = RollValue::new("9s");
    assert!(roll.set("8s")); // begin a same-shape roll
    assert!(roll.set("10s")); // shape change mid-roll → rebuild
    assert!(!roll.animating(), "a rebuild appears settled, never rolls");
    let el = roll.element("t", gpui::px(13.), FontWeight::NORMAL, gpui::black());
    assert_eq!(el.progress, 1.0, "rebuilt value paints at rest");
}
