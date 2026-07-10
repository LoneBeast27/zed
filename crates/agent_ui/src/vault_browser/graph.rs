//! The GRAPH view — OKF documents as nodes, edges from `[[wikilinks]]` +
//! `[md](links.md)` in bodies plus frontmatter relations (`supersedes:`,
//! `project:` grouping). Node color by type, size by message_count/body.
//!
//! Borrows the constellation/task-board graph IDIOM (absolutely-positioned
//! nodes over a `canvas()` edge layer keyed by stable ids) but SIMPLIFIES to a
//! deterministic static layout (spec §3 explicitly permits this v1 fallback):
//! a project-grouped column layout, no physics stepper, so the panel schedules
//! zero idle frames (§8 idle-cost discipline — an unmoving graph pumps nothing).

use std::collections::HashMap;

use gpui::{
    App, ElementId, Hsla, PathBuilder, Pixels, Point, SharedString, WeakEntity, canvas, point, px,
};
use ui::prelude::*;

use crate::agent_accents::{
    STATUS_DONE, STATUS_RUNNING, accent_for_agent, color_for_status, rgba_hex,
};

use super::index::{DocKind, VaultDoc};
use super::panel::VaultBrowserPanel;
use super::style::HAIRLINE_HI;

/// A resolved edge between two doc indices (into the `docs` slice passed to
/// [`resolve_edges`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Edge {
    pub from: usize,
    pub to: usize,
    pub kind: EdgeKind,
}

/// Why two docs are connected — drives edge color/weight/dash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeKind {
    /// A `[[wiki]]` or `[md](link.md)` body reference. Solid hairline.
    Link,
    /// A `supersedes:` frontmatter correction edge. Dashed + directional.
    Supersedes,
    /// An `index.md` → typed-doc structural edge. Dimmed (the vault's spine,
    /// not a semantic link) — drawn faint so real links read over it.
    Structural,
    /// A doc → its project HUB doc (the doc whose id/title names the
    /// `project:` it belongs to). Faintest of all — the connective tissue
    /// that makes a cluster read as one project instead of loose dots
    /// (user report 2026-07-10: "not interlinking in terms of the overall
    /// project connections").
    Project,
}

/// Resolve outbound links + frontmatter relations into edges between the given
/// docs. Targets are matched against, in priority order: exact `id`, exact
/// `rel_path`, `rel_path` basename, and title (case-insensitive) — the vault
/// index.md uses relative paths, typed notes use ids/titles. Unresolved
/// targets (dangling links) are dropped (no phantom nodes in v1).
pub fn resolve_edges(docs: &[&VaultDoc]) -> Vec<Edge> {
    // Build lookup tables once.
    let mut by_id: HashMap<&str, usize> = HashMap::new();
    let mut by_rel: HashMap<String, usize> = HashMap::new();
    let mut by_base: HashMap<String, usize> = HashMap::new();
    let mut by_title: HashMap<String, usize> = HashMap::new();
    for (i, doc) in docs.iter().enumerate() {
        by_id.entry(doc.id.as_str()).or_insert(i);
        by_rel.entry(doc.rel_path.clone()).or_insert(i);
        if let Some(base) = doc.rel_path.rsplit('/').next() {
            by_base.entry(base.to_string()).or_insert(i);
            by_base
                .entry(base.trim_end_matches(".md").to_string())
                .or_insert(i);
        }
        by_title
            .entry(doc.title.to_lowercase())
            .or_insert(i);
    }

    let resolve = |target: &str| -> Option<usize> {
        let t = target.trim();
        by_id
            .get(t)
            .or_else(|| by_rel.get(&normalize_rel(t)))
            .or_else(|| by_base.get(t))
            .or_else(|| by_base.get(t.rsplit('/').next().unwrap_or(t)))
            .or_else(|| by_title.get(&t.to_lowercase()))
            .copied()
    };

    let mut edges: Vec<Edge> = Vec::new();
    let mut seen: std::collections::HashSet<(usize, usize, u8)> = std::collections::HashSet::new();
    for (from, doc) in docs.iter().enumerate() {
        // The reserved `index.md` links to the whole vault — its edges are the
        // structural spine, drawn dimmed so semantic links read over them.
        let link_kind = if doc.kind == DocKind::Index {
            EdgeKind::Structural
        } else {
            EdgeKind::Link
        };
        for link in &doc.links {
            let target = match link {
                super::parser::LinkTarget::Wiki(t) => t.as_str(),
                super::parser::LinkTarget::Md(t) => t.as_str(),
            };
            if let Some(to) = resolve(target) {
                push_edge(&mut edges, &mut seen, from, to, link_kind);
            }
        }
        for sref in &doc.supersedes {
            if let Some(to) = resolve(sref) {
                push_edge(&mut edges, &mut seen, from, to, EdgeKind::Supersedes);
            }
        }
        // Project hub spokes: a doc connects to the doc that NAMES its
        // project (id or title match — the vault's `type: project` docs).
        // This is the missing connective tissue: run digests / briefs /
        // routines rarely wikilink each other, so without it a project
        // cluster renders as unrelated dots.
        if !doc.project.is_empty() {
            if let Some(to) = resolve(&doc.project) {
                if to != from {
                    push_edge(&mut edges, &mut seen, from, to, EdgeKind::Project);
                }
            }
        }
    }
    edges
}

/// Group docs into project buckets (the `project:` frontmatter relation) for
/// the static column layout. Docs with no project land in an "(unassigned)"
/// bucket. Returns buckets in stable order (named projects first, sorted).
pub fn project_groups<'a>(docs: &[&'a VaultDoc]) -> Vec<(String, Vec<&'a VaultDoc>)> {
    let mut groups: HashMap<String, Vec<&'a VaultDoc>> = HashMap::new();
    for doc in docs {
        let key = if doc.project.is_empty() {
            "(unassigned)".to_string()
        } else {
            doc.project.clone()
        };
        groups.entry(key).or_default().push(doc);
    }
    let mut out: Vec<(String, Vec<&'a VaultDoc>)> = groups.into_iter().collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn push_edge(
    edges: &mut Vec<Edge>,
    seen: &mut std::collections::HashSet<(usize, usize, u8)>,
    from: usize,
    to: usize,
    kind: EdgeKind,
) {
    if from == to {
        return; // no self-loops
    }
    let k = match kind {
        EdgeKind::Link => 0,
        EdgeKind::Supersedes => 1,
        EdgeKind::Structural => 2,
        EdgeKind::Project => 3,
    };
    if seen.insert((from, to, k)) {
        edges.push(Edge { from, to, kind });
    }
}

/// Normalise a relative link target for `by_rel` matching (backslashes, `./`).
fn normalize_rel(target: &str) -> String {
    target.trim_start_matches("./").replace('\\', "/")
}

// ── Static layout constants (deterministic column layout) ──
const NODE_SIZE_MIN: f32 = 10.;
const NODE_SIZE_MAX: f32 = 26.;
const COL_W: f32 = 220.;
const ROW_H: f32 = 44.;
const PAD_X: f32 = 28.;
const PAD_TOP: f32 = 40.;
const GROUP_GAP: f32 = 24.;

/// A placed node: its center + geometry, for both the canvas edge layer and
/// the absolutely-positioned node chips.
#[derive(Clone)]
struct Placed<'a> {
    doc: &'a VaultDoc,
    cx_pos: f32,
    cy_pos: f32,
    size: f32,
}

/// Node fill color by OKF type (PARITY_SPEC: vendor identity in graph nodes;
/// type otherwise). Sessions read their vendor accent (`accent_for_agent` is
/// substring-tolerant, so `claude_code`/`antigravity` resolve directly); runs
/// read status; other kinds read a type-tonal fill.
pub(super) fn node_color(doc: &VaultDoc) -> Hsla {
    match doc.kind {
        DocKind::Session => accent_for_agent(&doc.vendor),
        DocKind::Run => color_for_status("completed"),
        DocKind::Project => STATUS_DONE.into(),
        DocKind::Routine => STATUS_RUNNING.into(),
        _ => rgba_hex(0xffffff8a).into(),
    }
}

/// Node radius by message_count (sessions) or body size, mapped into
/// [MIN, MAX] on a log scale so a 1959-message session doesn't dwarf a note.
pub(super) fn node_size(doc: &VaultDoc) -> f32 {
    let weight = if doc.message_count > 0 {
        doc.message_count as f32
    } else {
        (doc.body_len as f32 / 512.).max(1.)
    };
    let t = (weight.ln() / 9.0).clamp(0., 1.); // ln(8100) ≈ 9
    NODE_SIZE_MIN + (NODE_SIZE_MAX - NODE_SIZE_MIN) * t
}

/// Build the graph view (spec §3/§4/§5 semantics). Chooses the physics FIELD
/// (the v2 upgrade) when the doc count is within [`field::FIELD_MAX_NODES`],
/// else honestly degrades to the deterministic STATIC layout with a banner
/// (§4 — no jank at scale). Zero-edge roots render nodes-only with a hint
/// (§5). The panel owns the advanced [`VaultField`] and passes it in.
pub fn graph_view(
    field: &super::field::VaultField,
    docs: &[&VaultDoc],
    scroll: &gpui::ScrollHandle,
    hovered: Option<&str>,
    edge_mode: super::panel::EdgeMode,
    panel: WeakEntity<VaultBrowserPanel>,
    cx: &App,
) -> gpui::AnyElement {
    if docs.is_empty() {
        return super::style::empty_state(
            IconName::ListTree,
            "Nothing to graph",
            "OKF notes appear here as a constellation once this root is indexed.",
            cx,
        )
        .into_any_element();
    }

    // Edge-kind declutter (galaxy backlog 2026-07-08): Links isolates the
    // semantic wiki-links, Chain isolates the supersedes corrections; the
    // structural index spine only paints under All.
    let mut edges = resolve_edges(docs);
    match edge_mode {
        super::panel::EdgeMode::All => {}
        super::panel::EdgeMode::Links => edges.retain(|edge| edge.kind == EdgeKind::Link),
        super::panel::EdgeMode::Supersedes => {
            edges.retain(|edge| edge.kind == EdgeKind::Supersedes)
        }
    }
    let degraded = docs.len() > super::field::FIELD_MAX_NODES;
    let truncated = docs.iter().filter(|d| d.link_scan_truncated).count();

    // Body: the physics field (in-scale) or the static column fallback.
    let body = if degraded {
        static_view(docs, &edges, scroll, panel, cx)
    } else {
        super::graph_render::field_view(field, docs, &edges, scroll, hovered, panel, cx)
    };

    // Overlay the honest banners: a scale-degrade banner (§4) and a
    // zero-edge / truncation hint (§5). They float top-left over the graph.
    let mut overlay = v_flex().absolute().top(px(10.)).left(px(12.)).gap(px(6.));
    if degraded {
        overlay = overlay.child(banner(
            IconName::Info,
            SharedString::from(format!(
                "field disabled at this scale — static layout ({} docs > {})",
                docs.len(),
                super::field::FIELD_MAX_NODES
            )),
            cx,
        ));
    }
    if edges.is_empty() {
        overlay = overlay.child(banner(
            IconName::Info,
            "no links yet — [[wikilinks]] in notes create edges".into(),
            cx,
        ));
    }
    if truncated > 0 {
        overlay = overlay.child(banner(
            IconName::Info,
            SharedString::from(format!(
                "{truncated} large doc{} scanned partially — some links may be missing",
                if truncated == 1 { "" } else { "s" }
            )),
            cx,
        ));
    }

    div()
        .relative()
        .size_full()
        .child(body)
        .child(overlay)
        .into_any_element()
}

/// A small honest banner chip (surface fill, hairline, muted) floated over the
/// graph — scale-degrade / no-edges / truncation notices.
fn banner(icon: IconName, text: SharedString, cx: &App) -> gpui::Div {
    let colors = cx.theme().colors();
    h_flex()
        .items_center()
        .gap(px(6.))
        .px(px(9.))
        .py(px(4.))
        .rounded(px(8.))
        .bg(super::style::SURFACE_1)
        .border_1()
        .border_color(HAIRLINE_HI)
        .child(
            Icon::new(icon)
                .size(ui::IconSize::XSmall)
                .color(Color::Custom(colors.text_placeholder)),
        )
        .child(
            div()
                .text_size(px(11.))
                .text_color(colors.text_muted)
                .child(text),
        )
}

/// The deterministic STATIC fallback (spec §4 degrade path): project-grouped
/// columns of nodes over a static edge canvas. No physics, no idle frame pump
/// — used above the field's node ceiling so a huge root never janks.
fn static_view(
    docs: &[&VaultDoc],
    edges: &[Edge],
    scroll: &gpui::ScrollHandle,
    panel: WeakEntity<VaultBrowserPanel>,
    cx: &App,
) -> gpui::AnyElement {
    let groups = project_groups(docs);

    // Place nodes in project columns (up to N columns, wrapping rows).
    let mut placed: Vec<Placed> = Vec::with_capacity(docs.len());
    // Map doc index (into `docs`) → placed index for edge lookup.
    let mut doc_to_placed: HashMap<usize, usize> = HashMap::new();
    let mut y = PAD_TOP;
    for (_group_name, group_docs) in &groups {
        for doc in group_docs {
            let size = node_size(doc);
            let cx_pos = PAD_X + COL_W * 0.5;
            let cy_pos = y + ROW_H * 0.5;
            let placed_ix = placed.len();
            // Find the doc's index in the original slice for edge mapping.
            if let Some(orig) = docs.iter().position(|d| std::ptr::eq(*d, *doc)) {
                doc_to_placed.insert(orig, placed_ix);
            }
            placed.push(Placed {
                doc,
                cx_pos,
                cy_pos,
                size,
            });
            y += ROW_H;
        }
        y += GROUP_GAP;
    }
    let content_height = y + PAD_TOP;

    // The edge canvas: static, keyed by the doc set. Paints resolved edges as
    // 1px hairlines (supersedes edges in the done-blue). Culls offscreen.
    let edge_lines: Vec<(Point<Pixels>, Point<Pixels>, Hsla)> = edges
        .iter()
        .filter_map(|edge| {
            let from = doc_to_placed.get(&edge.from)?;
            let to = doc_to_placed.get(&edge.to)?;
            let a = &placed[*from];
            let b = &placed[*to];
            let color: Hsla = match edge.kind {
                EdgeKind::Supersedes => Hsla::from(STATUS_DONE).opacity(0.5),
                EdgeKind::Link => HAIRLINE_HI.into(),
                EdgeKind::Structural => Hsla::from(HAIRLINE_HI).opacity(0.35),
                // Faintest: connective tissue, must sit under semantic links.
                EdgeKind::Project => Hsla::from(HAIRLINE_HI).opacity(0.22),
            };
            Some((
                point(px(a.cx_pos), px(a.cy_pos)),
                point(px(b.cx_pos), px(b.cy_pos)),
                color,
            ))
        })
        .collect();

    let edge_canvas = canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            if !bounds.intersects(&window.content_mask().bounds) {
                return;
            }
            for (a, b, color) in &edge_lines {
                let mut builder = PathBuilder::stroke(px(1.));
                builder.move_to(point(bounds.origin.x + a.x, bounds.origin.y + a.y));
                builder.line_to(point(bounds.origin.x + b.x, bounds.origin.y + b.y));
                if let Ok(path) = builder.build() {
                    window.paint_path(path, *color);
                }
            }
        },
    )
    .absolute()
    .size_full();

    let nodes: Vec<gpui::AnyElement> = placed
        .iter()
        .map(|p| node_chip(p, panel.clone(), cx))
        .collect();

    div()
        .id("vault-graph-scroll")
        .size_full()
        .overflow_scroll()
        .track_scroll(scroll)
        .child(
            div()
                .relative()
                .w(px(PAD_X * 2. + COL_W))
                .h(px(content_height))
                .child(edge_canvas)
                .children(nodes),
        )
        .into_any_element()
}

/// One graph node: the colored dot + a truncated title label, absolutely
/// placed at its slot. Click → open the md in a center editor tab (deferred by
/// the panel per §11).
fn node_chip(
    placed: &Placed,
    panel: WeakEntity<VaultBrowserPanel>,
    cx: &App,
) -> gpui::AnyElement {
    let colors = cx.theme().colors();
    let color = node_color(placed.doc);
    let size = placed.size;
    let abs_path = placed.doc.abs_path.clone();
    let title: SharedString = placed
        .doc
        .title
        .chars()
        .take(28)
        .collect::<String>()
        .into();
    // The dot's top-left so its CENTER lands on (cx, cy).
    let left = placed.cx_pos - size * 0.5;
    let top = placed.cy_pos - size * 0.5;

    h_flex()
        .id(ElementId::Name(
            format!("vnode-{}", placed.doc.node_key()).into(),
        ))
        .absolute()
        .left(px(left))
        .top(px(top))
        .items_center()
        .gap(px(9.))
        .cursor_pointer()
        .on_click(move |_, window, cx| {
            let path = abs_path.clone();
            panel
                .update(cx, |panel, cx| panel.request_open(path, window, cx))
                .ok();
        })
        .child(
            // The node dot: type/vendor-colored fill + status-glow bloom
            // (RUST_PORT_NOTES §1 — real BoxShadow on OLED).
            div()
                .size(px(size))
                .rounded_full()
                .bg(color)
                .shadow(vec![gpui::BoxShadow {
                    color: color.opacity(0.25),
                    offset: point(px(0.), px(0.)),
                    blur_radius: px(12.),
                    spread_radius: px(0.),
                }]),
        )
        .child(
            div()
                .max_w(px(COL_W - size - 20.))
                .text_size(px(12.))
                .text_color(colors.text_muted)
                .truncate()
                .child(title),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::super::index::{DocKind, VaultRoot};
    use super::super::parser::LinkTarget;
    use super::*;
    use std::path::PathBuf;

    /// Minimal doc builder for edge tests (real docs come from `index`).
    fn doc(id: &str, rel: &str, title: &str) -> VaultDoc {
        VaultDoc {
            abs_path: PathBuf::from(rel),
            rel_path: rel.to_string(),
            root: VaultRoot::Vault,
            kind: DocKind::Note,
            id: id.to_string(),
            title: title.to_string(),
            doc_type: "note".to_string(),
            vendor: String::new(),
            run_id: String::new(),
            status: String::new(),
            schedule: String::new(),
            project: String::new(),
            updated: String::new(),
            message_count: 0,
            redactions: 0,
            body_len: 0,
            link_scan_truncated: false,
            search_key: title.to_lowercase(),
            links: Vec::new(),
            supersedes: Vec::new(),
            bundle_dir: None,
        }
    }

    #[test]
    fn resolves_link_by_id_relpath_and_title() {
        let mut a = doc("id-a", "a.md", "Note A");
        let target = doc("id-b", "sub/b.md", "Note B");
        // by id
        a.links.push(LinkTarget::Wiki("id-b".to_string()));
        // by relative path
        a.links.push(LinkTarget::Md("sub/b.md".to_string()));
        // by title
        a.links.push(LinkTarget::Wiki("Note B".to_string()));
        let docs = [&a, &target];
        let edges = resolve_edges(&docs);
        // Three link forms → but deduped to one (from=0, to=1, Link).
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].from, 0);
        assert_eq!(edges[0].to, 1);
        assert_eq!(edges[0].kind, EdgeKind::Link);
    }

    #[test]
    fn drops_dangling_links() {
        let mut a = doc("id-a", "a.md", "A");
        a.links.push(LinkTarget::Wiki("does-not-exist".to_string()));
        let docs = [&a];
        assert!(resolve_edges(&docs).is_empty());
    }

    #[test]
    fn project_membership_spokes_to_the_hub_doc() {
        // The missing connective tissue (user report 2026-07-10): docs of a
        // project connect to the doc that NAMES the project (id or title),
        // so a cluster reads as one project instead of loose dots.
        let mut hub = doc("agentic-ide", "projects/agentic-ide.md", "agentic-ide");
        hub.kind = DocKind::Project;
        let mut member = doc("run-1", "artifacts/runs/run-1/digest.md", "Echo task");
        member.project = "agentic-ide".to_string();
        let mut by_title = doc("note-2", "notes/n2.md", "Design notes");
        by_title.project = "agentic-ide".to_string();
        let mut orphan = doc("run-9", "artifacts/runs/run-9/digest.md", "Loose");
        orphan.project = "no-such-project".to_string(); // dangling → no edge
        let docs = [&hub, &member, &by_title, &orphan];
        let edges = resolve_edges(&docs);
        let spokes: Vec<_> = edges
            .iter()
            .filter(|e| e.kind == EdgeKind::Project)
            .collect();
        assert_eq!(spokes.len(), 2, "two members spoke to the hub, orphan drops");
        assert!(spokes.iter().all(|e| e.to == 0), "spokes land on the hub");
        // The hub itself never self-spokes even though its project field may
        // name itself downstream.
        assert!(!spokes.iter().any(|e| e.from == 0));
    }

    #[test]
    fn supersedes_makes_a_correction_edge() {
        let mut new = doc("id-new", "new.md", "New");
        let old = doc("id-old", "old.md", "Old");
        new.supersedes.push("id-old".to_string());
        let docs = [&new, &old];
        let edges = resolve_edges(&docs);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].kind, EdgeKind::Supersedes);
    }

    #[test]
    fn no_self_loops() {
        let mut a = doc("id-a", "a.md", "A");
        // A link that resolves back to itself is dropped.
        a.links.push(LinkTarget::Wiki("id-a".to_string()));
        let docs = [&a];
        assert!(resolve_edges(&docs).is_empty());
    }

    #[test]
    fn project_groups_bucket_unassigned() {
        let mut a = doc("a", "a.md", "A");
        a.project = "proj-x".to_string();
        let b = doc("b", "b.md", "B"); // no project
        let docs = [&a, &b];
        let groups = project_groups(&docs);
        assert_eq!(groups.len(), 2);
        // "(unassigned)" sorts before "proj-x".
        assert_eq!(groups[0].0, "(unassigned)");
        assert_eq!(groups[1].0, "proj-x");
    }

    #[test]
    fn index_links_are_structural_dimmed_edges() {
        // The reserved index.md is the vault spine: its links to typed docs
        // resolve as Structural (dimmed), not semantic Link edges.
        let mut idx = doc("Vault:index.md", "index.md", "index");
        idx.kind = DocKind::Index;
        idx.links.push(LinkTarget::Wiki("a-note".to_string()));
        let target = doc("a-note", "notes/a.md", "A Note");
        let docs = [&idx, &target];
        let edges = resolve_edges(&docs);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].kind, EdgeKind::Structural);

        // A NON-index doc's identical link stays a semantic Link.
        let mut note = doc("n", "n.md", "N");
        note.links.push(LinkTarget::Wiki("a-note".to_string()));
        let docs2 = [&note, &target];
        assert_eq!(resolve_edges(&docs2)[0].kind, EdgeKind::Link);
    }
}
