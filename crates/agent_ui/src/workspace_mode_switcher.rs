//! Workspace-modes switcher (M2 slice).
//!
//! Spec: `.planning/zed-fork/WORKSPACE_MODES.md` §5 + §7 (M2), mode set per
//! `PRD_V2.md` §6. Applies a mode's `layout` spec to the workspace docks:
//! which panel is visible in the left/right/bottom dock and at what size.
//!
//! Scope for M2 (minimal viable path — documented limitation):
//!   - Panels are opened **in the dock they currently live in**. Cross-dock
//!     moves are NOT performed: Zed panels derive their dock position from
//!     settings (`Panel::position` reads e.g. `agent.dock`), and `set_position`
//!     round-trips through an async settings-file write. Moving a panel as
//!     part of a mode switch would mutate the user's settings.json — too
//!     invasive for M2. A layout key whose panel lives elsewhere opens that
//!     panel where it is and logs the divergence.
//!   - `center_dock` specs are ignored (the center is the editor pane, not a
//!     dock) — logged at debug level.
//!   - Unknown panel names are skipped gracefully with a log line.
//!   - Modes with `open_url` set open that URL in the system default browser.
//!
//! Everything here is reachable only when the `agent.workspace_modes` setting
//! is enabled: the activity bar (click path) is only installed then, and the
//! `Ctrl+Alt+1..9` keymap asset is only loaded then (`zed::load_default_keymap`).

use gpui::{Action, Window, px};
use schemars::JsonSchema;
use serde::Deserialize;
use workspace::Workspace;
use workspace::dock::DockPosition;

use crate::activity_bar::ActivityBar;
use crate::workspace_modes::WorkspaceMode;

/// Switches the workspace to the mode at the given activity-bar index
/// (zero-based, in the bar's display order). Bound to `Ctrl+Alt+1..9` when
/// `agent.workspace_modes` is enabled.
#[derive(Clone, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = agent)]
#[serde(deny_unknown_fields)]
pub struct SwitchWorkspaceMode {
    /// Zero-based index into the activity bar's mode list.
    pub mode_index: usize,
}

/// Layout keys that map onto real workspace docks, in apply order.
const DOCK_KEYS: [(&str, DockPosition); 3] = [
    ("left_dock", DockPosition::Left),
    ("right_dock", DockPosition::Right),
    ("bottom_dock", DockPosition::Bottom),
];

/// All dock positions, for host-dock lookup and the close pass.
const ALL_DOCKS: [DockPosition; 3] = [
    DockPosition::Left,
    DockPosition::Right,
    DockPosition::Bottom,
];

/// Handle the `SwitchWorkspaceMode` action: resolve the mode at `mode_index`
/// from the workspace's activity bar, mark it active, and apply its layout.
pub fn handle_switch_mode_action(
    workspace: &mut Workspace,
    mode_index: usize,
    window: &mut Window,
    cx: &mut gpui::Context<Workspace>,
) {
    let Some(bar) = workspace
        .activity_bar_item()
        .and_then(|view| view.downcast::<ActivityBar>().ok())
    else {
        // No activity bar installed (flag off, or no modes dir) — nothing to do.
        return;
    };
    let Some(mode) = bar.read(cx).modes().get(mode_index).cloned() else {
        log::info!(
            "workspace_modes: no mode at activity-bar index {} — ignoring switch",
            mode_index
        );
        return;
    };
    bar.update(cx, |bar, cx| bar.set_active(mode.id.clone(), cx));
    switch_to_mode(&mode, workspace, window, cx);
}

/// Switch the workspace to the mode with the given id, if the activity bar
/// carries one (Z2: the usage island's expanded-card rows route to the
/// `usage` mode — the native `location.hash = "#/usage"`). Returns `false`
/// when no activity bar is installed or no such mode exists, so callers can
/// fall back to focusing a panel directly.
pub fn switch_to_mode_id(
    id: &str,
    workspace: &mut Workspace,
    window: &mut Window,
    cx: &mut gpui::Context<Workspace>,
) -> bool {
    let Some(bar) = workspace
        .activity_bar_item()
        .and_then(|view| view.downcast::<ActivityBar>().ok())
    else {
        return false;
    };
    let Some(mode) = bar
        .read(cx)
        .modes()
        .iter()
        .find(|mode| mode.id == id)
        .cloned()
    else {
        return false;
    };
    bar.update(cx, |bar, cx| bar.set_active(mode.id.clone(), cx));
    switch_to_mode(&mode, workspace, window, cx);
    true
}

/// Switch the workspace into `mode`: apply its dock layout, then honor any
/// `open_url` side effect (system default browser).
pub fn switch_to_mode(
    mode: &WorkspaceMode,
    workspace: &mut Workspace,
    window: &mut Window,
    cx: &mut gpui::Context<Workspace>,
) {
    log::info!("workspace_modes: switching to mode '{}'", mode.id);
    apply_mode_layout(mode, workspace, window, cx);
    if let Some(url) = &mode.open_url {
        log::info!("workspace_modes: mode '{}' opens url {}", mode.id, url);
        cx.open_url(url);
    }
}

/// Apply a mode's `layout` spec to the workspace docks.
///
/// Two passes so dock-key order can't fight the panels' actual host docks:
/// pass 1 resolves every visible layout entry to `(host dock, panel index,
/// size)`; pass 2 opens/activates/resizes the targeted docks and closes the
/// rest. Docks absent from the layout (or `visible: false`) are closed —
/// switching modes pivots the whole workspace.
fn apply_mode_layout(
    mode: &WorkspaceMode,
    workspace: &mut Workspace,
    window: &mut Window,
    cx: &mut gpui::Context<Workspace>,
) {
    if mode.layout.contains_key("center_dock") {
        log::debug!(
            "workspace_modes: mode '{}' has a center_dock spec — the center is \
             the editor pane, not a dock; ignored (M2)",
            mode.id
        );
    }

    // Pass 1 — resolve each visible layout entry to its hosting dock.
    let mut open_targets: Vec<(DockPosition, usize, Option<u32>)> = Vec::new();
    for (key, target_position) in DOCK_KEYS {
        let Some(spec) = mode.layout.get(key) else {
            continue;
        };
        if !spec.visible {
            continue;
        }
        let Some(panel_raw) = spec.panel.as_deref() else {
            continue;
        };
        let Some(persistent) = resolve_panel_persistent_name(panel_raw) else {
            log::info!(
                "workspace_modes: mode '{}' references unknown panel '{}' in {} — skipping",
                mode.id,
                panel_raw,
                key
            );
            continue;
        };
        let host = ALL_DOCKS.into_iter().find_map(|position| {
            workspace
                .dock_at_position(position)
                .read(cx)
                .panel_index_for_persistent_name(persistent, cx)
                .map(|ix| (position, ix))
        });
        match host {
            Some((host_position, panel_ix)) => {
                if host_position != target_position {
                    log::info!(
                        "workspace_modes: panel '{}' lives in the {} dock, not the {} — \
                         opening it there (cross-dock moves are not part of M2)",
                        persistent,
                        dock_label(host_position),
                        dock_label(target_position),
                    );
                }
                // One visible panel per dock — last layout entry wins.
                open_targets.retain(|(position, ..)| *position != host_position);
                open_targets.push((host_position, panel_ix, spec.size_px));
            }
            None => log::info!(
                "workspace_modes: panel '{}' is not registered in any dock — skipping",
                persistent
            ),
        }
    }

    // Pass 2 — apply: open + activate + resize targeted docks, close the rest.
    for position in ALL_DOCKS {
        let dock = workspace.dock_at_position(position).clone();
        let target = open_targets
            .iter()
            .find(|(p, ..)| *p == position)
            .copied();
        dock.update(cx, |dock, cx| match target {
            Some((_, panel_ix, size_px)) => {
                dock.activate_panel(panel_ix, window, cx);
                dock.set_open(true, window, cx);
                if let Some(size) = size_px {
                    dock.resize_active_panel(Some(px(size as f32)), None, window, cx);
                }
            }
            None => dock.set_open(false, window, cx),
        });
    }
    cx.notify();
}

/// Human-readable dock name for log lines (`DockPosition::label` is private
/// to the workspace crate).
fn dock_label(position: DockPosition) -> &'static str {
    match position {
        DockPosition::Left => "left",
        DockPosition::Right => "right",
        DockPosition::Bottom => "bottom",
    }
}

/// Map a mode-JSON panel name to Zed's `Panel::persistent_name` for the
/// panels that exist in-tree. Matching is case- and separator-insensitive
/// ("ProjectPanel", "Project Panel", "project_panel" all resolve). Unknown
/// names return `None` — callers log and skip.
fn resolve_panel_persistent_name(raw: &str) -> Option<&'static str> {
    let normalized: String = raw
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase();
    match normalized.as_str() {
        "agentpanel" => Some("AgentPanel"),
        "projectpanel" => Some("Project Panel"),
        "terminalpanel" => Some("TerminalPanel"),
        "gitpanel" => Some("GitPanel"),
        "outlinepanel" => Some("Outline Panel"),
        "collabpanel" => Some("CollabPanel"),
        "debugpanel" | "debuggerpanel" => Some("DebugPanel"),
        "taskboard" | "taskboardpanel" | "board" => Some("TaskBoardPanel"),
        "usage" | "usagepanel" => Some("UsagePanel"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_panel_names_maps_known_panels() {
        assert_eq!(
            resolve_panel_persistent_name("AgentPanel"),
            Some("AgentPanel")
        );
        assert_eq!(
            resolve_panel_persistent_name("ProjectPanel"),
            Some("Project Panel")
        );
        assert_eq!(
            resolve_panel_persistent_name("TerminalPanel"),
            Some("TerminalPanel")
        );
        assert_eq!(resolve_panel_persistent_name("GitPanel"), Some("GitPanel"));
        assert_eq!(
            resolve_panel_persistent_name("OutlinePanel"),
            Some("Outline Panel")
        );
    }

    #[test]
    fn resolve_panel_names_is_separator_and_case_insensitive() {
        assert_eq!(
            resolve_panel_persistent_name("project_panel"),
            Some("Project Panel")
        );
        assert_eq!(
            resolve_panel_persistent_name("Project Panel"),
            Some("Project Panel")
        );
        assert_eq!(
            resolve_panel_persistent_name("agentpanel"),
            Some("AgentPanel")
        );
    }

    #[test]
    fn resolve_panel_names_maps_task_board() {
        // Z1 — the `taskboard` mode mounts the native task board panel.
        assert_eq!(
            resolve_panel_persistent_name("TaskBoard"),
            Some("TaskBoardPanel")
        );
        assert_eq!(
            resolve_panel_persistent_name("task_board_panel"),
            Some("TaskBoardPanel")
        );
        assert_eq!(
            resolve_panel_persistent_name("Board"),
            Some("TaskBoardPanel")
        );
    }

    #[test]
    fn resolve_panel_names_maps_usage() {
        // Z2 — the `usage` mode mounts the native usage panel.
        assert_eq!(resolve_panel_persistent_name("usage"), Some("UsagePanel"));
        assert_eq!(
            resolve_panel_persistent_name("Usage Panel"),
            Some("UsagePanel")
        );
    }

    #[test]
    fn resolve_panel_names_rejects_unknown_panels() {
        // Symphony-era and not-yet-native panels must skip gracefully.
        assert_eq!(resolve_panel_persistent_name("SymphonyRunCanvas"), None);
        assert_eq!(resolve_panel_persistent_name("ConversationView"), None);
        assert_eq!(resolve_panel_persistent_name("SettingsPanel"), None);
        assert_eq!(resolve_panel_persistent_name(""), None);
    }

    #[test]
    fn switch_workspace_mode_action_deserializes_from_keymap_args() {
        // Mirrors the JSON shape used in `assets/keymaps/workspace_modes.json`.
        let action: SwitchWorkspaceMode =
            serde_json::from_str(r#"{ "mode_index": 3 }"#).unwrap();
        assert_eq!(action.mode_index, 3);
    }
}
