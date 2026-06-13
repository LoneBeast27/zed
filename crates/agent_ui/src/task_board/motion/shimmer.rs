//! The §4.1 in-progress-label shimmer — a gradient text-sweep replacing the
//! `pulsating_between` placeholder (RUST_PORT_NOTES §0/§4.1: "a true gradient
//! text-sweep needs a custom `canvas()` paint; pulsate is the idiomatic first
//! pass"). Web reference: `chat.css` `.shimmer-label` — a 105° linear
//! gradient (`transparent 40% · rgba(250,249,245,0.85) 50% · transparent
//! 60%`) clipped to the text and swept left→right over 2.4s linear, holding
//! between the `40%` and `100%` keyframes.
//!
//! **The conscious divergence (decided here): per-character highlight band,
//! not glyph-masked gradient.** GPUI rasterises glyphs as SOLID-colour
//! sprites — `background-clip: text` over a moving gradient has no direct
//! paint primitive (a glyph's fill is one `Hsla`, not a per-pixel shader).
//! So rather than a fake (a gradient quad behind opaque text would wash the
//! background, not the letters), we shape the label PER CHARACTER and tint
//! each glyph by the highlight band sampled at the glyph's centre as the band
//! sweeps across — a discretised text-sweep. At the island's small label
//! sizes this reads as a shine travelling through the letters, which is the
//! web's intent; the only loss is sub-glyph gradient smoothness, invisible at
//! a few px per char. This is the "masked moving-gradient overlay" the notes
//! sanction when canvas text-masking is impractical.
//!
//! **Reduced-motion / fallback.** [`ShimmerMode::Pulsate`] keeps the standing
//! `pulsating_between(0.4, 0.92)` opacity pulse (the Z3/Z4 ruling) as the
//! reduced-motion path; consumers pick it when a global reduce-motion signal
//! exists. [`ShimmerMode::Sweep`] is the new default gradient sweep. Both
//! "stop cleanly": the element only exists while the label is busy — dropping
//! it leaves the next (landed) representation with no residual animation.

use std::time::Duration;

use gpui::{
    App, Bounds, Element, ElementId, FontWeight, GlobalElementId, Hsla, InspectorElementId,
    IntoElement, LayoutId, Pixels, ShapedLine, Style, TextAlign, TextRun, Window, point, size,
};

/// Sweep period — web `.shimmer-label { animation: shimmer 2.4s linear }`.
pub const SWEEP_MS: Duration = Duration::from_millis(2400);
/// Pulsate period — the standing `pulsating_between` fallback (1.4s, matching
/// the existing orchestrator/adversary labels).
pub const PULSATE_MS: Duration = Duration::from_millis(1400);

/// How the in-progress label animates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShimmerMode {
    /// The gradient text-sweep (new default) — a highlight band travels
    /// through the glyphs.
    Sweep,
    /// The `pulsating_between(0.4, 0.92)` opacity pulse — the reduced-motion
    /// fallback (and the pre-polish behaviour).
    Pulsate,
}

/// The shimmer element for an in-progress label. Self-pumped: it owns a
/// repeating gpui animation (the sweep clock / the pulse clock), so consumers
/// just build it with a stable id while busy and drop it when the work lands.
pub struct Shimmer {
    id: ElementId,
    text: gpui::SharedString,
    font_size: Pixels,
    weight: FontWeight,
    /// Resting label colour (web `--text-3`).
    base: Hsla,
    /// Highlight colour the band sweeps in (web `rgba(250,249,245,0.85)`).
    highlight: Hsla,
    mode: ShimmerMode,
}

/// Build a shimmer for `text`. `base` is the resting colour (`--text-3`),
/// `highlight` the sweep colour. `mode` selects sweep vs the pulsate
/// fallback.
pub fn shimmer(
    id: impl Into<ElementId>,
    text: impl Into<gpui::SharedString>,
    font_size: Pixels,
    weight: FontWeight,
    base: Hsla,
    highlight: Hsla,
    mode: ShimmerMode,
) -> Shimmer {
    Shimmer {
        id: id.into(),
        text: text.into(),
        font_size,
        weight,
        base,
        highlight,
        mode,
    }
}

/// The highlight weight at fractional position `x` ∈ [0,1] along the label
/// for a band centred at `center` (also in [0,1] of the SWEEP travel, which
/// over-runs both edges so the band fully enters and exits). Mirrors the
/// web's `transparent 40% · solid 50% · transparent 60%` stop ramp: a
/// triangular band 0.2-wide in label space, 0 outside, 1 at the centre.
pub fn band_weight(x: f32, center: f32) -> f32 {
    const HALF: f32 = 0.1; // 10% half-width → the web's 40%→60% span.
    let d = (x - center).abs();
    if d >= HALF { 0. } else { 1. - d / HALF }
}

/// The band centre for sweep progress `t` ∈ [0,1]. The web sweeps the
/// background-position from `-120px` to `+280px` over the first 40% of the
/// clock, then holds — so map [0,0.4]→[-0.2,1.2] (over-running both edges so
/// the highlight fully traverses) and clamp the hold tail at 1.2.
pub fn sweep_center(t: f32) -> f32 {
    let travel = (t / 0.4).min(1.);
    -0.2 + travel * 1.4
}

impl Shimmer {
    /// Shape `text` (a `SharedString` — clones are Arc bumps, no realloc) at
    /// `color`. Shaping is a line-layout cache hit across frames (the cache key
    /// excludes colour), so a per-frame recolour is cheap.
    fn run(&self, text: gpui::SharedString, color: Hsla, window: &Window) -> ShapedLine {
        let mut font = window.text_style().font();
        font.weight = self.weight;
        let run = TextRun {
            len: text.len(),
            font,
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        window
            .text_system()
            .shape_line(text, self.font_size, &[run], None)
    }

    /// Build the per-char `(x, SharedString)` layout once (P8 cache miss): shape
    /// the whole label to get exact per-char x-advances, then record each
    /// character's start x and its single-char string. The single-char
    /// `SharedString`s are allocated here ONCE and reused across frames.
    fn build_char_cache(&self, window: &Window) -> ShimmerCharCache {
        let full = self.run(self.text.clone(), self.base, window);
        let mut chars = Vec::new();
        for (byte, ch) in self.text.char_indices() {
            let x = full.x_for_index(byte);
            chars.push((x, gpui::SharedString::from(ch.to_string())));
        }
        ShimmerCharCache {
            text: self.text.clone(),
            font_size: self.font_size,
            chars,
        }
    }
}

impl IntoElement for Shimmer {
    type Element = gpui::AnimationElement<ShimmerInner>;
    fn into_element(self) -> Self::Element {
        use gpui::AnimationExt as _;
        let period = match self.mode {
            ShimmerMode::Sweep => SWEEP_MS,
            ShimmerMode::Pulsate => PULSATE_MS,
        };
        // The repeating clock IS the frame pump — a stable id keeps the
        // infinite animation from resetting across re-renders (the web's CSS
        // `animation: … infinite` on a persistent node never restarts; §8.7a
        // identity rule). The inner element re-keys off the same id.
        let id = self.id.clone();
        let inner = ShimmerInner { shimmer: self, phase: 0. };
        inner.with_animation(id, gpui::Animation::new(period).repeat(), |mut inner, delta| {
            inner.phase = delta;
            inner
        })
    }
}

impl IntoElement for ShimmerInner {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
    }
}

/// The painting half — receives the 0→1 animation phase each frame from the
/// `with_animation` wrapper (the frame pump) and paints accordingly.
pub struct ShimmerInner {
    shimmer: Shimmer,
    phase: f32,
}

/// Per-character shaped glyphs + their x-offsets, prepared in prepaint.
pub struct ShimmerPrepaint {
    /// (x-offset within bounds, the single-char string) for each character.
    chars: Vec<(Pixels, gpui::SharedString)>,
    /// Full label width / line height for layout + the pulsate path.
    line_height: Pixels,
}

/// Cross-frame element-state cache (P8): the per-char `(x, SharedString)`
/// layout is STABLE while busy — only the band centre moves — so it is built
/// once and reused, keyed by `(text, font_size)`. Without it, prepaint
/// (re-run every frame while the `.repeat()` animation pumps) rebuilt the Vec
/// and allocated one `SharedString` per char EVERY frame (~840 allocs/s for a
/// 14-char label at 60fps). Shaping itself is a line-layout cache HIT (the
/// cache key excludes colour), so only the per-frame recolour remains.
#[derive(Clone)]
struct ShimmerCharCache {
    text: gpui::SharedString,
    font_size: Pixels,
    chars: Vec<(Pixels, gpui::SharedString)>,
}

impl ShimmerCharCache {
    /// Whether this cache is still valid for `(text, font_size)` — a hit means
    /// prepaint reuses it (no per-frame string allocation); a miss rebuilds.
    /// The layout depends ONLY on text + size (colour is applied per-frame), so
    /// those two are the whole cache key.
    fn matches(&self, text: &gpui::SharedString, font_size: Pixels) -> bool {
        self.text == *text && self.font_size == font_size
    }
}

impl Element for ShimmerInner {
    type RequestLayoutState = ();
    type PrepaintState = ShimmerPrepaint;

    fn id(&self) -> Option<ElementId> {
        Some(self.shimmer.id.clone())
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let line = self.shimmer.run(self.shimmer.text.clone(), self.shimmer.base, window);
        let mut style = Style::default();
        style.size = size(line.width().into(), window.line_height().into());
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        window: &mut Window,
        _cx: &mut App,
    ) -> ShimmerPrepaint {
        let line_height = window.line_height();
        // The per-char (x, SharedString) layout is stable across frames (only
        // the band centre moves), so cache it in element state keyed by
        // (text, font_size) and reuse — only the per-frame recolour stays
        // (P8). `chars` clones are Arc bumps, not string allocations. Without a
        // stable id (defensive — the shimmer always has one), rebuild.
        let chars = match id {
            Some(id) => window.with_element_state::<ShimmerCharCache, _>(id, |cached, window| {
                let hit =
                    cached.filter(|c| c.matches(&self.shimmer.text, self.shimmer.font_size));
                let cache = hit.unwrap_or_else(|| self.shimmer.build_char_cache(window));
                (cache.chars.clone(), cache)
            }),
            None => self.shimmer.build_char_cache(window).chars,
        };
        ShimmerPrepaint { chars, line_height }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        prepaint: &mut ShimmerPrepaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        let ShimmerPrepaint { chars, line_height } = prepaint;
        let lh = *line_height;
        let origin = bounds.origin;
        match self.shimmer.mode {
            ShimmerMode::Pulsate => {
                // The fallback: one shaped line whose opacity pulses between
                // 0.4 and 0.92 (the standing `pulsating_between` behaviour).
                let alpha = 0.4 + 0.52 * pulse(self.phase);
                let color = self.shimmer.base.opacity(alpha);
                let line = self.shimmer.run(self.shimmer.text.clone(), color, window);
                line.paint(origin, lh, TextAlign::Left, None, window, cx).ok();
            }
            ShimmerMode::Sweep => {
                // The gradient text-sweep: a highlight band travels through
                // the glyphs. Each character is tinted by the band weight at
                // its centre x (in label-fractional space).
                let width = f32::from(bounds.size.width).max(1.);
                let center = sweep_center(self.phase);
                for (x, ch) in chars.iter() {
                    let frac = (f32::from(*x) / width).clamp(0., 1.); // glyph start as the sample point
                    let w = band_weight(frac, center);
                    let color = mix_hsla(self.shimmer.base, self.shimmer.highlight, w);
                    // `ch` is the cached single-char string — clone is an Arc
                    // bump, so the per-frame recolour allocates nothing.
                    let glyph = self.shimmer.run(ch.clone(), color, window);
                    glyph
                        .paint(point(origin.x + *x, origin.y), lh, TextAlign::Left, None, window, cx)
                        .ok();
                }
            }
        }
    }
}

/// Triangle pulse 0→1→0 over a 0→1 phase — the shape `pulsating_between`
/// traces (ease is cosmetic at these sizes; the endpoints are what matter).
fn pulse(phase: f32) -> f32 {
    1. - (phase * 2. - 1.).abs()
}

/// sRGB component mix (the same idiom as [`super::animated::mix`], inlined to
/// avoid a cross-module dep for one call).
fn mix_hsla(a: Hsla, b: Hsla, t: f32) -> Hsla {
    let (a, b) = (gpui::Rgba::from(a), gpui::Rgba::from(b));
    gpui::Rgba {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: a.a + (b.a - a.a) * t,
    }
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn band_peaks_at_center_and_vanishes_outside() {
        assert_eq!(band_weight(0.5, 0.5), 1., "centre is full highlight");
        assert!(band_weight(0.55, 0.5) > 0. && band_weight(0.55, 0.5) < 1.);
        assert_eq!(band_weight(0.7, 0.5), 0., "far edge is bare base colour");
        assert_eq!(band_weight(0.3, 0.5), 0., "near edge is bare base colour");
    }

    #[test]
    fn band_is_symmetric() {
        assert!((band_weight(0.45, 0.5) - band_weight(0.55, 0.5)).abs() < 1e-6);
    }

    #[test]
    fn sweep_traverses_the_whole_label_then_holds() {
        // Over-runs both edges so the highlight fully enters and exits.
        assert!(sweep_center(0.0) < 0., "starts off the left edge");
        assert!(sweep_center(0.4) > 1., "fully past the right edge by 40%");
        // The hold tail (web 40%→100% keyframe pause) stays clamped past 1.
        assert_eq!(
            sweep_center(0.4),
            sweep_center(1.0),
            "the band holds off-screen through the pause"
        );
    }

    #[test]
    fn pulse_is_a_clean_triangle() {
        assert_eq!(pulse(0.0), 0.);
        assert_eq!(pulse(0.5), 1.);
        assert_eq!(pulse(1.0), 0.);
    }

    #[test]
    fn mix_endpoints_are_exact() {
        let a: Hsla = gpui::Rgba { r: 1., g: 0., b: 0., a: 1. }.into();
        let b: Hsla = gpui::Rgba { r: 0., g: 0., b: 1., a: 1. }.into();
        let lo = gpui::Rgba::from(mix_hsla(a, b, 0.));
        let hi = gpui::Rgba::from(mix_hsla(a, b, 1.));
        assert!((lo.r - 1.).abs() < 1e-4 && lo.b < 1e-4);
        assert!((hi.b - 1.).abs() < 1e-4 && hi.r < 1e-4);
    }

    // --- P5: the landing-frame tint snap is bounded — for the majority of the
    // cycle the band is fully off the label, so a drop has NO tint to snap.
    // (The remaining mid-sweep landings are concurrent with the landed
    // content's own entrance fade — adversary col opacity 0→1 spring-in,
    // transcript reply view — which covers the drop. A true exit crossfade
    // would need consumer-side "finishing" state and is GUI-trace-gated; this
    // is NOT a §8.7 violation — a sub-glyph one-shot tint, not a geometry/
    // opacity snap. Pinned here: the all-base hold the disposition relies on.)

    #[test]
    fn band_is_fully_off_the_label_through_the_hold_tail() {
        // The sweep traverses in the first 40% of the clock, then holds the
        // band off the right edge for the remaining ~60%. Across that hold,
        // EVERY glyph position is at base weight (0) — so ~60% of landings
        // drop a label that is already all-base, with no tint to snap.
        const BAND_HALF: f32 = 0.1; // band reaches BAND_HALF past its centre
        for i in 0..=60 {
            let t = 0.4 + 0.6 * (i as f32 / 60.); // the hold range [0.4, 1.0]
            let center = sweep_center(t);
            assert!(
                center >= 1.0 + BAND_HALF,
                "during the hold the band must clear the right edge (t={t}, center={center})"
            );
            // Sample glyph positions across the whole label — all bare base.
            for j in 0..=20 {
                let x = j as f32 / 20.;
                assert_eq!(
                    band_weight(x, center),
                    0.,
                    "glyph at x={x} must be base during the hold (t={t})"
                );
            }
        }
    }

    // --- P8: the char-layout cache reuses across frames, rebuilding only on a
    // text/size change — this is the predicate that decides reuse-vs-realloc.

    #[test]
    fn char_cache_is_reused_for_the_same_text_and_size_and_rebuilt_otherwise() {
        let cache = ShimmerCharCache {
            text: gpui::SharedString::from("Orchestrating…"),
            font_size: gpui::px(13.),
            chars: Vec::new(),
        };
        // Same text + size → HIT: prepaint reuses the layout (no per-frame
        // SharedString allocation), which is the whole point of P8.
        assert!(
            cache.matches(&gpui::SharedString::from("Orchestrating…"), gpui::px(13.)),
            "identical text + size must reuse the cached layout"
        );
        // A changed label → MISS (the band still sweeps the SAME label every
        // frame, so this only fires on a real text change, e.g. busy→landed).
        assert!(
            !cache.matches(&gpui::SharedString::from("Thinking…"), gpui::px(13.)),
            "a different label must rebuild the layout"
        );
        // A changed size → MISS (x-advances depend on font size).
        assert!(
            !cache.matches(&gpui::SharedString::from("Orchestrating…"), gpui::px(15.)),
            "a different font size must rebuild the layout"
        );
    }
}
