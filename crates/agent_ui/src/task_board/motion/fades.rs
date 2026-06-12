//! Tracked hover/focus crossfades, one map for a whole panel — the web's
//! `transition: … var(--effects-curve)` interactive-state class
//! (PARITY_SPEC §0 "state changes 150–250ms", "hovers are gentle fades").
//!
//! Render idiom (the §8.7a stable-identity rule): sites read
//! [`StateFades::t`] per frame and mix their rest/engaged styles directly —
//! no per-site animation wrappers, no element-identity churn. The owning
//! panel arms ONE `window.request_animation_frame()` off
//! [`StateFades::any_animating`] at the end of its render; settled frames
//! schedule nothing (§8 idle cost), and a mid-fade flip re-bases from the
//! interpolated current value ([`AnimatedValue`] retarget law — never the
//! opposite endpoint, the Z2 #4 endpoint-flash class).

use std::collections::HashMap;
use std::time::Duration;

use gpui::ElementId;

use super::animated::AnimatedValue;
use super::curves::EFFECTS;

/// Per-element engaged-state crossfades, keyed by the element's own id.
#[derive(Default)]
pub struct StateFades {
    fades: HashMap<ElementId, AnimatedValue>,
}

impl StateFades {
    pub fn new() -> Self {
        Self::default()
    }

    /// Flip `id` toward engaged (hover/focus) or rest on the effects curve.
    /// `duration` is the site's CSS transition time (.15s chat chrome, .12s
    /// island chrome). Same-state flips are no-ops; returns whether a
    /// repaint is needed.
    pub fn set(&mut self, id: impl Into<ElementId>, engaged: bool, duration: Duration) -> bool {
        let id = id.into();
        let target = if engaged { 1. } else { 0. };
        match self.fades.get_mut(&id) {
            Some(fade) if fade.target() == target => false,
            Some(fade) => {
                fade.retarget(target);
                self.prune();
                true
            }
            // Never hovered and going to rest: nothing to allocate.
            None if !engaged => false,
            None => {
                let mut fade = AnimatedValue::settled(0., EFFECTS, duration);
                fade.retarget(1.);
                self.fades.insert(id, fade);
                true
            }
        }
    }

    /// The crossfade scalar for an element right now: 0 = rest,
    /// 1 = engaged, in-between mid-fade. Unknown ids are at rest.
    pub fn t(&self, id: &ElementId) -> f32 {
        self.fades
            .get(id)
            .map_or(0., |fade| fade.current().clamp(0., 1.))
    }

    /// Whether any crossfade is mid-flight — the owner's frame-pump signal.
    pub fn any_animating(&self) -> bool {
        self.fades.values().any(AnimatedValue::animating)
    }

    /// Drop entries settled back at rest so the map tracks only the
    /// engaged/fading set (bounded by what the pointer can touch).
    fn prune(&mut self) {
        self.fades
            .retain(|_, fade| fade.target() > 0.5 || fade.animating());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const D: Duration = Duration::from_millis(150);

    fn id(name: &'static str) -> ElementId {
        ElementId::Name(name.into())
    }

    #[test]
    fn flip_engages_fades_and_settles() {
        let mut fades = StateFades::new();
        assert_eq!(fades.t(&id("a")), 0., "untouched elements are at rest");
        assert!(!fades.any_animating());

        assert!(fades.set(id("a"), true, D), "a flip needs a repaint");
        assert!(fades.any_animating(), "the crossfade window is open");
        assert!(!fades.set(id("a"), true, D), "same-state flips are no-ops");

        std::thread::sleep(Duration::from_millis(160));
        assert_eq!(fades.t(&id("a")), 1., "settled engaged renders at 1");
        assert!(!fades.any_animating(), "settled fades pump nothing");
    }

    #[test]
    fn concurrent_elements_fade_independently() {
        // The single-slot drop (Z3 #5 secondary): sweeping across elements
        // must fade each departed one concurrently, like per-element CSS
        // transitions.
        let mut fades = StateFades::new();
        fades.set(id("a"), true, D);
        std::thread::sleep(Duration::from_millis(160));
        fades.set(id("a"), false, D);
        fades.set(id("b"), true, D);
        let (a, b) = (fades.t(&id("a")), fades.t(&id("b")));
        assert!(a > 0.0 || b < 1.0, "at least one mid-fade right after the sweep");
        assert!(
            fades.fades.contains_key(&id("a")) && fades.fades.contains_key(&id("b")),
            "both elements tracked concurrently"
        );
    }

    #[test]
    fn mid_fade_flip_rebases_from_current_not_the_opposite_endpoint() {
        let mut fades = StateFades::new();
        fades.set(id("a"), true, D);
        std::thread::sleep(Duration::from_millis(40));
        let mid = fades.t(&id("a"));
        assert!(mid > 0. && mid < 1., "mid-fade, got {mid}");
        fades.set(id("a"), false, D);
        let rebased = fades.t(&id("a"));
        assert!(
            (rebased - mid).abs() < 0.25,
            "the fade-out must continue from ~{mid}, got {rebased} (endpoint flash)"
        );
    }

    #[test]
    fn rest_settled_entries_are_pruned() {
        let mut fades = StateFades::new();
        assert!(!fades.set(id("a"), false, D), "rest→rest allocates nothing");
        assert!(fades.fades.is_empty());

        fades.set(id("a"), true, D);
        fades.set(id("a"), false, D);
        std::thread::sleep(Duration::from_millis(160));
        // The next flip anywhere prunes the settled-at-rest entry.
        fades.set(id("b"), true, D);
        fades.set(id("b"), false, D);
        assert!(
            !fades.fades.contains_key(&id("a")),
            "settled-at-rest entries must not accumulate"
        );
    }
}
