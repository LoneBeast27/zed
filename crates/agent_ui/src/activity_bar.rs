//! Workspace-modes activity bar (M1 slice).
//!
//! Spec: `.planning/zed-fork/WORKSPACE_MODES.md` §3 + §7. Renders a fixed
//! 48 px-wide vertical rail at the left edge of the workspace with one icon
//! per mode loaded from `<workspace_root>/.agents/modes/*.json` (the M0
//! substrate at `workspace_modes::load_modes_from_dir`).
//!
//! Scope for M1 (+M2):
//!   - Enumerate modes via the existing loader.
//!   - Render the bar (active mode tinted, inactive muted, hover bg, tooltip).
//!   - On click, switch the workspace to that mode — apply its dock layout via
//!     `workspace_mode_switcher::switch_to_mode` (M2).
//!
//! Gated on `agent.workspace_modes` — when false the bar is never instantiated
//! and the workspace render is byte-identical to stock Zed.

use std::path::PathBuf;
use std::str::FromStr;

use gpui::{
    AnyElement, Context, Entity, Hsla, IntoElement, MouseButton, ParentElement, Rgba,
    SharedString, Styled, WeakEntity, Window, div, prelude::FluentBuilder, rgb,
};
use ui::{Color, Icon, IconName, IconSize, Tooltip, h_flex, prelude::*, v_flex};
use workspace::Workspace;

use crate::workspace_mode_switcher;
use crate::workspace_modes::{PinnedPosition, WorkspaceMode, load_modes_from_dir};

/// Fixed width of the activity bar in pixels. Mirrors VSCode (48 px) — see
/// `WORKSPACE_MODES.md` §3.
pub const ACTIVITY_BAR_WIDTH_PX: f32 = 48.0;

/// Width of the active-mode accent stripe drawn at the left edge of the
/// selected icon (mirrors VSCode's active-tab indicator).
const ACTIVE_STRIPE_WIDTH_PX: f32 = 3.0;

/// Default icon used when a mode's `icon` string does not match a known
/// `IconName` variant. `ZedAgent` is the closest in-tree analog to the
/// spec's "Hub" icon.
const FALLBACK_ICON: IconName = IconName::ZedAgent;

/// Entity backing the left-edge activity bar.
pub struct ActivityBar {
    /// All modes loaded from disk at construction. Order is the loader's
    /// canonical sort (Top-pinned, then alphabetical, then Bottom-pinned).
    modes: Vec<WorkspaceMode>,
    /// `id` of the currently active mode. May be empty when no mode matches
    /// the configured default — in that case the first non-bottom-pinned
    /// mode is highlighted as a soft fallback.
    active_mode_id: String,
    /// Weak handle to the hosting workspace, used by the M2 switcher to apply
    /// a mode's dock layout on click. `None` in unit tests that exercise the
    /// bar without a workspace — clicks then only update the highlight.
    workspace: Option<WeakEntity<Workspace>>,
}

impl ActivityBar {
    /// Construct an `ActivityBar` Entity anchored at `modes_dir`. `default_mode`
    /// selects which icon renders as active on first paint. The constructor
    /// reads the filesystem once — hot-reload on file change is M-future.
    pub fn new(
        modes_dir: PathBuf,
        default_mode: impl Into<String>,
        workspace: Option<WeakEntity<Workspace>>,
        _cx: &mut Context<Self>,
    ) -> Self {
        let modes = load_modes_from_dir(&modes_dir);
        let active_mode_id = default_mode.into();
        Self {
            modes,
            active_mode_id,
            workspace,
        }
    }

    pub fn modes(&self) -> &[WorkspaceMode] {
        &self.modes
    }

    pub fn active_mode_id(&self) -> &str {
        &self.active_mode_id
    }

    /// Set the active mode id and request a redraw. M1 uses this for
    /// click-to-highlight; M2 will pair this with `apply_layout`.
    pub fn set_active(&mut self, mode_id: impl Into<String>, cx: &mut Context<Self>) {
        let new_id = mode_id.into();
        if new_id != self.active_mode_id {
            self.active_mode_id = new_id;
            cx.notify();
        }
    }

    /// Returns the effective active mode id — the explicitly-set id if it
    /// matches a loaded mode, otherwise the first non-`Bottom`-pinned mode.
    /// Used both for rendering and for the M2 layout-apply dispatch.
    fn effective_active(&self) -> Option<&WorkspaceMode> {
        if let Some(m) = self.modes.iter().find(|m| m.id == self.active_mode_id) {
            return Some(m);
        }
        self.modes
            .iter()
            .find(|m| m.pinned_position != Some(PinnedPosition::Bottom))
    }
}

/// Map a mode's `icon` JSON field to an `IconName` variant.
///
/// The `IconName` enum is `EnumString` with `serialize_all = "snake_case"`,
/// so we lowercase + camel-to-snake the input before lookup. Unknown icons
/// fall back to `FALLBACK_ICON` (logged once at WARN level).
fn icon_name_for(spec: &str) -> IconName {
    let snake = camel_to_snake(spec);
    IconName::from_str(&snake).unwrap_or_else(|_| {
        log::warn!(
            "activity_bar: icon '{}' (snake='{}') not a known IconName — using fallback",
            spec,
            snake
        );
        FALLBACK_ICON
    })
}

/// Convert a CamelCase or PascalCase string to snake_case. Idempotent on
/// already-snake input ("Hub" → "hub", "AiOpenAi" → "ai_open_ai", "settings"
/// → "settings").
fn camel_to_snake(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    for (i, ch) in s.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if i > 0 && !out.ends_with('_') {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

/// Parse a 6-digit hex color (no leading `#`) into an `Hsla`. Falls back to
/// Apple system gray (`#8E8E93`) on parse failure.
fn parse_accent_hex(hex: &str) -> Hsla {
    let trimmed = hex.trim_start_matches('#');
    match u32::from_str_radix(trimmed, 16) {
        Ok(rgb_val) if trimmed.len() == 6 => Rgba::from(rgb(rgb_val)).into(),
        _ => Rgba::from(rgb(0x8E8E93)).into(),
    }
}

impl gpui::Render for ActivityBar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme_colors = cx.theme().colors().clone();

        // Split modes into top/middle and bottom-pinned. Bottom-pinned modes
        // render at the foot of the bar with a flex spacer between them and
        // the middle group (per the §3 ASCII mockup — Settings is at bottom).
        let (bottom, top_middle): (Vec<&WorkspaceMode>, Vec<&WorkspaceMode>) = self
            .modes
            .iter()
            .partition(|m| m.pinned_position == Some(PinnedPosition::Bottom));

        let active_id = self
            .effective_active()
            .map(|m| m.id.clone())
            .unwrap_or_default();

        let mut top_children: Vec<AnyElement> = Vec::with_capacity(top_middle.len());
        for mode in &top_middle {
            top_children
                .push(render_mode_icon(mode, &active_id, &theme_colors, cx).into_any_element());
        }
        let mut bottom_children: Vec<AnyElement> = Vec::with_capacity(bottom.len());
        for mode in &bottom {
            bottom_children
                .push(render_mode_icon(mode, &active_id, &theme_colors, cx).into_any_element());
        }

        v_flex()
            .id("activity-bar")
            .w(gpui::px(ACTIVITY_BAR_WIDTH_PX))
            .h_full()
            .flex_none()
            .bg(theme_colors.panel_background)
            .border_r_1()
            .border_color(theme_colors.border)
            .py_1()
            .gap_0p5()
            .child(v_flex().gap_0p5().children(top_children))
            .child(div().flex_1())
            .child(v_flex().gap_0p5().children(bottom_children))
    }
}

/// Free fn so the `render` loop can call it once per mode without re-borrowing
/// `self` — keeps the closure-capture chain simple.
fn render_mode_icon(
    mode: &WorkspaceMode,
    active_id: &str,
    theme_colors: &theme::ThemeColors,
    cx: &mut Context<ActivityBar>,
) -> impl IntoElement {
    let is_active = mode.id == active_id;
    let accent = parse_accent_hex(&mode.accent_color_hex);
    let icon = icon_name_for(&mode.icon);

    let icon_color = if is_active {
        Color::Custom(accent)
    } else {
        Color::Muted
    };

    let tooltip_text = match &mode.default_keybinding {
        Some(kb) => format!("{} ({})", mode.display_name, kb),
        None => mode.display_name.clone(),
    };

    let mode_id_for_click = mode.id.clone();
    let element_id =
        SharedString::from(format!("activity-bar-mode-{}", mode.id.as_str()));

    // Left-edge accent stripe rendered absolutely. Only shown when active.
    let stripe = if is_active {
        Some(
            div()
                .absolute()
                .left_0()
                .top_1()
                .bottom_1()
                .w(gpui::px(ACTIVE_STRIPE_WIDTH_PX))
                .rounded_r_sm()
                .bg(accent),
        )
    } else {
        None
    };

    let hover_bg = theme_colors.element_hover;
    let active_bg = theme_colors.element_selected;

    h_flex()
        .id(element_id)
        .relative()
        .w_full()
        .h(gpui::px(40.0))
        .justify_center()
        .items_center()
        .hover(move |s| s.bg(hover_bg))
        .when(is_active, move |this| this.bg(active_bg))
        .children(stripe)
        .child(Icon::new(icon).size(IconSize::Medium).color(icon_color))
        .tooltip(move |_window, cx| Tooltip::simple(tooltip_text.clone(), cx))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _ev, window, cx| {
                this.set_active(mode_id_for_click.clone(), cx);
                // M2: apply the mode's dock layout on the hosting workspace.
                let Some(mode) = this
                    .modes
                    .iter()
                    .find(|m| m.id == mode_id_for_click)
                    .cloned()
                else {
                    return;
                };
                let Some(workspace) = this.workspace.as_ref().and_then(|w| w.upgrade()) else {
                    log::warn!(
                        "activity_bar: no workspace handle — cannot apply mode '{}'",
                        mode.id
                    );
                    return;
                };
                workspace.update(cx, |workspace, cx| {
                    workspace_mode_switcher::switch_to_mode(&mode, workspace, window, cx);
                });
            }),
        )
}

/// Convenience constructor mirroring `resource_banner::build_gpu_banner` —
/// builds an `ActivityBar` Entity anchored at `<workspace_root>/.agents/modes/`
/// unless `modes_dir_override` is supplied (settings escape hatch).
pub fn build_activity_bar(
    workspace_root: PathBuf,
    modes_dir_override: Option<PathBuf>,
    default_mode: impl Into<String>,
    workspace: WeakEntity<Workspace>,
    cx: &mut gpui::App,
) -> Entity<ActivityBar> {
    let modes_dir = modes_dir_override.unwrap_or_else(|| workspace_root.join(".agents/modes"));
    let default_mode = default_mode.into();
    cx.new(|cx| ActivityBar::new(modes_dir, default_mode, Some(workspace), cx))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_mode(dir: &std::path::Path, id: &str, icon: &str, pin: Option<&str>) {
        std::fs::create_dir_all(dir).unwrap();
        let pin_json = match pin {
            Some(p) => format!(", \"pinned_position\": \"{}\"", p),
            None => String::new(),
        };
        let json = format!(
            r#"{{
                "schema_version": 1,
                "id": "{id}",
                "display_name": "{id}",
                "description": "test mode",
                "icon": "{icon}",
                "accent_color_hex": "DA7756",
                "layout": {{}},
                "default_keybinding": "Ctrl+Alt+1"
                {pin_json}
            }}"#,
            id = id,
            icon = icon,
            pin_json = pin_json,
        );
        std::fs::write(dir.join(format!("{id}.json")), json).unwrap();
    }

    #[test]
    fn camel_to_snake_handles_known_cases() {
        assert_eq!(camel_to_snake("Hub"), "hub");
        assert_eq!(camel_to_snake("Settings"), "settings");
        assert_eq!(camel_to_snake("ZedAgent"), "zed_agent");
        assert_eq!(camel_to_snake("AiOpenAi"), "ai_open_ai");
        // Already-snake input is preserved.
        assert_eq!(camel_to_snake("zed_agent"), "zed_agent");
    }

    #[test]
    fn icon_name_for_falls_back_on_unknown() {
        // "Hub" is not a real IconName variant in the current Zed tree, but
        // the activity bar must not panic — it falls back to FALLBACK_ICON.
        let icon = icon_name_for("Hub");
        assert_eq!(icon, FALLBACK_ICON);
        // "Settings" exists as a real variant — should resolve to it.
        assert_eq!(icon_name_for("Settings"), IconName::Settings);
        // Snake-cased input also works (forward-compat for hand-authored
        // mode files that already used snake_case).
        assert_eq!(icon_name_for("zed_agent"), IconName::ZedAgent);
    }

    #[test]
    fn parse_accent_hex_handles_with_and_without_hash() {
        let a: Rgba = parse_accent_hex("DA7756").into();
        let b: Rgba = parse_accent_hex("#DA7756").into();
        // Both representations resolve to the same color.
        assert_eq!(a.r, b.r);
        assert_eq!(a.g, b.g);
        assert_eq!(a.b, b.b);
    }

    #[test]
    fn parse_accent_hex_falls_back_on_garbage() {
        // Garbage input → falls back to system gray, no panic.
        let gray: Rgba = parse_accent_hex("not-a-color").into();
        let expected: Rgba = parse_accent_hex("8E8E93").into();
        assert_eq!(gray.r, expected.r);
        assert_eq!(gray.g, expected.g);
        assert_eq!(gray.b, expected.b);
    }

    #[test]
    fn mode_enumeration_from_temp_dir_sorts_correctly() {
        // Verify the M0 loader's sort contract is preserved end-to-end:
        // pinned-top first, then unpinned alphabetical, then pinned-bottom.
        let temp = TempDir::new().unwrap();
        let modes_dir = temp.path().join(".agents").join("modes");
        write_mode(&modes_dir, "zeta", "Hub", Some("bottom"));
        write_mode(&modes_dir, "alpha", "Hub", Some("top"));
        write_mode(&modes_dir, "beta", "Hub", None);

        let modes = load_modes_from_dir(&modes_dir);
        let ids: Vec<&str> = modes.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["alpha", "beta", "zeta"]);
    }

    #[test]
    fn empty_modes_dir_yields_no_modes() {
        // Empty (but existing) dir → empty list, no crash.
        let temp = TempDir::new().unwrap();
        let modes_dir = temp.path().join(".agents").join("modes");
        std::fs::create_dir_all(&modes_dir).unwrap();
        let modes = load_modes_from_dir(&modes_dir);
        assert!(modes.is_empty());
    }

    #[test]
    fn missing_modes_dir_yields_no_modes() {
        // Nonexistent dir → empty list, logged warning, no crash.
        let temp = TempDir::new().unwrap();
        let modes_dir = temp.path().join(".agents").join("does-not-exist");
        let modes = load_modes_from_dir(&modes_dir);
        assert!(modes.is_empty());
    }

    #[gpui::test]
    fn effective_active_falls_back_when_default_mode_missing(cx: &mut gpui::TestAppContext) {
        let temp = TempDir::new().unwrap();
        let modes_dir = temp.path().join(".agents").join("modes");
        write_mode(&modes_dir, "orchestrator", "Hub", None);
        write_mode(&modes_dir, "settings", "Settings", Some("bottom"));

        cx.update(|cx| {
            let entity = cx.new(|cx| {
                ActivityBar::new(modes_dir.clone(), "nonexistent-mode".to_string(), None, cx)
            });
            entity.update(cx, |bar, _cx| {
                // No mode matches "nonexistent-mode", so the bar falls back
                // to the first non-bottom-pinned mode (orchestrator).
                let active = bar.effective_active();
                assert!(active.is_some());
                assert_eq!(active.unwrap().id, "orchestrator");
            });
        });
    }

    #[gpui::test]
    fn set_active_updates_and_notifies(cx: &mut gpui::TestAppContext) {
        let temp = TempDir::new().unwrap();
        let modes_dir = temp.path().join(".agents").join("modes");
        write_mode(&modes_dir, "orchestrator", "Hub", None);
        write_mode(&modes_dir, "symphony", "AiOpenAi", None);

        cx.update(|cx| {
            let entity = cx.new(|cx| {
                ActivityBar::new(modes_dir.clone(), "orchestrator".to_string(), None, cx)
            });
            entity.update(cx, |bar, cx| {
                assert_eq!(bar.active_mode_id(), "orchestrator");
                bar.set_active("symphony", cx);
                assert_eq!(bar.active_mode_id(), "symphony");
                // effective_active reflects the new id.
                assert_eq!(bar.effective_active().unwrap().id, "symphony");
            });
        });
    }
}
