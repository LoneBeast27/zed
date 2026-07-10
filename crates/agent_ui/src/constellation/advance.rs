//! The per-frame sim step (graph.js `tick`, minus the DOM): anchor tweens,
//! the gobble reel-in + finalize, mass swells, the physics field behind its
//! sleep gate, and the seeded Lissajous drift — all folded into each node's
//! `out_*` fields, which the draw layer renders verbatim. Runs BEFORE
//! element building each frame; the render path only reads.

use super::physics::{self, PhysBody};
use super::sim::{ABSORB_MS, ARRANGE_MS, LINGER_MS, SPAWN_MS, Sim, ease_out_back};
use crate::task_board::motion::SPATIAL;

/// Drift amplitude (px).
const DRIFT_A: f32 = 3.6;
/// Absorb shrink floor (prey crossing the rim).
const ABSORB_MIN_SCALE: f32 = 0.3;
/// Root gulp keyframes: 420ms jelly spring (web `dot.animate`).
pub const GULP_MS: f32 = 420.;
const GULP_KEYS: [(f32, f32); 4] = [(0., 1.), (0.35, 1.24), (0.7, 0.94), (1., 1.)];
/// Compaction exhale keyframes: ~600ms spring.
pub const EXHALE_MS: f32 = 600.;
const EXHALE_KEYS: [(f32, f32); 4] = [(0., 1.), (0.45, 0.86), (0.8, 1.04), (1., 1.)];
/// Pulse/exhale ring lifetime (ms, board.css `gpulse`/`gexhale` .6s).
pub const RING_MS: f32 = 600.;

/// Seeded Lissajous drift — deterministic periods/phases from the node's
/// render index: organic desynchronized breathing, zero randomness, zero
/// solver (graph.js `drift`, exact).
pub fn drift(i: usize, t_ms: f32) -> (f32, f32) {
    const TAU: f32 = std::f32::consts::TAU;
    let ix = i as f32;
    (
        DRIFT_A * ((t_ms / (5200. + (i % 4) as f32 * 1100.)) * TAU + ix * 1.7).sin(),
        DRIFT_A * ((t_ms / (6800. + (i % 3) as f32 * 1400.)) * TAU + ix * 2.9).sin(),
    )
}

/// The web's jelly easing `cubic-bezier(.34, 1.56, .64, 1)` (dot.animate).
const JELLY: crate::task_board::motion::CubicBezier =
    crate::task_board::motion::CubicBezier::new(0.34, 1.56, 0.64, 1.0);

/// Piecewise keyframe evaluation with the jelly easing applied per segment
/// (WAAPI semantics: the options easing runs between each keyframe pair).
fn keyframed(keys: &[(f32, f32)], k: f32) -> f32 {
    if k <= 0. {
        return keys[0].1;
    }
    if k >= 1. {
        return keys[keys.len() - 1].1;
    }
    for pair in keys.windows(2) {
        let (k0, v0) = pair[0];
        let (k1, v1) = pair[1];
        if k <= k1 {
            let local = (k - k0) / (k1 - k0);
            return v0 + (v1 - v0) * JELLY.eval(local);
        }
    }
    keys[keys.len() - 1].1
}

/// The root dot's gulp scale at `now` (1 when idle/finished).
pub fn gulp_scale(gulp_at: Option<f32>, now: f32) -> f32 {
    match gulp_at {
        None => 1.,
        Some(t0) => keyframed(&GULP_KEYS, (now - t0) / GULP_MS),
    }
}

/// The root dot's compaction-exhale scale at `now` (rides the newest exhale).
pub fn exhale_scale(exhales: &[f32], now: f32) -> f32 {
    match exhales.last() {
        None => 1.,
        Some(t0) => keyframed(&EXHALE_KEYS, (now - t0) / EXHALE_MS),
    }
}

impl Sim {
    /// Step the sim to `now` (sim ms). Returns `true` while anything on
    /// stage needs further frames. (Since the drift gate, a settled populated
    /// constellation is perfectly still — `frame_hot` tracks the real hot
    /// sources per frame instead of "any nodes exist", which pumped
    /// scratch-rebuild frames forever on a provably frozen field.)
    pub fn advance(&mut self, now: f32) -> bool {
        let dt = ((now - self.last_tick) / 1000.).clamp(0.008, 0.033);
        self.last_tick = now;
        self.channels.tick(now);
        let mut frame_hot = false;

        let mut gobbled: Vec<(usize, usize)> = Vec::new();
        for (conv_ix, conv) in self.convs.iter_mut().enumerate() {
            // Root box tween (swell toward f(ctx) / exhale) — same spring
            // curve as the arrange tween, never a snap.
            if let Some(tween) = conv.root_anim {
                let (value, done) = tween.eval(now);
                conv.root_cur = value;
                if done {
                    conv.root_anim = None;
                }
            }
            conv.pulses.retain(|t0| now - t0 < RING_MS);
            conv.exhales.retain(|t0| now - t0 < RING_MS + super::sim::ROOT_MASS_MS);
            if conv.gulp_at.is_some_and(|t0| now - t0 >= GULP_MS) {
                conv.gulp_at = None;
            }

            let root = (conv.layout.rx, conv.layout.ry);
            let root_r = conv.root_cur / 2.;

            // Per-node tweens: arrange (anchor), absorb (reel-in), mass.
            for (node_ix, node) in conv.nodes.iter_mut().enumerate() {
                if let Some(tween) = node.arranging {
                    let k = ((now - tween.t0) / ARRANGE_MS).clamp(0., 1.);
                    let e = ease_out_back(k);
                    node.ax = tween.fx + (tween.tx - tween.fx) * e;
                    node.ay = tween.fy + (tween.ty - tween.fy) * e;
                    if k >= 1. {
                        node.arranging = None;
                    }
                }
                if let Some(tween) = node.mass_anim {
                    let (value, done) = tween.eval(now);
                    node.mass_cur = value;
                    if done {
                        node.mass_anim = None;
                    }
                }
                // Completion linger elapsed → schedule the gobble.
                if node.absorbing.is_none()
                    && self
                        .completed_at
                        .get(&node.run_id)
                        .is_some_and(|at| now - at > LINGER_MS)
                {
                    node.absorbing = Some((node.ax, node.ay, now));
                    self.completed_at.remove(&node.run_id);
                }
                // The reel-in: anchor accelerates into the root (Agar.io
                // slurp = cubic ease-in); the dot shrinks crossing the rim
                // and drags its own edge in behind it.
                if let Some((fx, fy, t0)) = node.absorbing {
                    let k = ((now - t0) / ABSORB_MS).clamp(0., 1.);
                    let e = k * k * k;
                    node.ax = fx + (root.0 - fx) * e;
                    node.ay = fy + (root.1 - fy) * e;
                    node.out_scale = (1. - 0.68 * k).max(ABSORB_MIN_SCALE);
                    node.out_edge_alpha = (1. - k * 1.4).max(0.);
                    if k >= 1. {
                        gobbled.push((conv_ix, node_ix));
                    }
                } else {
                    node.out_scale = 1.;
                    node.out_edge_alpha = 1.;
                }
                // Spawn flight progress (SPATIAL overshoot, staggered).
                if let Some((t0, delay)) = node.spawn {
                    let k = (now - t0 - delay) / SPAWN_MS;
                    if k >= 1. {
                        node.spawn = None;
                        node.out_spawn = 1.;
                    } else {
                        node.out_spawn = if k <= 0. { 0. } else { SPATIAL.eval(k) };
                    }
                }
            }

            // The physics field behind its sleep gate (d3 alpha idea): run
            // only while HOT — an interaction live, a tween in flight, or
            // momentum remaining. Cold = displacements frozen, zero
            // work/frame.
            let dragging = self.dragging.as_deref();
            self.scratch.clear();
            self.scratch.extend(conv.nodes.iter().map(|node| PhysBody {
                x: node.ax,
                y: node.ay,
                dx: node.dx,
                dy: node.dy,
                vx: node.vx,
                vy: node.vy,
                r: node.mass_cur / 2.,
                dragged: dragging == Some(node.run_id.as_str()),
            }));
            let tweening = dragging.is_some()
                || now < self.wake_until
                || conv.root_anim.is_some()
                || conv
                    .nodes
                    .iter()
                    .any(|n| n.arranging.is_some() || n.absorbing.is_some() || n.mass_anim.is_some());
            let momentum = physics::any_hot(&self.scratch);
            let hot = tweening || momentum;
            frame_hot |= hot
                || !conv.pulses.is_empty()
                || !conv.exhales.is_empty()
                || conv.gulp_at.is_some();
            if hot {
                physics::step(&mut self.scratch, root.0, root.1, root_r, dt);
                // Momentum-only hot cap: residual momentum with nothing
                // driving it past MOMENTUM_CAP_MS is a collide-floor limit
                // cycle (the bistable alternation the user reported
                // 2026-07-10), not a settle in progress — freeze it. The
                // timer is PER CONV (review find #3: a single Sim-level
                // field was reset every frame by any cold sibling conv, so
                // the cap never fired on multi-conversation boards).
                if tweening {
                    conv.momentum_only_since = None;
                } else {
                    let since = *conv.momentum_only_since.get_or_insert(now);
                    if now - since > super::sim::MOMENTUM_CAP_MS {
                        physics::freeze(&mut self.scratch);
                        conv.momentum_only_since = None;
                    }
                }
                for (node, body) in conv.nodes.iter_mut().zip(self.scratch.iter()) {
                    node.dx = body.dx;
                    node.dy = body.dy;
                    node.vx = body.vx;
                    node.vy = body.vy;
                }
            } else {
                conv.momentum_only_since = None;
            }

            // Outputs: anchor + physics + drift + spawn flight offset. A
            // dragged or absorbing node holds still (no drift); a spawning
            // node's position tracks the FLIGHT so its edge stretches with
            // the dot. The Lissajous drift rides ONLY while the field is hot
            // (user report 2026-07-10: perpetual ambient drift reads as
            // "brownian motion in slow motion" — settled must mean STILL;
            // the ≤3.6px offset snap at the hot→cold edge is sub-perceptual).
            for node in conv.nodes.iter_mut() {
                let still = !hot
                    || dragging == Some(node.run_id.as_str())
                    || node.absorbing.is_some();
                let (drift_x, drift_y) = if still {
                    (0., 0.)
                } else {
                    drift(node.drift_ix, now)
                };
                let mut x = node.ax + node.dx + drift_x;
                let mut y = node.ay + node.dy + drift_y;
                if node.spawn.is_some() {
                    // Born at the root: offset by (root − here)·(1 − s).
                    let hold = 1. - node.out_spawn;
                    x += (root.0 - x) * hold;
                    y += (root.1 - y) * hold;
                }
                node.out_x = x;
                node.out_y = y;
            }
        }

        // The eat moment: prey vanishes INTO the root; the root gulps,
        // pulses a ring outward, and GAINS the ingested receipt (T4 honesty:
        // the receipt, NOT the prey's own burn).
        for (conv_ix, node_ix) in gobbled.into_iter().rev() {
            let node = self.convs[conv_ix].nodes.remove(node_ix);
            self.mark_absorbed(node.run_id.clone());
            let conv = &mut self.convs[conv_ix];
            conv.fed += 1;
            if let Some(mass) = &mut conv.root_mass {
                mass.add_bonus(node.ingest);
            }
            conv.retarget_root(now, true);
            conv.gulp_at = Some(now);
            conv.pulses.push(now);
            self.wake_until = self.wake_until.max(now + super::sim::WAKE_MS);
        }

        self.last_frame_hot = frame_hot;
        self.animating()
    }

    /// Whether anything on stage needs frames: a hot field / live tween /
    /// ring this frame, channel particles/retracts in flight, or a gobble
    /// linger pending. A settled populated constellation is STILL (the drift
    /// gate) — "any nodes exist" used to pump frames forever over a frozen
    /// field (2026-07-10 review nit).
    pub fn animating(&self) -> bool {
        self.last_frame_hot
            || self.channels.animating()
            || !self.completed_at.is_empty()
    }

    /// The live visual position of a run's dot (channel edges + drawer
    /// anchors ride it).
    pub fn position_of(&self, run_id: &str) -> Option<(f32, f32)> {
        self.convs.iter().find_map(|conv| {
            conv.nodes
                .iter()
                .find(|node| node.run_id == run_id)
                .map(|node| (node.out_x, node.out_y))
        })
    }
}

#[cfg(test)]
#[path = "advance_tests.rs"]
mod tests;
