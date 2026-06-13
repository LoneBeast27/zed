//! The retargetable §4.9 animation primitives. Everything here is
//! Instant-clocked: each value owns its `started` timestamp, so gpui
//! animation wrappers can be pure FRAME PUMPS — a wrapper restart (element
//! identity bump) re-bases NOTHING, and every retarget continues from the
//! interpolated current value (§8.7a — interrupts are continuous on ALL
//! axes, geometry and tint alike).

use std::time::{Duration, Instant};

use gpui::{Hsla, Rgba};

use super::curves::MotionCurve;
use super::spring::Spring;

/// Interactive-state crossfade duration — the web's
/// `transition: … .15s var(--effects-curve)` on seg-toggle buttons, inbox
/// rows, drawer tabs, and the drawer close button (board.css:23,38,183,198;
/// PARITY_SPEC §0 "state changes 150–250ms").
pub const STATE_FADE: Duration = Duration::from_millis(150);
/// How long after a state flip the animated wrapper stays attached: the fade
/// plus margin. Outside this window elements render bare, so settled states
/// cost nothing per frame and an element-state drop (scroll culling, panel
/// re-show) can never replay a finished fade.
pub const STATE_FADE_WINDOW: Duration = Duration::from_millis(300);

/// Tracks one piece of flip-state for a one-shot effects-curve crossfade:
/// [`bump`](Self::bump) on every state change; [`fresh`](Self::fresh) says
/// whether the crossfade window is still open (attach the animated wrapper
/// only then); [`generation`](Self::generation) keys the animation identity
/// so each flip animates exactly once.
#[derive(Debug, Clone, Default)]
pub struct StateFade {
    generation: usize,
    changed_at: Option<Instant>,
}

impl StateFade {
    /// A tracker whose state changed right now (already mid-fade).
    pub fn begun() -> Self {
        let mut fade = Self::default();
        fade.bump();
        fade
    }

    /// Record a state flip: opens the crossfade window, new generation.
    pub fn bump(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.changed_at = Some(Instant::now());
    }

    /// Whether the crossfade window from the last flip is still open.
    pub fn fresh(&self) -> bool {
        self.changed_at
            .is_some_and(|at| at.elapsed() < STATE_FADE_WINDOW)
    }

    /// Animation-identity key for the latest flip.
    pub fn generation(&self) -> usize {
        self.generation
    }
}

/// Componentwise sRGB mix between two colors — the interpolation a CSS color
/// `transition` performs, used by the 150ms state crossfades.
pub fn mix(a: Hsla, b: Hsla, t: f32) -> Hsla {
    let (a, b) = (Rgba::from(a), Rgba::from(b));
    Rgba {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: a.a + (b.a - a.a) * t,
    }
    .into()
}

/// A retargetable eased scalar — the positional half of the §4.9 morph
/// primitive (COMPLEMENTARY_SOLUTIONS B gap 2). gpui's `with_animation`
/// restarts at t=0 with no memory, so retargeting mid-flight would snap;
/// this tracker re-bases `from` to the *interpolated current value* at each
/// retarget, making interrupts continuous (positional retarget only — no
/// velocity carry; a true spring layer stays a Z4 item). Render idiom:
/// attach `with_animation` keyed by [`generation`](Self::generation) while
/// [`animating`](Self::animating) and read [`current`](Self::current) in
/// the animator (the wrapper is only a frame pump — its restart re-bases
/// nothing); settled values render bare at [`target`](Self::target).
#[derive(Debug, Clone)]
pub struct AnimatedValue {
    from: f32,
    to: f32,
    started: Option<Instant>,
    generation: usize,
    curve: MotionCurve,
    duration: Duration,
    /// Opt-in velocity-carrying backing (the Z4 spring layer). When `Some`,
    /// `retarget`/`current`/`animating` delegate to the [`Spring`] so a
    /// mid-flight retarget carries momentum instead of re-basing position at
    /// rest velocity (COMPLEMENTARY_SOLUTIONS gap B-2). `None` is the
    /// curve-based default — every existing consumer is byte-identical.
    spring: Option<Spring>,
}

impl AnimatedValue {
    /// A settled value (no entrance animation).
    pub fn settled(value: f32, curve: impl Into<MotionCurve>, duration: Duration) -> Self {
        Self {
            from: value,
            to: value,
            started: None,
            generation: 0,
            curve: curve.into(),
            duration,
            spring: None,
        }
    }

    /// A settled value backed by a velocity-carrying [`Spring`] (the opt-in
    /// Z4 mode for interrupt-heavy values — hover widths, reveal extents).
    /// `omega` is the spring's angular frequency ([`super::CHROME_OMEGA`] for
    /// chrome). `retarget` then carries momentum across re-aims; `curve`/
    /// `duration` are retained only so a later `retarget_with` could drop
    /// back to a curve if ever needed.
    pub fn spring(value: f32, omega: f32) -> Self {
        Self {
            from: value,
            to: value,
            started: None,
            generation: 0,
            curve: super::curves::EFFECTS.into(),
            duration: Duration::from_millis(250),
            spring: Some(Spring::settled(value, omega)),
        }
    }

    /// Animate toward `to` from wherever the value is *right now* (mid-
    /// flight retargets are continuous). Same-target retargets are no-ops,
    /// so render-loop calls never restart a settled animation.
    pub fn retarget(&mut self, to: f32) {
        if to == self.to {
            return;
        }
        // Spring-backed values carry velocity through the retarget (the Z4
        // gap fix); the curve path re-bases position at rest velocity.
        if let Some(spring) = self.spring.as_mut() {
            spring.retarget(to);
            self.to = to;
            self.generation = spring.generation();
            return;
        }
        self.from = self.current();
        self.to = to;
        self.started = Some(Instant::now());
        self.generation = self.generation.wrapping_add(1);
    }

    /// [`retarget`](Self::retarget) onto a NEW curve/duration — the §4.9
    /// asymmetric-exit interrupt (an emerge cut short by a retract continues
    /// from its current offset onto the sharper exit spline, never snapping
    /// to either endpoint first). Same-target calls are no-ops like
    /// `retarget`.
    ///
    /// **Spring handoff (the P4 fix — "drop back to a curve").** The affordance
    /// the [`spring`](Self::spring) constructor documents. On a spring-backed
    /// value it samples the spring's CONTINUOUS current position, then DROPS the
    /// spring (`self.spring = None`) so the curve fields drive it from here.
    /// Position is continuous (no §8.7 snap — the curve starts exactly where the
    /// spring was); velocity is intentionally NOT carried (a curve has no
    /// momentum — a true velocity-carry interrupt stays on plain
    /// [`retarget`](Self::retarget), which keeps the spring). Before this fix the
    /// written fields were dead: `current`/`animating` dispatch to `self.spring`
    /// FIRST, so the value kept its stale spring trajectory — a silent no-op.
    pub fn retarget_with(
        &mut self,
        to: f32,
        curve: impl Into<MotionCurve>,
        duration: Duration,
    ) -> &mut Self {
        if to == self.to {
            return self;
        }
        // Sample the in-flight value under the OLD clock first (spring position
        // or old curve) — the current value belongs to the old flight. Then
        // drop any spring so the new curve fields actually drive the value
        // (`current`/`animating` dispatch to `self.spring` FIRST when `Some`).
        self.from = self.current();
        self.spring = None;
        self.curve = curve.into();
        self.duration = duration;
        self.to = to;
        self.started = Some(Instant::now());
        self.generation = self.generation.wrapping_add(1);
        self
    }

    /// Jump without animating (initial paints, offscreen updates).
    pub fn jump(&mut self, value: f32) {
        if let Some(spring) = self.spring.as_mut() {
            spring.jump(value);
        }
        self.from = value;
        self.to = value;
        self.started = None;
    }

    /// The interpolated value at this instant.
    pub fn current(&self) -> f32 {
        if let Some(spring) = self.spring.as_ref() {
            return spring.position();
        }
        match self.started {
            None => self.to,
            Some(started) => {
                let t = started.elapsed().as_secs_f32() / self.duration.as_secs_f32();
                self.value_at(t)
            }
        }
    }

    /// The value at animation progress `t` ∈ [0,1] — for `with_animation`
    /// animator closures (the eased lerp; overshooting curves overshoot
    /// the segment, which is the visible spring settle).
    pub fn value_at(&self, t: f32) -> f32 {
        self.from + (self.to - self.from) * self.curve.eval(t)
    }

    pub fn target(&self) -> f32 {
        self.to
    }

    /// Whether the last retarget's animation window is still open.
    pub fn animating(&self) -> bool {
        if let Some(spring) = self.spring.as_ref() {
            return spring.animating();
        }
        self.started
            .is_some_and(|started| started.elapsed() < self.duration)
    }

    /// Current velocity (spring-backed only; curve values report 0 — they
    /// carry no momentum, which is exactly the gap the spring closes).
    pub fn velocity(&self) -> f32 {
        self.spring.as_ref().map_or(0., Spring::velocity)
    }

    /// Animation-identity key for the latest retarget.
    pub fn generation(&self) -> usize {
        self.generation
    }

    pub fn duration(&self) -> Duration {
        self.duration
    }
}

/// A retargetable eased color — the chromatic twin of [`AnimatedValue`],
/// interpolating componentwise sRGB via [`mix`] (what a CSS color
/// `transition` does). Same retarget law: a mid-flight retarget re-bases
/// from the interpolated CURRENT color, never a stale endpoint — so a
/// container tint can NEVER repaint from the opposite endpoint when an
/// unrelated axis restarts the shared animation wrapper (§8.7a; the
/// "critical tint flash" blocker class).
#[derive(Debug, Clone)]
pub struct AnimatedColor {
    from: Hsla,
    to: Hsla,
    started: Option<Instant>,
    generation: usize,
    curve: MotionCurve,
    duration: Duration,
}

impl AnimatedColor {
    /// A settled color (no entrance fade).
    pub fn settled(color: Hsla, curve: impl Into<MotionCurve>, duration: Duration) -> Self {
        Self {
            from: color,
            to: color,
            started: None,
            generation: 0,
            curve: curve.into(),
            duration,
        }
    }

    /// Crossfade toward `to` from the color rendered *right now*. Same-
    /// target retargets are no-ops (render loops call this per frame).
    pub fn retarget(&mut self, to: Hsla) {
        if to == self.to {
            return;
        }
        self.from = self.current();
        self.to = to;
        self.started = Some(Instant::now());
        self.generation = self.generation.wrapping_add(1);
    }

    /// Jump without animating (initial paints).
    pub fn jump(&mut self, color: Hsla) {
        self.from = color;
        self.to = color;
        self.started = None;
    }

    /// The interpolated color at this instant.
    pub fn current(&self) -> Hsla {
        match self.started {
            None => self.to,
            Some(started) => {
                let t = started.elapsed().as_secs_f32() / self.duration.as_secs_f32();
                mix(self.from, self.to, self.curve.eval(t))
            }
        }
    }

    pub fn target(&self) -> Hsla {
        self.to
    }

    /// Whether the last retarget's crossfade window is still open.
    pub fn animating(&self) -> bool {
        self.started
            .is_some_and(|started| started.elapsed() < self.duration)
    }

    /// Animation-identity key for the latest retarget.
    pub fn generation(&self) -> usize {
        self.generation
    }
}

#[cfg(test)]
#[path = "animated_tests.rs"]
mod tests;
