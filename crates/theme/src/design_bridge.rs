//! The override bridge — forces the fork's [`crate::design`] palette onto the
//! CHROME of any resolved theme, at the single `configured_theme` choke point
//! (theme_settings). This is the "if Zed has it, we own it" lock: no matter
//! which theme JSON is selected, every panel, tab, title bar, sidebar, button,
//! and border re-flows to our tokens — so nothing stands out un-retheme'd.
//!
//! Deliberately scoped to CHROME. Syntax highlighting, diff hunks, terminal
//! ANSI, and player colors are left to the base theme (code must still read
//! well; that is a separate, later pass). Editing a token in [`crate::design`]
//! re-flows all of this on the next theme reload.

use gpui::Hsla;

use crate::Theme;
use crate::design::color as c;

#[inline]
fn h(rgba: gpui::Rgba) -> Hsla {
    rgba.into()
}

/// Overwrite `theme`'s chrome colors + semantic status with the design tokens.
/// Called at the tail of `configured_theme` on the cloned active theme.
pub fn apply_canonical(theme: &mut Theme) {
    let col = &mut theme.styles.colors;

    // ── Backgrounds (base for the shell, raised for panels/tabs/toolbar) ──
    col.background = h(c::SURFACE_BASE);
    col.surface_background = h(c::SURFACE_BASE);
    col.elevated_surface_background = h(c::SURFACE_RAISED);
    col.panel_background = h(c::SURFACE_RAISED);
    col.status_bar_background = h(c::SURFACE_BASE);
    col.title_bar_background = h(c::SURFACE_BASE);
    col.title_bar_inactive_background = h(c::SURFACE_BASE);
    col.toolbar_background = h(c::SURFACE_RAISED);
    col.tab_bar_background = h(c::SURFACE_BASE);
    col.tab_inactive_background = h(c::SURFACE_BASE);
    col.tab_active_background = h(c::SURFACE_RAISED);
    col.editor_background = h(c::SURFACE_BASE);
    col.editor_gutter_background = h(c::SURFACE_BASE);
    col.terminal_background = h(c::SURFACE_BASE);

    // ── Interactive element states ──
    col.element_background = h(c::SURFACE_RAISED);
    col.element_hover = h(c::HOVER);
    col.element_active = h(c::SURFACE_OVERLAY);
    col.element_selected = h(c::SURFACE_OVERLAY);
    col.ghost_element_hover = h(c::HOVER);
    col.ghost_element_active = h(c::SURFACE_OVERLAY);
    col.ghost_element_selected = h(c::SURFACE_OVERLAY);

    // ── Borders / hairlines ──
    col.border = h(c::BORDER);
    col.border_variant = h(c::BORDER_SUBTLE);
    col.border_focused = h(c::BORDER_FOCUS);
    col.border_selected = h(c::BORDER_FOCUS);
    col.border_disabled = h(c::BORDER_SUBTLE);

    // ── Text ramp ──
    col.text = h(c::TEXT);
    col.text_muted = h(c::TEXT_MUTED);
    col.text_placeholder = h(c::TEXT_PLACEHOLDER);
    col.text_disabled = h(c::TEXT_DISABLED);
    col.text_accent = h(c::TEXT_ACCENT);

    // ── Icons ──
    col.icon = h(c::TEXT);
    col.icon_muted = h(c::TEXT_MUTED);
    col.icon_disabled = h(c::TEXT_DISABLED);
    col.icon_placeholder = h(c::TEXT_PLACEHOLDER);
    col.icon_accent = h(c::ACCENT);

    // ── Single-accent chrome ──
    col.pane_focused_border = h(c::BORDER_FOCUS);
    col.panel_focused_border = h(c::BORDER_FOCUS);
    col.link_text_hover = h(c::ACCENT);

    // ── Scrollbars → hairline tones ──
    col.scrollbar_thumb_hover_background = h(c::HAIRLINE_HI);
    col.scrollbar_thumb_active_background = h(c::HAIRLINE_HI);
    col.scrollbar_thumb_border = h(c::HAIRLINE);
    col.scrollbar_track_border = h(c::HAIRLINE);

    // ── Semantic status (foreground + border; backgrounds derive elsewhere) ──
    let s = &mut theme.styles.status;
    s.error = h(c::STATUS_ERROR);
    s.error_border = h(c::STATUS_ERROR);
    s.warning = h(c::STATUS_WARNING);
    s.warning_border = h(c::STATUS_WARNING);
    s.success = h(c::STATUS_SUCCESS);
    s.success_border = h(c::STATUS_SUCCESS);
    s.info = h(c::STATUS_INFO);
    s.info_border = h(c::STATUS_INFO);
    s.hint = h(c::ACCENT);
    s.hint_border = h(c::ACCENT);
}
