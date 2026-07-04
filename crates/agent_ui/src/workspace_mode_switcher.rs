//! Workspace-modes switcher (M2 slice; center-pane semantics per
//! PARITY_SPEC Amendment 2026-07-04 (2)).
//!
//! Spec: `.planning/zed-fork/WORKSPACE_MODES.md` §5 + §7 (M2), mode set per
//! `PRD_V2.md` §6. Applies a mode's `layout` spec to the workspace:
//!
//! - **`center` (primary key).** The mode's primary surface (orchestrator
//!   chat, task board, symphony, adversary, usage, settings-status) opens
//!   in the CENTER pane as a workspace item — the editor-tab idiom,
//!   full-bleed (`size_px`/`fill` are ignored for center). Opening is
//!   IDEMPOTENT: an existing tab of that surface is activated, never
//!   duplicated ([`crate::mode_item::open_center_item`]). `center_dock`
//!   is accepted as an alias (the old "`center_dock` is ignored" rule is
//!   superseded).
//! - **`right_dock` / `bottom_dock`.** Open/activate/resize the named dock
//!   panel (e.g. the constellation in the right dock); docks absent from
//!   the layout are closed. Panels are opened in the dock they live in —
//!   cross-dock moves are still not performed (the M2 limitation).
//! - **`left_dock` — the user's navigation space.** Modes never open or
//!   close the left dock (user ruling 2026-07-04: project tree / git /
//!   outline stay under user control there). A LEGACY `left_dock` spec
//!   naming a center surface — the pre-amendment layout shape — is
//!   promoted to `center` (logged migration shim, backward compat); any
//!   other `left_dock` spec is logged and ignored.
//! - Unknown panel names are skipped gracefully with a log line.
//! - Modes with `open_url` set open that URL in the system default browser.
//!
//! Everything here is reachable only when the `agent.workspace_modes`
//! setting is enabled: the activity bar (click path) is only installed
//! then, and the `Ctrl+Alt+1..9` keymap asset is only loaded then
//! (`zed::load_default_keymap`).

use std::collections::HashMap;

use gpui::{Action, SharedString, Window, px};
use schemars::JsonSchema;
use serde::Deserialize;
use workspace::Workspace;
use workspace::dock::DockPosition;

use crate::activity_bar::ActivityBar;
use crate::mode_icons::icon_name_for;
use crate::mode_item::{self, CenterSurface};
use crate::workspace_modes::{LayoutSpec, WorkspaceMode};

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

/// Layout keys that map onto docks the switcher manages. The left dock is
/// deliberately absent — it belongs to the user (Amendment 2026-07-04 (2)).
const DOCK_KEYS: [(&str, DockPosition); 2] = [
    ("right_dock", DockPosition::Right),
    ("bottom_dock", DockPosition::Bottom),
];

/// Docks the switcher opens/closes on a mode switch (the close pass never
/// touches the left dock).
const MANAGED_DOCKS: [DockPosition; 2] = [DockPosition::Right, DockPosition::Bottom];

/// All dock positions, for the host-dock lookup (a stock panel a layout
/// names may live in any dock, including the left).
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
/// fall back to opening a center item directly.
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

/// Switch the workspace into `mode`: apply its layout (center item + docks),
/// then honor any `open_url` side effect (system default browser).
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

/// Where a mode's primary surface comes from in the layout map.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CenterTarget {
    pub surface: CenterSurface,
    /// `true` when the spec came from a legacy `left_dock` primary rather
    /// than the `center` key (pre-amendment layout shape — logged as a
    /// migration hint at the apply site).
    pub promoted_from_left_dock: bool,
}

/// Resolve a mode layout's center surface: the `center` key (or its
/// `center_dock` alias) wins; a legacy `left_dock` spec naming a center
/// surface is promoted (backward compat — the pre-amendment mode JSONs put
/// their primary there). Pure — unit-tested directly.
pub(crate) fn resolve_center_target(layout: &HashMap<String, LayoutSpec>) -> Option<CenterTarget> {
    for key in ["center", "center_dock"] {
        let Some(spec) = layout.get(key).filter(|spec| spec.visible) else {
            continue;
        };
        let Some(panel) = spec.panel.as_deref() else {
            continue;
        };
        match resolve_center_surface(panel) {
            Some(surface) => {
                return Some(CenterTarget {
                    surface,
                    promoted_from_left_dock: false,
                });
            }
            None => log::info!(
                "workspace_modes: {key} spec names '{panel}', which is not a known center \
                 surface — ignoring"
            ),
        }
    }
    let spec = layout.get("left_dock").filter(|spec| spec.visible)?;
    let surface = resolve_center_surface(spec.panel.as_deref()?)?;
    Some(CenterTarget {
        surface,
        promoted_from_left_dock: true,
    })
}

/// Apply a mode's `layout` spec: right/bottom docks first (open/activate/
/// resize named ones, close unnamed ones), the center item LAST so focus
/// lands in the primary surface. The left dock is never touched.
fn apply_mode_layout(
    mode: &WorkspaceMode,
    workspace: &mut Workspace,
    window: &mut Window,
    cx: &mut gpui::Context<Workspace>,
) {
    let center = resolve_center_target(&mode.layout);
    match &center {
        Some(target) if target.promoted_from_left_dock => log::info!(
            "workspace_modes: mode '{}' uses the legacy left_dock primary shape — promoted \
             to the center pane (migrate the mode file to a `center` key)",
            mode.id
        ),
        Some(_) => {}
        None => {
            if mode.layout.contains_key("left_dock") {
                log::info!(
                    "workspace_modes: mode '{}' has a left_dock spec naming no center surface \
                     — the left dock is the user's navigation space (Amendment 2026-07-04 (2)); \
                     ignored",
                    mode.id
                );
            }
        }
    }

    // Pass 1 — resolve each managed-dock layout entry to its hosting dock.
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
            Some((DockPosition::Left, _)) => log::info!(
                "workspace_modes: panel '{persistent}' lives in the left dock — modes leave \
                 the left dock under user control; skipping"
            ),
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

    // Pass 2 — apply: open + activate + resize targeted managed docks,
    // close the unnamed ones. The left dock is exempt (user space).
    for position in MANAGED_DOCKS {
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

    // Center LAST — focus lands in the mode's primary surface.
    if let Some(target) = center {
        let tab = (
            SharedString::from(mode.display_name.clone()),
            icon_name_for(&mode.icon),
        );
        mode_item::open_center_surface(target.surface, Some(tab), workspace, window, cx);
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

/// Case- and separator-insensitive panel-name normalization ("Project
/// Panel", "project_panel", "ProjectPanel" all collapse to "projectpanel").
fn normalize_panel_name(raw: &str) -> String {
    raw.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase()
}

/// Map a mode-JSON panel name to a center surface (Amendment 2026-07-04
/// (2)): the six mode primaries live in the center pane, not in docks.
pub(crate) fn resolve_center_surface(raw: &str) -> Option<CenterSurface> {
    match normalize_panel_name(raw).as_str() {
        "orchestrator" | "orchestratorpanel" | "chat" => Some(CenterSurface::Orchestrator),
        "taskboard" | "taskboardpanel" | "board" => Some(CenterSurface::TaskBoard),
        "symphony" | "symphonypanel" => Some(CenterSurface::Symphony),
        "adversary" | "adversarypanel" => Some(CenterSurface::Adversary),
        "usage" | "usagepanel" => Some(CenterSurface::Usage),
        "settings" | "settingspanel" | "settingsstatuspanel" => Some(CenterSurface::SettingsStatus),
        _ => None,
    }
}

/// Map a mode-JSON panel name to Zed's `Panel::persistent_name` for the
/// DOCK panels that exist in-tree (stock Zed panels + the constellation).
/// The six center surfaces are deliberately absent — they resolve via
/// [`resolve_center_surface`] and are no longer dock-mountable. Unknown
/// names return `None` — callers log and skip.
fn resolve_panel_persistent_name(raw: &str) -> Option<&'static str> {
    match normalize_panel_name(raw).as_str() {
        "agentpanel" => Some("AgentPanel"),
        "projectpanel" => Some("Project Panel"),
        "terminalpanel" => Some("TerminalPanel"),
        "gitpanel" => Some("GitPanel"),
        "outlinepanel" => Some("Outline Panel"),
        "collabpanel" => Some("CollabPanel"),
        "debugpanel" | "debuggerpanel" => Some("DebugPanel"),
        // §10 — the subagent constellation, the right dock's resident
        // beside the orchestrator conversation.
        "constellation" | "constellationpanel" => Some("ConstellationPanel"),
        _ => None,
    }
}

#[cfg(test)]
#[path = "workspace_mode_switcher_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "workspace_mode_switcher_layout_tests.rs"]
mod layout_tests;
