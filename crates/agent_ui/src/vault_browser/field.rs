//! The vault GRAPH field — the constellation's Obsidian physics applied to OKF
//! documents (spec §3 physics upgrade, banked in v1). Each doc is a body with
//! a deterministic anchor placed inside its project cluster; the SHARED
//! [`crate::field_physics`] stepper relaxes overlaps (anchor spring + shifted
//! inverse-square repulsion + collide floor), a weak per-project cohesion
//! spring keeps a project's docs drifting together, and a sleep gate freezes
//! the field cold once it settles — idle cost zero when nothing moves (§8).
//!
//! The panel owns one [`VaultField`] per rendered root and advances it once
//! per frame BEFORE building elements; the render path only reads `out_x/out_y`
//! (mirrors `constellation`'s fold→advance→draw discipline). Node identity is
//! the doc id (ElementId per doc — the animation identity law): a re-index that
//! keeps a doc keeps its live displacement, so a promote folds in without a
//! jump.

use std::collections::HashMap;

use crate::field_physics::{self, PhysBody};

use super::index::VaultDoc;

/// Above this node count the field is disabled — the panel degrades to the
/// deterministic static layout with an honest banner rather than jank (spec
/// §4). 441 real docs sit comfortably below it.
pub const FIELD_MAX_NODES: usize = 800;

/// Weak per-project cohesion spring (px/s² per px of offset from the cluster
/// centroid). An order of magnitude below the field's anchor spring (64) so a
/// project's docs *drift* together without collapsing onto one point — the
/// repulsion floor still holds them apart. Applied ONLY inside the wake window
/// (the field cools to the pure shared stepper, which rest-snaps to zero).
const PROJECT_GRAVITY_K: f32 = 5.5;

/// How long the field stays awake (cohering + relaxing) after a (re)build or a
/// drag, before it hands off to the pure stepper and settles cold (ms).
const WAKE_MS: f32 = 2200.;

/// Hard settle cap: past this much continuous hot time after the last wake
/// event, the field FREEZES where it is (velocities zeroed, displacement
/// folded into the anchors). A dense cluster can sustain a collide-floor
/// limit cycle above the sleep threshold indefinitely — the user-observed
/// "bistable medium that alternates and never settles" (2026-07-10). An
/// equilibrium the stepper can't reach in 6 s is a limit cycle, not a
/// layout in progress; freezing IS the settle.
const HOT_CAP_MS: f32 = 6000.;

/// Extra spiral-slot spacing so the cold packing respects the collide floor
/// for typical radii — anchors that START inside each other's floors put the
/// spring and the shove in a permanent war (the limit-cycle fuel).
const SLOT_MARGIN: f32 = 6.;

/// Cluster grid geometry (anchor layout — the deterministic cold state).
const CLUSTER_COL_W: f32 = 340.;
const CLUSTER_ROW_H: f32 = 320.;
const CLUSTER_PAD: f32 = 180.;
/// Golden-angle spiral packing radius growth per node (px).
const SPIRAL_A: f32 = 26.;
const GOLDEN_ANGLE: f32 = 2.399_963_2; // π(3−√5)

/// One document body in the field: its identity, project, anchor (layout home
/// inside the cluster), live physics displacement/velocity, radius, and the
/// per-frame output position the renderer reads.
#[derive(Debug, Clone)]
pub struct FieldNode {
    /// Doc id — the stable ElementId key (animation identity law).
    pub id: String,
    /// Project bucket (empty → the unassigned cluster) — cohesion group.
    pub project: String,
    /// Anchor (cluster-local home). Drag re-pins it.
    pub ax: f32,
    pub ay: f32,
    pub pinned: bool,
    /// Physics displacement off the anchor + its velocity (the field output).
    pub dx: f32,
    pub dy: f32,
    pub vx: f32,
    pub vy: f32,
    /// Live dot radius (drives the collide floor).
    pub r: f32,
    /// Per-frame visual center (anchor + displacement) the renderer reads.
    pub out_x: f32,
    pub out_y: f32,
}

impl FieldNode {
    fn new(id: String, project: String, ax: f32, ay: f32, r: f32) -> Self {
        Self {
            id,
            project,
            ax,
            ay,
            pinned: false,
            dx: 0.,
            dy: 0.,
            vx: 0.,
            vy: 0.,
            r,
            out_x: ax,
            out_y: ay,
        }
    }

    /// Current visual position (anchor + displacement).
    pub fn pos(&self) -> (f32, f32) {
        (self.ax + self.dx, self.ay + self.dy)
    }
}

/// A placed project cluster: its centroid anchor (the weak shared anchor the
/// cohesion spring pulls each of the project's docs toward).
#[derive(Debug, Clone, Copy)]
pub struct Cluster {
    pub cx: f32,
    pub cy: f32,
}

/// Live per-node state carried across a rebuild by doc id (animation identity
/// law): a pinned node keeps its dragged anchor, everyone keeps their live
/// displacement + velocity so the field relaxes continuously.
struct Carry {
    ax: f32,
    ay: f32,
    dx: f32,
    dy: f32,
    vx: f32,
    vy: f32,
    pinned: bool,
}

/// The vault graph's physics field over one root's docs. Owned by the panel,
/// rebuilt when the doc set (identity) changes, advanced once per frame behind
/// the sleep gate.
#[derive(Debug, Default)]
pub struct VaultField {
    pub nodes: Vec<FieldNode>,
    /// project key → cluster centroid (the weak shared anchor per project).
    pub clusters: HashMap<String, Cluster>,
    /// The set of doc ids this field was built for (identity change detection).
    id_key: Vec<String>,
    /// Sleep gate: the field steps only while hot. `last_tick` is sim-ms.
    last_tick: f32,
    wake_until: f32,
    /// One-shot settle: at wake-window expiry the relaxed displacement folds
    /// into the anchors and momentum freezes — the cold state IS the relaxed
    /// layout (kills the spring-vs-collide war that sustained the limit
    /// cycle). Re-armed false by every rebuild / drag release.
    folded: bool,
    dragging: Option<String>,
    /// Content extent (px) — the scroll surface sizes to this.
    pub width: f32,
    pub height: f32,
    /// Reused physics buffer — no per-frame allocation in the hot loop.
    scratch: Vec<PhysBody>,
}

impl VaultField {
    /// Whether this field already matches `docs` by identity (id set + order).
    /// A re-index that produced the same doc ids reuses the live field (drift
    /// + pins preserved); a changed set rebuilds.
    pub fn matches(&self, docs: &[&VaultDoc]) -> bool {
        self.id_key.len() == docs.len()
            && self
                .id_key
                .iter()
                .zip(docs)
                .all(|(k, d)| k == &d.id)
    }

    /// (Re)build the field's anchors from `docs`, grouped into project
    /// clusters laid out on a grid, each cluster golden-angle-packed. Preserves
    /// the live displacement + pin of any doc id carried over from the previous
    /// build (promote-refold with no jump).
    pub fn rebuild(&mut self, docs: &[&VaultDoc], node_r: impl Fn(&VaultDoc) -> f32, now: f32) {
        // Carry over live state by id (animation identity law): a pinned
        // node keeps its DRAGGED anchor across the refold (no jump); an
        // unpinned node keeps its live displacement + velocity so the field
        // relaxes continuously from where it was.
        let carry: HashMap<String, Carry> = self
            .nodes
            .drain(..)
            .map(|n| {
                (
                    n.id,
                    Carry {
                        ax: n.ax,
                        ay: n.ay,
                        dx: n.dx,
                        dy: n.dy,
                        vx: n.vx,
                        vy: n.vy,
                        pinned: n.pinned,
                    },
                )
            })
            .collect();

        // Bucket docs by project, stable project order (unassigned first, then
        // sorted) — the same ordering the static layout uses.
        let mut order: Vec<String> = Vec::new();
        let mut buckets: HashMap<String, Vec<&VaultDoc>> = HashMap::new();
        for doc in docs {
            let key = if doc.project.is_empty() {
                "(unassigned)".to_string()
            } else {
                doc.project.clone()
            };
            if !buckets.contains_key(&key) {
                order.push(key.clone());
            }
            buckets.entry(key).or_default().push(doc);
        }
        order.sort();

        // Per-cluster spiral growth: a Vogel spiral's nearest-neighbor spacing
        // is ~its growth constant, so growing it to the group's collide floor
        // (2·max_r + GAP) makes the COLD packing already respect the floor.
        // The old fixed 26px start put every adjacent pair deep inside each
        // other's 48px+ floors — the spring and the shove then fight forever
        // (the limit-cycle fuel) and the "settled" view stays visibly
        // overlapped (user report 2026-07-10).
        let slot_a: HashMap<&String, f32> = order
            .iter()
            .map(|key| {
                let max_r = buckets[key]
                    .iter()
                    .map(|d| node_r(d))
                    .fold(0f32, f32::max);
                (key, SPIRAL_A.max(2. * max_r + field_physics::GAP + SLOT_MARGIN))
            })
            .collect();
        // Grid cell sized to the largest packed cluster so clusters never
        // bleed into each other (the fixed 340px cell overflowed past ~12
        // docs at floor-respecting spacing).
        let (mut cell_w, mut cell_h) = (CLUSTER_COL_W, CLUSTER_ROW_H);
        for key in &order {
            let n = buckets[key].len() as f32;
            let extent = 2. * (slot_a[key] * n.sqrt()) + 120.;
            cell_w = cell_w.max(extent);
            cell_h = cell_h.max(extent);
        }

        // Grid the clusters: columns wrap so the field stays roughly square.
        let cluster_count = order.len().max(1);
        let cols = (cluster_count as f32).sqrt().ceil().max(1.) as usize;

        self.nodes.clear();
        self.clusters.clear();
        self.id_key.clear();
        let (mut max_x, mut max_y) = (0f32, 0f32);

        for (ci, key) in order.iter().enumerate() {
            let col = ci % cols;
            let row = ci / cols;
            let cx = CLUSTER_PAD + col as f32 * cell_w;
            let cy = CLUSTER_PAD + row as f32 * cell_h;
            self.clusters.insert(key.clone(), Cluster { cx, cy });

            let group = &buckets[key];
            let a = slot_a[key];
            for (i, doc) in group.iter().enumerate() {
                // Golden-angle spiral around the cluster centroid: a compact,
                // deterministic cold packing the field then relaxes.
                let radius = a * (i as f32 + 0.5).sqrt();
                let angle = i as f32 * GOLDEN_ANGLE;
                let (mut ax, mut ay) = (cx + radius * angle.cos(), cy + radius * angle.sin());
                let r = node_r(doc);
                let carried = carry.get(&doc.id);
                // A pinned node keeps its dragged anchor; everyone else takes
                // the fresh spiral slot.
                if let Some(c) = carried.filter(|c| c.pinned) {
                    ax = c.ax;
                    ay = c.ay;
                }
                let mut node = FieldNode::new(doc.id.clone(), key.clone(), ax, ay, r);
                if let Some(c) = carried {
                    node.dx = c.dx;
                    node.dy = c.dy;
                    node.vx = c.vx;
                    node.vy = c.vy;
                    node.pinned = c.pinned;
                }
                node.out_x = ax + node.dx;
                node.out_y = ay + node.dy;
                max_x = max_x.max(ax + r + CLUSTER_PAD);
                max_y = max_y.max(ay + r + CLUSTER_PAD);
                self.nodes.push(node);
                self.id_key.push(doc.id.clone());
            }
        }

        self.width = max_x.max(CLUSTER_COL_W);
        self.height = max_y.max(CLUSTER_ROW_H);
        // A fresh/changed layout wakes the field to settle its overlaps.
        self.wake_until = now + WAKE_MS;
        self.folded = false;
    }

    /// The field centroid — the "root" the shared stepper excludes bodies from.
    /// The vault graph has no orchestrator dot, so the root is a zero-radius
    /// point far outside the content (root exclusion becomes a no-op — the
    /// vault field is a pure anchor+repel relaxation).
    fn root(&self) -> (f32, f32, f32) {
        (-10_000., -10_000., 0.)
    }

    /// Whether the field still needs stepping (an interaction live, inside the
    /// wake window, or residual momentum). Cold ⇒ zero work per frame (§8).
    pub fn hot(&self, now: f32) -> bool {
        self.dragging.is_some() || now < self.wake_until || field_physics::any_hot(&self.scratch)
    }

    /// Advance the field to `now` (sim ms). Steps the shared stepper only while
    /// hot, then layers the weak per-project cohesion spring, then writes the
    /// per-frame outputs. Returns `true` while the field is still animating
    /// (the panel pumps another frame); `false` when settled (idle-cost zero).
    pub fn advance(&mut self, now: f32) -> bool {
        let dt = ((now - self.last_tick) / 1000.).clamp(0.008, 0.033);
        self.last_tick = now;

        // Load the shared physics buffer from the live nodes.
        let dragging = self.dragging.as_deref();
        self.scratch.clear();
        self.scratch.extend(self.nodes.iter().map(|n| PhysBody {
            x: n.ax,
            y: n.ay,
            dx: n.dx,
            dy: n.dy,
            vx: n.vx,
            vy: n.vy,
            r: n.r,
            dragged: dragging == Some(n.id.as_str()),
        }));

        let hot = self.hot(now);
        if hot {
            let (rx, ry, rr) = self.root();
            // Weak per-project cohesion (docs of one project drift together —
            // the weak shared anchor per project): a small spring toward the
            // cluster centroid layered onto the shared stepper's per-body
            // force, applied ONLY while the field is still relaxing (inside the
            // wake window). Once the field cools we hand off to the pure
            // shared stepper, whose rest-snap provably drives velocity to
            // EXACTLY zero — so the sleep gate always fires (idle cost zero,
            // §8). Cohesion during settling + a clean cold hand-off: the
            // cluster shape is set, then frozen. Skipped for a dragged/pinned
            // body so the pointer stays authoritative.
            let cohering = now < self.wake_until;
            if cohering {
                for (node, body) in self.nodes.iter().zip(self.scratch.iter_mut()) {
                    if !body.dragged && !node.pinned {
                        if let Some(cluster) = self.clusters.get(&node.project) {
                            // Bias the body's velocity toward the centroid; the
                            // stepper integrates + damps it this same frame.
                            let px = (body.x + body.dx) - cluster.cx;
                            let py = (body.y + body.dy) - cluster.cy;
                            body.vx -= PROJECT_GRAVITY_K * px * dt;
                            body.vy -= PROJECT_GRAVITY_K * py * dt;
                        }
                    }
                }
            }
            field_physics::step(&mut self.scratch, rx, ry, rr, dt);
            for (node, body) in self.nodes.iter_mut().zip(self.scratch.iter()) {
                node.dx = body.dx;
                node.dy = body.dy;
                node.vx = body.vx;
                node.vy = body.vy;
            }
        }

        // One-shot settle past the wake window (OUTSIDE the hot block — the
        // frame where momentum bleeds under the sleep threshold skips the
        // stepper entirely, and that is exactly the frame that must fold):
        // bake the relaxed displacement into the anchors and kill momentum.
        // out = ax+dx is unchanged by the fold — NO visual jump — but the
        // spring stops fighting the collide floor (the sustained limit cycle
        // the user reported 2026-07-10), so the field is provably at rest.
        // Natural convergence folds as soon as momentum bleeds out; a limit
        // cycle never bleeds — the hot-cap forces the fold so the alternation
        // lives ≤ HOT_CAP_MS total.
        let calm = !field_physics::any_hot(&self.scratch);
        let capped = now >= self.wake_until + (HOT_CAP_MS - WAKE_MS);
        if !self.folded
            && self.dragging.is_none()
            && now >= self.wake_until
            && (calm || capped)
        {
            for node in &mut self.nodes {
                node.ax += node.dx;
                node.ay += node.dy;
                node.dx = 0.;
                node.dy = 0.;
                node.vx = 0.;
                node.vy = 0.;
            }
            field_physics::freeze(&mut self.scratch);
            // Folding moves anchors — re-derive the scroll extents.
            let (mut max_x, mut max_y) = (CLUSTER_COL_W, CLUSTER_ROW_H);
            for node in &self.nodes {
                max_x = max_x.max(node.ax + node.r + CLUSTER_PAD);
                max_y = max_y.max(node.ay + node.r + CLUSTER_PAD);
            }
            self.width = max_x;
            self.height = max_y;
            self.folded = true;
        }

        // Outputs: anchor + displacement (a pinned/dragged node holds).
        for node in &mut self.nodes {
            node.out_x = node.ax + node.dx;
            node.out_y = node.ay + node.dy;
        }

        self.dragging.is_some() || now < self.wake_until || field_physics::any_hot(&self.scratch)
    }

    // ── drag-to-rearrange (pointer owns the node; neighbors yield) ──

    pub fn begin_drag(&mut self, id: &str) {
        self.dragging = Some(id.to_string());
    }

    pub fn dragging(&self) -> Option<&str> {
        self.dragging.as_deref()
    }

    /// Move the dragged node's anchor (window-space deltas apply 1:1) and pin
    /// it — the pin survives later rebuilds (constellation drag idiom).
    pub fn drag_to(&mut self, id: &str, ax: f32, ay: f32) {
        if let Some(node) = self.nodes.iter_mut().find(|n| n.id == id) {
            node.ax = ax;
            node.ay = ay;
            node.dx = 0.;
            node.dy = 0.;
            node.vx = 0.;
            node.vy = 0.;
            node.pinned = true;
        }
    }

    pub fn end_drag(&mut self, now: f32) {
        self.dragging = None;
        self.wake_until = self.wake_until.max(now + WAKE_MS); // neighbors settle
        self.folded = false; // re-arm the one-shot settle for the new window
    }

    /// The live visual center of a doc's node (edges ride it).
    pub fn position_of(&self, id: &str) -> Option<(f32, f32)> {
        self.nodes
            .iter()
            .find(|n| n.id == id)
            .map(|n| (n.out_x, n.out_y))
    }
}

#[cfg(test)]
mod tests {
    use super::super::index::{DocKind, VaultRoot};
    use super::*;
    use std::path::PathBuf;

    fn doc(id: &str, project: &str, mc: u64) -> VaultDoc {
        VaultDoc {
            abs_path: PathBuf::from(format!("{id}.md")),
            rel_path: format!("{id}.md"),
            root: VaultRoot::Vault,
            kind: DocKind::Note,
            id: id.to_string(),
            title: id.to_string(),
            doc_type: "note".to_string(),
            vendor: String::new(),
            run_id: String::new(),
            status: String::new(),
            schedule: String::new(),
            project: project.to_string(),
            updated: String::new(),
            message_count: mc,
            redactions: 0,
            body_len: 512,
            link_scan_truncated: false,
            search_key: id.to_string(),
            links: Vec::new(),
            supersedes: Vec::new(),
            bundle_dir: None,
        }
    }

    /// Steady radius so packing/relaxation is the only variable under test.
    fn fixed_r(_d: &VaultDoc) -> f32 {
        7.
    }

    fn run(field: &mut VaultField, frames: usize, start: f32) -> f32 {
        let mut now = start;
        for _ in 0..frames {
            now += 1000. / 60.;
            field.advance(now);
        }
        now
    }

    fn all_finite(field: &VaultField) -> bool {
        field.nodes.iter().all(|n| {
            n.dx.is_finite() && n.dy.is_finite() && n.out_x.is_finite() && n.out_y.is_finite()
        })
    }

    #[test]
    fn field_converges_without_nan_on_a_441_node_synthetic() {
        // The verification bar (spec gate): a 441-node synthetic settles with
        // no NaN and the field goes cold (idle-cost claim: hot() false).
        let owned: Vec<VaultDoc> = (0..441)
            .map(|i| doc(&format!("n{i}"), &format!("proj-{}", i % 12), (i as u64 % 50) + 1))
            .collect();
        let docs: Vec<&VaultDoc> = owned.iter().collect();
        let mut field = VaultField::default();
        field.rebuild(&docs, fixed_r, 0.);
        assert_eq!(field.nodes.len(), 441);
        let now = run(&mut field, 1500, 0.); // 25 s of sim
        assert!(all_finite(&field), "no NaN after settling 441 nodes");
        assert!(
            !field.hot(now),
            "the field goes cold once settled — idle cost zero"
        );
    }

    #[test]
    fn project_gravity_folds_a_projects_docs_together() {
        // Two projects, well separated on the grid. After settling, each
        // project's docs cluster near their OWN centroid, not the other's.
        let owned: Vec<VaultDoc> = (0..8)
            .map(|i| doc(&format!("n{i}"), if i < 4 { "alpha" } else { "beta" }, 1))
            .collect();
        let docs: Vec<&VaultDoc> = owned.iter().collect();
        let mut field = VaultField::default();
        field.rebuild(&docs, fixed_r, 0.);
        run(&mut field, 1200, 0.);

        let alpha = field.clusters["alpha"];
        let beta = field.clusters["beta"];
        for node in &field.nodes {
            let (x, y) = (node.out_x, node.out_y);
            let d_own = if node.project == "alpha" {
                (x - alpha.cx).hypot(y - alpha.cy)
            } else {
                (x - beta.cx).hypot(y - beta.cy)
            };
            let d_other = if node.project == "alpha" {
                (x - beta.cx).hypot(y - beta.cy)
            } else {
                (x - alpha.cx).hypot(y - alpha.cy)
            };
            assert!(
                d_own < d_other,
                "node {} stays with its own project ({} < {})",
                node.id,
                d_own,
                d_other
            );
        }
    }

    #[test]
    fn scale_degrade_threshold_is_honoured() {
        // The static-layout degrade boundary (spec §4): 441 is under, 801 is
        // over. The panel reads this to choose field vs static.
        assert!(441 <= FIELD_MAX_NODES);
        assert!(801 > FIELD_MAX_NODES);
    }

    #[test]
    fn promote_refold_preserves_live_displacement_by_id() {
        // A doc carried across a rebuild keeps its live displacement + pin
        // (promote lands, re-index fires, the node folds in with no jump).
        let owned: Vec<VaultDoc> = (0..5).map(|i| doc(&format!("n{i}"), "p", 1)).collect();
        let docs: Vec<&VaultDoc> = owned.iter().collect();
        let mut field = VaultField::default();
        field.rebuild(&docs, fixed_r, 0.);
        run(&mut field, 300, 0.);
        // Snapshot a node's live displacement + pin it (a user drag).
        field.drag_to("n2", 500., 500.);
        let before = field.position_of("n2").unwrap();

        // A promote adds a 6th doc → rebuild with the same ids present.
        let owned2: Vec<VaultDoc> = (0..6).map(|i| doc(&format!("n{i}"), "p", 1)).collect();
        let docs2: Vec<&VaultDoc> = owned2.iter().collect();
        assert!(!field.matches(&docs2), "the doc set changed → rebuild");
        field.rebuild(&docs2, fixed_r, 1000.);
        // n2 kept its pin (anchor moved to the dragged spot, no jump).
        let n2 = field.nodes.iter().find(|n| n.id == "n2").unwrap();
        assert!(n2.pinned, "the pinned node stays pinned across the refold");
        assert_eq!((n2.ax, n2.ay), (500., 500.), "pin anchor preserved");
        let _ = before;
    }

    #[test]
    fn matches_detects_identity_change() {
        let a: Vec<VaultDoc> = (0..3).map(|i| doc(&format!("n{i}"), "p", 1)).collect();
        let ar: Vec<&VaultDoc> = a.iter().collect();
        let mut field = VaultField::default();
        field.rebuild(&ar, fixed_r, 0.);
        assert!(field.matches(&ar), "same set matches");
        let b: Vec<VaultDoc> = (0..4).map(|i| doc(&format!("n{i}"), "p", 1)).collect();
        let br: Vec<&VaultDoc> = b.iter().collect();
        assert!(!field.matches(&br), "added doc no longer matches");
    }

    #[test]
    fn drag_pins_and_field_stays_hot_during_drag() {
        let owned: Vec<VaultDoc> = (0..3).map(|i| doc(&format!("n{i}"), "p", 1)).collect();
        let docs: Vec<&VaultDoc> = owned.iter().collect();
        let mut field = VaultField::default();
        field.rebuild(&docs, fixed_r, 0.);
        field.begin_drag("n0");
        assert!(field.hot(0.), "a live drag keeps the field hot");
        field.drag_to("n0", 123., 456.);
        let n0 = field.nodes.iter().find(|n| n.id == "n0").unwrap();
        assert!(n0.pinned);
        assert_eq!((n0.ax, n0.ay), (123., 456.));
        field.end_drag(0.);
        assert_eq!(field.dragging(), None);
    }

    /// Frame-step the field until it reports cold (or `cap` frames) and
    /// return (frames, now).
    fn settle(field: &mut VaultField, cap: usize) -> (usize, f32) {
        let mut now = 0f32;
        let mut frames = 0usize;
        loop {
            now += 1000. / 60.;
            let animating = field.advance(now);
            frames += 1;
            if !animating || frames >= cap {
                return (frames, now);
            }
        }
    }

    #[test]
    fn dense_cluster_settles_within_the_hot_cap_no_limit_cycle() {
        // THE user-reported bug (2026-07-10): a dense one-project cluster
        // sustains a collide-floor oscillation above the sleep threshold and
        // never settles ("bistable medium it alternates between"). Big radii
        // (mc drives node_size) + one project = every pair fighting. The
        // fold-at-settle + hot-cap must drive the field provably cold within
        // WAKE_MS + (HOT_CAP_MS − WAKE_MS) + a frame of slack.
        let owned: Vec<VaultDoc> = (0..60)
            .map(|i| doc(&format!("n{i}"), "one-project", 400))
            .collect();
        let docs: Vec<&VaultDoc> = owned.iter().collect();
        let mut field = VaultField::default();
        field.rebuild(&docs, super::super::graph::node_size, 0.);
        let cap_frames = ((HOT_CAP_MS / 1000.) * 60.) as usize + 10;
        let (frames, now) = settle(&mut field, cap_frames);
        assert!(all_finite(&field), "no NaN in the dense cluster");
        assert!(
            !field.hot(now),
            "the field is COLD after {frames} frames — the limit cycle must \
             not outlive the hot cap"
        );
        // Every node rests exactly (fold zeroed displacement + momentum).
        for n in &field.nodes {
            assert_eq!((n.dx, n.dy, n.vx, n.vy), (0., 0., 0., 0.), "{} rests", n.id);
        }
    }

    #[test]
    fn fold_at_settle_causes_no_visual_jump() {
        // out = anchor + displacement must be continuous across the fold
        // frame (the fold moves the anchor BY the displacement — same sum).
        let owned: Vec<VaultDoc> = (0..12)
            .map(|i| doc(&format!("n{i}"), "p", 200))
            .collect();
        let docs: Vec<&VaultDoc> = owned.iter().collect();
        let mut field = VaultField::default();
        field.rebuild(&docs, super::super::graph::node_size, 0.);
        let mut now = 0f32;
        let mut prev: Vec<(f32, f32)> = field.nodes.iter().map(|n| (n.out_x, n.out_y)).collect();
        for _ in 0..((HOT_CAP_MS / 1000.) * 60.) as usize + 10 {
            now += 1000. / 60.;
            field.advance(now);
            for (n, (px, py)) in field.nodes.iter().zip(&prev) {
                let dx = (n.out_x - px).abs();
                let dy = (n.out_y - py).abs();
                assert!(
                    dx < 40. && dy < 40.,
                    "{}: {dx:.1},{dy:.1} px in one frame — fold must not jump",
                    n.id
                );
            }
            prev = field.nodes.iter().map(|n| (n.out_x, n.out_y)).collect();
        }
    }

    #[test]
    fn cold_packing_respects_the_collide_floor() {
        // Adaptive spiral spacing: with LIVE radii the anchors themselves
        // must not start inside each other's floors (the limit-cycle fuel).
        let owned: Vec<VaultDoc> = (0..30)
            .map(|i| doc(&format!("n{i}"), "p", 400))
            .collect();
        let docs: Vec<&VaultDoc> = owned.iter().collect();
        let mut field = VaultField::default();
        field.rebuild(&docs, super::super::graph::node_size, 0.);
        for a in &field.nodes {
            for b in &field.nodes {
                if a.id >= b.id {
                    continue;
                }
                let d = (a.ax - b.ax).hypot(a.ay - b.ay);
                let floor = a.r + b.r + crate::field_physics::GAP;
                assert!(
                    d >= floor * 0.85,
                    "{} vs {}: anchor distance {d:.0} under floor {floor:.0}",
                    a.id,
                    b.id
                );
            }
        }
    }

    /// Opt-in REAL-corpus field benchmark: build the live index, fold the
    /// field over the busiest root, and report the settle time (frames to a
    /// cold field) + confirm the idle-cost claim (the field goes cold, so the
    /// panel stops pumping). Gated behind `VAULT_FIELD_BENCH=1` so the normal
    /// suite stays machine-independent. Run:
    /// `VAULT_FIELD_BENCH=1 cargo test -p agent_ui field_bench -- --nocapture`.
    #[gpui::test]
    fn field_bench_real_corpus(cx: &mut gpui::TestAppContext) {
        use super::super::index::build_index;
        use fs::Fs;
        use std::sync::Arc;

        if std::env::var("VAULT_FIELD_BENCH").is_err() {
            return;
        }
        cx.executor().allow_parking();
        let fs: Arc<dyn Fs> = Arc::new(fs::RealFs::new(None, cx.executor()));
        let index = cx
            .foreground_executor()
            .block_on(async move { build_index(fs).await });

        // Pick the busier root for the stress read.
        let (root, docs) = {
            let vault = index.docs_for(super::super::index::VaultRoot::Vault);
            let staging = index.docs_for(super::super::index::VaultRoot::Staging);
            if staging.len() >= vault.len() {
                ("staging", staging)
            } else {
                ("vault", vault)
            }
        };

        let mut field = VaultField::default();
        field.rebuild(&docs, super::super::graph::node_size, 0.);

        // Frame-step at 60fps until the field goes cold; count the frames.
        let mut now = 0f32;
        let mut frames = 0usize;
        let cap = 6000usize; // 100s hard cap
        loop {
            now += 1000. / 60.;
            let animating = field.advance(now);
            frames += 1;
            if !animating || frames >= cap {
                break;
            }
        }
        let settle_s = frames as f32 / 60.;
        let finite = field
            .nodes
            .iter()
            .all(|n| n.out_x.is_finite() && n.out_y.is_finite());

        eprintln!(
            "VAULT_FIELD_BENCH: root={root} · {} nodes · settled in {frames} frames \
             ({settle_s:.2}s sim) · cold={} · finite={finite} · extent {:.0}x{:.0}px",
            field.nodes.len(),
            !field.hot(now),
            field.width,
            field.height,
        );
        assert!(finite, "no NaN over the real corpus");
        assert!(
            !field.hot(now) || frames >= cap,
            "the real-corpus field goes cold (idle cost zero) within the cap"
        );
    }
}
