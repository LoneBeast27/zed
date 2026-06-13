//! Tests for the §4.9 retargetable animation primitives — kept in a sibling
//! file (`#[path]`-included from `animated.rs`) so the source stays under the
//! 500-line ceiling while retaining private-field access via `super::*`.

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

// --- P4: retarget_with is spring-aware (drops to a curve, no silent break) ---

#[test]
fn retarget_with_on_a_spring_drops_to_the_curve_continuously() {
    use super::super::spring::CHROME_OMEGA;
    // A spring-backed value, mid-flight toward 1.0.
    let mut value = AnimatedValue::spring(0., CHROME_OMEGA);
    value.retarget(1.0);
    std::thread::sleep(Duration::from_millis(20));
    let pos_before = value.current();
    assert!(
        pos_before > 0.0 && pos_before < 1.0,
        "mid-flight spring position, got {pos_before}"
    );
    assert!(value.velocity() > 0.0, "spring is moving");

    // Drop back to a curve toward a NEW target. BEFORE the fix this was a
    // silent no-op (current()/animating() kept dispatching to the live spring,
    // ignoring every written field). It must now: (a) drop the spring, (b)
    // start the curve from the spring's continuous current position (no snap),
    // (c) drive toward the new target under the curve clock.
    value.retarget_with(2.0, EFFECTS, Duration::from_millis(200));

    // (a) the spring is gone — velocity is a curve's 0 (curves carry no
    // momentum), proving dispatch no longer goes through the spring.
    assert_eq!(value.velocity(), 0.0, "drop-to-curve forgoes momentum");
    // (b) position is continuous across the handoff — the curve's t=0 value is
    // exactly where the spring was (no §8.7 snap to either endpoint).
    assert!(
        (value.value_at(0.0) - pos_before).abs() < 0.01,
        "curve must re-base from the spring's position ({pos_before}), got {}",
        value.value_at(0.0)
    );
    // (c) the new target is honoured and the curve actually reaches it (the
    // pre-fix bug: the value stayed on the stale spring trajectory toward 1.0
    // and NEVER reached 2.0).
    assert_eq!(value.target(), 2.0);
    assert_eq!(value.value_at(1.0), 2.0, "the curve lands on the new target");
    assert!(value.animating(), "the curve flight is live, on its own clock");
}

#[test]
fn retarget_with_on_a_curve_value_is_unchanged_by_the_spring_fix() {
    // The non-spring path must be byte-identical: a plain curve value still
    // re-bases continuously onto the new curve (the existing asymmetric-exit
    // contract), and velocity stays 0 throughout.
    let mut value = AnimatedValue::settled(1.0, DECEL, Duration::from_millis(500));
    value.retarget(0.0);
    std::thread::sleep(Duration::from_millis(20));
    let in_flight = value.current();
    value.retarget_with(1.0, MotionCurve::Exit, Duration::from_millis(400));
    assert_eq!(value.velocity(), 0.0, "curve values never carry momentum");
    assert!(
        (value.value_at(0.0) - in_flight).abs() < 0.05,
        "still re-bases from the in-flight value ({in_flight})"
    );
    assert_eq!(value.value_at(1.0), 1.0);
}
