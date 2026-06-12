//! Paint caching for the graph's STATIC geometry (RUST_PORT_NOTES §8:
//! "per-frame allocations near zero in paint closures — pre-build
//! `PathBuilder` outputs"). While any run is running, the spinner keeps the
//! frame clock hot and every canvas paint closure re-executes per frame;
//! settled edges and duotone root rings never change shape, so their lyon
//! stroke tessellation is cached keyed by paint origin + inputs and rebuilt
//! only on change. The graph scrolls → origin changes → misses during scroll
//! frames, hits on the dominant steady-state spinner-driven redraws.
//! (Painting still clones the built `Path` — `paint_path` takes it by value —
//! but a vertex-buffer memcpy is far cheaper than re-tessellating.)

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui::{Bounds, Hsla, Path, Pixels, Point};

/// Panel-owned cache, shared into `'static` paint closures via `Rc`
/// (GPUI paints on the UI thread only).
pub(super) type SharedPaintCache = Rc<RefCell<GraphPaintCache>>;

/// One conv's settled-edge paths, keyed by paint origin + edge inputs.
struct EdgesEntry {
    origin: Point<Pixels>,
    inputs: Vec<(f32, Hsla)>,
    paths: Vec<(Path<Pixels>, Hsla)>,
}

/// One conv's root-ring segment paths, keyed by the ring bounds.
struct RingEntry {
    bounds: Bounds<Pixels>,
    paths: Vec<(Path<Pixels>, Hsla)>,
}

#[derive(Default)]
pub(super) struct GraphPaintCache {
    edges: HashMap<String, EdgesEntry>,
    rings: HashMap<String, RingEntry>,
}

impl GraphPaintCache {
    /// The built settled-edge paths for `conv`; `build` runs only when the
    /// paint origin or the `(y, color)` edge set changed since last frame.
    pub fn settled_edge_paths(
        &mut self,
        conv: &str,
        origin: Point<Pixels>,
        inputs: &[(f32, Hsla)],
        build: impl FnOnce() -> Vec<(Path<Pixels>, Hsla)>,
    ) -> Vec<(Path<Pixels>, Hsla)> {
        let hit = self
            .edges
            .get(conv)
            .is_some_and(|entry| entry.origin == origin && entry.inputs == inputs);
        if !hit {
            self.edges.insert(
                conv.to_string(),
                EdgesEntry {
                    origin,
                    inputs: inputs.to_vec(),
                    paths: build(),
                },
            );
        }
        self.edges[conv].paths.clone()
    }

    /// The built root-ring segment paths for `conv`; `build` runs only when
    /// the ring bounds changed since last frame.
    pub fn ring_paths(
        &mut self,
        conv: &str,
        bounds: Bounds<Pixels>,
        build: impl FnOnce() -> Vec<(Path<Pixels>, Hsla)>,
    ) -> Vec<(Path<Pixels>, Hsla)> {
        let hit = self
            .rings
            .get(conv)
            .is_some_and(|entry| entry.bounds == bounds);
        if !hit {
            self.rings.insert(
                conv.to_string(),
                RingEntry {
                    bounds,
                    paths: build(),
                },
            );
        }
        self.rings[conv].paths.clone()
    }

    /// Drop entries for conversations no longer on the board.
    pub fn retain_convs(&mut self, live: impl Fn(&str) -> bool) {
        self.edges.retain(|conv, _| live(conv));
        self.rings.retain(|conv, _| live(conv));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{PathBuilder, point, px, size};

    fn test_path() -> Path<Pixels> {
        let mut builder = PathBuilder::stroke(px(1.));
        builder.move_to(point(px(0.), px(0.)));
        builder.line_to(point(px(10.), px(10.)));
        builder.build().unwrap()
    }

    #[test]
    fn settled_edges_rebuild_only_on_origin_or_input_change() {
        let mut cache = GraphPaintCache::default();
        let color = gpui::black();
        let inputs = vec![(70.0_f32, color)];
        let origin = point(px(5.), px(5.));
        let mut builds = 0;

        for _ in 0..3 {
            cache.settled_edge_paths("conv-1", origin, &inputs, || {
                builds += 1;
                vec![(test_path(), color)]
            });
        }
        assert_eq!(builds, 1, "steady-state frames hit the cache");

        cache.settled_edge_paths("conv-1", point(px(5.), px(-40.)), &inputs, || {
            builds += 1;
            vec![(test_path(), color)]
        });
        assert_eq!(builds, 2, "scroll (origin change) rebuilds");

        let grown = vec![(70.0_f32, color), (126.0_f32, color)];
        cache.settled_edge_paths("conv-1", point(px(5.), px(-40.)), &grown, || {
            builds += 1;
            vec![(test_path(), color), (test_path(), color)]
        });
        assert_eq!(builds, 3, "edge-set change rebuilds");
    }

    #[test]
    fn rings_key_on_bounds_and_prune_with_convs() {
        let mut cache = GraphPaintCache::default();
        let bounds = Bounds::new(point(px(10.), px(10.)), size(px(22.), px(22.)));
        let mut builds = 0;
        for _ in 0..2 {
            cache.ring_paths("conv-1", bounds, || {
                builds += 1;
                vec![(test_path(), gpui::white())]
            });
        }
        assert_eq!(builds, 1);

        cache.retain_convs(|conv| conv != "conv-1");
        cache.ring_paths("conv-1", bounds, || {
            builds += 1;
            vec![(test_path(), gpui::white())]
        });
        assert_eq!(builds, 2, "pruned entry rebuilds on next sight");
    }
}
