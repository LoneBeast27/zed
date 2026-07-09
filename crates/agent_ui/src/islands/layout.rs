//! Pure corner-cluster anchor geometry (PARITY_SPEC §4.8/§4.9, Finding 1).
//!
//! The §4.9 island law: the head is ANCHORED top-right of the content area and
//! the card grows DOWN out of that anchor. This module is the anchor math with
//! no gpui/window dependency, so the "the pill never intersects the tab-bar
//! row" invariant is CPU-testable at any width and any UI-font scale — the
//! render in [`super::corner_cluster`] only feeds it the measured tab-bar row
//! height and paints the result.

/// The resolved corner-cluster geometry: where the head pill anchors and where
/// the toast stack begins, both in the workspace content div's local space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClusterAnchor {
    /// Top inset of the island head — measured from the content div's top,
    /// which is the top of the center pane (and thus of the tab-bar row).
    pub head_top: f32,
    /// Top inset of the toast stack (below the head).
    pub stack_top: f32,
    /// Right inset shared by the head and the stack (right-anchored).
    pub right: f32,
}

/// Clearance for a right-dock panel HEADER row (uniformity audit 2026-07-06):
/// the pill is right-anchored over the workspace, and when a right dock is
/// open its header ("Constellation" + arrange button, ~56px of title row) is
/// TALLER than the tab bar — the pill was landing on the dock's header
/// buttons (audit-2's buried Graph button). The anchor now clears both rows.
pub const DOCK_HEADER_CLEARANCE: f32 = 44.;

/// Fold the tab-bar row height + insets into the cluster anchor.
///
/// The head drops below the tab-bar row (`tab_bar_h`), the dock-header
/// clearance, and the corner inset, so the right-anchored pill clears BOTH
/// the tab bar's right-corner `+` / split buttons AND an open right dock's
/// header row at ANY window width and any UI-font zoom (`tab_bar_h` scales
/// with the same spacing token the tab bar uses). The stack sits one
/// pill-plus-gap below the head. When the tab bar is hidden the caller passes
/// `tab_bar_h == 0.` and the head anchors at clearance + inset.
pub fn cluster_anchor(tab_bar_h: f32, corner_inset: f32, stack_gap: f32) -> ClusterAnchor {
    // Negative/NaN heights can't push the pill UP into the chrome.
    let tab_bar_h = if tab_bar_h.is_finite() {
        tab_bar_h.max(0.)
    } else {
        0.
    };
    let head_top = tab_bar_h + DOCK_HEADER_CLEARANCE + corner_inset;
    ClusterAnchor {
        head_top,
        stack_top: head_top + stack_gap,
        right: corner_inset,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A representative tab-bar row height range: Base32 is ~30px at the
    /// smallest UI font and grows with zoom. The anchor must clear the row at
    /// every size.
    const TAB_ROWS: [f32; 5] = [26., 30., 32., 40., 56.];
    const INSET: f32 = 14.;
    const GAP: f32 = 42.;

    #[test]
    fn head_never_intersects_the_tab_bar_row_at_any_scale() {
        for &tab_bar_h in &TAB_ROWS {
            let anchor = cluster_anchor(tab_bar_h, INSET, GAP);
            assert!(
                anchor.head_top >= tab_bar_h,
                "head top {} must clear the {tab_bar_h}px tab-bar row so the \
                 pill never overlaps the corner buttons",
                anchor.head_top
            );
            // And it must clear the dock header + corner inset, not just touch.
            assert_eq!(anchor.head_top, tab_bar_h + DOCK_HEADER_CLEARANCE + INSET);
        }
    }

    #[test]
    fn geometry_is_width_independent() {
        // The anchor is a function of the tab-bar height + insets ONLY — no
        // width term — so the same clearance holds at the narrowest window.
        // (The render right-anchors both head and stack; nothing here depends
        // on viewport width, which is exactly why narrow widths are safe.)
        let a = cluster_anchor(32., INSET, GAP);
        let b = cluster_anchor(32., INSET, GAP);
        assert_eq!(a, b, "anchor must not vary — it takes no width input");
        assert_eq!(a.right, INSET, "right-pinned at the corner inset");
    }

    #[test]
    fn stack_sits_below_the_head() {
        let anchor = cluster_anchor(32., INSET, GAP);
        assert_eq!(anchor.stack_top, anchor.head_top + GAP);
        assert!(
            anchor.stack_top > anchor.head_top,
            "the toast stack starts below the head, never overlapping it"
        );
    }

    #[test]
    fn hidden_tab_bar_still_clears_the_dock_header() {
        // No tab bar → the head anchors at clearance + inset (the dock-header
        // clearance is unconditional — a right dock's header row exists
        // whether or not the center pane shows a tab bar). NOTE: the corner
        // cluster is retired (2026-07-08, composer island home) — this module
        // is the reference implementation; the pin tracks its actual math.
        let anchor = cluster_anchor(0., INSET, GAP);
        assert_eq!(anchor.head_top, DOCK_HEADER_CLEARANCE + INSET);
        assert_eq!(anchor.stack_top, DOCK_HEADER_CLEARANCE + INSET + GAP);
    }

    #[test]
    fn nonfinite_or_negative_height_cannot_push_the_pill_into_chrome() {
        // A degenerate measurement must never lift the head above the corner
        // inset (which would re-introduce the overlap).
        for bad in [f32::NAN, f32::INFINITY, -50.] {
            let anchor = cluster_anchor(bad, INSET, GAP);
            assert!(
                anchor.head_top >= INSET,
                "degenerate tab height {bad} produced head_top {} — must not \
                 sink below the corner inset",
                anchor.head_top
            );
        }
    }
}
