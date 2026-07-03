//! Billiards-aim fan layout + auto-arrange seriation (graph.js
//! `fanPositions` / `linkWeights` / `weightedOrder`, EXACT math).
//!
//! The fan is LOCKED to a ±45° cone pointed into the OPEN area — subagents
//! never spawn between the root and a nearby wall. The container's major
//! axis decides the table: wide panel → root seats left-center, cone opens
//! rightward; tall panel → root seats top-center, cone opens downward.
//!
//! Auto-arrange (T3) reorders fan SLOTS so heavily-linked pairs land
//! angularly adjacent — weight = overlap venn score + boost batch count,
//! seriated greedily. One-shot and tweened by the sim; NEVER a live solver.

use std::collections::HashMap;

use crate::bridge::{ChannelRow, OverlapRow};

/// The tall/wide switch threshold (px).
pub const WIDE_MIN_W: f32 = 560.;

/// A conversation's fan layout: root seat + one slot per run.
#[derive(Debug, Clone, PartialEq)]
pub struct FanLayout {
    /// Wide table (root left-center, cone rightward) vs tall (root
    /// top-center, cone downward).
    pub wide: bool,
    /// Root dot center.
    pub rx: f32,
    pub ry: f32,
    /// Block height (the conv container's height).
    pub h: f32,
    /// One anchor slot per run, in run order.
    pub positions: Vec<(f32, f32)>,
}

/// `fanPositions(N, W)` — the ±45° cone. Exact port.
pub fn fan_positions(n: usize, w: f32) -> FanLayout {
    let nf = n as f32;
    let spread: f32 = if n <= 1 { 0. } else { ((nf - 1.) * 32.).min(90.) };
    let rad = spread.to_radians();
    let wide = w >= WIDE_MIN_W;
    let slot_angle = |i: usize| -> f32 {
        if n == 1 {
            0.
        } else {
            (-spread / 2. + (spread / (nf - 1.)) * i as f32).to_radians()
        }
    };
    if wide {
        let r = if n > 1 {
            (62. * (nf - 1.) / rad).max(170.)
        } else {
            170.
        };
        let half = r * (rad / 2.).min(std::f32::consts::FRAC_PI_4).sin();
        let h = (half * 2. + 130.).max(250.);
        let (rx, ry) = (84., h / 2.);
        let positions = (0..n)
            .map(|i| {
                let a = slot_angle(i);
                (rx + r * a.cos(), ry + r * a.sin())
            })
            .collect();
        return FanLayout {
            wide,
            rx,
            ry,
            h,
            positions,
        };
    }
    let r = if n > 1 {
        (56. * (nf - 1.) / rad).max(150.)
    } else {
        150.
    };
    let rx = (w / 2.).max(170.);
    let ry = 48.;
    let h = ry + r + 96.;
    let positions = (0..n)
        .map(|i| {
            let a = slot_angle(i);
            (rx + r * a.sin(), ry + r * a.cos())
        })
        .collect();
    FanLayout {
        wide,
        rx,
        ry,
        h,
        positions,
    }
}

/// Ordered pair key (web: `a < b ? a+" "+b : b+" "+a`).
fn pair_key(a: &str, b: &str) -> (String, String) {
    if a < b {
        (a.to_string(), b.to_string())
    } else {
        (b.to_string(), a.to_string())
    }
}

/// `weight(a,b)` = venn overlap score (shared artifact paths + code anchors)
/// + relayed boost batches — "what was inter-connecting more", both signals.
pub fn link_weights(
    overlap: &[OverlapRow],
    channels: &[ChannelRow],
) -> HashMap<(String, String), f64> {
    let mut weights = HashMap::new();
    let mut add = |a: &str, b: &str, v: f64| {
        if !a.is_empty() && !b.is_empty() && v > 0. {
            *weights.entry(pair_key(a, b)).or_insert(0.) += v;
        }
    };
    for row in overlap {
        add(&row.a, &row.b, row.score);
    }
    for channel in channels {
        add(&channel.a, &channel.b, channel.k as f64);
    }
    weights
}

/// Greedy weighted seriation: seed a segment with the heaviest remaining
/// pair, grow it at whichever end keeps the strongest link, repeat per
/// component, then append weightless ids in their original (spawn) order.
/// Returns `None` when NO pair among `ids` carries weight — the tidy-fan
/// fallback signal. Deterministic: id-slice order stands in for the web's
/// Set insertion order; strict `>` keeps first ties.
pub fn weighted_order(
    ids: &[String],
    weights: &HashMap<(String, String), f64>,
) -> Option<Vec<String>> {
    let wt = |a: &str, b: &str| weights.get(&pair_key(a, b)).copied().unwrap_or(0.);
    let mut left: Vec<&str> = ids.iter().map(String::as_str).collect();
    let mut order: Vec<String> = Vec::with_capacity(ids.len());
    let mut any = false;
    while !left.is_empty() {
        // Heaviest remaining pair.
        let mut best: Option<(usize, usize, f64)> = None;
        for (i, a) in left.iter().enumerate() {
            for (j, b) in left.iter().enumerate() {
                if i == j {
                    continue;
                }
                let v = wt(a, b);
                if v > 0. && best.is_none_or(|(_, _, bv)| v > bv) {
                    best = Some((i, j, v));
                }
            }
        }
        let Some((i, j, _)) = best else {
            // No weighted pair left: append leftovers in spawn order.
            for id in ids {
                if left.contains(&id.as_str()) {
                    order.push(id.clone());
                }
            }
            break;
        };
        any = true;
        let mut segment = vec![left[i].to_string(), left[j].to_string()];
        // Remove the higher index first so the lower stays valid.
        let (hi, lo) = if i > j { (i, j) } else { (j, i) };
        left.remove(hi);
        left.remove(lo);
        // Grow at whichever end keeps the strongest link.
        loop {
            let head = segment[0].clone();
            let tail = segment[segment.len() - 1].clone();
            let mut ext: Option<(usize, f64, bool)> = None;
            for (k, id) in left.iter().enumerate() {
                let vh = wt(id, &head);
                let vt = wt(id, &tail);
                let v = vh.max(vt);
                if v > 0. && ext.is_none_or(|(_, ev, _)| v > ev) {
                    ext = Some((k, v, vh >= vt));
                }
            }
            let Some((k, _, at_head)) = ext else { break };
            let id = left.remove(k).to_string();
            if at_head {
                segment.insert(0, id);
            } else {
                segment.push(id);
            }
        }
        order.append(&mut segment);
    }
    any.then_some(order)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_node_sits_on_the_aim_axis() {
        // Wide: aim is +x, R = 170 from (84, H/2), H = 250.
        let wide = fan_positions(1, 800.);
        assert!(wide.wide);
        assert_eq!((wide.rx, wide.ry, wide.h), (84., 125., 250.));
        assert_eq!(wide.positions, vec![(84. + 170., 125.)]);
        // Tall: aim is +y, R = 150 from (W/2 max 170, 48).
        let tall = fan_positions(1, 440.);
        assert!(!tall.wide);
        assert_eq!((tall.rx, tall.ry), (220., 48.));
        assert_eq!(tall.h, 48. + 150. + 96.);
        assert_eq!(tall.positions, vec![(220., 48. + 150.)]);
    }

    #[test]
    fn wide_threshold_is_560() {
        assert!(!fan_positions(2, 559.).wide);
        assert!(fan_positions(2, 560.).wide);
    }

    #[test]
    fn cone_is_locked_to_45_degrees_each_side() {
        // N = 4 → spread = min(90, 3·32) = 90° → extreme slots at ±45°.
        let fan = fan_positions(4, 800.);
        let (x0, y0) = fan.positions[0];
        let (x3, y3) = fan.positions[3];
        let a0 = (y0 - fan.ry).atan2(x0 - fan.rx).to_degrees();
        let a3 = (y3 - fan.ry).atan2(x3 - fan.rx).to_degrees();
        assert!((a0 + 45.).abs() < 0.01, "first slot at −45°, got {a0}");
        assert!((a3 - 45.).abs() < 0.01, "last slot at +45°, got {a3}");
        // N = 8 → spread stays capped at 90 (7·32 = 224 > 90).
        let fan = fan_positions(8, 800.);
        let (x, y) = fan.positions[7];
        let a = (y - fan.ry).atan2(x - fan.rx).to_degrees();
        assert!((a - 45.).abs() < 0.01, "cap holds at N=8, got {a}");
    }

    #[test]
    fn slots_keep_arc_separation_by_growing_the_radius() {
        // R = max(150, 56·(N−1)/rad) tall: at N=6 the radius must beat the
        // floor so adjacent slots keep ~56px of arc.
        let fan = fan_positions(6, 440.);
        let (x0, y0) = fan.positions[0];
        let (x1, y1) = fan.positions[1];
        let d = (x1 - x0).hypot(y1 - y0);
        assert!(d > 50., "adjacent slot spacing {d} keeps the fan legible");
        // The block height tracks the grown radius.
        let r = ((56. * 5.) / 90_f32.to_radians()).max(150.);
        assert_eq!(fan.h, 48. + r + 96.);
    }

    #[test]
    fn link_weights_sum_overlap_and_batches() {
        let overlap = vec![OverlapRow {
            a: "r1".into(),
            b: "r2".into(),
            score: 3.,
        }];
        let channels = vec![ChannelRow {
            a: "r2".into(),
            b: "r1".into(), // reversed — same ordered pair
            k: 4,
            ..Default::default()
        }];
        let weights = link_weights(&overlap, &channels);
        assert_eq!(
            weights.get(&("r1".to_string(), "r2".to_string())),
            Some(&7.)
        );
    }

    #[test]
    fn weighted_order_seats_linked_pairs_adjacent() {
        // The staged demo scenario (board-demo.js T3): impl-a↔impl-b score 3,
        // designer↔tester score 2 (+ channel k) — arrange must seat both
        // pairs angularly adjacent.
        let ids: Vec<String> = ["plan", "designer", "impl-a", "impl-b", "tester", "research"]
            .into_iter()
            .map(String::from)
            .collect();
        let overlap = vec![
            OverlapRow {
                a: "impl-a".into(),
                b: "impl-b".into(),
                score: 3.,
            },
            OverlapRow {
                a: "designer".into(),
                b: "tester".into(),
                score: 2.,
            },
        ];
        let order = weighted_order(&ids, &link_weights(&overlap, &[])).unwrap();
        let at = |id: &str| order.iter().position(|x| x == id).unwrap();
        assert_eq!(at("impl-a").abs_diff(at("impl-b")), 1, "impl pair adjacent");
        assert_eq!(
            at("designer").abs_diff(at("tester")),
            1,
            "designer/tester adjacent"
        );
        // Weightless ids survive in spawn order at the end.
        assert!(order.contains(&"plan".to_string()));
        assert!(order.contains(&"research".to_string()));
        assert_eq!(order.len(), ids.len());
    }

    #[test]
    fn no_weights_signals_the_tidy_fan_fallback() {
        let ids = vec!["a".to_string(), "b".to_string()];
        assert_eq!(weighted_order(&ids, &HashMap::new()), None);
    }

    #[test]
    fn weighted_order_is_deterministic() {
        let ids: Vec<String> = ["a", "b", "c", "d"].into_iter().map(String::from).collect();
        let overlap = vec![
            OverlapRow {
                a: "a".into(),
                b: "c".into(),
                score: 2.,
            },
            OverlapRow {
                a: "b".into(),
                b: "d".into(),
                score: 2.,
            },
        ];
        let weights = link_weights(&overlap, &[]);
        let first = weighted_order(&ids, &weights).unwrap();
        for _ in 0..10 {
            assert_eq!(weighted_order(&ids, &weights).unwrap(), first);
        }
    }
}
