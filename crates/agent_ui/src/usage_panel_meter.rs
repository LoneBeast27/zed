//! The usage panel's pool meter (PARITY_SPEC §4.4 / panels.css `.meter`) —
//! per-pool animation state + the track/fill render. Width changes morph on
//! the spatial curve; tone-color crossfades ride effects and fire on band
//! flips ONLY (width-only changes hold the color steady — §8.7a, the
//! previous-band-flash regression). Meters NEVER silently vanish — a `None`
//! used-% degrades to the unknown tone at full width (the Antigravity
//! quota-opacity lesson).

use gpui::{Animation, AnimationExt as _, AnyElement, relative};
use ui::prelude::*;

use crate::agent_accents::{Tone, tone_for_used};
use crate::task_board::motion::{AnimatedColor, AnimatedValue, EFFECTS, SPATIAL};

/// Meter-fill morph duration — width changes ride the spatial curve
/// (PARITY_SPEC §4.9 geometry class).
const FILL_MORPH: std::time::Duration = std::time::Duration::from_millis(500);
/// Meter tone crossfade (effects class) — band flips only; width-only
/// changes hold the color steady (web: `background .3s` transitions only
/// when the band class actually swaps).
const TONE_FADE: std::time::Duration = std::time::Duration::from_millis(200);

/// Per-pool meter animation state: the retargetable width fill plus the
/// retargetable tone-color crossfade. Both are Instant-clocked, so a
/// width-only retarget restarting the shared animation wrapper can never
/// replay the previous band color (§8.7a — the tone holds steady unless
/// the band actually flips, and a flip crossfades from the color rendered
/// right now).
pub(crate) struct MeterState {
    width: AnimatedValue,
    tone: Tone,
    color: AnimatedColor,
}

impl MeterState {
    /// New pools fill from 0 to their value on first sight (the web's
    /// `width:0%` skeleton + first `updatePool`).
    pub(crate) fn new(tone: Tone) -> Self {
        Self {
            width: AnimatedValue::settled(0.0, SPATIAL, FILL_MORPH),
            tone,
            color: AnimatedColor::settled(tone.color(), EFFECTS, TONE_FADE),
        }
    }

    pub(crate) fn update(&mut self, used: Option<f64>) {
        // Unknown degrades to a full-width dim fill, never an empty bar.
        let target = used.unwrap_or(100.0) as f32;
        self.width.retarget(target);
        self.tone = tone_for_used(used);
        // Same-target retargets are no-ops: only a real band flip opens a
        // crossfade, and it re-bases from the interpolated current color.
        self.color.retarget(self.tone.color());
    }
}

/// The thin meter: tone-colored fill whose width morphs on the spatial
/// curve (500ms) while its color crossfades on effects (200ms, band flips
/// only) — ONE animation wrapper carrying both property classes so they
/// are concurrent from the first frame (§8.7b). The wrapper is a frame
/// pump over Instant-clocked values: a width-only retarget restarting it
/// can never replay the previous band color.
pub(crate) fn render_meter(state: &MeterState, pool_name: &str) -> AnyElement {
    let dim = state.tone == Tone::Unknown;

    let fill = div()
        .h_full()
        .rounded(px(2.))
        .bg(state.color.target())
        .when(dim, |fill| fill.opacity(0.35));

    // `.meter` (panels.css:55): the full-page track is 4px / 2px radius on
    // white@0.08 — deliberately lighter than the island card's mini-meter
    // (0.10), preserving the page-vs-island hierarchy.
    let track = div()
        .w_full()
        .h(px(4.))
        .rounded(px(2.))
        .bg(gpui::white().opacity(0.08))
        .overflow_hidden();

    if state.width.animating() || state.color.animating() {
        let value = state.width.clone();
        let color = state.color.clone();
        // Animation identity covers both the width retarget and the tone
        // flip — either bumps the key, extending the pump window.
        let generation = (state.width.generation() as u64) << 16
            | (state.color.generation() as u64 & 0xffff);
        track
            .child(fill.with_animation(
                ElementId::NamedInteger(format!("meter-fill-{pool_name}").into(), generation),
                // Frame pump: SPATIAL is mapped inside `current()` (the
                // animator-closure rule for overshooting curves — motion
                // GUARD). Overshoot past the track clips on
                // overflow_hidden: the visible spring settle.
                Animation::new(FILL_MORPH),
                move |fill, _| {
                    fill.w(relative(value.current().max(0.0) / 100.0))
                        .bg(color.current())
                },
            ))
            .into_any_element()
    } else {
        track
            .child(fill.w(relative(state.width.target().max(0.0) / 100.0)))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meter_state_animates_value_changes_and_tone_flips() {
        let mut state = MeterState::new(tone_for_used(Some(40.0)));
        state.update(Some(40.0));
        // First sight: fills 0 → 40 (the web's width:0% skeleton).
        assert!(state.width.animating());
        assert_eq!(state.width.target(), 40.0);
        assert_eq!(state.tone, Tone::Ok);
        assert_eq!(state.color.generation(), 0, "same tone — no crossfade");

        // Crossing into warn: width retargets AND the tone crossfades.
        state.update(Some(80.0));
        assert_eq!(state.width.target(), 80.0);
        assert_eq!(state.tone, Tone::Warn);
        assert_eq!(state.color.generation(), 1);
        assert_eq!(state.color.target(), Tone::Warn.color());

        // Unknown degrades to a full-width fill, never an empty bar.
        state.update(None);
        assert_eq!(state.width.target(), 100.0);
        assert_eq!(state.tone, Tone::Unknown);
    }

    #[test]
    fn width_only_changes_hold_the_band_color() {
        // The previous-band flash: after Ok→Warn, every later width-only
        // retarget restarted the color animation from the OLD band color.
        // The tone must hold steady unless the band actually flips (web:
        // `background .3s` transitions only on a class swap).
        let mut state = MeterState::new(tone_for_used(Some(40.0)));
        state.update(Some(40.0));
        state.update(Some(80.0)); // Ok → Warn flip
        let flip_generation = state.color.generation();
        assert_eq!(flip_generation, 1);

        // Width-only changes inside the warn band: the color crossfade is
        // NEVER restarted (same-target retargets are no-ops).
        for used in [81.0, 79.5, 85.0, 89.9] {
            state.update(Some(used));
            assert_eq!(
                state.color.generation(),
                flip_generation,
                "width-only change at {used}% must not restart the tone fade"
            );
            assert_eq!(state.color.target(), Tone::Warn.color());
        }

        // The next REAL flip crossfades again.
        state.update(Some(95.0));
        assert_eq!(state.color.generation(), flip_generation + 1);
        assert_eq!(state.color.target(), Tone::Crit.color());
    }
}
