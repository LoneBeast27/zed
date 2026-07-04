//! Obsidian node physics (graph.js `physStep`, EXACT constants) — a LOCAL
//! perturbation field over deterministic anchors, never a live layout solver
//! (PARITY_SPEC §4.9 as-built law).
//!
//! This is the SHARED field stepper: the constellation's subagent board
//! (`constellation`) and the vault browser's document graph (`vault_browser`)
//! both step the SAME physics so the two surfaces feel like one product — an
//! anchor spring pulls each body home, a shifted inverse-square charge repels
//! neighbours, a collide floor keeps rims apart, a root exclusion clears the
//! central body, and a sleep gate freezes the field cold once it settles.
//! `constellation::physics` re-exports this verbatim (parity preserved by its
//! own re-exported tests); `vault_browser::field` builds its bodies here too.
//!
//! Forces per body:
//!
//! - **anchor spring** `-K·d` pulling the displacement home,
//! - **shifted inverse-square repulsion** between bodies within `RANGE`
//!   (`REPEL·(1/d² − 1/RANGE²)` — continuous at the boundary; a hard cutoff
//!   step-kicked rim nodes into a limit cycle, killed web-side 2026-07-03),
//! - **per-pair collide floor** `r_a + r_b + GAP` with a hard linear shove,
//! - **root exclusion** `root_r + r + ROOT_GAP` (the orchestrator is a fixed
//!   body too),
//!
//! integrated semi-implicitly (Euler) with exponential damping `e^(−C·dt)`,
//! and a rest snap so the cold state is EXACTLY the anchors. The web steps
//! bodies in order within one pass (Gauss–Seidel style: earlier bodies expose
//! their updated displacement to later ones) — ported verbatim for parity.
//!
//! Pure and allocation-free: the caller owns the [`PhysBody`] buffer and calls
//! [`step`] only while the sim is hot (the sleep gate lives in the caller).

/// Pair gap: hard minimum EXTRA distance between two dot rims (px).
pub const GAP: f32 = 30.;
/// Charge range: repulsion acts only within this distance (px).
pub const RANGE: f32 = 96.;
/// Root gap: extra clearance around the root dot's rim (px).
pub const ROOT_GAP: f32 = 43.;
/// Anchor spring constant.
pub const SPRING_K: f32 = 64.;
/// Exponential damping coefficient.
pub const DAMP_C: f32 = 13.;
/// Inverse-square charge strength.
pub const REPEL: f32 = 4.2e6;
/// Collide-floor shove stiffness (per px of overlap).
const COLLIDE_SHOVE: f32 = 600.;
/// Root-exclusion shove stiffness (per px of overlap).
const ROOT_SHOVE: f32 = 900.;
/// Rest snap: displacement below this…
const REST_DX: f32 = 0.02;
/// …with velocity below this zeroes the axis (rest = exact anchor).
const REST_V: f32 = 0.5;

/// One physics body: the anchor position (post-tween), its displacement +
/// velocity, live dot radius, and whether the pointer owns it.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PhysBody {
    /// Anchor (layout home) position.
    pub x: f32,
    pub y: f32,
    /// Displacement off the anchor (the field's output).
    pub dx: f32,
    pub dy: f32,
    /// Velocity of the displacement.
    pub vx: f32,
    pub vy: f32,
    /// Live dot radius (token mass, T4) — the collide floor reads it.
    pub r: f32,
    /// Pointer-authoritative: displacement pinned to 0, neighbors yield.
    pub dragged: bool,
}

impl PhysBody {
    /// A resting body at `(x, y)` with radius `r`.
    pub fn at(x: f32, y: f32, r: f32) -> Self {
        Self {
            x,
            y,
            r,
            ..Default::default()
        }
    }

    /// Current visual position (anchor + displacement).
    pub fn pos(&self) -> (f32, f32) {
        (self.x + self.dx, self.y + self.dy)
    }
}

/// Whether any body still carries momentum above the sleep-gate threshold
/// (graph.js: `|vx| + |vy| > 1.5`).
pub fn any_hot(bodies: &[PhysBody]) -> bool {
    bodies.iter().any(|b| b.vx.abs() + b.vy.abs() > 1.5)
}

/// One physics step over `bodies` around a root at `(root_x, root_y)` with
/// radius `root_r`. `dt` is in seconds — callers clamp it to the web's
/// [0.008, 0.033] window. In-place, allocation-free, deterministic.
pub fn step(bodies: &mut [PhysBody], root_x: f32, root_y: f32, root_r: f32, dt: f32) {
    let range_inv2 = 1. / (RANGE * RANGE);
    let damp = (-DAMP_C * dt).exp();
    for i in 0..bodies.len() {
        let body = bodies[i];
        if body.dragged {
            let b = &mut bodies[i];
            b.dx = 0.;
            b.dy = 0.;
            b.vx = 0.;
            b.vy = 0.;
            continue;
        }
        // Anchor spring.
        let mut fx = -SPRING_K * body.dx;
        let mut fy = -SPRING_K * body.dy;
        // Pairwise charge + collide floor (Gauss–Seidel: bodies already
        // stepped this pass expose their NEW displacement — web loop parity).
        for j in 0..bodies.len() {
            if j == i {
                continue;
            }
            let other = bodies[j];
            let ax = (body.x + body.dx) - (other.x + other.dx);
            let ay = (body.y + body.dy) - (other.y + other.dy);
            let d = ax.hypot(ay).max(0.01);
            if d < RANGE {
                let mut push = REPEL * (1. / (d * d) - range_inv2);
                let min_d = body.r + other.r + GAP;
                if d < min_d {
                    push += (min_d - d) * COLLIDE_SHOVE;
                }
                fx += (ax / d) * push;
                fy += (ay / d) * push;
            }
        }
        // Root exclusion.
        let rx = (body.x + body.dx) - root_x;
        let ry = (body.y + body.dy) - root_y;
        let rd = rx.hypot(ry).max(0.01);
        let root_min = root_r + body.r + ROOT_GAP;
        if rd < root_min {
            let push = (root_min - rd) * ROOT_SHOVE;
            fx += (rx / rd) * push;
            fy += (ry / rd) * push;
        }
        // Semi-implicit Euler + exponential damping + rest snap.
        let b = &mut bodies[i];
        b.vx = (b.vx + fx * dt) * damp;
        b.vy = (b.vy + fy * dt) * damp;
        b.dx += b.vx * dt;
        b.dy += b.vy * dt;
        if b.dx.abs() < REST_DX && b.vx.abs() < REST_V {
            b.dx = 0.;
            b.vx = 0.;
        }
        if b.dy.abs() < REST_DX && b.vy.abs() < REST_V {
            b.dy = 0.;
            b.vy = 0.;
        }
    }
}

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
    fn near_coincident_anchors_separate_without_nan() {
        // Degenerate stack: two bodies 1px apart (a drag drop onto a
        // neighbor). The 0.01 distance floor keeps the math finite and the
        // charge splits them. (EXACTLY coincident positions have a zero
        // direction vector and never separate — web parity: graph.js's
        // `ax/d` is 0/0.01 there too; fan slots + drag deltas make exact
        // coincidence unreachable in practice.)
        let mut bodies = vec![PhysBody::at(300., 200., 9.), PhysBody::at(301., 200., 9.)];
        run(&mut bodies, (84., 200., 12.), 600);
        assert!(finite(&bodies), "no NaN from the near-coincident pair");
        let (ax, ay) = bodies[0].pos();
        let (bx, by) = bodies[1].pos();
        let d = (ax - bx).hypot(ay - by);
        assert!(d > 10., "stacked bodies split apart, d = {d}");
    }

    #[test]
    fn overlapping_anchors_settle_into_a_pushed_apart_equilibrium() {
        // Anchors closer than the collide floor (9+9+30 = 48): the field
        // holds them apart at a spring/charge balance, then goes cold (the
        // sleep gate freezes displacement at the equilibrium).
        let mut bodies = vec![PhysBody::at(300., 200., 9.), PhysBody::at(320., 200., 9.)];
        run(&mut bodies, (84., 200., 12.), 1200); // 20 s
        assert!(finite(&bodies));
        let (ax, ay) = bodies[0].pos();
        let (bx, by) = bodies[1].pos();
        let d = (ax - bx).hypot(ay - by);
        assert!(d > 30., "held apart (anchor gap 20 → visual {d})");
        assert!(!any_hot(&bodies), "equilibrium is cold, momentum bled off");
    }

    #[test]
    fn dragged_body_is_pointer_authoritative_and_neighbors_yield() {
        let mut bodies = vec![
            PhysBody {
                dragged: true,
                ..PhysBody::at(300., 200., 9.)
            },
            // Neighbor inside the dragged body's collide floor.
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
    fn root_exclusion_pushes_a_body_off_the_root() {
        // Anchor sits ON the root: rootMin = 12 + 9 + 43 = 64 px.
        let mut bodies = vec![PhysBody::at(90., 200., 9.)];
        run(&mut bodies, (84., 200., 12.), 1200);
        assert!(finite(&bodies));
        let (x, y) = bodies[0].pos();
        let d = (x - 84.).hypot(y - 200.);
        assert!(d > 40., "pushed off the root body, d = {d}");
        assert!(!any_hot(&bodies), "settles cold at the exclusion boundary");
    }

    #[test]
    fn stepper_is_deterministic() {
        let seed = || {
            vec![
                PhysBody::at(300., 200., 9.),
                PhysBody::at(330., 210., 15.),
                PhysBody {
                    vx: 25.,
                    ..PhysBody::at(280., 260., 12.)
                },
            ]
        };
        let mut a = seed();
        let mut b = seed();
        run(&mut a, (84., 210., 12.), 300);
        run(&mut b, (84., 210., 12.), 300);
        assert_eq!(a, b, "identical inputs step to bitwise-identical states");
    }

    #[test]
    fn constants_match_graph_js() {
        // The approved design's tuning — lock it (graph.js physStep).
        assert_eq!(
            (GAP, RANGE, ROOT_GAP, SPRING_K, DAMP_C, REPEL),
            (30., 96., 43., 64., 13., 4.2e6)
        );
    }
}
