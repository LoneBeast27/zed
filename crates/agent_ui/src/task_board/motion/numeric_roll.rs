//! The §0 numeric-roll primitive — "a small custom element painting two
//! offset text runs during the transition" (RUST_PORT_NOTES §0; web
//! `roll.js`). One helper, consumed everywhere a live counter lives: the
//! usage island rest-%, the usage panel pool %, the task-board run count +
//! "N live", the tasks-island "N subagents" count.
//!
//! **Genuinely per-digit.** The web rolls PER DIGIT — `roll.js:41`
//! `if (before === ch) return;` leaves an unchanged digit untouched, `:39`
//! `setSep` swaps a separator with no transform; ONLY a changed digit's stack
//! slides (`:84-88` `translateY(0)`→`translateY(-1em)`). Tabular-nums gives
//! every digit a fixed advance, so the paint mirrors that exactly: unchanged
//! digits and separators paint ONCE at rest (the `static_glyphs` group, dy=0)
//! and never move; only the changed digits ride the slide (`incoming`/
//! `outgoing`). A shape change (different digit count) takes the `needRebuild`
//! branch — the new value appears settled with no roll (`roll.js:28-33`).
//!
//! **Identity & the pump.** A stateless paint helper, not a timer-owning
//! element: the caller owns one [`RollValue`] plus a single frame pump (the
//! `request_animation_frame` / `any_animating` idiom the islands arm), reads
//! the roll progress per frame, and keys the element by a stable id so it
//! never churns identity (§8.7a). Settled, it paints one bare line — zero
//! per-frame cost (§8 idle). A roll lays out at its TARGET width, so the
//! island's geometry math stays frame-exact across a roll (no measure drift).

use std::time::Duration;

use gpui::{
    App, Bounds, Element, ElementId, FontWeight, GlobalElementId, Hsla, InspectorElementId,
    IntoElement, LayoutId, Pixels, ShapedLine, Style, TextAlign, TextRun, Window, point, size,
};

use super::animated::AnimatedValue;
use super::curves::EFFECTS;

/// Per-digit roll duration — web `roll.js` `ROLL_MS = 200` on the effects
/// curve.
pub const ROLL_MS: Duration = Duration::from_millis(200);

/// Tracks one live counter's rolling state: the displayed string plus a
/// 0→1 roll-progress scalar that slides changed digits old-up / new-in.
///
/// **Mid-roll continuity (the honest contract):** a single 0→1 progress
/// scalar drives ALL changed digits in lockstep, exactly like `roll.js`'s one
/// `requestAnimationFrame` per `rollNumber` call. When a NEW value arrives
/// while a roll is still in flight, [`set`](Self::set) re-bases the slide to
/// roll-start (progress 0) with the CURRENTLY-DISPLAYED value as the new
/// outgoing — the same restart-from-0 the web performs (`roll.js:79-89`:
/// `transition="none"; transform=translateY(0)` then re-climbs, with the
/// prior *target* as `before`). The in-flight blend is NOT carried across the
/// re-aim — that would need per-column progress, which diverges from the
/// web's single-clock roll and (per the design-check) produces a WORSE
/// changed-digit jump (incoming→outgoing role flip = a full 1em) than the
/// 0.5em a clean restart costs. Board counters tick ≈1Hz, far longer than the
/// 200ms roll, so this mid-flight path is rarely reached; the snap is
/// minimised by reusing the rendered value as the new outgoing.
#[derive(Debug, Clone)]
pub struct RollValue {
    /// The string currently on screen (the digits the NEW value rolls in).
    current: String,
    /// The string being rolled away from (the digits sliding up and out).
    previous: String,
    /// 0 = `previous` shown, 1 = `current` settled. Eased on the effects
    /// curve, frame-pumped by the caller.
    progress: AnimatedValue,
}

impl RollValue {
    /// A settled counter showing `value` (no entrance roll).
    pub fn new(value: impl Into<String>) -> Self {
        let value = value.into();
        Self {
            previous: value.clone(),
            current: value,
            progress: AnimatedValue::settled(1., EFFECTS, ROLL_MS),
        }
    }

    /// Record the latest value. A real change rolls from the string on
    /// screen toward `value`; an unchanged value is a no-op (render loops
    /// call this every frame). Returns whether a repaint is needed.
    pub fn set(&mut self, value: impl Into<String>) -> bool {
        let value = value.into();
        if value == self.current {
            return false;
        }
        // A digit-COUNT / shape change takes the web's `needRebuild` branch
        // (`roll.js:28-33`): the new value appears SETTLED with no roll — no
        // glyph rolls out, and (the P2 fix) the new line does NOT slide in
        // from +1em either. `cells()` already settles every cell on a shape
        // change; settling progress at 1 makes `enter_dy = 0` so the rebuilt
        // value is painted at rest. The container width morphs on its own
        // SPATIAL clock (the usage island), independent of this content swap.
        if !same_shape(&value, &self.current) {
            self.current = value;
            self.previous = self.current.clone();
            self.progress = AnimatedValue::settled(1., EFFECTS, ROLL_MS);
            return true;
        }
        // Same shape: the changed digits roll. The outgoing digits are
        // whatever is rendered RIGHT NOW (the value being rolled in). If a
        // prior roll is still mid-flight, this RE-BASES the slide to roll-
        // start (progress 0) with the current value as outgoing — the same
        // restart the web does (`roll.js:79-89`); the in-flight blend is not
        // carried (see the type doc for why per-column continuity is rejected).
        self.previous = std::mem::replace(&mut self.current, value);
        self.progress.jump(0.);
        self.progress.retarget(1.);
        true
    }

    /// Whether a roll is mid-flight — the caller's frame-pump signal.
    pub fn animating(&self) -> bool {
        self.progress.animating()
    }

    /// Animation-identity key for the latest roll — lets a caller fold the
    /// roll into a shared `with_animation` wrapper's generation so a
    /// roll-only update restarts (and thus pumps) that wrapper.
    pub fn generation(&self) -> usize {
        self.progress.generation()
    }

    /// The settled display string.
    pub fn value(&self) -> &str {
        &self.current
    }

    /// The roll element for this value at `font_size`/`weight`, tinted
    /// `color`, keyed by `id`. Tabular-nums is applied by the caller's
    /// font features on the shaped runs here.
    pub fn element(
        &self,
        id: impl Into<ElementId>,
        font_size: Pixels,
        weight: FontWeight,
        color: Hsla,
    ) -> NumericRoll {
        NumericRoll {
            id: id.into(),
            current: self.current.clone(),
            previous: self.previous.clone(),
            progress: self.progress.current().clamp(0., 1.),
            font_size,
            weight,
            color,
        }
    }
}

/// Pairs the displayed string against the one being rolled away from and
/// classifies each cell as a digit that rolls or a separator that swaps
/// instantly — the web `roll.js` `shapeOf`/`isDigit` split, aligned from the
/// ones place so the ones digit stays put as magnitude grows ("9s"→"10s").
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Cell {
    /// A digit cell. `from` is the previous glyph (when the digit changed
    /// and shapes matched — it rolls up and out), `to` is the settled glyph.
    Digit { from: Option<char>, to: char },
    /// A non-digit (".", "s", "%", " ") — swaps instantly, never rolls.
    Sep(char),
}

/// True when the two strings have the same digit/sep SHAPE — only then do
/// changed digits roll (web `roll.js`: a different digit count rebuilds and
/// shows the new value with no roll).
fn same_shape(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.chars()
            .zip(b.chars())
            .all(|(x, y)| x.is_ascii_digit() == y.is_ascii_digit())
}

/// Classify `current` against `previous` into roll/swap cells (the digit-
/// transition math, unit-tested). When shapes differ every cell is settled
/// (`from = None`) — the rebuild branch.
pub fn cells(current: &str, previous: &str) -> Vec<Cell> {
    let rolls = same_shape(current, previous);
    let prev: Vec<char> = previous.chars().collect();
    current
        .chars()
        .enumerate()
        .map(|(i, ch)| {
            if !ch.is_ascii_digit() {
                return Cell::Sep(ch);
            }
            let from = if rolls {
                prev.get(i).copied().filter(|before| *before != ch)
            } else {
                None
            };
            Cell::Digit { from, to: ch }
        })
        .collect()
}

/// Where a cell's NEW glyph paints during a roll — the pure routing the
/// per-digit paint follows (extracted so the "only changed digits move" law is
/// unit-testable WITHOUT a `Window`). `Static` glyphs are painted once at rest
/// and never move; a `Rolling` digit slides its new glyph in and its `from`
/// glyph out.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CellMotion {
    /// Painted at rest (dy=0), never moves: unchanged digit, separator, or any
    /// cell once the roll has settled / on a shape rebuild.
    Static(char),
    /// A changed digit mid-roll: `to` enters from +1em, `from` leaves to −1em.
    Rolling { from: char, to: char },
}

/// Classify a cell for paint given the current roll progress. Once settled
/// (`progress >= 1.`) EVERY cell is `Static` — so a finished roll, and the
/// `needRebuild` shape-change case (all cells `from: None`), paint one bare
/// row at rest (no slide, no phantom entrance). This is the single source of
/// truth the prepaint loop and the tests share.
pub fn cell_motion(cell: Cell, progress: f32) -> CellMotion {
    match cell {
        Cell::Digit { from: Some(from), to } if progress < 1. => CellMotion::Rolling { from, to },
        Cell::Digit { to, .. } => CellMotion::Static(to),
        Cell::Sep(ch) => CellMotion::Static(ch),
    }
}

/// Vertical offset of a CHANGED digit's NEW (entering) glyph at `progress`:
/// +1 line-height at roll-start (just below the row), 0 at rest
/// (`roll.js`: the stack's lower glyph rises into view).
pub fn enter_dy(line_height: Pixels, progress: f32) -> Pixels {
    line_height * (1. - progress)
}

/// Vertical offset (above the row, so the paint subtracts it) of a CHANGED
/// digit's OLD (leaving) glyph at `progress`: 0 at roll-start, +1 line-height
/// at rest (slid up and out — `roll.js`: the stack's upper glyph rises away).
pub fn leave_dy(line_height: Pixels, progress: f32) -> Pixels {
    line_height * progress
}

/// The roll element: a small custom element painting, per changed digit, the
/// old glyph and the new glyph offset vertically and clipped to one line so
/// the old slides up and out while the new slides in (the §0 "two offset text
/// runs" idiom). Lays out at the target string's shaped width so consumers'
/// geometry measurement is unaffected.
pub struct NumericRoll {
    id: ElementId,
    current: String,
    previous: String,
    progress: f32,
    font_size: Pixels,
    weight: FontWeight,
    color: Hsla,
}

/// Shaped runs grouped by motion so only the changed digits ever move:
/// `static_glyphs` (non-rolling new glyphs — unchanged digits, separators, and
/// when settled EVERY glyph — painted once at rest), `incoming` (each changed
/// digit's NEW glyph, entering from +1em), `outgoing` (each changed digit's OLD
/// glyph, leaving to −1em).
pub struct RollPrepaint {
    line_height: Pixels,
    /// (x within bounds, shaped glyph) for each non-rolling new glyph.
    static_glyphs: Vec<(Pixels, ShapedLine)>,
    /// (x within bounds, shaped NEW glyph) for each changed digit, entering.
    incoming: Vec<(Pixels, ShapedLine)>,
    /// (x within bounds, shaped OLD glyph) for each changed digit, leaving.
    outgoing: Vec<(Pixels, ShapedLine)>,
}

impl NumericRoll {
    fn run(&self, text: &str, window: &Window) -> (ShapedLine, TextRun) {
        let mut font = window.text_style().font();
        font.weight = self.weight;
        // Always tabular — fixed digit advances keep changed cells from
        // reflowing neighbours (PARITY_SPEC §0); every consumer gets it free.
        font.features = crate::task_board::style::tabular_nums();
        let run = TextRun {
            len: text.len(),
            font,
            color: self.color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let line = window
            .text_system()
            .shape_line(text.to_string().into(), self.font_size, &[run.clone()], None);
        (line, run)
    }
}

impl IntoElement for NumericRoll {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for NumericRoll {
    type RequestLayoutState = ();
    type PrepaintState = RollPrepaint;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        // Lay out at the TARGET extent so geometry consumers never drift.
        let (line, _) = self.run(&self.current, window);
        let line_height = window.line_height();
        let mut style = Style::default();
        style.size = size((line.width()).into(), line_height.into());
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        window: &mut Window,
        _cx: &mut App,
    ) -> RollPrepaint {
        // The full target line gives every cell its exact x-advance
        // (tabular-nums keeps every digit box fixed-width, so a per-cell paint
        // lands precisely and never disturbs a neighbour). One pass classifies
        // each cell via the shared `cell_motion` law and routes its glyph(s) to
        // a motion group at the cell's x. Settled counters (progress >= 1, or a
        // shape rebuild whose cells are all `from: None`) classify EVERY cell
        // `Static` → one bare row at rest, zero slide, zero phantom entrance.
        let (line, _) = self.run(&self.current, window);
        let line_height = window.line_height();
        let mut static_glyphs = Vec::new();
        let mut incoming = Vec::new();
        let mut outgoing = Vec::new();
        let cells = cells(&self.current, &self.previous);
        for ((byte, _), cell) in self.current.char_indices().zip(cells) {
            let x = line.x_for_index(byte);
            match cell_motion(cell, self.progress) {
                // A changed digit, mid-roll: its NEW glyph enters from +1em
                // and its OLD glyph leaves to −1em — the only moving cells.
                CellMotion::Rolling { from, to } => {
                    let (new_glyph, _) = self.run(&to.to_string(), window);
                    incoming.push((x, new_glyph));
                    let (old_glyph, _) = self.run(&from.to_string(), window);
                    outgoing.push((x, old_glyph));
                }
                // Unchanged digit, separator, or any cell once settled: paint
                // the new glyph once at rest (`roll.js:39,41` leave it put).
                CellMotion::Static(ch) => {
                    let (glyph, _) = self.run(&ch.to_string(), window);
                    static_glyphs.push((x, glyph));
                }
            }
        }
        RollPrepaint { line_height, static_glyphs, incoming, outgoing }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        prepaint: &mut RollPrepaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        let RollPrepaint { line_height, static_glyphs, incoming, outgoing } = prepaint;
        let origin = bounds.origin;
        let lh = *line_height;
        // Clip everything to one line height: the old glyph leaving above
        // and the new glyph entering are both masked to this single row (the
        // web's `.roll-digit { overflow: hidden; height: 1em }`).
        window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
            // Unchanged digits + separators (and, when settled, every glyph)
            // paint ONCE at rest and NEVER move (`roll.js:39,41`). At progress
            // 1 `incoming`/`outgoing` are empty — the entire bare line, no cost.
            for (x, glyph) in static_glyphs.iter() {
                glyph
                    .paint(point(origin.x + *x, origin.y), lh, TextAlign::Left, None, window, cx)
                    .ok();
            }
            // Each CHANGED digit's NEW glyph enters from +1em (0 at rest).
            let enter = enter_dy(lh, self.progress);
            for (x, glyph) in incoming.iter() {
                glyph
                    .paint(
                        point(origin.x + *x, origin.y + enter),
                        lh,
                        TextAlign::Left,
                        None,
                        window,
                        cx,
                    )
                    .ok();
            }
            // Each CHANGED digit's OLD glyph slides from rest (0) up and out (−1em).
            let leave = leave_dy(lh, self.progress);
            for (x, glyph) in outgoing.iter() {
                glyph
                    .paint(
                        point(origin.x + *x, origin.y - leave),
                        lh,
                        TextAlign::Left,
                        None,
                        window,
                        cx,
                    )
                    .ok();
            }
        });
    }
}

#[cfg(test)]
#[path = "numeric_roll_tests.rs"]
mod tests;
