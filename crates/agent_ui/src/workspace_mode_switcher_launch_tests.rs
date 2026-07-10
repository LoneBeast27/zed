//! t9 launch-path tests for the workspace-modes switcher: applying the
//! configured `default_mode` on launch (no rail click), against an activity
//! bar whose modes are LOADED FROM DISK. Split from
//! `workspace_mode_switcher_layout_tests.rs` (the switch-layout behaviors)
//! per the 500-line ceiling; shares that sibling's harness helpers.

use super::layout_tests::{active_item_is, mode_item_count};
use super::*;

use gpui::{AppContext as _, TestAppContext, VisualTestContext, WeakEntity};
use project::Project;

use crate::activity_bar::ActivityBar;
use crate::adversary_panel::AdversaryPanel;
use crate::briefing_panel::BriefingPanel;
use crate::mode_item::ModeSurfaces;
use crate::orchestrator_panel::OrchestratorPanel;
use crate::settings_status_panel::SettingsStatusPanel;
use crate::symphony_panel::SymphonyPanel;
use crate::task_board::TaskBoardPanel;
use crate::usage_panel::UsagePanel;

/// Install the six surfaces on an activity bar whose modes are LOADED FROM
/// DISK (a real `.agents/modes` dir), so `switch_to_mode_id` can resolve a
/// mode by id the way the launch path does — the t9 fixture. Mirrors
/// `agent_ui.rs`'s init, minus the click/keymap wiring t9 does not exercise.
fn install_surfaces_from_modes_dir(
    modes_dir: std::path::PathBuf,
    default_mode: &str,
    workspace: &gpui::Entity<Workspace>,
    cx: &mut VisualTestContext,
) -> ModeSurfaces {
    workspace.update_in(cx, |workspace, window, cx| {
        let weak = cx.weak_entity();
        let surfaces = ModeSurfaces {
            artifact: cx.new(|cx| crate::artifact_surface::ArtifactSurface::new(cx)),
            briefing: cx.new(|cx| BriefingPanel::new(cx)),
            orchestrator: cx.new(|cx| {
                let stack = cx.new(|cx| crate::islands::NotifStack::new(weak.clone(), cx));
                OrchestratorPanel::new(weak.clone(), stack, window, cx)
            }),
            task_board: cx.new(|cx| TaskBoardPanel::new(cx)),
            symphony: cx.new(|cx| SymphonyPanel::new(cx)),
            adversary: cx.new(|cx| AdversaryPanel::new(window, cx)),
            usage: cx.new(|cx| UsagePanel::new(cx)),
            settings_status: cx.new(|cx| SettingsStatusPanel::new(cx)),
        };
        let no_workspace: Option<WeakEntity<Workspace>> = None;
        let bar = cx.new(|cx| ActivityBar::new(modes_dir, default_mode, no_workspace, cx));
        bar.update(cx, |bar, _| bar.set_surfaces(surfaces.clone()));
        workspace.set_activity_bar_item(Some(bar.into()), window, cx);
        surfaces
    })
}

/// t9 — a fresh workspace with `workspace_modes` on and a `default_mode`
/// set opens that mode's CENTER item on launch WITHOUT any rail click. This
/// drives the exact call the workspace-init observer now makes
/// (`switch_to_mode_id(default_mode, …)` — the same idempotent path a click
/// uses), against a bar whose modes are loaded from disk, and asserts the
/// center pane is no longer the empty void it was before the fix.
#[gpui::test]
async fn default_mode_opens_its_center_item_on_launch_without_a_click(
    cx: &mut TestAppContext,
) {
    crate::test_support::init_test(cx);
    let fs = fs::FakeFs::new(cx.executor());
    let project = Project::test(fs, [], cx).await;
    let (workspace, cx) =
        cx.add_window_view(|window, cx| Workspace::test_new(project.clone(), window, cx));

    // A real `.agents/modes` dir with the shipped orchestrator layout shape
    // (center = orchestrator + a right_dock constellation) — the loader reads
    // real files (`std::fs`), so the fixture writes one to a temp dir.
    let modes_dir = tempfile::tempdir().unwrap();
    std::fs::write(
        modes_dir.path().join("orchestrator.json"),
        r#"{
            "schema_version": 1,
            "id": "orchestrator",
            "display_name": "Orchestrator",
            "description": "The orchestrator conversation",
            "icon": "Chat",
            "accent_color_hex": "d97757",
            "layout": {
                "center": { "panel": "orchestrator" },
                "right_dock": { "panel": "constellation", "size_px": 480 }
            }
        }"#,
    )
    .unwrap();
    // A second mode so "apply the DEFAULT, not just the first" is meaningful.
    std::fs::write(
        modes_dir.path().join("usage.json"),
        r#"{
            "schema_version": 1,
            "id": "usage",
            "display_name": "Usage",
            "description": "Usage console",
            "icon": "Sliders",
            "accent_color_hex": "8ab4f8",
            "layout": { "center": { "panel": "usage" } }
        }"#,
    )
    .unwrap();

    install_surfaces_from_modes_dir(
        modes_dir.path().to_path_buf(),
        "usage",
        &workspace,
        cx,
    );

    // Precondition: no mode surface is mounted in the center yet — the void
    // the fix targets.
    assert_eq!(
        mode_item_count::<UsagePanel>(&workspace, cx),
        0,
        "no center item before launch applies the default layout"
    );

    // The launch-apply the observer performs — NO click handler involved.
    let applied = workspace.update_in(cx, |workspace, window, cx| {
        crate::workspace_mode_switcher::switch_to_mode_id("usage", workspace, window, cx)
    });
    assert!(applied, "the configured default_mode resolves and applies");

    assert_eq!(
        mode_item_count::<UsagePanel>(&workspace, cx),
        1,
        "launch opens exactly the DEFAULT mode's center item (usage), no click"
    );
    assert!(
        active_item_is::<UsagePanel>(&workspace, cx),
        "the default mode's center item is the active item on launch"
    );
    // The default is honored specifically — the non-default mode's surface is
    // NOT what launched.
    assert_eq!(
        mode_item_count::<OrchestratorPanel>(&workspace, cx),
        0,
        "only the default mode's surface opens, not another mode's"
    );
}

/// t9 negative path: a `default_mode` matching no loaded mode leaves the
/// center pane empty (the observer logs and applies nothing — never a
/// forced blank tab).
#[gpui::test]
async fn unknown_default_mode_applies_no_layout_on_launch(cx: &mut TestAppContext) {
    crate::test_support::init_test(cx);
    let fs = fs::FakeFs::new(cx.executor());
    let project = Project::test(fs, [], cx).await;
    let (workspace, cx) =
        cx.add_window_view(|window, cx| Workspace::test_new(project.clone(), window, cx));

    let modes_dir = tempfile::tempdir().unwrap();
    std::fs::write(
        modes_dir.path().join("orchestrator.json"),
        r#"{
            "schema_version": 1, "id": "orchestrator",
            "display_name": "Orchestrator", "description": "", "icon": "Chat",
            "accent_color_hex": "d97757",
            "layout": { "center": { "panel": "orchestrator" } }
        }"#,
    )
    .unwrap();

    install_surfaces_from_modes_dir(
        modes_dir.path().to_path_buf(),
        "no-such-mode",
        &workspace,
        cx,
    );

    let applied = workspace.update_in(cx, |workspace, window, cx| {
        crate::workspace_mode_switcher::switch_to_mode_id("no-such-mode", workspace, window, cx)
    });
    assert!(!applied, "an unknown default_mode resolves to nothing");
    assert_eq!(
        mode_item_count::<OrchestratorPanel>(&workspace, cx),
        0,
        "no center item is forced open when the default matches no mode"
    );
}
