//! The §0 numeric-roll primitive — "a small custom element painting two
//! offset text runs during the transition" (RUST_PORT_NOTES §0; web
//! `roll.js`). One helper, consumed everywhere a live counter lives: the
//! usage island rest-%, the usage panel pool %, the task-board run count +
//! "N live", the tasks-island "N subagents" count.
//!
//! **Per-digit vs whole-number (the conscious divergence, decided here):**
//! the web rolls PER DIGIT — only the changed digits slide, aligned from the
//! ones place. Per-digit is *cheap* in gpui too: tabular-nums gives every
//! digit a fixed advance, so a changed cell is a fixed-width sub-box that can
//! clip + paint its old/new glyph at a vertical offset without disturbing its
//! neighbours. We therefore port the web's per-digit roll faithfully rather
//! than a whole-number slide. Non-digit characters (".", "s", "%", " ") and
//! any cell whose shape changed (different digit count) render bare and swap
//! instantly — exactly `roll.js`'s `needRebuild` / `setSep` branches.
//!
//! **Identity & the pump.** This is a stateless paint helper, not a
//! timer-owning element: the caller owns one [`RollValue`] in its view state
//! and a single frame pump (the same `request_animation_frame` /
//! `any_animating` idiom the islands already arm). Each frame the caller
//! reads the roll progress and hands it here; the element is keyed by the
//! caller's stable id, so it never churns identity (§8.7a). When settled it
//! paints one bare shaped line — zero per-frame cost (§8 idle).
//!
//! **Measurement honesty.** A roll lays out at the shaped width of its
//! TARGET string. The usage island morphs its container to the measured face
//! width; because tabular-nums fixes digit advances and the roll always
//! reports the target extent, the island's existing geometry math stays
//! frame-exact across a roll (no measure drift, RUST_PORT_NOTES §0).

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
/// `set` records a value change and re-bases the progress from the
/// interpolated current value (a mid-roll change is continuous, never a
/// snap — the [`AnimatedValue`] retarget law, §8.7a).
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
        // The digits leaving are whatever is rendered RIGHT NOW: if a prior
        // roll is still mid-flight we freeze its visible blend as the new
        // `previous` so the slide is continuous (no opposite-endpoint snap).
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

/// Shaped runs prepared in prepaint and handed to paint: the settled line,
/// plus per-cell (x-offset, outgoing glyph) for digits mid-roll.
pub struct RollPrepaint {
    line: ShapedLine,
    line_height: Pixels,
    /// (x within bounds, outgoing shaped glyph) for each digit still rolling.
    outgoing: Vec<(Pixels, ShapedLine)>,
}

impl NumericRoll {
    fn run(&self, text: &str, window: &Window) -> (ShapedLine, TextRun) {
        let mut font = window.text_style().font();
        font.weight = self.weight;
        // A numeric roll is always tabular — fixed digit advances are what
        // keep changed cells from reflowing their neighbours (PARITY_SPEC §0
        // "tabular-nums on counters"). Applied here so every consumer gets it
        // for free.
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
        let (line, _) = self.run(&self.current, window);
        let line_height = window.line_height();
        // Only build outgoing glyphs while a roll is in flight; settled
        // counters paint one bare line (zero extra shaping, §8 idle cost).
        let mut outgoing = Vec::new();
        if self.progress < 1. {
            // One pass: classify cells, then place each rolling digit's
            // outgoing glyph at its exact x-advance in the current line
            // (`x_for_index` over byte offsets — tabular-nums keeps every
            // digit box fixed-width, so the slide never disturbs neighbours).
            let cells = cells(&self.current, &self.previous);
            for ((byte, _), cell) in self.current.char_indices().zip(cells) {
                if let Cell::Digit { from: Some(from), .. } = cell {
                    let x = line.x_for_index(byte);
                    let (glyph, _) = self.run(&from.to_string(), window);
                    outgoing.push((x, glyph));
                }
            }
        }
        RollPrepaint { line, line_height, outgoing }
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
        let RollPrepaint { line, line_height, outgoing } = prepaint;
        let origin = bounds.origin;
        let lh = *line_height;
        // Clip everything to one line height: the old glyph leaving above
        // and the new glyph entering are both masked to this single row (the
        // web's `.roll-digit { overflow: hidden; height: 1em }`).
        window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
            // The settled line carries every NEW glyph (digits + seps). It
            // slides UP from +1em as the roll completes: at progress 0 it sits
            // one line below (entering), at 1 it rests at origin.
            let enter_dy = lh * (1. - self.progress);
            line.paint(
                point(origin.x, origin.y + enter_dy),
                lh,
                TextAlign::Left,
                None,
                window,
                cx,
            )
            .ok();
            // Each outgoing OLD digit slides from rest (0) up and out (−1em).
            let leave_dy = lh * self.progress;
            for (x, glyph) in outgoing.iter() {
                glyph
                    .paint(
                        point(origin.x + *x, origin.y - leave_dy),
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
mod tests {
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
}
