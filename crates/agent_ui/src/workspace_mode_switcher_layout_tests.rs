//! Behavioral tests for `apply_mode_layout` under the center-pane
//! semantics (PARITY_SPEC Amendment 2026-07-04 (2)), end-to-end against a
//! real `Workspace` with the surface registry installed: a center spec
//! opens/activates ONE `ModeItem` tab (idempotent — never a duplicate), a
//! legacy `left_dock` primary promotes to the center, the left dock is
//! never opened or closed by a switch, and right-dock specs still
//! open/close their dock (the constellation's contract). Sibling of
//! `workspace_mode_switcher_tests.rs` (the pure-resolution units) — split
//! keeps both under the 500-line ceiling.

use super::tests::{layout_of, spec};
use super::*;

use gpui::{
    App, AppContext as _, Context, EventEmitter, FocusHandle, Focusable, IntoElement, Pixels,
    Render, TestAppContext, VisualTestContext, Window, div,
};
use project::Project;
use ui::IconName;
use workspace::dock::{Panel, PanelEvent};

use crate::adversary_panel::AdversaryPanel;
use crate::mode_item::{ModeItem, ModeSurfaces};
use crate::orchestrator_panel::OrchestratorPanel;
use crate::settings_status_panel::SettingsStatusPanel;
use crate::symphony_panel::SymphonyPanel;
use crate::task_board::TaskBoardPanel;
use crate::usage_panel::UsagePanel;
use crate::workspace_modes::LayoutSpec;

/// A distinct persistent name + activation priority per test dock panel —
/// `Panel::persistent_name` is a static fn, so each name we register needs
/// its own marker type; the dock also asserts unique activation priorities
/// (dock.rs), so each marker carries a distinct one.
trait PanelName {
    const NAME: &'static str;
    const PRIORITY: u32;
}

/// Stands in for a stock left-dock navigation panel (project tree).
struct Project_;
impl PanelName for Project_ {
    const NAME: &'static str = "Project Panel";
    const PRIORITY: u32 = 3;
}

/// Stands in for the constellation (right-dock resident).
struct Constellation;
impl PanelName for Constellation {
    const NAME: &'static str = "ConstellationPanel";
    const PRIORITY: u32 = 4;
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

/// Build the six REAL surface entities and install them on an activity bar
/// (the registry anchor `apply_mode_layout` resolves through), mirroring
/// the `agent_ui.rs` workspace-modes init.
fn install_surfaces(
    workspace: &gpui::Entity<Workspace>,
    cx: &mut VisualTestContext,
) -> ModeSurfaces {
    workspace.update_in(cx, |workspace, window, cx| {
        let weak = cx.weak_entity();
        let surfaces = ModeSurfaces {
            orchestrator: cx.new(|cx| OrchestratorPanel::new(weak, window, cx)),
            task_board: cx.new(|cx| TaskBoardPanel::new(cx)),
            symphony: cx.new(|cx| SymphonyPanel::new(cx)),
            adversary: cx.new(|cx| AdversaryPanel::new(window, cx)),
            usage: cx.new(|cx| UsagePanel::new(cx)),
            settings_status: cx.new(|cx| SettingsStatusPanel::new(cx)),
        };
        let bar = cx.new(|cx| {
            ActivityBar::new(
                std::path::PathBuf::from("no-such-modes-dir"),
                "",
                None,
                cx,
            )
        });
        bar.update(cx, |bar, _| bar.set_surfaces(surfaces.clone()));
        workspace.set_activity_bar_item(Some(bar.into()), window, cx);
        surfaces
    })
}

/// A `WorkspaceMode` carrying just a `layout` map — the only fields
/// `apply_mode_layout` reads beyond `id`/`display_name`/`icon` (tab
/// chrome + log lines).
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

/// Count the `ModeItem<S>` tabs in the workspace's active pane.
fn mode_item_count<S: crate::mode_item::ModeSurface>(
    workspace: &gpui::Entity<Workspace>,
    cx: &mut VisualTestContext,
) -> usize {
    workspace.read_with(cx, |workspace, cx| {
        workspace
            .active_pane()
            .read(cx)
            .items()
            .filter(|item| item.downcast::<ModeItem<S>>().is_some())
            .count()
    })
}

/// Whether the active pane's ACTIVE item is a `ModeItem<S>`.
fn active_item_is<S: crate::mode_item::ModeSurface>(
    workspace: &gpui::Entity<Workspace>,
    cx: &mut VisualTestContext,
) -> bool {
    workspace.read_with(cx, |workspace, cx| {
        workspace
            .active_pane()
            .read(cx)
            .active_item()
            .is_some_and(|item| item.downcast::<ModeItem<S>>().is_some())
    })
}

/// A `center` primary opens ONE ModeItem tab in the active pane and never
/// opens or closes the left dock (the user's navigation space).
#[gpui::test]
async fn center_primary_opens_a_center_item_and_leaves_left_dock_alone(
    cx: &mut TestAppContext,
) {
    init_layout_test(cx);
    let fs = fs::FakeFs::new(cx.executor());
    let project = Project::test(fs, [], cx).await;
    let (workspace, cx) =
        cx.add_window_view(|window, cx| Workspace::test_new(project.clone(), window, cx));
    install_surfaces(&workspace, cx);

    // The user's navigation setup: a project panel open in the left dock.
    workspace.update_in(cx, |workspace, window, cx| {
        let project_panel = cx.new(|cx| NamedPanel::<Project_>::new(DockPosition::Left, cx));
        workspace.add_panel(project_panel, window, cx);
        workspace
            .dock_at_position(DockPosition::Left)
            .update(cx, |dock, cx| dock.set_open(true, window, cx));
    });

    let mode = mode_with_layout(
        "taskboard",
        layout_of(&[("center", spec("taskboard", None, true))]),
    );
    workspace.update_in(cx, |workspace, window, cx| {
        apply_mode_layout(&mode, workspace, window, cx);
    });

    assert_eq!(
        mode_item_count::<TaskBoardPanel>(&workspace, cx),
        1,
        "the center spec opens exactly one task-board tab"
    );
    assert!(
        active_item_is::<TaskBoardPanel>(&workspace, cx),
        "the task-board tab is the active item"
    );
    workspace.read_with(cx, |workspace, cx| {
        assert!(
            workspace
                .dock_at_position(DockPosition::Left)
                .read(cx)
                .is_open(),
            "a mode switch must NOT close the user's left dock"
        );
    });
}

/// Re-applying a mode (and returning to it after another mode) activates
/// the EXISTING tab — never a duplicate (the idempotency law).
#[gpui::test]
async fn center_item_reactivation_is_idempotent(cx: &mut TestAppContext) {
    init_layout_test(cx);
    let fs = fs::FakeFs::new(cx.executor());
    let project = Project::test(fs, [], cx).await;
    let (workspace, cx) =
        cx.add_window_view(|window, cx| Workspace::test_new(project.clone(), window, cx));
    install_surfaces(&workspace, cx);

    let taskboard = mode_with_layout(
        "taskboard",
        layout_of(&[("center", spec("taskboard", None, true))]),
    );
    let usage = mode_with_layout("usage", layout_of(&[("center", spec("usage", None, true))]));

    for mode in [&taskboard, &taskboard, &usage, &taskboard] {
        workspace.update_in(cx, |workspace, window, cx| {
            apply_mode_layout(mode, workspace, window, cx);
        });
    }

    assert_eq!(
        mode_item_count::<TaskBoardPanel>(&workspace, cx),
        1,
        "four switches produce exactly one task-board tab"
    );
    assert_eq!(
        mode_item_count::<UsagePanel>(&workspace, cx),
        1,
        "and exactly one usage tab"
    );
    assert!(
        active_item_is::<TaskBoardPanel>(&workspace, cx),
        "the last-applied mode's tab is active"
    );
}

/// THE backward-compat guarantee: a legacy `left_dock` primary spec (the
/// pre-amendment mode-JSON shape) opens the surface in the CENTER pane and
/// leaves the left dock closed.
#[gpui::test]
async fn legacy_left_dock_primary_opens_in_the_center_not_the_dock(cx: &mut TestAppContext) {
    init_layout_test(cx);
    let fs = fs::FakeFs::new(cx.executor());
    let project = Project::test(fs, [], cx).await;
    let (workspace, cx) =
        cx.add_window_view(|window, cx| Workspace::test_new(project.clone(), window, cx));
    install_surfaces(&workspace, cx);

    // The exact pre-amendment orchestrator.json layout shape.
    let mode = mode_with_layout(
        "orchestrator",
        layout_of(&[("left_dock", spec("orchestratorpanel", Some(640), true))]),
    );
    workspace.update_in(cx, |workspace, window, cx| {
        apply_mode_layout(&mode, workspace, window, cx);
    });

    assert_eq!(
        mode_item_count::<OrchestratorPanel>(&workspace, cx),
        1,
        "the legacy left_dock primary is promoted to a center tab"
    );
    workspace.read_with(cx, |workspace, cx| {
        assert!(
            !workspace
                .dock_at_position(DockPosition::Left)
                .read(cx)
                .is_open(),
            "the promoted spec must not open the left dock"
        );
    });
}

/// Right-dock specs still work: named → open + activated; unnamed on the
/// next switch → closed. (The constellation's contract.)
#[gpui::test]
async fn right_dock_spec_applies_and_closes_when_unnamed(cx: &mut TestAppContext) {
    init_layout_test(cx);
    let fs = fs::FakeFs::new(cx.executor());
    let project = Project::test(fs, [], cx).await;
    let (workspace, cx) =
        cx.add_window_view(|window, cx| Workspace::test_new(project.clone(), window, cx));
    install_surfaces(&workspace, cx);

    workspace.update_in(cx, |workspace, window, cx| {
        let constellation =
            cx.new(|cx| NamedPanel::<Constellation>::new(DockPosition::Right, cx));
        workspace.add_panel(constellation, window, cx);
    });

    // The shipped orchestrator layout: center chat + right-dock constellation.
    let orchestrator = mode_with_layout(
        "orchestrator",
        layout_of(&[
            ("center", spec("orchestrator", None, true)),
            ("right_dock", spec("constellation", Some(480), true)),
        ]),
    );
    workspace.update_in(cx, |workspace, window, cx| {
        apply_mode_layout(&orchestrator, workspace, window, cx);
    });
    workspace.read_with(cx, |workspace, cx| {
        let right = workspace.dock_at_position(DockPosition::Right).read(cx);
        assert!(right.is_open(), "the named right dock opens");
        assert_eq!(
            right.active_panel_index(),
            right.panel_index_for_persistent_name("ConstellationPanel", cx),
            "the constellation is the active right panel"
        );
    });
    assert!(
        active_item_is::<OrchestratorPanel>(&workspace, cx),
        "the orchestrator tab is active alongside the right dock"
    );

    // A mode without a right_dock spec closes it (and still leaves the
    // left dock alone — closed stays closed).
    let usage = mode_with_layout("usage", layout_of(&[("center", spec("usage", None, true))]));
    workspace.update_in(cx, |workspace, window, cx| {
        apply_mode_layout(&usage, workspace, window, cx);
    });
    workspace.read_with(cx, |workspace, cx| {
        assert!(
            !workspace
                .dock_at_position(DockPosition::Right)
                .read(cx)
                .is_open(),
            "an unnamed right dock closes on switch"
        );
        assert!(
            !workspace
                .dock_at_position(DockPosition::Left)
                .read(cx)
                .is_open(),
            "the left dock stays untouched (closed) across switches"
        );
    });
}
