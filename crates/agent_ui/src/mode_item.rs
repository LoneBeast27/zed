//! Center-pane host for mode primary surfaces (PARITY_SPEC Amendment
//! 2026-07-04 (2)).
//!
//! The six mode primaries (orchestrator chat, task board, symphony,
//! adversary, usage, settings-status) open in the CENTER pane as workspace
//! items — the editor-tab idiom, full-bleed `1fr` of the shell grid — not in
//! a dock. [`ModeItem`] is the generic `workspace::Item` wrapper: it hosts
//! the surface's view entity (built once per workspace, registered in
//! [`ModeSurfaces`]) and contributes only tab chrome (mode display name +
//! mode icon) and lifecycle plumbing. Closing the tab drops the host, never
//! the surface — reopening re-hosts the same entity with its state intact.
//!
//! Visibility gating (the Lightness law the dock's `Panel::set_active`
//! used to carry): [`ModeSurface::set_surface_active`] is the surface's
//! mount/unmount signal. The ON edge is render — a frame renders exactly
//! when the tab is the pane's visible item, gated by one bool so settled
//! frames do no work. The OFF edges are `Item::deactivated` (another tab
//! took the pane) and `Item::on_removed` (tab closed) — both drop the
//! surface's poll watch while the entity itself survives in the registry.

use gpui::{App, Context, Entity, EventEmitter, FocusHandle, Focusable, SharedString, Window};
use ui::prelude::*;
use workspace::Workspace;
use workspace::item::{Item, ItemEvent};

use crate::activity_bar::ActivityBar;
use crate::adversary_panel::AdversaryPanel;
use crate::artifact_surface::ArtifactSurface;
use crate::briefing_panel::BriefingPanel;
use crate::orchestrator_panel::OrchestratorPanel;
use crate::settings_status_panel::SettingsStatusPanel;
use crate::symphony_panel::SymphonyPanel;
use crate::task_board::TaskBoardPanel;
use crate::usage_panel::UsagePanel;

/// A view that can live in the center pane as a mode's primary surface.
///
/// Implementors are the panel view entities themselves — the same structs
/// that rendered inside docks before the amendment. The trait adds tab
/// fallbacks (used when a surface is opened outside a mode switch, e.g. a
/// `ToggleFocus` action) and the visibility hook that replaced
/// `Panel::set_active`.
pub trait ModeSurface: Render + Focusable + Sized + 'static {
    /// Tab title when no mode context supplies one.
    fn fallback_tab_title() -> SharedString;
    /// Tab icon when no mode context supplies one (kept in lockstep with
    /// the shipped mode JSONs' icons).
    fn fallback_tab_icon() -> IconName;
    /// Visibility signal — `true` while the hosting tab is the pane's
    /// visible item. Watch-gated surfaces acquire/release their polls here
    /// (the web's route mount/unmount; formerly `Panel::set_active`).
    fn set_surface_active(&mut self, _active: bool, _cx: &mut Context<Self>) {}
}

/// The six center surfaces, one long-lived entity each per workspace.
/// Built at workspace-modes init (push-fed stores stay warm from the
/// start, as in the dock era) and stashed on the [`ActivityBar`] — the
/// mode system's per-workspace anchor.
#[derive(Clone)]
pub struct ModeSurfaces {
    pub artifact: Entity<ArtifactSurface>,
    pub briefing: Entity<BriefingPanel>,
    pub orchestrator: Entity<OrchestratorPanel>,
    pub task_board: Entity<TaskBoardPanel>,
    pub symphony: Entity<SymphonyPanel>,
    pub adversary: Entity<AdversaryPanel>,
    pub usage: Entity<UsagePanel>,
    pub settings_status: Entity<SettingsStatusPanel>,
}

/// Which center surface a mode layout names — the switcher resolves the
/// mode-JSON panel string to one of these
/// ([`crate::workspace_mode_switcher`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CenterSurface {
    Artifact,
    Briefing,
    Orchestrator,
    TaskBoard,
    Symphony,
    Adversary,
    Usage,
    SettingsStatus,
}

/// The generic center-pane host: owns a clone of the surface entity and
/// renders it full-bleed under a tab carrying the mode's name + icon.
pub struct ModeItem<S: ModeSurface> {
    inner: Entity<S>,
    title: SharedString,
    icon: IconName,
    /// Whether the surface has been told it is visible — the render-edge
    /// gate (one bool check per frame; no per-frame entity work once live).
    surface_live: bool,
}

impl<S: ModeSurface> ModeItem<S> {
    pub fn new(inner: Entity<S>, tab: Option<(SharedString, IconName)>) -> Self {
        let (title, icon) =
            tab.unwrap_or_else(|| (S::fallback_tab_title(), S::fallback_tab_icon()));
        Self {
            inner,
            title,
            icon,
            surface_live: false,
        }
    }

    /// The hosted surface entity (tests + run-routing helpers).
    pub fn inner(&self) -> &Entity<S> {
        &self.inner
    }

    /// Refresh tab chrome on re-activation (a mode file's display name or
    /// icon may have changed between opens).
    fn set_tab(&mut self, title: SharedString, icon: IconName, cx: &mut Context<Self>) {
        if self.title != title || self.icon != icon {
            self.title = title;
            self.icon = icon;
            cx.emit(ItemEvent::UpdateTab);
        }
    }

    fn release_surface(&self, cx: &mut App) {
        self.inner
            .update(cx, |surface, cx| surface.set_surface_active(false, cx));
    }
}

impl<S: ModeSurface> Render for ModeItem<S> {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Render happens exactly while this tab is visible — the ON edge.
        // `deactivated`/`on_removed` reset the gate (OFF edges), so a tab
        // click re-acquires on its first frame back.
        if !self.surface_live {
            self.surface_live = true;
            self.inner
                .update(cx, |surface, cx| surface.set_surface_active(true, cx));
        }
        div().size_full().child(self.inner.clone())
    }
}

impl<S: ModeSurface> Focusable for ModeItem<S> {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        // Focusing the tab focuses the surface (which may itself delegate,
        // e.g. the orchestrator's composer editor).
        self.inner.read(cx).focus_handle(cx)
    }
}

impl<S: ModeSurface> EventEmitter<ItemEvent> for ModeItem<S> {}

impl<S: ModeSurface> Item for ModeItem<S> {
    type Event = ItemEvent;

    fn tab_content_text(&self, _detail: usize, _cx: &App) -> SharedString {
        self.title.clone()
    }

    fn tab_icon(&self, _window: &Window, _cx: &App) -> Option<Icon> {
        Some(Icon::new(self.icon))
    }

    fn show_toolbar(&self) -> bool {
        false
    }

    fn deactivated(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.surface_live = false;
        self.release_surface(cx);
    }

    fn on_removed(&self, cx: &mut Context<Self>) {
        // Tab closed: the host drops but the surface entity survives in the
        // registry — release its watch so a closed tab never polls.
        self.release_surface(cx);
    }

    fn to_item_events(event: &Self::Event, f: &mut dyn FnMut(ItemEvent)) {
        f(*event)
    }
}

/// The workspace's surface registry, if workspace modes are installed
/// (it rides the activity bar, the mode system's per-workspace anchor).
///
// REGISTRY RELOCATION: this `bar.read(cx)` is the structural aggravator of the
// entity re-entrancy crash class (2026-07-04). Because the ModeSurfaces
// registry rides the ActivityBar, EVERY layout op (switch_to_mode →
// open_center_surface → here; open_task_board_run → here; usage-island row →
// here) reads the bar. Any of those invoked synchronously from inside the
// bar's own update panics (the rail-click crash). The interaction-dispatch law
// now defers all such calls out of listeners, which closes the crash
// behaviorally — but the clean structural fix is to move the registry OFF the
// bar so these paths never touch it. Contained relocation (spec'd in
// RUST_PORT_NOTES 2026-07-04, deferred because it crosses into stock
// workspace.rs, out of the agent_ui audit scope):
//   1. Add `mode_surfaces: Option<ModeSurfaces>` to `Workspace` (mirrors the
//      existing fork slots `activity_bar_item` / `corner_cluster_item`) with
//      `set_mode_surfaces` / `mode_surfaces` accessors.
//   2. agent_ui init: `workspace.set_mode_surfaces(surfaces)` instead of
//      `bar.update(cx, |bar, _| bar.set_surfaces(...))`.
//   3. This fn: `workspace.mode_surfaces().cloned()` — no entity read at all,
//      so no layout op ever re-enters the bar; the ActivityBar goes back to
//      owning only its own rail state.
// Two anchor points (write @ agent_ui.rs set_surfaces call, read here) + the
// storage on ActivityBar. NOT a wide fan-out; blocked only by the scope line.
pub fn workspace_surfaces(workspace: &Workspace, cx: &App) -> Option<ModeSurfaces> {
    workspace
        .activity_bar_item()
        .and_then(|view| view.downcast::<ActivityBar>().ok())
        .and_then(|bar| bar.read(cx).surfaces().cloned())
}

/// Open-or-activate the center item hosting `inner` — IDEMPOTENT: an
/// existing `ModeItem<S>` anywhere in the workspace is activated in place
/// (never a duplicate tab); otherwise one is added to the active pane.
/// `tab` carries the mode's display name + icon; `None` falls back to the
/// surface defaults.
pub fn open_center_item<S: ModeSurface>(
    inner: &Entity<S>,
    tab: Option<(SharedString, IconName)>,
    workspace: &mut Workspace,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let existing = workspace
        .active_pane()
        .read(cx)
        .items()
        .find_map(|item| item.downcast::<ModeItem<S>>())
        .or_else(|| {
            workspace.panes().iter().find_map(|pane| {
                pane.read(cx)
                    .items()
                    .find_map(|item| item.downcast::<ModeItem<S>>())
            })
        });
    match existing {
        Some(existing) => {
            if let Some((title, icon)) = tab {
                existing.update(cx, |item, cx| item.set_tab(title, icon, cx));
            }
            workspace.activate_item(&existing, true, true, window, cx);
        }
        None => {
            let item = cx.new(|_| ModeItem::new(inner.clone(), tab));
            workspace.add_item_to_active_pane(Box::new(item), None, true, window, cx);
        }
    }
}

/// Route to the task board and open `run_id`'s drawer — the web's
/// `location.hash = "#/board"; openDrawer(runId)`. Prefers a real mode
/// switch (keeps the rail highlight honest) when a `taskboard` mode is
/// installed; falls back to opening the board tab directly.
pub fn open_task_board_run(
    run_id: SharedString,
    workspace: &mut Workspace,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    if !crate::workspace_mode_switcher::switch_to_mode_id("taskboard", workspace, window, cx)
        && let Some(surfaces) = workspace_surfaces(workspace, cx)
    {
        open_center_item(&surfaces.task_board, None, workspace, window, cx);
    }
    if let Some(surfaces) = workspace_surfaces(workspace, cx) {
        surfaces
            .task_board
            .update(cx, |board, cx| board.open_run(run_id, cx));
    }
}

/// Open-or-activate the center item for a resolved [`CenterSurface`].
/// Returns `false` (logged) when no surface registry is installed.
pub fn open_center_surface(
    surface: CenterSurface,
    tab: Option<(SharedString, IconName)>,
    workspace: &mut Workspace,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> bool {
    let Some(surfaces) = workspace_surfaces(workspace, cx) else {
        log::warn!(
            "mode_item: no mode surfaces registered — cannot open {surface:?} in the center pane"
        );
        return false;
    };
    match surface {
        CenterSurface::Artifact => open_center_item(&surfaces.artifact, tab, workspace, window, cx),
        CenterSurface::Briefing => open_center_item(&surfaces.briefing, tab, workspace, window, cx),
        CenterSurface::Orchestrator => {
            open_center_item(&surfaces.orchestrator, tab, workspace, window, cx)
        }
        CenterSurface::TaskBoard => {
            open_center_item(&surfaces.task_board, tab, workspace, window, cx)
        }
        CenterSurface::Symphony => open_center_item(&surfaces.symphony, tab, workspace, window, cx),
        CenterSurface::Adversary => {
            open_center_item(&surfaces.adversary, tab, workspace, window, cx)
        }
        CenterSurface::Usage => open_center_item(&surfaces.usage, tab, workspace, window, cx),
        CenterSurface::SettingsStatus => {
            open_center_item(&surfaces.settings_status, tab, workspace, window, cx)
        }
    }
    true
}
