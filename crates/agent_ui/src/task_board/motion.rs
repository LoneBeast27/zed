//! Caelestia motion tokens (PARITY_SPEC §4.9 motion law) — split by concern:
//!
//! - [`curves`]: exact CSS timing-function evaluation — `cubic-bezier()` via
//!   a bisection solver, the `linear()` exit table, and [`MotionCurve`]
//!   unifying both so animated values can retarget across curve families.
//! - [`animated`]: the retargetable §4.9 primitives — [`StateFade`] one-shot
//!   crossfade bookkeeping, [`AnimatedValue`] / [`AnimatedColor`]
//!   Instant-clocked eased scalars/colors whose mid-flight retargets always
//!   re-base from the interpolated CURRENT value (§8.7a — no snaps, no
//!   opposite-endpoint replays), and the componentwise sRGB [`mix`].
//!
//! GUARD: gpui's `animation.rs` debug-asserts easing output ∈ [0,1], so the
//! overshooting [`SPATIAL`] curve must be evaluated INSIDE animator closures
//! (pass linear delta to `Animation`, call `SPATIAL.eval(delta)` or
//! `AnimatedValue::value_at`/`current` in the animator). [`DECEL`] and
//! [`EFFECTS`] stay within [0,1] and may be used directly via
//! `Animation::with_easing(DECEL.easing())`.

mod animated;
mod curves;

pub use animated::{
    AnimatedColor, AnimatedValue, STATE_FADE, STATE_FADE_WINDOW, StateFade, mix,
};
pub use curves::{CubicBezier, DECEL, EFFECTS, EXIT_POINTS, MotionCurve, SPATIAL, exit_eval};
