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
        }
    }

    /// Set the damping ratio (1.0 critical, <1 under-damped overshoot,
    /// >1 over-damped sluggish). Chrome leaves it at critical.
    pub fn with_damping(mut self, zeta: f32) -> Self {
        self.zeta = zeta.max(0.0);
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

    /// Whether the spring is still settling — within ~0.5px of target AND
    /// nearly stopped means done (a critically-damped spring approaches
    /// asymptotically; this is the practical settle test the frame pump uses
    /// to stop scheduling frames, §8 idle cost).
    pub fn animating(&self) -> bool {
        match self.started {
            None => false,
            Some(started) => {
                let t = started.elapsed().as_secs_f32();
                let dx = (self.position_at(t) - self.target).abs();
                let v = self.velocity_at(t).abs();
                dx > 0.5 || v > 0.5
            }
        }
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
}
