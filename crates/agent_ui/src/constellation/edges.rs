//! Boost-channel edges (TEAMS §4/T2 — graph-channels.js, STRICTLY the
//! banked Kiali grammar, no new choreography):
//!
//! - a DIRECT sibling↔sibling edge per open channel; a silent glowing edge
//!   IS the "open-idle" state (no ambient marching ants),
//! - ONE particle travels the edge per relayed batch (k delta), never
//!   continuous — staggered 180ms, capped at 3 per fold so a stale frame
//!   never bursts the edge,
//! - a k/max batch-count badge at the edge midpoint (n8n items-count steal),
//! - edge tint binds to the state: working/awaiting = alive vs held glow,
//!   terminal = 400ms retract (both endpoints ease into the midpoint on p²,
//!   exits sharper than entries) while the badge absorbs on the same beat.
//!
//! Endpoints ride the two nodes' LIVE positions — the sim's advance feeds
//! the per-frame position map, so the channel edge is part of the same
//! breathing body as the star edges. Time is the sim clock (ms since sim
//! start), so every transition is unit-testable without sleeping.

use std::collections::{HashMap, HashSet};

use crate::bridge::ChannelRow;

/// Particle flight duration (ms).
pub const PARTICLE_MS: f32 = 700.;
/// Stagger between particles from one k jump (ms).
const PARTICLE_STAGGER_MS: f32 = 180.;
/// Max particles fired per fold (a stale frame never bursts the edge).
const PARTICLE_CAP: u64 = 3;
/// Terminal retract duration (ms) — exits sharper than entries.
pub const RETRACT_MS: f32 = 400.;
/// Rim trim for a live channel edge (px, the 13px rim contract).
pub const CHANNEL_TRIM: f32 = 13.;
/// Rim trim while retracting (the shrinking edge hugs the midpoint).
pub const RETRACT_TRIM: f32 = 2.;

/// Terminal channel states (graph-channels.js `TERMINAL`).
pub fn is_terminal(state: &str) -> bool {
    matches!(
        state,
        "converged" | "expired" | "exhausted" | "killed" | "peer_died"
    )
}

/// One in-flight relay particle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Particle {
    /// Start time (sim ms) — staggered starts sit in the future.
    pub t0: f32,
    /// Travels b→a instead of a→b (`last_from == b`).
    pub rev: bool,
}

impl Particle {
    /// Position along `from → to` at `now` (ease-out `k(2−k)`), `None`
    /// before the staggered start or after landing.
    pub fn pos_at(&self, now: f32, from: (f32, f32), to: (f32, f32)) -> Option<(f32, f32)> {
        let k = (now - self.t0) / PARTICLE_MS;
        if !(0. ..1.).contains(&k) {
            return None;
        }
        let e = k * (2. - k);
        Some((from.0 + (to.0 - from.0) * e, from.1 + (to.1 - from.1) * e))
    }

    /// Whether the flight is over (landed — pruned by the sim).
    pub fn done(&self, now: f32) -> bool {
        now - self.t0 >= PARTICLE_MS
    }
}

/// One channel's live runtime: the latest row + watched-k particle state +
/// the terminal retract clock.
#[derive(Debug, Clone, PartialEq)]
pub struct ChannelSim {
    pub row: ChannelRow,
    last_k: u64,
    pub particles: Vec<Particle>,
    /// Sim-ms the terminal state was first seen — the retract start.
    pub dead_at: Option<f32>,
}

impl ChannelSim {
    /// Retract progress ∈ [0,1] (p², exits sharper than entries); 0 while
    /// alive.
    pub fn retract(&self, now: f32) -> f32 {
        match self.dead_at {
            None => 0.,
            Some(at) => (((now - at) / RETRACT_MS).clamp(0., 1.)).powi(2),
        }
    }

    /// Fully retracted — destroy (and never re-render) this channel.
    pub fn gone(&self, now: f32) -> bool {
        self.dead_at
            .is_some_and(|at| now - at >= RETRACT_MS)
    }
}

/// The channel set: folds `/channels`-shaped rows (from SSE or the poll)
/// into per-channel runtimes. Fully retracted channels enter `gone` and are
/// never re-rendered (graph-channels.js `destroy` + `gone`).
#[derive(Debug, Default)]
pub struct ChannelSet {
    sims: HashMap<String, ChannelSim>,
    gone: HashSet<String>,
}

impl ChannelSet {
    /// Fold the latest channel rows at sim time `now` (ms). Idempotent for
    /// unchanged rows; k deltas fire particles (watched transitions only —
    /// a channel discovered mid-flight adopts k as-is, same rule as the
    /// gobble); rows vanishing from the feed destroy their runtime.
    pub fn fold(&mut self, rows: &[ChannelRow], now: f32) {
        let mut live: HashSet<&str> = HashSet::with_capacity(rows.len());
        for row in rows {
            if self.gone.contains(&row.channel_id) {
                continue;
            }
            live.insert(row.channel_id.as_str());
            match self.sims.get_mut(&row.channel_id) {
                None => {
                    // First sight: adopt k as-is — no retro-particles. A
                    // channel discovered ALREADY terminal still retracts
                    // (web syncChannels arms deadAt regardless of adoption).
                    self.sims.insert(
                        row.channel_id.clone(),
                        ChannelSim {
                            last_k: row.k,
                            row: row.clone(),
                            particles: Vec::new(),
                            dead_at: is_terminal(&row.state).then_some(now),
                        },
                    );
                }
                Some(sim) => {
                    let fresh = row.k.saturating_sub(sim.last_k).min(PARTICLE_CAP);
                    for i in 0..fresh {
                        sim.particles.push(Particle {
                            t0: now + i as f32 * PARTICLE_STAGGER_MS,
                            rev: row.last_from.as_deref() == Some(row.b.as_str()),
                        });
                    }
                    sim.last_k = sim.last_k.max(row.k);
                    if is_terminal(&row.state) && sim.dead_at.is_none() {
                        sim.dead_at = Some(now);
                    }
                    sim.row = row.clone();
                }
            }
        }
        self.sims.retain(|id, _| live.contains(id.as_str()));
    }

    /// Prune landed particles and move fully-retracted channels to `gone`.
    /// Called once per sim advance.
    pub fn tick(&mut self, now: f32) {
        let gone = &mut self.gone;
        self.sims.retain(|id, sim| {
            // A dead channel drops its particles at once (web tickParticle:
            // `k >= 1 || rt.deadAt` removes).
            let dead = sim.dead_at.is_some();
            sim.particles.retain(|p| !dead && !p.done(now));
            if sim.gone(now) {
                gone.insert(id.clone());
                return false;
            }
            true
        });
    }

    /// Live channel runtimes (render order: channel id, deterministic).
    pub fn iter_sorted(&self) -> Vec<&ChannelSim> {
        let mut sims: Vec<&ChannelSim> = self.sims.values().collect();
        sims.sort_by(|a, b| a.row.channel_id.cmp(&b.row.channel_id));
        sims
    }

    pub fn is_empty(&self) -> bool {
        self.sims.is_empty()
    }

    /// Any motion in flight (particles or retracts) — the frame-pump signal.
    pub fn animating(&self) -> bool {
        self.sims
            .values()
            .any(|sim| !sim.particles.is_empty() || sim.dead_at.is_some())
    }

    /// The latest rows (auto-arrange link weights read batch counts).
    pub fn rows(&self) -> impl Iterator<Item = &ChannelRow> {
        self.sims.values().map(|sim| &sim.row)
    }
}

/// A straight segment from `(ax, ay)` to `(bx, by)` trimmed by `ta`/`tb` at
/// each end so it meets dot rims, not centers (graph.js `edgePath` +
/// graph-channels.js `trimmedPath` unified — the star edge passes per-end
/// rim trims, the channel edge a symmetric trim clamped to half length).
pub fn trimmed_segment(
    ax: f32,
    ay: f32,
    bx: f32,
    by: f32,
    ta: f32,
    tb: f32,
) -> ((f32, f32), (f32, f32)) {
    let len = (bx - ax).hypot(by - ay).max(1e-6);
    let half = (len / 2. - 0.5).max(0.);
    let (ta, tb) = (ta.min(half), tb.min(half));
    let ux = (bx - ax) / len;
    let uy = (by - ay) / len;
    (
        (ax + ux * ta, ay + uy * ta),
        (bx - ux * tb, by - uy * tb),
    )
}

/// The retract's live endpoints: both ends ease into the midpoint by `p`.
pub fn retract_endpoints(
    pa: (f32, f32),
    pb: (f32, f32),
    p: f32,
) -> ((f32, f32), (f32, f32), (f32, f32)) {
    let mid = ((pa.0 + pb.0) / 2., (pa.1 + pb.1) / 2.);
    let a = (pa.0 + (mid.0 - pa.0) * p, pa.1 + (mid.1 - pa.1) * p);
    let b = (pb.0 + (mid.0 - pb.0) * p, pb.1 + (mid.1 - pb.1) * p);
    (a, b, mid)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, k: u64, state: &str) -> ChannelRow {
        ChannelRow {
            channel_id: id.into(),
            conv: "c".into(),
            a: "r1".into(),
            b: "r2".into(),
            state: state.into(),
            k,
            max_batches: 5,
            last_from: Some("r2".into()),
            ..Default::default()
        }
    }

    #[test]
    fn first_sight_adopts_k_without_retro_particles() {
        let mut set = ChannelSet::default();
        set.fold(&[row("ch", 3, "working")], 1000.);
        let sims = set.iter_sorted();
        assert_eq!(sims.len(), 1);
        assert!(sims[0].particles.is_empty(), "discovered mid-flight: no burst");
        assert_eq!(sims[0].last_k, 3);
    }

    #[test]
    fn k_delta_fires_one_staggered_particle_per_batch_capped() {
        let mut set = ChannelSet::default();
        set.fold(&[row("ch", 0, "awaiting_a")], 0.);
        set.fold(&[row("ch", 2, "working")], 1000.);
        {
            let sims = set.iter_sorted();
            assert_eq!(sims[0].particles.len(), 2);
            assert_eq!(sims[0].particles[0].t0, 1000.);
            assert_eq!(sims[0].particles[1].t0, 1180., "180ms stagger");
            assert!(sims[0].particles[0].rev, "last_from == b → reversed");
        }
        // A missed-poll jump of 7 fires at most 3.
        set.fold(&[row("ch", 9, "working")], 2000.);
        assert_eq!(set.iter_sorted()[0].particles.len(), 2 + 3);
        // k never regresses (max), so a stale frame can't re-fire.
        set.fold(&[row("ch", 9, "working")], 3000.);
        assert_eq!(set.iter_sorted()[0].particles.len(), 5);
    }

    #[test]
    fn particle_travel_is_ease_out_and_prunable() {
        let p = Particle { t0: 0., rev: false };
        assert_eq!(p.pos_at(-10., (0., 0.), (100., 0.)), None, "pre-stagger");
        let (x, _) = p.pos_at(350., (0., 0.), (100., 0.)).unwrap();
        let k: f32 = 0.5;
        assert!((x - k * (2. - k) * 100.).abs() < 1e-3, "k(2−k) ease-out");
        assert!(p.pos_at(700., (0., 0.), (100., 0.)).is_none(), "landed");
        assert!(p.done(700.));
    }

    #[test]
    fn terminal_state_retracts_once_then_channel_is_gone_forever() {
        let mut set = ChannelSet::default();
        set.fold(&[row("ch", 5, "working")], 0.);
        set.fold(&[row("ch", 5, "converged")], 1000.);
        let sim = set.iter_sorted()[0].clone();
        assert_eq!(sim.dead_at, Some(1000.));
        assert_eq!(sim.retract(1000.), 0.);
        assert!((sim.retract(1200.) - 0.25).abs() < 1e-4, "p² at halfway");
        assert_eq!(sim.retract(1400.), 1.);
        // A second terminal fold does NOT re-arm the clock.
        set.fold(&[row("ch", 5, "converged")], 1300.);
        assert_eq!(set.iter_sorted()[0].dead_at, Some(1000.));
        // Tick past the retract → destroyed and never re-rendered.
        set.tick(1500.);
        assert!(set.is_empty());
        set.fold(&[row("ch", 5, "converged")], 2000.);
        assert!(set.is_empty(), "gone channels never resurrect");
    }

    #[test]
    fn first_sight_terminal_channel_still_retracts() {
        // Discovered already converged (panel opened late): the edge must
        // retract instead of lingering forever.
        let mut set = ChannelSet::default();
        set.fold(&[row("ch", 5, "converged")], 1000.);
        assert_eq!(set.iter_sorted()[0].dead_at, Some(1000.));
        set.tick(1500.);
        assert!(set.is_empty(), "retracted and destroyed");
    }

    #[test]
    fn vanished_rows_destroy_their_runtime() {
        let mut set = ChannelSet::default();
        set.fold(&[row("ch", 1, "working")], 0.);
        set.fold(&[], 500.);
        assert!(set.is_empty());
    }

    #[test]
    fn trimmed_segment_meets_rims_not_centers() {
        let ((x0, y0), (x1, y1)) = trimmed_segment(0., 0., 100., 0., 15., 13.);
        assert_eq!((x0, y0), (15., 0.));
        assert_eq!((x1, y1), (87., 0.));
        // Degenerate short edge: trims clamp to half length, never cross.
        let ((x0, _), (x1, _)) = trimmed_segment(0., 0., 10., 0., 15., 13.);
        assert!(x0 <= x1, "trims never invert the segment");
    }

    #[test]
    fn retract_endpoints_meet_at_the_midpoint() {
        let (a, b, mid) = retract_endpoints((0., 0.), (100., 40.), 1.);
        assert_eq!(mid, (50., 20.));
        assert_eq!(a, mid);
        assert_eq!(b, mid);
        let (a, b, _) = retract_endpoints((0., 0.), (100., 40.), 0.);
        assert_eq!(a, (0., 0.));
        assert_eq!(b, (100., 40.));
    }
}
