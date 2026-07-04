//! The constellation's Obsidian node physics — now the SHARED field stepper
//! ([`crate::field_physics`]), re-exported verbatim so the vault-browser
//! document graph and the subagent board step the identical field (one
//! product). Constellation code keeps importing `super::physics::{PhysBody,
//! step, any_hot}`; this module is a thin pass-through. The parity tests
//! below run against the re-exported symbols, so a change to the shared
//! stepper that breaks the constellation's contract fails HERE.

pub use crate::field_physics::{
    DAMP_C, GAP, PhysBody, RANGE, REPEL, ROOT_GAP, SPRING_K, any_hot, step,
};

#[cfg(test)]
mod tests {
    use super::*;

    /// Step `n` frames at 60fps.
    fn run(bodies: &mut [PhysBody], root: (f32, f32, f32), n: usize) {
        for _ in 0..n {
            step(bodies, root.0, root.1, root.2, 1. / 60.);
        }
    }

    fn finite(bodies: &[PhysBody]) -> bool {
        bodies
            .iter()
            .all(|b| b.dx.is_finite() && b.dy.is_finite() && b.vx.is_finite() && b.vy.is_finite())
    }

    #[test]
    fn well_separated_bodies_converge_to_exact_anchors() {
        // The verification bar: given a node set + steps, positions converge,
        // no NaN. Bodies far apart (outside RANGE, outside the root) with
        // initial displacement + velocity must return to rest = EXACT anchors.
        let mut bodies = vec![
            PhysBody {
                dx: 12.,
                dy: -8.,
                vx: 40.,
                vy: -25.,
                ..PhysBody::at(300., 100., 9.)
            },
            PhysBody {
                dx: -20.,
                dy: 14.,
                vx: -10.,
                vy: 60.,
                ..PhysBody::at(300., 320., 9.)
            },
            PhysBody::at(560., 210., 15.),
        ];
        run(&mut bodies, (84., 210., 12.), 600); // 10 s
        assert!(finite(&bodies), "no NaN/inf after 600 steps");
        for (i, b) in bodies.iter().enumerate() {
            assert_eq!((b.dx, b.dy), (0., 0.), "body {i} rests at its anchor");
            assert_eq!((b.vx, b.vy), (0., 0.), "body {i} has no residual velocity");
        }
        assert!(!any_hot(&bodies), "cold field after settle");
    }

    #[test]
    fn dragged_body_is_pointer_authoritative_and_neighbors_yield() {
        let mut bodies = vec![
            PhysBody {
                dragged: true,
                ..PhysBody::at(300., 200., 9.)
            },
            PhysBody::at(310., 200., 9.),
        ];
        run(&mut bodies, (84., 400., 12.), 120);
        assert_eq!(
            (bodies[0].dx, bodies[0].dy, bodies[0].vx, bodies[0].vy),
            (0., 0., 0., 0.),
            "dragged displacement stays pinned"
        );
        assert!(
            bodies[1].dx.abs() + bodies[1].dy.abs() > 5.,
            "the neighbor yields around the dragged body"
        );
    }

    #[test]
    fn constants_match_graph_js() {
        // The constellation's contract on the shared constants.
        assert_eq!(
            (GAP, RANGE, ROOT_GAP, SPRING_K, DAMP_C, REPEL),
            (30., 96., 43., 64., 13., 4.2e6)
        );
    }
}
