//! Fold-layer tests: board/conv frames → sim state (the event→state folding
//! half of the verification bar; advance_tests.rs covers the per-frame half).

use crate::bridge::{ConversationRow, OverlapRow, RunRow, RunTokens};

use super::super::mass::dot_px;
use super::*;

fn run(id: &str, conv: &str, status: &str, weighted: f64) -> RunRow {
    RunRow {
        run_id: id.into(),
        conv: conv.into(),
        status: status.into(),
        agent: "claude".into(),
        tokens: (weighted > 0.).then_some(RunTokens {
            weighted,
            ingest: 200.,
        }),
        ..Default::default()
    }
}

fn conv_row(id: &str, ctx: Option<f64>, compactions: u64) -> ConversationRow {
    ConversationRow {
        id: id.into(),
        title: "Demo conv".into(),
        ctx_tokens: ctx,
        compactions,
        ..Default::default()
    }
}

#[test]
fn fold_groups_runs_by_conversation_in_board_order() {
    let mut sim = Sim::default();
    let board = vec![
        run("r1", "c1", "running", 0.),
        run("r2", "c2", "running", 0.),
        run("r3", "c1", "running", 0.),
    ];
    sim.fold(&board, &[], 440., 0.);
    assert_eq!(sim.convs.len(), 2);
    assert_eq!(sim.convs[0].conv_id, "c1");
    assert_eq!(sim.convs[0].nodes.len(), 2);
    assert_eq!(sim.convs[1].conv_id, "c2");
    assert_eq!(sim.convs[1].nodes.len(), 1);
    // Anchors seed at the fan slots.
    let slot = sim.convs[0].layout.positions[0];
    assert_eq!((sim.convs[0].nodes[0].ax, sim.convs[0].nodes[0].ay), slot);
}

#[test]
fn first_sight_mass_adopts_without_a_tween_growth_tweens() {
    let mut sim = Sim::default();
    sim.fold(&[run("r1", "c1", "running", 20_000.)], &[], 440., 0.);
    let node = &sim.convs[0].nodes[0];
    assert_eq!(node.mass_cur, dot_px(20_000.), "adopted as-is");
    assert!(node.mass_anim.is_none(), "no retro-swell at first sight");
    // Growth retargets with a swell tween and wakes the field.
    sim.fold(&[run("r1", "c1", "running", 40_000.)], &[], 440., 1000.);
    let node = &sim.convs[0].nodes[0];
    assert_eq!(node.mass_target, dot_px(40_000.));
    assert!(node.mass_anim.is_some(), "growth swells via tween, never snaps");
    assert!(sim.wake_until >= 1000. + MASS_WAKE_MS);
}

#[test]
fn watched_completion_starts_the_linger_preexisting_completed_is_history() {
    let mut sim = Sim::default();
    // Pre-existing completed at FIRST sight: static history, no gobble.
    sim.fold(&[run("old", "c1", "completed", 0.)], &[], 440., 0.);
    assert!(sim.completed_at.is_empty(), "no retro-gobble at first sight");
    // A watched running→completed transition schedules the linger.
    sim.fold(&[run("r1", "c1", "running", 0.)], &[], 440., 100.);
    sim.fold(&[run("r1", "c1", "completed", 0.)], &[], 440., 2000.);
    assert_eq!(sim.completed_at.get("r1"), Some(&2000.));
    // Re-folding the same completed status never re-arms the clock.
    sim.fold(&[run("r1", "c1", "completed", 0.)], &[], 440., 3000.);
    assert_eq!(sim.completed_at.get("r1"), Some(&2000.));
}

#[test]
fn drag_pins_survive_folds_and_membership_reslots_only_unpinned() {
    let mut sim = Sim::default();
    sim.fold(
        &[run("r1", "c1", "running", 0.), run("r2", "c1", "running", 0.)],
        &[],
        440.,
        0.,
    );
    sim.begin_drag("r1");
    sim.drag_to("r1", 400., 300.);
    sim.end_drag(50.);
    // Membership change (a third run) re-slots the fan.
    let board = vec![
        run("r1", "c1", "running", 0.),
        run("r2", "c1", "running", 0.),
        run("r3", "c1", "running", 0.),
    ];
    sim.fold(&board, &[], 440., 1000.);
    let n1 = &sim.convs[0].nodes[0];
    assert!(n1.pinned);
    assert_eq!((n1.ax, n1.ay), (400., 300.), "pins are sacred");
    assert!(n1.arranging.is_none(), "pinned nodes never re-slot");
    let n2 = &sim.convs[0].nodes[1];
    assert!(
        n2.arranging.is_some(),
        "unpinned nodes re-slot via the arrange tween on membership change"
    );
    assert!(sim.wake_until >= 1000. + WAKE_MS, "new bodies wake the field");
}

#[test]
fn root_ctx_first_sight_adopts_then_exhales_on_compaction() {
    let mut sim = Sim::default();
    let board = vec![run("r1", "c1", "running", 0.)];
    sim.fold(&board, &[conv_row("c1", Some(10_000.), 0)], 440., 0.);
    let conv = &sim.convs[0];
    assert!(conv.root_mass.is_some());
    assert_eq!(conv.root_cur, conv.root_target(), "adopted silently");
    assert!(conv.root_anim.is_none());
    assert!(conv.exhales.is_empty());
    // Growth retargets the root box.
    sim.fold(&board, &[conv_row("c1", Some(30_000.), 0)], 440., 1000.);
    assert!(sim.convs[0].root_anim.is_some(), "ctx growth swells the root");
    // A compaction increment exhales (ring + forced retarget).
    sim.fold(&board, &[conv_row("c1", Some(8_000.), 1)], 440., 2000.);
    let conv = &sim.convs[0];
    assert_eq!(conv.exhales, vec![2000.]);
    assert_eq!(conv.root_mass.unwrap().compactions, 1);
}

#[test]
fn conv_title_resolves_and_falls_back() {
    let mut sim = Sim::default();
    sim.fold(
        &[run("r1", "c1", "running", 0.)],
        &[conv_row("c1", None, 0)],
        440.,
        0.,
    );
    assert_eq!(sim.convs[0].title.as_ref(), "Demo conv");
    sim.fold(&[run("r2", "c9", "running", 0.)], &[], 440., 100.);
    let unknown = sim.convs.iter().find(|c| c.conv_id == "c9").unwrap();
    assert_eq!(unknown.title.as_ref(), "conversation");
}

#[test]
fn auto_arrange_seats_weighted_pairs_adjacent_and_clears_pins() {
    let mut sim = Sim::default();
    let board = vec![
        run("plan", "c1", "running", 0.),
        run("impl-a", "c1", "running", 0.),
        run("designer", "c1", "running", 0.),
        run("impl-b", "c1", "running", 0.),
    ];
    sim.fold(&board, &[], 440., 0.);
    sim.begin_drag("impl-a");
    sim.drag_to("impl-a", 999., 999.);
    sim.end_drag(10.);
    sim.set_overlap(vec![OverlapRow {
        a: "impl-a".into(),
        b: "impl-b".into(),
        score: 3.,
    }]);
    sim.auto_arrange(1000.);
    let conv = &sim.convs[0];
    let slot_of = |id: &str| {
        let tween = conv
            .nodes
            .iter()
            .find(|n| n.run_id == id)
            .unwrap()
            .arranging
            .expect("arrange tweens every node");
        (tween.tx, tween.ty)
    };
    // The weighted pair lands in the first two (adjacent) fan slots.
    assert_eq!(slot_of("impl-a"), conv.layout.positions[0]);
    assert_eq!(slot_of("impl-b"), conv.layout.positions[1]);
    let pinned = conv.nodes.iter().find(|n| n.run_id == "impl-a").unwrap();
    assert!(!pinned.pinned, "arranging clears drag pins");
}

#[test]
fn vanished_runs_drop_all_ledger_state() {
    let mut sim = Sim::default();
    sim.fold(&[run("r1", "c1", "running", 0.)], &[], 440., 0.);
    sim.fold(&[run("r1", "c1", "completed", 0.)], &[], 440., 100.);
    assert!(sim.completed_at.contains_key("r1"));
    sim.fold(&[], &[], 440., 200.);
    assert!(sim.convs.is_empty());
    assert!(sim.completed_at.is_empty());
    assert!(!sim.is_absorbed("r1"));
}

#[test]
fn ease_out_back_matches_the_web_curve() {
    // Endpoints exact, overshoot past 1 mid-flight (the spring feel).
    assert!((ease_out_back(0.) - 0.).abs() < 1e-6);
    assert!((ease_out_back(1.) - 1.).abs() < 1e-6);
    let peak = (0..=100)
        .map(|i| ease_out_back(i as f32 / 100.))
        .fold(f32::MIN, f32::max);
    assert!(peak > 1.05 && peak < 1.15, "easeOutBack overshoot, peak {peak}");
}
