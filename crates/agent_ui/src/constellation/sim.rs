//! The constellation simulation state — the event→state folding layer
//! (graph.js module-level maps, made a struct). Board/conv/channel frames
//! fold into per-node and per-conv sim records; [`advance`](Sim::advance)
//! (advance.rs) steps tweens + physics + drift once per frame.
//!
//! Everything is clocked in **sim milliseconds** (f32 since [`Sim`]
//! creation) so every transition is unit-testable without sleeping; the
//! panel feeds the real clock via [`Sim::clock_ms`].

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use gpui::SharedString;

use crate::bridge::{ChannelRow, ConversationRow, OverlapRow, RunRow};

use super::edges::ChannelSet;
use super::layout::{FanLayout, fan_positions, link_weights, weighted_order};
use super::mass::{RootMass, dot_px};

/// Solid-green linger before the reel-in (ms).
pub const LINGER_MS: f32 = 4000.;
/// The gobble reel-in duration (ms, cubic ease-in).
pub const ABSORB_MS: f32 = 480.;
/// Anchor auto-arrange tween (ms, ease-out-back).
pub const ARRANGE_MS: f32 = 500.;
/// Dot mass swell tween (ms, ease-out-back).
pub const MASS_MS: f32 = 450.;
/// Root mass swell/exhale tween (ms, ease-out-back).
pub const ROOT_MASS_MS: f32 = 600.;
/// Spawn flight duration (ms, the `--spatial` overshoot).
pub const SPAWN_MS: f32 = 500.;
/// Spawn stagger: `0.15 + 0.07·i` seconds.
pub const SPAWN_BASE_DELAY_MS: f32 = 150.;
pub const SPAWN_STEP_DELAY_MS: f32 = 70.;
/// Post-membership-change physics wake window (ms).
pub const WAKE_MS: f32 = 1500.;
/// Mass-retarget physics wake window (ms) — radii moved, resolve overlaps.
pub const MASS_WAKE_MS: f32 = 900.;
/// Anchor re-slot threshold (px): closer than this stays put.
const RESLOT_EPS: f32 = 2.;
/// Layout width bucket (px): the fan re-aims only when the measured width
/// crosses a bucket, so a live dock resize doesn't re-tween every frame.
const WIDTH_BUCKET: f32 = 24.;

/// `easeOutBack` (graph.js, exact).
pub fn ease_out_back(k: f32) -> f32 {
    const C1: f32 = 1.70158;
    const C3: f32 = C1 + 1.;
    1. + C3 * (k - 1.).powi(3) + C1 * (k - 1.).powi(2)
}

/// A scalar tween on the ease-out-back curve.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tween {
    pub from: f32,
    pub to: f32,
    pub t0: f32,
    pub dur: f32,
}

impl Tween {
    /// Value at `now`; `.1` is done.
    pub fn eval(&self, now: f32) -> (f32, bool) {
        let k = ((now - self.t0) / self.dur).clamp(0., 1.);
        if k >= 1. {
            (self.to, true)
        } else {
            (self.from + (self.to - self.from) * ease_out_back(k), false)
        }
    }
}

/// An anchor re-slot tween (ease-out-back on both axes).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ArrangeTween {
    pub fx: f32,
    pub fy: f32,
    pub tx: f32,
    pub ty: f32,
    pub t0: f32,
}

/// One subagent node's sim record.
#[derive(Debug, Clone)]
pub struct NodeSim {
    pub run_id: String,
    pub agent: String,
    pub archetype: Option<String>,
    pub task: Option<String>,
    pub status: String,
    pub elapsed_s: f64,
    /// The receipt the root gains when this run is gobbled.
    pub ingest: f64,
    /// Anchor (layout home). Drag re-pins it; arrange tweens move it.
    pub ax: f32,
    pub ay: f32,
    pub pinned: bool,
    /// Physics displacement + velocity (the local perturbation field).
    pub dx: f32,
    pub dy: f32,
    pub vx: f32,
    pub vy: f32,
    pub arranging: Option<ArrangeTween>,
    /// The gobble reel-in: (from_x, from_y, t0).
    pub absorbing: Option<(f32, f32, f32)>,
    /// Token-mass dot box (px): rendered, target, in-flight swell.
    pub mass_cur: f32,
    pub mass_target: f32,
    pub mass_anim: Option<Tween>,
    /// Spawn flight (fresh nodes only): start time + stagger delay (ms).
    pub spawn: Option<(f32, f32)>,
    /// Render index — seeds the Lissajous drift.
    pub drift_ix: usize,
    // ── per-frame outputs (advance.rs) ──
    /// Visual dot-center position.
    pub out_x: f32,
    pub out_y: f32,
    /// Absorb shrink (1 = full size).
    pub out_scale: f32,
    /// Star-edge opacity (absorb fade).
    pub out_edge_alpha: f32,
    /// Spawn progress 0..≈1 (SPATIAL-eased, may overshoot 1; 1 = settled).
    pub out_spawn: f32,
}

impl NodeSim {
    fn new(run: &RunRow, slot: (f32, f32), mass: f32, ix: usize) -> Self {
        Self {
            run_id: run.run_id.clone(),
            agent: run.agent.clone(),
            archetype: run.archetype.clone(),
            task: run.task.clone(),
            status: run.status.clone(),
            elapsed_s: run.elapsed_s,
            ingest: run.tokens.map_or(0., |t| t.ingest),
            ax: slot.0,
            ay: slot.1,
            pinned: false,
            dx: 0.,
            dy: 0.,
            vx: 0.,
            vy: 0.,
            arranging: None,
            absorbing: None,
            mass_cur: mass,
            mass_target: mass,
            mass_anim: None,
            spawn: None,
            drift_ix: ix,
            out_x: slot.0,
            out_y: slot.1,
            out_scale: 1.,
            out_edge_alpha: 1.,
            out_spawn: 1.,
        }
    }
}

/// One conversation's constellation: root + fan + nodes.
#[derive(Debug, Clone)]
pub struct ConvSim {
    pub conv_id: String,
    pub title: SharedString,
    pub layout: FanLayout,
    layout_key: (usize, i32),
    pub nodes: Vec<NodeSim>,
    /// Ctx-driven root mass (`None` on an older bridge → fed fallback).
    pub root_mass: Option<RootMass>,
    /// Legacy fallback: gobbled-run count grows the root without ctx data.
    pub fed: u64,
    /// Rendered root box (px) + in-flight swell/exhale tween.
    pub root_cur: f32,
    pub root_anim: Option<Tween>,
    /// The gobble gulp keyframe start (jelly spring on the root dot).
    pub gulp_at: Option<f32>,
    /// Outward feed-pulse ring starts (gobble) / inward exhale ring starts
    /// (compaction) — pruned as they finish.
    pub pulses: Vec<f32>,
    pub exhales: Vec<f32>,
}

impl ConvSim {
    /// The root's current target box (ctx mass or the fed fallback).
    pub fn root_target(&self) -> f32 {
        match &self.root_mass {
            Some(mass) => mass.target_px(),
            None => 24. + (self.fed as f32 * 1.5).min(9.),
        }
    }

    /// Retarget the root box tween toward the current target (no-op within
    /// half a pixel unless `force`).
    pub(super) fn retarget_root(&mut self, now: f32, force: bool) {
        let to = self.root_target();
        if (to - self.root_cur).abs() < 0.5 && !force {
            return;
        }
        self.root_anim = Some(Tween {
            from: self.root_cur,
            to,
            t0: now,
            dur: ROOT_MASS_MS,
        });
    }
}

/// The whole constellation sim (all conversations + channels + the gobble
/// ledger). Owned by the panel; folded from bridge frames; advanced once
/// per animation frame.
pub struct Sim {
    start: Instant,
    pub(super) last_tick: f32,
    pub convs: Vec<ConvSim>,
    pub channels: ChannelSet,
    /// Latest overlap venn rows (auto-arrange weights, read at arrange time).
    pub overlap: Vec<OverlapRow>,
    /// Run ids ever seen — spawn choreography fires exactly once per run.
    seen: HashSet<String>,
    /// run id → last status (watched-transition detection).
    statuses: HashMap<String, String>,
    /// run id → sim-ms the completed transition was WATCHED.
    pub(super) completed_at: HashMap<String, f32>,
    /// Gobbled runs — excluded from the constellation.
    absorbed: HashSet<String>,
    /// Physics sleep gate: the field runs only while hot.
    pub(super) wake_until: f32,
    pub(super) dragging: Option<String>,
    /// Scratch physics buffer (reused every frame — no per-frame allocation
    /// in the hot loop).
    pub(super) scratch: Vec<super::physics::PhysBody>,
}

impl Default for Sim {
    fn default() -> Self {
        Self {
            start: Instant::now(),
            last_tick: 0.,
            convs: Vec::new(),
            channels: ChannelSet::default(),
            overlap: Vec::new(),
            seen: HashSet::new(),
            statuses: HashMap::new(),
            completed_at: HashMap::new(),
            absorbed: HashSet::new(),
            wake_until: 0.,
            dragging: None,
            scratch: Vec::new(),
        }
    }
}

impl Sim {
    /// The sim clock (ms since creation) — the panel's `now` for
    /// [`fold`](Self::fold) / [`advance`](Self::advance).
    pub fn clock_ms(&self) -> f32 {
        self.start.elapsed().as_secs_f32() * 1000.
    }

    /// Fold a board + conversations frame at sim time `now` (ms), laying the
    /// fan out for a container `width` px. Idempotent for unchanged data.
    pub fn fold(
        &mut self,
        board: &[RunRow],
        convs: &[ConversationRow],
        width: f32,
        now: f32,
    ) {
        // Gobble detection: only transitions we WATCHED get absorbed —
        // pre-existing completed runs at first sight stay static history.
        for run in board {
            let watched = self
                .statuses
                .get(&run.run_id)
                .is_some_and(|prev| prev != "completed");
            if run.status == "completed" && watched && !self.absorbed.contains(&run.run_id) {
                self.completed_at.insert(run.run_id.clone(), now);
            }
            self.statuses
                .insert(run.run_id.clone(), run.status.clone());
        }
        // Prune ledgers to live runs.
        let live: HashSet<&str> = board.iter().map(|run| run.run_id.as_str()).collect();
        self.statuses.retain(|id, _| live.contains(id.as_str()));
        self.completed_at.retain(|id, _| live.contains(id.as_str()));
        self.absorbed.retain(|id| live.contains(id.as_str()));
        self.seen.retain(|id| live.contains(id.as_str()));

        // Group visible (non-absorbed) runs by conversation, board order.
        let mut conv_order: Vec<&str> = Vec::new();
        let mut by_conv: HashMap<&str, Vec<&RunRow>> = HashMap::new();
        for run in board {
            if self.absorbed.contains(&run.run_id) {
                continue;
            }
            if !by_conv.contains_key(run.conv.as_str()) {
                conv_order.push(run.conv.as_str());
            }
            by_conv.entry(run.conv.as_str()).or_default().push(run);
        }

        // Reconcile ConvSims, preserving per-conv and per-node state.
        let mut old: HashMap<String, ConvSim> = self
            .convs
            .drain(..)
            .map(|conv| (conv.conv_id.clone(), conv))
            .collect();
        let mut membership_changed = false;
        for conv_id in conv_order {
            let runs = by_conv.remove(conv_id).unwrap_or_default();
            let title: SharedString = convs
                .iter()
                .find(|c| c.id == conv_id)
                .map(|c| c.title.clone())
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| "conversation".to_string())
                .into();
            let layout_key = (runs.len(), (width / WIDTH_BUCKET) as i32);
            let mut conv = old.remove(conv_id).unwrap_or_else(|| ConvSim {
                conv_id: conv_id.to_string(),
                title: title.clone(),
                layout: fan_positions(runs.len(), width),
                layout_key,
                nodes: Vec::new(),
                root_mass: None,
                fed: 0,
                root_cur: 24.,
                root_anim: None,
                gulp_at: None,
                pulses: Vec::new(),
                exhales: Vec::new(),
            });
            conv.title = title;
            if conv.layout_key != layout_key {
                conv.layout = fan_positions(runs.len(), width);
                conv.layout_key = layout_key;
            }
            let mut old_nodes: HashMap<String, NodeSim> = conv
                .nodes
                .drain(..)
                .map(|node| (node.run_id.clone(), node))
                .collect();
            for (i, run) in runs.iter().enumerate() {
                let slot = conv.layout.positions[i];
                let mass_target = dot_px(run.tokens.map_or(0., |t| t.weighted));
                let node = match old_nodes.remove(&run.run_id) {
                    Some(mut node) => {
                        node.status = run.status.clone();
                        node.elapsed_s = run.elapsed_s;
                        node.task = run.task.clone();
                        node.archetype = run.archetype.clone();
                        if let Some(tokens) = run.tokens {
                            if tokens.ingest > 0. {
                                node.ingest = tokens.ingest;
                            }
                        }
                        node.drift_ix = i;
                        // Spawn-with-autosort: membership changes re-slot
                        // every UNPINNED node via the arrange tween — pins
                        // are sacred; nothing ever hard-jumps.
                        let moved = (node.ax - slot.0).abs() + (node.ay - slot.1).abs();
                        let retween = !node.pinned
                            && moved > RESLOT_EPS
                            && node
                                .arranging
                                .is_none_or(|t| (t.tx, t.ty) != slot);
                        if retween {
                            node.arranging = Some(ArrangeTween {
                                fx: node.ax,
                                fy: node.ay,
                                tx: slot.0,
                                ty: slot.1,
                                t0: now,
                            });
                        }
                        // Mass retarget: swell toward f(weighted), spring
                        // tween, never a snap; sub-half-pixel jitter is quiet.
                        if (mass_target - node.mass_target).abs() >= 0.5 {
                            node.mass_target = mass_target;
                            node.mass_anim = Some(Tween {
                                from: node.mass_cur,
                                to: mass_target,
                                t0: now,
                                dur: MASS_MS,
                            });
                            self.wake_until = self.wake_until.max(now + MASS_WAKE_MS);
                        }
                        node
                    }
                    None => {
                        membership_changed = true;
                        let fresh = self.seen.insert(run.run_id.clone());
                        let mut node = NodeSim::new(run, slot, mass_target, i);
                        if fresh {
                            // Obsidian spawn: born AT the root, springs out
                            // along its own radial vector, staggered.
                            node.spawn = Some((
                                now,
                                SPAWN_BASE_DELAY_MS + SPAWN_STEP_DELAY_MS * i as f32,
                            ));
                            node.out_spawn = 0.;
                        }
                        node
                    }
                };
                conv.nodes.push(node);
            }
            if !old_nodes.is_empty() {
                membership_changed = true; // runs left this conversation
            }
            // Root ctx mass (T4) — first sight adopts silently; a watched
            // compaction increment exhales.
            if let Some(row) = convs.iter().find(|c| c.id == conv_id)
                && let Some(ctx) = row.ctx_tokens
            {
                match conv.root_mass {
                    None => {
                        conv.root_mass = Some(RootMass::first_sight(ctx, row.compactions));
                        conv.root_cur = conv.root_target();
                    }
                    Some(mut mass) => {
                        let exhale = mass.fold(ctx, row.compactions);
                        conv.root_mass = Some(mass);
                        if exhale {
                            conv.exhales.push(now);
                        }
                        conv.retarget_root(now, exhale);
                        if conv.root_anim.is_some() {
                            self.wake_until = self.wake_until.max(now + MASS_WAKE_MS);
                        }
                    }
                }
            }
            self.convs.push(conv);
        }
        if !old.is_empty() {
            membership_changed = true; // whole conversations left the board
        }
        if membership_changed {
            // New bodies → let the field settle.
            self.wake_until = self.wake_until.max(now + WAKE_MS);
        }
    }

    /// Fold the latest boost-channel rows (SSE `channels` event or the
    /// supplemental poll — both delta on k, so double-feeding never
    /// double-fires particles).
    pub fn fold_channels(&mut self, rows: &[ChannelRow], now: f32) {
        self.channels.fold(rows, now);
    }

    /// Latest overlap venn rows (the supplemental poll feeds this; arrange
    /// always reads the freshest weights).
    pub fn set_overlap(&mut self, rows: Vec<OverlapRow>) {
        self.overlap = rows;
    }

    // ── drag-to-rearrange (pointer owns the node; neighbors yield) ──

    pub fn begin_drag(&mut self, run_id: &str) {
        self.dragging = Some(run_id.to_string());
    }

    pub fn dragging(&self) -> Option<&str> {
        self.dragging.as_deref()
    }

    /// Move the dragged node's anchor (window-space deltas apply 1:1) and
    /// pin it — the pin survives every later fold.
    pub fn drag_to(&mut self, run_id: &str, ax: f32, ay: f32) {
        for conv in &mut self.convs {
            if let Some(node) = conv.nodes.iter_mut().find(|n| n.run_id == run_id) {
                node.ax = ax;
                node.ay = ay;
                node.pinned = true;
                node.arranging = None;
                return;
            }
        }
    }

    pub fn end_drag(&mut self, now: f32) {
        self.dragging = None;
        self.wake_until = self.wake_until.max(now + WAKE_MS); // neighbors settle
    }

    /// Auto-arrange: one-shot settled layout, tweened — NEVER a live solver.
    /// Channel/overlap weights order the fan so heavily-linked pairs land
    /// angularly adjacent; no data falls back to the tidy default fan.
    /// Arranging clears drag pins; dragging re-pins.
    pub fn auto_arrange(&mut self, now: f32) {
        let channel_rows: Vec<ChannelRow> = self.channels.rows().cloned().collect();
        let weights = link_weights(&self.overlap, &channel_rows);
        for conv in &mut self.convs {
            let ids: Vec<String> = conv.nodes.iter().map(|n| n.run_id.clone()).collect();
            let order = weighted_order(&ids, &weights).unwrap_or(ids);
            for (slot, run_id) in order.iter().enumerate() {
                let Some(position) = conv.layout.positions.get(slot).copied() else {
                    continue;
                };
                if let Some(node) = conv.nodes.iter_mut().find(|n| n.run_id == *run_id) {
                    node.arranging = Some(ArrangeTween {
                        fx: node.ax,
                        fy: node.ay,
                        tx: position.0,
                        ty: position.1,
                        t0: now,
                    });
                    node.pinned = false;
                }
            }
        }
        self.wake_until = self.wake_until.max(now + WAKE_MS);
    }

    /// Whether `run_id` has been gobbled (tests + drawer guards).
    pub fn is_absorbed(&self, run_id: &str) -> bool {
        self.absorbed.contains(run_id)
    }

    pub(super) fn mark_absorbed(&mut self, run_id: String) {
        self.completed_at.remove(&run_id);
        self.absorbed.insert(run_id);
    }
}

#[cfg(test)]
#[path = "sim_tests.rs"]
mod tests;
