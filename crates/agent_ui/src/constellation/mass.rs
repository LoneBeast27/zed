//! Token mass (MASS+COMPACT T4, graph.js `dotPx`/`rootPx`/`updateMass`
//! EXACT): area-proportional dots — the dot's AREA rises linearly with
//! weighted tokens (fresh_in + out + 0.1·cache_read, computed bridge-side),
//! so its box ~ √tokens: twice the ink = twice the tokens, perceptually
//! honest. Floor 14px, cap 30px (~120K weighted saturates). The root maps
//! `ctx_tokens + gobble bonus` the same way (cap ~48K ctx) and DEFLATES on a
//! compaction (the exhale).
//!
//! The gobble bonus is the T4 honesty rule: the root gains the tokens the
//! orchestrator actually INGESTS (the run's receipt/digest, ~120–2K tok) —
//! NOT the prey's own burn; the bonus then deflates as the poll's
//! `ctx_tokens` catches up, so nothing is double-counted.

/// Dot box floor/cap (px, spec-pinned).
pub const DOT_PX_MIN: f32 = 14.;
pub const DOT_PX_MAX: f32 = 30.;
/// Root box floor/cap (px).
pub const ROOT_PX_MIN: f32 = 24.;
pub const ROOT_PX_MAX: f32 = 44.;
/// Weighted tokens at which the dot saturates.
const DOT_SATURATE_TOKENS: f32 = 120_000.;
/// Ctx tokens at which the root saturates.
const ROOT_SATURATE_TOKENS: f32 = 48_000.;
/// Fallback receipt size when a gobbled run reported no ingest tokens.
pub const INGEST_FALLBACK: f64 = 120.;

/// `dotPx(weighted)` — the run dot's box (diameter) for a weighted token
/// count.
pub fn dot_px(weighted: f64) -> f32 {
    px_for(weighted, DOT_PX_MIN, DOT_PX_MAX, DOT_SATURATE_TOKENS)
}

/// `rootPx(ctxTokens)` — the root dot's box for a context occupancy.
pub fn root_px(ctx_tokens: f64) -> f32 {
    px_for(ctx_tokens, ROOT_PX_MIN, ROOT_PX_MAX, ROOT_SATURATE_TOKENS)
}

fn px_for(tokens: f64, min: f32, max: f32, saturate: f32) -> f32 {
    if tokens <= 0. {
        return min;
    }
    let k = (max * max - min * min) / saturate;
    (min * min + k * tokens as f32).sqrt().min(max)
}

/// The root's ctx-mass state: live tokens + the gobble bonus + the
/// compaction counter (graph.js `rootCtx` entries).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct RootMass {
    pub tokens: f64,
    pub bonus: f64,
    pub compactions: u64,
}

impl RootMass {
    /// First sight adopts as-is — no retro-swell, no retro-exhale.
    pub fn first_sight(ctx_tokens: f64, compactions: u64) -> Self {
        Self {
            tokens: ctx_tokens,
            bonus: 0.,
            compactions,
        }
    }

    /// Fold a fresh `(ctx_tokens, compactions)` poll: the bonus deflates as
    /// the ingested receipt becomes REAL ctx (no double count). Returns
    /// `true` when a compaction happened — the exhale trigger.
    pub fn fold(&mut self, ctx_tokens: f64, compactions: u64) -> bool {
        if ctx_tokens > self.tokens {
            self.bonus = (self.bonus - (ctx_tokens - self.tokens)).max(0.);
        }
        let exhale = compactions > self.compactions;
        self.compactions = compactions;
        self.tokens = ctx_tokens;
        exhale
    }

    /// Bank a gobbled run's receipt (the eat moment's mass gain).
    pub fn add_bonus(&mut self, ingest: f64) {
        self.bonus += if ingest > 0. { ingest } else { INGEST_FALLBACK };
    }

    /// The root's target box for the current state.
    pub fn target_px(&self) -> f32 {
        root_px(self.tokens + self.bonus)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floors_and_caps_are_spec_pinned() {
        assert_eq!(dot_px(0.), DOT_PX_MIN);
        assert_eq!(dot_px(-5.), DOT_PX_MIN);
        assert_eq!(dot_px(120_000.), DOT_PX_MAX);
        assert_eq!(dot_px(10_000_000.), DOT_PX_MAX);
        assert_eq!(root_px(0.), ROOT_PX_MIN);
        assert_eq!(root_px(48_000.), ROOT_PX_MAX);
        assert_eq!(root_px(1e9), ROOT_PX_MAX);
    }

    #[test]
    fn area_rises_linearly_with_tokens() {
        // Twice the tokens = twice the ink (area), i.e. px² is affine in
        // tokens between floor and cap: px²(t) − px²(0) doubles with t.
        let a1 = dot_px(20_000.).powi(2) - DOT_PX_MIN.powi(2);
        let a2 = dot_px(40_000.).powi(2) - DOT_PX_MIN.powi(2);
        assert!((a2 / a1 - 2.).abs() < 1e-4, "area ratio {}", a2 / a1);
        // And is monotonic.
        assert!(dot_px(10_000.) < dot_px(30_000.));
        assert!(root_px(5_000.) < root_px(20_000.));
    }

    #[test]
    fn matches_graph_js_reference_values() {
        // k = (30² − 14²)/120000; px(w) = √(14² + k·w).
        let k = (30_f32.powi(2) - 14_f32.powi(2)) / 120_000.;
        for w in [500., 12_000., 60_000., 119_000.] {
            let want = (14_f32.powi(2) + k * w as f32).sqrt();
            assert!((dot_px(w) - want).abs() < 1e-4);
        }
    }

    #[test]
    fn bonus_deflates_as_ctx_catches_up_without_double_count() {
        let mut root = RootMass::first_sight(10_000., 0);
        root.add_bonus(800.);
        assert_eq!(root.bonus, 800.);
        // Ctx grows 300 → bonus deflates by 300 (the receipt became real).
        assert!(!root.fold(10_300., 0));
        assert_eq!(root.bonus, 500.);
        assert_eq!(root.tokens, 10_300.);
        // Ctx overtakes the whole bonus → clamps at 0, never negative.
        assert!(!root.fold(11_500., 0));
        assert_eq!(root.bonus, 0.);
        // Ctx SHRINKING (compaction) never inflates the bonus.
        assert!(root.fold(4_000., 1), "compaction increment = exhale");
        assert_eq!(root.bonus, 0.);
        assert_eq!(root.tokens, 4_000.);
    }

    #[test]
    fn ingest_fallback_feeds_a_thin_receipt() {
        let mut root = RootMass::first_sight(0., 0);
        root.add_bonus(0.); // agy run with no token surface
        assert_eq!(root.bonus, INGEST_FALLBACK);
        root.add_bonus(420.);
        assert_eq!(root.bonus, INGEST_FALLBACK + 420.);
    }

    #[test]
    fn exhale_fires_only_on_a_watched_increment() {
        // First sight at compactions=3 must NOT retro-exhale.
        let mut root = RootMass::first_sight(9_000., 3);
        assert!(!root.fold(9_100., 3));
        assert!(root.fold(2_800., 4), "watched 3→4 = exhale");
        assert!(!root.fold(3_000., 4), "same counter, no exhale");
    }
}
