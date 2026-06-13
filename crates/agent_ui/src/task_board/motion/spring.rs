//! The velocity-carrying spring layer — "the Z4 gap" (COMPLEMENTARY_SOLUTIONS
//! gap B-2; Z1/Z2/Z3 deferrals: "Pill elapsed width-spring", "Velocity-
//! carrying spring layer for retarget-heavy morphs"). The recurring
//! observation across the design-check ledgers was that the curve-based
//! [`AnimatedValue`](super::AnimatedValue) re-bases POSITION on a mid-flight
//! retarget but DROPS velocity — a spring hit while still moving restarts
//! from a dead stop, which reads as a stutter on interrupt-heavy values (a
//! hover-width that the pointer keeps re-aiming). A real spring carries its
//! momentum THROUGH the retarget: the target moves, the mass keeps its
//! current velocity, and it settles continuously.
//!
//! COMPLEMENTARY_SOLUTIONS ruled this a build, not a dependency: no external
//! crate (blinc_animation was watch-list-only). This is a compact analytic
//! critically-damped spring — the closed-form solution of
//! `x'' = -k(x - target) - c·x'`, evaluated at an arbitrary elapsed time, so
//! it shares [`AnimatedValue`]'s Instant-clocked, frame-pump-agnostic model
//! (no per-frame integration state to drift; `position`/`velocity` at time
//! `t` are pure functions of the spring's start conditions).
//!
//! Critical damping (`c = 2√k`) is the default — no overshoot, fastest
//! non-oscillating settle, the right feel for chrome geometry (a width that
//! springs past its target then back would fight the SPATIAL overshoot the
//! geometry morphs already own). [`Spring::new`] takes the angular frequency
//! `omega` (ω = √k); higher ω = snappier. The damping ratio is configurable
//! for cases that want a touch of overshoot, but chrome uses critical.

use std::time::Instant;

/// Angular frequency for chrome hover/width springs — settles in ~250ms to
/// within a pixel, the feel of the web's `.15s` interactive transitions but
/// momentum-preserving. (ω in rad/s; the 90%-settle time ≈ 3.4/ω for a
/// critically-damped spring, so ω ≈ 14 → ~240ms.)
pub const CHROME_OMEGA: f32 = 14.0;

/// Default settle threshold as a FRACTION of the retarget span (P6): the
/// spring is done when it is within this fraction of `|target − x0|` of its
/// target. Span-relative so the predicate is SCALE-INVARIANT — a unit-scale
/// reveal (0→1) and a pixel-scale width spring settle at the same *visual*
/// closeness, comfortably under the §8.7 ">10% of delta in one frame = snap"
/// gate. (The old hardcoded `0.5` was a pixel-scale assumption that, on the
/// unit-scale reveal, meant "settled within 50% of the whole range".)
const SETTLE_FRAC: f32 = 0.005;

/// Absolute floor for the span used in the settle test, so a near-zero-span
/// retarget (degenerate; `retarget` already no-ops an exact-same target) can
/// never make the threshold collapse to 0 and deadlock the frame pump.
const MIN_SETTLE_SPAN: f32 = 1e-3;

/// A critically-damped (or configurable-ratio) analytic spring. State is the
/// start position/velocity and the target captured at the last retarget plus
/// a `started` Instant; `position`/`velocity` at any later instant are
/// closed-form, so this needs no per-frame stepping and a frame pump only
/// reads it (like [`AnimatedValue`](super::AnimatedValue)).
#[derive(Debug, Clone)]
pub struct Spring {
    /// Position at `started`.
    x0: f32,
    /// Velocity at `started` (the carried momentum on a retarget).
    v0: f32,
    /// The rest target.
    target: f32,
    /// Angular frequency ω.
    omega: f32,
    /// Damping ratio ζ (1.0 = critical).
    zeta: f32,
    /// Clock origin for the current flight; `None` = settled (at rest).
    started: Option<Instant>,
    /// Identity key for the latest retarget (frame-pump wrapper keying).
    generation: usize,
    /// `|target − x0|` captured at the last retarget — the span the settle
    /// test scales against so it is scale-invariant (P6).
    span: f32,
    /// Optional ABSOLUTE settle epsilons (position, velocity) that override the
    /// span-relative default — for a consumer that wants an explicit fixed
    /// tolerance (e.g. a pixel-scale "within 0.25px"). `None` = span-relative.
    abs_eps: Option<(f32, f32)>,
}

impl Spring {
    /// A settled spring resting at `value`, critically damped at `omega`.
    pub fn settled(value: f32, omega: f32) -> Self {
        Self {
            x0: value,
            v0: 0.,
            target: value,
            omega,
            zeta: 1.0,
            started: None,
            generation: 0,
            span: 0.,
            abs_eps: None,
        }
    }

    /// Set the damping ratio (1.0 critical, <1 under-damped overshoot,
    /// >1 over-damped sluggish). Chrome leaves it at critical.
    pub fn with_damping(mut self, zeta: f32) -> Self {
        self.zeta = zeta.max(0.0);
        self
    }

    /// Pin ABSOLUTE settle epsilons (position, velocity) instead of the
    /// span-relative default (P6) — for a consumer that wants a fixed tolerance
    /// regardless of span (e.g. pixel geometry: "settled within 0.25px and
    /// 0.25px/s"). Without this, [`animating`](Self::animating) scales the
    /// threshold to the retarget span so it is correct at any scale.
    pub fn with_epsilons(mut self, dx_eps: f32, v_eps: f32) -> Self {
        self.abs_eps = Some((dx_eps.max(0.0), v_eps.max(0.0)));
        self
    }

    /// Retarget toward `to`, CARRYING the current velocity (the whole point
    /// of the layer): the new flight starts from `(position, velocity)`
    /// sampled right now, so an interrupt mid-motion keeps its momentum
    /// instead of restarting from rest. Same-target retargets are no-ops
    /// (render loops call this per frame).
    pub fn retarget(&mut self, to: f32) {
        if to == self.target {
            return;
        }
        let now = Instant::now();
        let t = self.elapsed_secs(now);
        self.x0 = self.position_at(t);
        self.v0 = self.velocity_at(t);
        self.target = to;
        // The span the settle test scales against (P6) — captured from the
        // continuous current position, so a mid-flight re-aim re-scopes the
        // tolerance to the NEW journey's size.
        self.span = (self.target - self.x0).abs();
        self.started = Some(now);
        self.generation = self.generation.wrapping_add(1);
    }

    /// Jump without animating (initial paints, offscreen updates) — position
    /// set, velocity zeroed, clock cleared.
    pub fn jump(&mut self, value: f32) {
        self.x0 = value;
        self.v0 = 0.;
        self.target = value;
        self.started = None;
    }

    /// The current position (settled springs return their target).
    pub fn position(&self) -> f32 {
        match self.started {
            None => self.target,
            Some(started) => self.position_at(started.elapsed().as_secs_f32()),
        }
    }

    /// The current velocity (0 when settled).
    pub fn velocity(&self) -> f32 {
        match self.started {
            None => 0.,
            Some(started) => self.velocity_at(started.elapsed().as_secs_f32()),
        }
    }

    pub fn target(&self) -> f32 {
        self.target
    }

    pub fn generation(&self) -> usize {
        self.generation
    }

    /// Whether the spring is still settling — close to target AND nearly
    /// stopped means done (a critically-damped spring approaches
    /// asymptotically; this is the practical settle test the frame pump uses
    /// to stop scheduling frames, §8 idle cost).
    ///
    /// The tolerances are SPAN-RELATIVE by default (P6): position within
    /// `SETTLE_FRAC × span` and velocity within `SETTLE_FRAC × span × ω` (peak
    /// velocity scales with span·ω). This is scale-invariant — a unit-scale
    /// reveal (0→1) and a pixel-scale width spring both settle at the same
    /// *visual* closeness, well under the §8.7 10%-of-delta gate. A consumer
    /// can pin fixed absolute tolerances via [`with_epsilons`](Self::with_epsilons).
    pub fn animating(&self) -> bool {
        match self.started {
            None => false,
            Some(started) => {
                let t = started.elapsed().as_secs_f32();
                let dx = (self.position_at(t) - self.target).abs();
                let v = self.velocity_at(t).abs();
                let (dx_eps, v_eps) = self.settle_epsilons();
                dx > dx_eps || v > v_eps
            }
        }
    }

    /// The (position, velocity) settle tolerances: the pinned absolute pair if
    /// set, else span-relative (`SETTLE_FRAC × span` for position, the same
    /// fraction of the span's characteristic velocity `span × ω` for velocity),
    /// with a span floor so a degenerate near-zero-span flight never deadlocks.
    fn settle_epsilons(&self) -> (f32, f32) {
        if let Some(eps) = self.abs_eps {
            return eps;
        }
        let span = self.span.max(MIN_SETTLE_SPAN);
        (SETTLE_FRAC * span, SETTLE_FRAC * span * self.omega)
    }

    fn elapsed_secs(&self, now: Instant) -> f32 {
        match self.started {
            None => 0.,
            Some(started) => now.saturating_duration_since(started).as_secs_f32(),
        }
    }

    /// Closed-form position at elapsed `t` seconds. Offset `p = x - target`
    /// obeys `p'' + 2ζω p' + ω² p = 0`.
    fn position_at(&self, t: f32) -> f32 {
        let p0 = self.x0 - self.target;
        let v0 = self.v0;
        let (omega, zeta) = (self.omega, self.zeta);
        let p = if (zeta - 1.0).abs() < 1e-4 {
            // Critically damped: p(t) = (p0 + (v0 + ω·p0)·t)·e^(−ω·t).
            let exp = (-omega * t).exp();
            (p0 + (v0 + omega * p0) * t) * exp
        } else if zeta < 1.0 {
            // Under-damped: damped oscillation.
            let wd = omega * (1.0 - zeta * zeta).sqrt();
            let exp = (-zeta * omega * t).exp();
            let a = p0;
            let b = (v0 + zeta * omega * p0) / wd;
            exp * (a * (wd * t).cos() + b * (wd * t).sin())
        } else {
            // Over-damped: two real roots.
            let s = omega * (zeta * zeta - 1.0).sqrt();
            let r1 = -zeta * omega + s;
            let r2 = -zeta * omega - s;
            let c2 = (v0 - r1 * p0) / (r2 - r1);
            let c1 = p0 - c2;
            c1 * (r1 * t).exp() + c2 * (r2 * t).exp()
        };
        self.target + p
    }

    /// Closed-form velocity at elapsed `t` seconds (derivative of `position_at`).
    fn velocity_at(&self, t: f32) -> f32 {
        let p0 = self.x0 - self.target;
        let v0 = self.v0;
        let (omega, zeta) = (self.omega, self.zeta);
        if (zeta - 1.0).abs() < 1e-4 {
            let exp = (-omega * t).exp();
            // d/dt[(p0 + (v0+ω·p0)t)e^(−ωt)] = (v0+ω·p0)e^(−ωt) − ω·(…)e^(−ωt).
            let inner = p0 + (v0 + omega * p0) * t;
            (v0 + omega * p0) * exp - omega * inner * exp
        } else if zeta < 1.0 {
            let wd = omega * (1.0 - zeta * zeta).sqrt();
            let exp = (-zeta * omega * t).exp();
            let a = p0;
            let b = (v0 + zeta * omega * p0) / wd;
            let osc = a * (wd * t).cos() + b * (wd * t).sin();
            let dosc = -a * wd * (wd * t).sin() + b * wd * (wd * t).cos();
            exp * (dosc - zeta * omega * osc)
        } else {
            let s = omega * (zeta * zeta - 1.0).sqrt();
            let r1 = -zeta * omega + s;
            let r2 = -zeta * omega - s;
            let c2 = (v0 - r1 * p0) / (r2 - r1);
            let c1 = p0 - c2;
            c1 * r1 * (r1 * t).exp() + c2 * r2 * (r2 * t).exp()
        }
    }
}

/// Approximate elapsed-time helper for tests: how long since `started`.
#[cfg(test)]
fn settle_within(spring: &Spring, max: std::time::Duration) -> bool {
    spring.started.is_some_and(|s| s.elapsed() < max) || !spring.animating()
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    /// Step a spring forward `dt` by re-basing its clock — lets tests assert
    /// trajectory analytically without sleeping.
    fn advance(spring: &mut Spring, dt: f32) {
        let now = Instant::now();
        let t = spring.elapsed_secs(now) + dt;
        let (x, v) = (spring.position_at(t), spring.velocity_at(t));
        spring.x0 = x;
        spring.v0 = v;
        spring.started = Some(now);
    }

    #[test]
    fn settled_spring_rests_at_target_with_no_velocity() {
        let s = Spring::settled(10.0, CHROME_OMEGA);
        assert_eq!(s.position(), 10.0);
        assert_eq!(s.velocity(), 0.0);
        assert!(!s.animating());
    }

    #[test]
    fn critically_damped_approaches_without_overshoot() {
        let mut s = Spring::settled(0.0, CHROME_OMEGA);
        s.retarget(100.0);
        let mut prev = 0.0;
        let mut max = 0.0_f32;
        for _ in 0..200 {
            advance(&mut s, 0.005);
            let x = s.position();
            assert!(x >= prev - 1e-3, "critical damping must not reverse (overshoot)");
            prev = x;
            max = max.max(x);
        }
        assert!(max <= 100.0 + 1e-2, "no overshoot past target, peaked {max}");
        assert!((s.position() - 100.0).abs() < 1.0, "settles near target");
    }

    #[test]
    fn retarget_carries_velocity_not_a_dead_stop() {
        // The whole point of the layer: a mid-flight retarget keeps momentum.
        let mut s = Spring::settled(0.0, CHROME_OMEGA);
        s.retarget(100.0);
        advance(&mut s, 0.05); // mid-flight: moving fast toward 100
        let v_before = s.velocity();
        assert!(v_before > 1.0, "should be moving, v = {v_before}");

        s.retarget(200.0); // re-aim higher MID-MOTION
        let v_after = s.velocity();
        // Velocity is continuous across the retarget — NOT reset to zero (the
        // Z4-gap bug). The carried momentum is the just-sampled velocity; the
        // only gap is the few µs of real wall-clock between the two reads
        // (the spring is Instant-clocked), so compare RELATIVE not exact.
        assert!(
            ((v_after - v_before) / v_before).abs() < 0.01,
            "retarget must carry velocity ~{v_before}, got {v_after}"
        );
        assert!(v_after > 1.0, "still moving after re-aim");
    }

    #[test]
    fn curve_value_would_drop_velocity_this_does_not() {
        // Contrast assertion documenting the fix: after a retarget the spring
        // is already in motion at t=0 of the new flight (nonzero velocity),
        // whereas a curve-based value starts every flight from v=0.
        let mut s = Spring::settled(0.0, CHROME_OMEGA);
        s.retarget(50.0);
        advance(&mut s, 0.04);
        s.retarget(10.0); // reverse direction mid-flight
        // Immediately after, velocity is still the (positive) carried value —
        // the mass overshoots toward the old target before the new force wins,
        // exactly the momentum a real spring shows (and a curve cannot).
        assert!(s.velocity() > 0.0, "momentum carries past the reversal point");
    }

    #[test]
    fn same_target_retarget_is_a_noop() {
        let mut s = Spring::settled(5.0, CHROME_OMEGA);
        s.retarget(20.0);
        let g = s.generation();
        s.retarget(20.0);
        assert_eq!(s.generation(), g, "same-target retargets never restart");
    }

    #[test]
    fn jump_clears_motion() {
        let mut s = Spring::settled(0.0, CHROME_OMEGA);
        s.retarget(100.0);
        advance(&mut s, 0.05);
        s.jump(42.0);
        assert_eq!(s.position(), 42.0);
        assert_eq!(s.velocity(), 0.0);
        assert!(!s.animating());
    }

    #[test]
    fn underdamped_overshoots_then_returns() {
        let mut s = Spring::settled(0.0, CHROME_OMEGA).with_damping(0.4);
        s.retarget(100.0);
        let mut max = 0.0_f32;
        for _ in 0..300 {
            advance(&mut s, 0.005);
            max = max.max(s.position());
        }
        assert!(max > 100.0, "an under-damped spring overshoots, peaked {max}");
        assert!((s.position() - 100.0).abs() < 2.0, "and still settles to target");
    }

    #[test]
    fn settle_helper_agrees_with_animating() {
        let s = Spring::settled(0.0, CHROME_OMEGA);
        assert!(settle_within(&s, Duration::from_millis(10)));
    }

    // --- P9: the settle predicate the frame pump gates on (§8 idle cost) is
    // pinned on the production RETARGET path, not just jump()/constructor. ----

    #[test]
    fn retarget_settles_and_stops_pumping() {
        // Advance a critically-damped flight well past its settle time and
        // assert BOTH halves of the pump's stop condition: position essentially
        // at target AND `!animating()` (so the frame pump stops scheduling).
        let mut s = Spring::settled(0.0, CHROME_OMEGA);
        s.retarget(100.0);
        for _ in 0..200 {
            advance(&mut s, 0.005); // 1.0s total — far past the ~250ms settle
        }
        assert!(
            (s.position() - 100.0).abs() < 0.5,
            "settled near target, at {}",
            s.position()
        );
        assert!(!s.animating(), "a settled retarget must stop the frame pump");
    }

    #[test]
    fn unit_scale_spring_does_not_report_settled_before_it_arrives() {
        // The P6 regression guard. The reveal drives the spring on a UNIT
        // fraction (0→1). The old hardcoded `dx>0.5 || v>0.5` test reported a
        // unit-scale spring SETTLED at ~96% revealed (the velocity clause bit
        // while position was still 0.04 short — 4% residual, scraping under
        // §8.7 by accident). With span-relative epsilons it must still report
        // ANIMATING at ~96%, only settling once genuinely at the target.
        let mut s = Spring::settled(0.0, CHROME_OMEGA);
        s.retarget(1.0);
        // Advance to ~96% revealed (the doc's measured premature-stop point).
        let mut guard = 0;
        while s.position() < 0.96 && guard < 1000 {
            advance(&mut s, 0.002);
            guard += 1;
        }
        assert!(s.position() >= 0.96 && s.position() < 1.0, "at {}", s.position());
        assert!(
            s.animating(),
            "a unit-scale spring at {} is NOT settled — the old 0.5 \
             pixel-scale threshold wrongly stopped here",
            s.position()
        );
        // And it DOES settle once it actually reaches the target.
        for _ in 0..300 {
            advance(&mut s, 0.005);
        }
        assert!((s.position() - 1.0).abs() < 0.005, "arrives at {}", s.position());
        assert!(!s.animating(), "settles once genuinely at the unit target");
    }

    #[test]
    fn with_epsilons_pins_absolute_tolerances() {
        // An explicit fixed tolerance overrides the span-relative default — a
        // consumer can opt into "settled within 2.0 units" regardless of span.
        let mut s = Spring::settled(0.0, CHROME_OMEGA).with_epsilons(2.0, 2.0);
        s.retarget(100.0);
        for _ in 0..200 {
            advance(&mut s, 0.005);
        }
        // The looser absolute epsilon settles earlier than the 0.5% span default
        // would (0.5 units) — within 2.0 here.
        assert!(!s.animating(), "absolute epsilons settle within their tolerance");
        assert!((s.position() - 100.0).abs() < 2.0);
    }
}
