//! Per-frame step tests: the gobble end-to-end, the spawn flight, the
//! physics field behind its sleep gate, and the frame-pump signal.

use crate::bridge::{RunRow, RunTokens};

use super::super::sim::{ABSORB_MS, LINGER_MS, MOMENTUM_CAP_MS, Sim, WAKE_MS};
use super::*;

fn run(id: &str, status: &str) -> RunRow {
    RunRow {
        run_id: id.into(),
        conv: "c1".into(),
        status: status.into(),
        agent: "claude".into(),
        tokens: Some(RunTokens {
            weighted: 10_000.,
            ingest: 420.,
        }),
        ..Default::default()
    }
}

/// Advance the sim in 16ms frames from `from` to `to` (exclusive).
fn pump(sim: &mut Sim, from: f32, to: f32) {
    let mut t = from;
    while t < to {
        sim.advance(t);
        t += 16.;
    }
}

#[test]
fn gobble_end_to_end_linger_reel_gulp_mass_gain() {
    let mut sim = Sim::default();
    sim.fold(&[run("r1", "running")], &[], 440., 0.);
    sim.advance(16.);
    // Watched completion at t=1000.
    sim.fold(&[run("r1", "completed")], &[], 440., 1000.);
    // Within the linger: still on stage, solid dot, NOT absorbing.
    pump(&mut sim, 1000., 1000. + LINGER_MS - 50.);
    assert_eq!(sim.convs[0].nodes.len(), 1);
    assert!(sim.convs[0].nodes[0].absorbing.is_none(), "linger holds 4s");
    // Linger elapses → the reel-in arms.
    sim.advance(1000. + LINGER_MS + 20.);
    assert!(sim.convs[0].nodes[0].absorbing.is_some(), "reel-in armed");
    // Mid-flight: the prey shrinks and its edge fades.
    let mid = 1000. + LINGER_MS + 20. + ABSORB_MS * 0.75;
    sim.advance(mid);
    let node = &sim.convs[0].nodes[0];
    assert!(node.out_scale < 0.7, "prey shrinks crossing the rim");
    assert!(node.out_edge_alpha < 0.6, "the edge follows it in");
    // Flight lands: node gone, root gulps + pulses + GAINS the receipt.
    sim.advance(1000. + LINGER_MS + 20. + ABSORB_MS + 20.);
    assert!(sim.convs[0].nodes.is_empty(), "prey vanished into the root");
    assert!(sim.is_absorbed("r1"));
    let conv = &sim.convs[0];
    assert_eq!(conv.fed, 1);
    assert!(conv.gulp_at.is_some(), "root gulps");
    assert_eq!(conv.pulses.len(), 1, "one outward feed ring");
    // Legacy fed fallback grows the root target (no ctx data in this fold).
    assert!(conv.root_target() > 24.);
    // Later folds keep the gobbled run off the stage (grid keeps them).
    sim.fold(&[run("r1", "completed")], &[], 440., 8000.);
    assert!(sim.convs.is_empty() || sim.convs[0].nodes.is_empty());
    assert!(sim.is_absorbed("r1"), "absorbed survives re-folds");
}

#[test]
fn gobble_banks_the_ingest_receipt_on_a_ctx_root() {
    use crate::bridge::ConversationRow;
    let mut sim = Sim::default();
    let convs = vec![ConversationRow {
        id: "c1".into(),
        ctx_tokens: Some(10_000.),
        ..Default::default()
    }];
    sim.fold(&[run("r1", "running")], &convs, 440., 0.);
    sim.fold(&[run("r1", "completed")], &convs, 440., 100.);
    pump(&mut sim, 100., 100. + LINGER_MS + ABSORB_MS + 100.);
    let mass = sim.convs[0].root_mass.unwrap();
    assert_eq!(mass.bonus, 420., "the root gains the RECEIPT, not the burn");
}

#[test]
fn spawn_flight_leaves_the_root_and_lands_on_the_slot() {
    let mut sim = Sim::default();
    sim.fold(&[run("r1", "running")], &[], 440., 0.);
    // During the stagger hold the node sits AT the root.
    sim.advance(16.);
    let conv = &sim.convs[0];
    let (rx, ry) = (conv.layout.rx, conv.layout.ry);
    let node = &conv.nodes[0];
    let d_root = (node.out_x - rx).hypot(node.out_y - ry);
    assert!(d_root < 1., "born at the root, {d_root}px away");
    // Well after the flight (+ stagger) it breathes around its slot.
    pump(&mut sim, 16., 1200.);
    let conv = &sim.convs[0];
    let node = &conv.nodes[0];
    let slot = conv.layout.positions[0];
    let d_slot = (node.out_x - slot.0).hypot(node.out_y - slot.1);
    assert!(
        d_slot < 12.,
        "settled near its slot (drift + overshoot slack), {d_slot}px away"
    );
    assert!(node.spawn.is_none(), "spawn choreography ran exactly once");
}

#[test]
fn physics_separates_crowded_nodes_then_the_field_sleeps() {
    let mut sim = Sim::default();
    // Six nodes fan into a 340px-wide panel — inner slots sit within the
    // collide floor of each other, so the wake window must push them apart.
    let board: Vec<RunRow> = (0..6).map(|i| run(&format!("r{i}"), "running")).collect();
    sim.fold(&board, &[], 340., 0.);
    pump(&mut sim, 0., 3000.);
    let conv = &sim.convs[0];
    for a in 0..conv.nodes.len() {
        for b in (a + 1)..conv.nodes.len() {
            let na = &conv.nodes[a];
            let nb = &conv.nodes[b];
            let d = (na.out_x - nb.out_x).hypot(na.out_y - nb.out_y);
            assert!(d.is_finite(), "no NaN in the settled field");
        }
    }
    // Past the wake window with tweens settled, the field is cold: only
    // drift moves (bounded by the 3.6px amplitude around anchors).
    let displaced = conv
        .nodes
        .iter()
        .map(|n| n.vx.abs() + n.vy.abs())
        .fold(0. , f32::max);
    assert!(displaced < 1.5, "momentum bled off, max |v| sum {displaced}");
}

#[test]
fn settled_field_is_perfectly_still_no_ambient_drift() {
    // User report 2026-07-10 ("brownian motion in slowmotion"): the Lissajous
    // ambience must NOT ride a cold field. Once settled, two reads at
    // different times give IDENTICAL positions.
    let mut sim = Sim::default();
    let board: Vec<RunRow> = (0..3).map(|i| run(&format!("r{i}"), "running")).collect();
    sim.fold(&board, &[], 900., 0.);
    pump(&mut sim, 0., 8000.); // well past every wake/tween/cap
    let snap_a: Vec<(f32, f32)> = sim.convs[0]
        .nodes
        .iter()
        .map(|n| (n.out_x, n.out_y))
        .collect();
    pump(&mut sim, 8000., 9000.);
    let snap_b: Vec<(f32, f32)> = sim.convs[0]
        .nodes
        .iter()
        .map(|n| (n.out_x, n.out_y))
        .collect();
    assert_eq!(snap_a, snap_b, "a settled constellation does not wander");
}

#[test]
fn momentum_only_limit_cycle_freezes_at_the_cap() {
    // THE user-reported constellation bug (2026-07-10): a dense sibling
    // cluster sustains a collide-floor oscillation above the sleep threshold
    // forever ("bistable medium it alternates between and doesn't settle").
    // Twelve fat nodes into a 340px panel = every slot deep inside its
    // neighbors' floors. Whatever the cycle does, momentum-only heat must
    // not outlive WAKE_MS + MOMENTUM_CAP_MS (+ slack): the field freezes.
    let mut sim = Sim::default();
    let board: Vec<RunRow> = (0..12).map(|i| run(&format!("r{i}"), "running")).collect();
    sim.fold(&board, &[], 340., 0.);
    let deadline = WAKE_MS + MOMENTUM_CAP_MS + 1500.;
    pump(&mut sim, 0., deadline);
    let conv = &sim.convs[0];
    let hottest = conv
        .nodes
        .iter()
        .map(|n| n.vx.abs() + n.vy.abs())
        .fold(0., f32::max);
    assert!(
        hottest < 1.5,
        "momentum-only heat outlived the cap — limit cycle not frozen, \
         max |v| sum {hottest}"
    );
    for n in &conv.nodes {
        assert!(n.out_x.is_finite() && n.out_y.is_finite());
    }
}

#[test]
fn drift_is_deterministic_bounded_and_desynchronized() {
    for i in 0..8 {
        for t in [0., 250., 1000., 60_000.] {
            let (dx, dy) = drift(i, t);
            assert_eq!((dx, dy), drift(i, t), "pure function of (i, t)");
            assert!(dx.abs() <= 3.6 && dy.abs() <= 3.6, "amplitude bound");
        }
    }
    // Neighboring indices don't move in lockstep.
    let (a, _) = drift(0, 1000.);
    let (b, _) = drift(1, 1000.);
    assert!((a - b).abs() > 1e-3, "desynchronized phases");
}

#[test]
fn frame_pump_signal_populated_vs_empty() {
    let mut sim = Sim::default();
    assert!(!sim.advance(16.), "an empty stage needs no frames");
    sim.fold(&[run("r1", "running")], &[], 440., 32.);
    assert!(sim.advance(48.), "a populated constellation breathes forever");
    sim.fold(&[], &[], 440., 64.);
    assert!(!sim.advance(80.), "cleared stage stops the pump");
}

#[test]
fn position_of_tracks_the_live_dot() {
    let mut sim = Sim::default();
    sim.fold(&[run("r1", "running")], &[], 440., 0.);
    pump(&mut sim, 0., 1000.);
    let node = &sim.convs[0].nodes[0];
    assert_eq!(sim.position_of("r1"), Some((node.out_x, node.out_y)));
    assert_eq!(sim.position_of("nope"), None);
}

#[test]
fn gulp_and_exhale_scales_start_and_end_at_unity() {
    assert_eq!(gulp_scale(None, 0.), 1.);
    assert_eq!(gulp_scale(Some(0.), 0.), 1.);
    assert_eq!(gulp_scale(Some(0.), GULP_MS), 1.);
    let peak = (0..=100)
        .map(|i| gulp_scale(Some(0.), GULP_MS * i as f32 / 100.))
        .fold(f32::MIN, f32::max);
    assert!(peak > 1.2, "the gulp swells past 1.2×, peaked {peak}");
    assert_eq!(exhale_scale(&[], 0.), 1.);
    let dip = (0..=100)
        .map(|i| exhale_scale(&[0.], EXHALE_MS * i as f32 / 100.))
        .fold(f32::MAX, f32::min);
    assert!(dip < 0.9, "the exhale dips below 0.9×, bottomed {dip}");
}
