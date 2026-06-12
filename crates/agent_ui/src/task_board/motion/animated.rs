//! The retargetable §4.9 animation primitives. Everything here is
//! Instant-clocked: each value owns its `started` timestamp, so gpui
//! animation wrappers can be pure FRAME PUMPS — a wrapper restart (element
//! identity bump) re-bases NOTHING, and every retarget continues from the
//! interpolated current value (§8.7a — interrupts are continuous on ALL
//! axes, geometry and tint alike).

use std::time::{Duration, Instant};

use gpui::{Hsla, Rgba};

use super::curves::MotionCurve;

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
        }
    }

    /// Animate toward `to` from wherever the value is *right now* (mid-
    /// flight retargets are continuous). Same-target retargets are no-ops,
    /// so render-loop calls never restart a settled animation.
    pub fn retarget(&mut self, to: f32) {
        if to == self.to {
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
    pub fn retarget_with(
        &mut self,
        to: f32,
        curve: impl Into<MotionCurve>,
        duration: Duration,
    ) -> &mut Self {
        if to == self.to {
            return self;
        }
        // Sample the in-flight value under the OLD curve/clock first — the
        // current value belongs to the old flight, not the new one.
        self.from = self.current();
        self.curve = curve.into();
        self.duration = duration;
        self.to = to;
        self.started = Some(Instant::now());
        self.generation = self.generation.wrapping_add(1);
        self
    }

    /// Jump without animating (initial paints, offscreen updates).
    pub fn jump(&mut self, value: f32) {
        self.from = value;
        self.to = value;
        self.started = None;
    }

    /// The interpolated value at this instant.
    pub fn current(&self) -> f32 {
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
        self.started
            .is_some_and(|started| started.elapsed() < self.duration)
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
mod tests {
    use super::super::curves::{DECEL, EFFECTS, MotionCurve};
    use super::*;

    #[test]
    fn state_fade_opens_window_and_advances_generation() {
        let mut fade = StateFade::default();
        assert!(!fade.fresh(), "untouched state is settled");
        assert_eq!(fade.generation(), 0);
        fade.bump();
        assert!(fade.fresh(), "a flip opens the crossfade window");
        assert_eq!(fade.generation(), 1);
        fade.bump();
        assert_eq!(fade.generation(), 2, "each flip is a new animation key");
        assert!(StateFade::begun().fresh());
    }

    #[test]
    fn animated_value_retargets_from_interpolated_current() {
        let mut value = AnimatedValue::settled(10.0, EFFECTS, Duration::from_millis(200));
        assert!(!value.animating());
        assert_eq!(value.current(), 10.0);

        value.retarget(20.0);
        assert!(value.animating());
        assert_eq!(value.generation(), 1);
        // t=0 starts at the old value; t=1 lands on the target; midway is
        // strictly between (no snap — §8.7a).
        assert_eq!(value.value_at(0.0), 10.0);
        assert_eq!(value.value_at(1.0), 20.0);
        let mid = value.value_at(0.5);
        assert!(mid > 10.0 && mid < 20.0, "mid = {mid}");

        // Same-target retarget is a no-op (render loops call this per frame).
        value.retarget(20.0);
        assert_eq!(value.generation(), 1);

        // A mid-flight retarget re-bases from the interpolated current value.
        let current = value.current();
        value.retarget(0.0);
        assert_eq!(value.generation(), 2);
        assert!(
            (value.value_at(0.0) - current).abs() < 0.5,
            "retarget must start from the in-flight value ({current}), got {}",
            value.value_at(0.0)
        );

        value.jump(5.0);
        assert!(!value.animating());
        assert_eq!(value.current(), 5.0);
    }

    #[test]
    fn retarget_with_switches_curve_and_rebases_continuously() {
        // The toast interrupt: a DECEL emerge (1 → 0) cut short by a
        // retract retargeting onto the exit spline. The new flight must
        // start from the in-flight offset — NOT the fully-emerged endpoint
        // (§8.7a; the mid-emerge dismiss snap).
        let mut emerge = AnimatedValue::settled(1.0, DECEL, Duration::from_millis(500));
        emerge.retarget(0.0);
        std::thread::sleep(Duration::from_millis(30));
        let in_flight = emerge.current();
        assert!(
            in_flight > 0.0 && in_flight < 1.0,
            "mid-emerge offset, got {in_flight}"
        );

        emerge.retarget_with(1.0, MotionCurve::Exit, Duration::from_millis(400));
        assert_eq!(emerge.generation(), 2);
        assert_eq!(emerge.duration(), Duration::from_millis(400));
        assert!(
            (emerge.value_at(0.0) - in_flight).abs() < 0.1,
            "exit must re-base from the in-flight value ({in_flight}), got {}",
            emerge.value_at(0.0)
        );
        assert_eq!(emerge.value_at(1.0), 1.0);

        // Same-target call is a no-op (double-retract).
        emerge.retarget_with(1.0, MotionCurve::Exit, Duration::from_millis(400));
        assert_eq!(emerge.generation(), 2);
    }

    #[test]
    fn animated_color_retargets_from_interpolated_current() {
        let red: Hsla = gpui::Rgba {
            r: 1.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        }
        .into();
        let blue: Hsla = gpui::Rgba {
            r: 0.0,
            g: 0.0,
            b: 1.0,
            a: 0.2,
        }
        .into();
        let mut tint = AnimatedColor::settled(red, EFFECTS, Duration::from_millis(200));
        assert!(!tint.animating());
        assert_eq!(tint.current(), red);

        tint.retarget(blue);
        assert!(tint.animating());
        assert_eq!(tint.generation(), 1);
        // Same-target retarget is a no-op — a ticking render can never
        // restart the crossfade.
        tint.retarget(blue);
        assert_eq!(tint.generation(), 1);

        std::thread::sleep(Duration::from_millis(30));
        let in_flight = Rgba::from(tint.current());
        assert!(
            in_flight.r < 1.0 && in_flight.b > 0.0,
            "mid-fade color, got {in_flight:?}"
        );

        // Retargeting back re-bases from the CURRENT mix — never the
        // opposite endpoint (the critical-tint-flash blocker).
        tint.retarget(red);
        assert_eq!(tint.generation(), 2);
        let rebased = Rgba::from(tint.current());
        assert!(
            (rebased.r - in_flight.r).abs() < 0.15 && (rebased.b - in_flight.b).abs() < 0.15,
            "re-base must continue from {in_flight:?}, got {rebased:?}"
        );

        tint.jump(blue);
        assert!(!tint.animating());
        assert_eq!(tint.current(), blue);
    }

    #[test]
    fn mix_interpolates_srgb_endpoints_exactly() {
        let a = gpui::Rgba {
            r: 1.0,
            g: 0.0,
            b: 0.5,
            a: 1.0,
        };
        let b = gpui::Rgba {
            r: 0.0,
            g: 1.0,
            b: 0.5,
            a: 0.0,
        };
        // Hsla round-trips cost a little float precision — compare with tolerance.
        let assert_rgba = |got: gpui::Rgba, want: gpui::Rgba| {
            assert!((got.r - want.r).abs() < 1e-4, "r: {got:?} vs {want:?}");
            assert!((got.g - want.g).abs() < 1e-4, "g: {got:?} vs {want:?}");
            assert!((got.b - want.b).abs() < 1e-4, "b: {got:?} vs {want:?}");
            assert!((got.a - want.a).abs() < 1e-4, "a: {got:?} vs {want:?}");
        };
        assert_rgba(gpui::Rgba::from(mix(a.into(), b.into(), 0.0)), a);
        assert_rgba(gpui::Rgba::from(mix(a.into(), b.into(), 1.0)), b);
        assert_rgba(
            gpui::Rgba::from(mix(a.into(), b.into(), 0.5)),
            gpui::Rgba {
                r: 0.5,
                g: 0.5,
                b: 0.5,
                a: 0.5,
            },
        );
    }
}
