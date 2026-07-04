//! Unit tests for the workspace mode switcher's pure resolution layer
//! (PARITY_SPEC Amendment 2026-07-04 (2)): the name resolvers (center
//! surfaces vs dock panels), the center-target resolution — including the
//! legacy `left_dock` promotion — and the keymap action shape. The
//! behavioral `apply_mode_layout` tests live in the
//! `workspace_mode_switcher_layout_tests.rs` sibling (both siblings keep
//! the switcher under the 500-line ceiling).

use super::*;

use crate::workspace_modes::LayoutSpec;

// ── Resolver units ──

#[test]
fn resolve_center_surface_maps_the_six_primaries() {
    assert_eq!(
        resolve_center_surface("orchestrator"),
        Some(CenterSurface::Orchestrator)
    );
    assert_eq!(
        resolve_center_surface("OrchestratorPanel"),
        Some(CenterSurface::Orchestrator)
    );
    assert_eq!(
        resolve_center_surface("taskboard"),
        Some(CenterSurface::TaskBoard)
    );
    assert_eq!(
        resolve_center_surface("Task Board Panel"),
        Some(CenterSurface::TaskBoard)
    );
    assert_eq!(resolve_center_surface("Board"), Some(CenterSurface::TaskBoard));
    assert_eq!(
        resolve_center_surface("symphony"),
        Some(CenterSurface::Symphony)
    );
    assert_eq!(
        resolve_center_surface("adversary"),
        Some(CenterSurface::Adversary)
    );
    assert_eq!(resolve_center_surface("usage"), Some(CenterSurface::Usage));
    assert_eq!(
        resolve_center_surface("settings"),
        Some(CenterSurface::SettingsStatus)
    );
    assert_eq!(
        resolve_center_surface("settings_status_panel"),
        Some(CenterSurface::SettingsStatus)
    );
    // Stock dock panels and stale-era names are NOT center surfaces.
    assert_eq!(resolve_center_surface("ProjectPanel"), None);
    assert_eq!(resolve_center_surface("constellation"), None);
    assert_eq!(resolve_center_surface("ConversationView"), None);
    assert_eq!(resolve_center_surface(""), None);
}

#[test]
fn resolve_panel_names_maps_known_dock_panels() {
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
    // §10 — the constellation is the right dock's resident.
    assert_eq!(
        resolve_panel_persistent_name("constellation"),
        Some("ConstellationPanel")
    );
    assert_eq!(
        resolve_panel_persistent_name("Constellation Panel"),
        Some("ConstellationPanel")
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
fn resolve_panel_names_rejects_center_surfaces_and_unknowns() {
    // Amendment 2026-07-04 (2): the six mode primaries are center
    // surfaces, no longer dock-mountable — the dock resolver must not
    // claim them.
    for name in [
        "orchestrator",
        "taskboard",
        "symphony",
        "adversary",
        "usage",
        "settings",
    ] {
        assert_eq!(
            resolve_panel_persistent_name(name),
            None,
            "'{name}' must resolve as a center surface, not a dock panel"
        );
    }
    assert_eq!(resolve_panel_persistent_name("SymphonyRunCanvas"), None);
    assert_eq!(resolve_panel_persistent_name("ConversationView"), None);
    assert_eq!(resolve_panel_persistent_name(""), None);
}

#[test]
fn switch_workspace_mode_action_deserializes_from_keymap_args() {
    // Mirrors the JSON shape used in `assets/keymaps/workspace_modes.json`.
    let action: SwitchWorkspaceMode =
        serde_json::from_str(r#"{ "mode_index": 3 }"#).unwrap();
    assert_eq!(action.mode_index, 3);
}

// ── Center-target resolution (pure) ──

/// Layout-spec literal (shared with the behavioral sibling).
pub(super) fn spec(panel: &str, size_px: Option<u32>, visible: bool) -> LayoutSpec {
    LayoutSpec {
        panel: Some(panel.to_string()),
        size_px,
        fill: false,
        visible,
    }
}

/// Layout-map literal (shared with the behavioral sibling).
pub(super) fn layout_of(entries: &[(&str, LayoutSpec)]) -> HashMap<String, LayoutSpec> {
    entries
        .iter()
        .map(|(key, spec)| (key.to_string(), spec.clone()))
        .collect()
}

#[test]
fn center_key_is_the_primary() {
    let layout = layout_of(&[("center", spec("orchestrator", None, true))]);
    let target = resolve_center_target(&layout).unwrap();
    assert_eq!(target.surface, CenterSurface::Orchestrator);
    assert!(!target.promoted_from_left_dock);
}

#[test]
fn center_dock_alias_is_accepted() {
    // The old "`center_dock` is ignored" rule is superseded.
    let layout = layout_of(&[("center_dock", spec("usage", None, true))]);
    let target = resolve_center_target(&layout).unwrap();
    assert_eq!(target.surface, CenterSurface::Usage);
    assert!(!target.promoted_from_left_dock);
}

#[test]
fn legacy_left_dock_primary_specs_still_parse_and_promote() {
    // Backward compat: the pre-amendment mode JSONs put their primary in
    // `left_dock` — those specs promote to center instead of breaking.
    let layout = layout_of(&[("left_dock", spec("adversary", Some(560), true))]);
    let target = resolve_center_target(&layout).unwrap();
    assert_eq!(target.surface, CenterSurface::Adversary);
    assert!(target.promoted_from_left_dock);
}

#[test]
fn center_key_wins_over_a_legacy_left_dock_spec() {
    let layout = layout_of(&[
        ("center", spec("symphony", None, true)),
        ("left_dock", spec("taskboard", Some(560), true)),
    ]);
    let target = resolve_center_target(&layout).unwrap();
    assert_eq!(target.surface, CenterSurface::Symphony);
    assert!(!target.promoted_from_left_dock);
}

#[test]
fn left_dock_stock_panels_are_not_promoted() {
    // A left_dock spec naming a stock panel is NOT a center primary — the
    // left dock is the user's space, so the whole spec is a no-op.
    let layout = layout_of(&[("left_dock", spec("ProjectPanel", Some(280), true))]);
    assert_eq!(resolve_center_target(&layout), None);
}

#[test]
fn invisible_center_specs_resolve_to_no_center() {
    let layout = layout_of(&[("center", spec("usage", None, false))]);
    assert_eq!(resolve_center_target(&layout), None);
}
