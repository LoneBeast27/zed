//! Tests for the workspace mode switcher: the panel-name resolver units AND
//! the behavioral `apply_mode_layout` guarantees (switch-opens-the-named-panel,
//! unnamed-docks-close, empty-layout-closes-all) — the real Z3-regression
//! coverage. Extracted to a sibling (the `#[path]` idiom) to keep
//! `workspace_mode_switcher.rs` under the 500-line ceiling.

use super::*;

use std::collections::HashMap;

use gpui::{
    App, AppContext as _, Context, EventEmitter, FocusHandle, Focusable, IntoElement, Pixels,
    Render, TestAppContext, Window, div,
};
use project::Project;
use ui::IconName;
use workspace::Workspace;
use workspace::dock::{Panel, PanelEvent};

use crate::workspace_modes::LayoutSpec;

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
fn resolve_panel_names_maps_orchestrator_and_adversary() {
    // Z4 (orchestrator entry was the Z3 gap): the resolver-entry unit —
    // both the mode seeds' names and reasonable aliases resolve to the
    // right persistent name. The *behavioral* Z3-regression guarantee
    // (switch-to-orchestrator opens the panel rather than closing every
    // dock) is pinned separately by
    // `switching_to_orchestrator_mode_opens_the_panel_not_closes_all_docks`.
    assert_eq!(
        resolve_panel_persistent_name("orchestratorpanel"),
        Some("OrchestratorPanel")
    );
    assert_eq!(
        resolve_panel_persistent_name("Orchestrator"),
        Some("OrchestratorPanel")
    );
    assert_eq!(
        resolve_panel_persistent_name("adversary"),
        Some("AdversaryPanel")
    );
    assert_eq!(
        resolve_panel_persistent_name("Adversary Panel"),
        Some("AdversaryPanel")
    );
}

#[test]
fn resolve_panel_names_maps_symphony() {
    // Z4 — the `symphony` mode mounts the native symphony panel.
    assert_eq!(
        resolve_panel_persistent_name("symphony"),
        Some("SymphonyPanel")
    );
    assert_eq!(
        resolve_panel_persistent_name("Symphony Panel"),
        Some("SymphonyPanel")
    );
}

#[test]
fn resolve_panel_names_maps_settings_status() {
    // Z4 — the `settings` mode mounts the read-only status panel.
    assert_eq!(
        resolve_panel_persistent_name("settings"),
        Some("SettingsStatusPanel")
    );
    assert_eq!(
        resolve_panel_persistent_name("SettingsPanel"),
        Some("SettingsStatusPanel")
    );
    assert_eq!(
        resolve_panel_persistent_name("settings_status_panel"),
        Some("SettingsStatusPanel")
    );
}

#[test]
fn resolve_panel_names_rejects_unknown_panels() {
    // Stale-era names and not-yet-native panels must skip gracefully.
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

// ── Behavioral layout tests (the real Z3-regression guarantee) ──
//
// The string-resolver units above prove the name map; these prove the
// *consumer* — `apply_mode_layout` end-to-end against a real Workspace
// with panels registered. They exercise exactly the Z3 bug path: a
// missing resolver arm returned `None`, so Pass 1 banked no targets and
// Pass 2's `None => set_open(false)` closed every dock. With the
// `orchestrator` arm in place, switching opens the named panel instead.

/// A distinct persistent name + activation priority per panel type —
/// `Panel::persistent_name` is a static fn, so each name we register needs
/// its own marker type; the dock also asserts unique activation priorities
/// (dock.rs:719), so each marker carries a distinct one.
trait PanelName {
    const NAME: &'static str;
    const PRIORITY: u32;
}

struct Orchestrator;
impl PanelName for Orchestrator {
    const NAME: &'static str = "OrchestratorPanel";
    const PRIORITY: u32 = 1;
}
struct Adversary;
impl PanelName for Adversary {
    const NAME: &'static str = "AdversaryPanel";
    const PRIORITY: u32 = 2;
}
struct Project_;
impl PanelName for Project_ {
    const NAME: &'static str = "Project Panel";
    const PRIORITY: u32 = 3;
}

/// A minimal real `Panel` that reports a fixed persistent name and home
/// dock — enough for `apply_mode_layout`'s name-lookup + open/close path.
struct NamedPanel<N: PanelName> {
    focus_handle: FocusHandle,
    position: DockPosition,
    _name: std::marker::PhantomData<N>,
}

impl<N: PanelName> NamedPanel<N> {
    fn new(position: DockPosition, cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            position,
            _name: std::marker::PhantomData,
        }
    }
}

impl<N: PanelName + 'static> Render for NamedPanel<N> {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

impl<N: PanelName + 'static> Focusable for NamedPanel<N> {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl<N: PanelName + 'static> EventEmitter<PanelEvent> for NamedPanel<N> {}

impl<N: PanelName + 'static> Panel for NamedPanel<N> {
    fn persistent_name() -> &'static str {
        N::NAME
    }
    fn panel_key() -> &'static str {
        N::NAME
    }
    fn position(&self, _window: &Window, _cx: &App) -> DockPosition {
        self.position
    }
    fn position_is_valid(&self, _position: DockPosition) -> bool {
        true
    }
    fn set_position(
        &mut self,
        position: DockPosition,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        self.position = position;
    }
    fn default_size(&self, _window: &Window, _cx: &App) -> Pixels {
        px(360.)
    }
    fn icon(&self, _window: &Window, _cx: &App) -> Option<IconName> {
        None
    }
    fn icon_tooltip(&self, _window: &Window, _cx: &App) -> Option<&'static str> {
        None
    }
    fn toggle_action(&self) -> Box<dyn Action> {
        Box::new(SwitchWorkspaceMode { mode_index: 0 })
    }
    fn activation_priority(&self) -> u32 {
        N::PRIORITY
    }
}

/// The proven agent_ui Workspace-test harness (sets the settings store,
/// theme, editor, release channel, and the agent panel) — the same setup
/// every `Workspace::test_new`-based test in this crate uses.
fn init_layout_test(cx: &mut TestAppContext) {
    crate::test_support::init_test(cx);
}

/// A `WorkspaceMode` carrying just a `layout` map — the only field
/// `apply_mode_layout` reads beyond `id` (used for log lines).
fn mode_with_layout(id: &str, layout: HashMap<String, LayoutSpec>) -> WorkspaceMode {
    WorkspaceMode {
        schema_version: 1,
        id: id.into(),
        display_name: id.into(),
        description: String::new(),
        icon: "Hub".into(),
        accent_color_hex: "DA7756".into(),
        layout,
        default_keybinding: None,
        pinned_position: None,
        open_url: None,
    }
}

fn left_layout(panel: &str, size_px: u32) -> HashMap<String, LayoutSpec> {
    let mut layout = HashMap::new();
    layout.insert(
        "left_dock".to_string(),
        LayoutSpec {
            panel: Some(panel.to_string()),
            size_px: Some(size_px),
            fill: false,
            visible: true,
        },
    );
    layout
}

/// THE Z3-regression test: switching to the orchestrator mode must OPEN
/// the OrchestratorPanel in the left dock — not close every dock (the
/// pre-fix behavior, when the missing resolver arm returned `None`).
#[gpui::test]
async fn switching_to_orchestrator_mode_opens_the_panel_not_closes_all_docks(
    cx: &mut TestAppContext,
) {
    init_layout_test(cx);
    let fs = fs::FakeFs::new(cx.executor());
    let project = Project::test(fs, [], cx).await;
    let (workspace, cx) =
        cx.add_window_view(|window, cx| Workspace::test_new(project.clone(), window, cx));

    // Register the panels a mode switch can target — left-dock
    // orchestrator + adversary, mirroring `agent_ui.rs`'s registration.
    workspace.update_in(cx, |workspace, window, cx| {
        let orchestrator = cx.new(|cx| NamedPanel::<Orchestrator>::new(DockPosition::Left, cx));
        workspace.add_panel(orchestrator, window, cx);
        let adversary = cx.new(|cx| NamedPanel::<Adversary>::new(DockPosition::Left, cx));
        workspace.add_panel(adversary, window, cx);
    });

    // Open a non-target dock so "closes every dock" would be observable —
    // and prove the regression direction (the bug closed THIS too).
    workspace.update_in(cx, |workspace, window, cx| {
        workspace
            .dock_at_position(DockPosition::Right)
            .update(cx, |dock, cx| dock.set_open(true, window, cx));
    });

    let mode = mode_with_layout("orchestrator", left_layout("orchestrator", 640));
    workspace.update_in(cx, |workspace, window, cx| {
        apply_mode_layout(&mode, workspace, window, cx);
    });

    workspace.update(cx, |workspace, cx| {
        let left = workspace.dock_at_position(DockPosition::Left).read(cx);
        // (a) the left dock is OPEN with the orchestrator active — the
        //     behavior the Z3 fix added, pinned by no test before now.
        assert!(left.is_open(), "switch must OPEN the left dock");
        let active = left
            .active_panel_index()
            .and_then(|ix| left.panel_index_for_persistent_name("OrchestratorPanel", cx).map(|t| (ix, t)));
        assert_eq!(
            active.map(|(active_ix, target_ix)| active_ix == target_ix),
            Some(true),
            "the active left panel must be the OrchestratorPanel"
        );
        // (b) docks the mode doesn't name are closed (the right dock we
        //     pre-opened).
        assert!(
            !workspace
                .dock_at_position(DockPosition::Right)
                .read(cx)
                .is_open(),
            "unnamed docks close on switch"
        );
        // (c) the regression direction: NOT every dock closed — the left
        //     dock the mode names stays open (the pre-fix bug closed it).
        assert!(
            workspace.dock_at_position(DockPosition::Left).read(cx).is_open(),
            "the named dock must NOT be closed (the Z3 regression)"
        );
    });
}

/// A second mode (adversary) opening its own panel proves the open path
/// is the resolver-driven name lookup, not an orchestrator special-case —
/// the whole rail switches in lockstep (PARITY_SPEC §3).
#[gpui::test]
async fn switching_to_adversary_mode_opens_the_adversary_panel(cx: &mut TestAppContext) {
    init_layout_test(cx);
    let fs = fs::FakeFs::new(cx.executor());
    let project = Project::test(fs, [], cx).await;
    let (workspace, cx) =
        cx.add_window_view(|window, cx| Workspace::test_new(project.clone(), window, cx));

    workspace.update_in(cx, |workspace, window, cx| {
        let orchestrator = cx.new(|cx| NamedPanel::<Orchestrator>::new(DockPosition::Left, cx));
        workspace.add_panel(orchestrator, window, cx);
        let adversary = cx.new(|cx| NamedPanel::<Adversary>::new(DockPosition::Left, cx));
        workspace.add_panel(adversary, window, cx);
    });

    let mode = mode_with_layout("adversary", left_layout("adversary", 560));
    workspace.update_in(cx, |workspace, window, cx| {
        apply_mode_layout(&mode, workspace, window, cx);
    });

    workspace.update(cx, |workspace, cx| {
        let left = workspace.dock_at_position(DockPosition::Left).read(cx);
        assert!(left.is_open(), "switch must open the left dock");
        assert_eq!(
            left.active_panel_index(),
            left.panel_index_for_persistent_name("AdversaryPanel", cx),
            "the active left panel must be the AdversaryPanel"
        );
    });
}

/// A mode whose layout names NO panel closes all docks — confirming the
/// close-all path is real (so the orchestrator test's open assertion
/// isn't trivially always-true).
#[gpui::test]
async fn empty_layout_closes_all_docks(cx: &mut TestAppContext) {
    init_layout_test(cx);
    let fs = fs::FakeFs::new(cx.executor());
    let project = Project::test(fs, [], cx).await;
    let (workspace, cx) =
        cx.add_window_view(|window, cx| Workspace::test_new(project.clone(), window, cx));

    workspace.update_in(cx, |workspace, window, cx| {
        let project_panel = cx.new(|cx| NamedPanel::<Project_>::new(DockPosition::Left, cx));
        workspace.add_panel(project_panel, window, cx);
        workspace
            .dock_at_position(DockPosition::Left)
            .update(cx, |dock, cx| dock.set_open(true, window, cx));
    });

    let mode = mode_with_layout("blank", HashMap::new());
    workspace.update_in(cx, |workspace, window, cx| {
        apply_mode_layout(&mode, workspace, window, cx);
    });

    workspace.update(cx, |workspace, cx| {
        assert!(
            !workspace.dock_at_position(DockPosition::Left).read(cx).is_open(),
            "a layout naming no panel closes every dock"
        );
    });
}
