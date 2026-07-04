//! The vault-browser `Panel` (LEFT dock). Owns the built [`VaultIndex`], the
//! filter editor, the root switcher + Graph/List toggle, and every interaction
//! handler. Indexing runs on a background task (`cx.background_spawn`) built on
//! first open; a manual refresh + a re-open-when-stale (>60s) rebuild it.
//!
//! Dispatch law (RUST_PORT_NOTES §11): opening a file and promoting a bundle
//! are LAYOUT/fs mutations reached from click listeners — they are deferred out
//! of the listener via `window.defer`. Arm/cancel/filter are pure entity-state
//! flips and stay synchronous (immediate visual feedback).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use editor::Editor;
use fs::Fs;
use gpui::{
    Action, App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable, Pixels,
    Subscription, Task, WeakEntity, Window, actions,
};
use ui::prelude::*;
use workspace::{
    OpenOptions, OpenVisible, Workspace,
    dock::{DockPosition, Panel, PanelEvent},
};

use super::index::{VaultIndex, VaultRoot, build_index};
use super::promote::{self, BundleState, PromoteState};

actions!(
    vault_browser,
    [
        /// Toggles focus on the OKF vault-browser panel.
        ToggleFocus
    ]
);

/// Re-index when the panel re-opens and the last build is older than this
/// (spec §5 "re-index on panel re-open if stale >60s"). No fs watcher in v1.
const STALE_AFTER: Duration = Duration::from_secs(60);

/// Which body is showing — mirrors the board's Graph/Grid seg-toggle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VaultView {
    List,
    Graph,
}

pub struct VaultBrowserPanel {
    focus_handle: FocusHandle,
    workspace: WeakEntity<Workspace>,
    fs: Arc<dyn Fs>,
    /// The built index, `None` until the first background walk lands.
    index: Option<VaultIndex>,
    /// True while a walk is in flight (drives the header spinner label).
    indexing: bool,
    /// When the current index finished building (staleness gate).
    built_at: Option<Instant>,
    root: VaultRoot,
    view: VaultView,
    filter_editor: Entity<Editor>,
    /// Per-bundle promote UI state.
    promote: PromoteState,
    /// GRAPH view scroll position.
    graph_scroll: gpui::ScrollHandle,
    /// The in-flight index task (kept so it isn't dropped/cancelled).
    _index_task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl VaultBrowserPanel {
    pub fn new(
        workspace: WeakEntity<Workspace>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let fs = <dyn Fs>::global(cx);
        let filter_editor = cx.new(|cx| {
            let mut editor = Editor::single_line(window, cx);
            editor.set_placeholder_text("Filter by title or tag…", window, cx);
            editor
        });
        let filter_sub = cx.subscribe(&filter_editor, |_, _, event, cx| {
            if let editor::EditorEvent::BufferEdited = event {
                cx.notify();
            }
        });
        let mut this = Self {
            focus_handle: cx.focus_handle(),
            workspace,
            fs,
            index: None,
            indexing: false,
            built_at: None,
            root: VaultRoot::Vault,
            view: VaultView::List,
            filter_editor,
            promote: PromoteState::default(),
            graph_scroll: gpui::ScrollHandle::new(),
            _index_task: None,
            _subscriptions: vec![filter_sub],
        };
        // Build the index on construction (the panel lazy-builds on first mode
        // switch anyway — RUST_PORT_NOTES §8 — so this is the "first open").
        this.start_index(cx);
        this
    }

    /// Kick a background index walk over both roots. Idempotent-ish: a second
    /// call replaces the in-flight task (the newer walk wins).
    fn start_index(&mut self, cx: &mut Context<Self>) {
        self.indexing = true;
        let fs = self.fs.clone();
        let task = cx.spawn(async move |this, cx| {
            let index = cx
                .background_spawn(async move { build_index(fs).await })
                .await;
            this.update(cx, |this, cx| {
                this.index = Some(index);
                this.indexing = false;
                this.built_at = Some(Instant::now());
                cx.notify();
            })
            .ok();
        });
        self._index_task = Some(task);
    }

    /// Manual refresh (header button) — always rebuilds.
    pub(super) fn refresh(&mut self, cx: &mut Context<Self>) {
        self.start_index(cx);
        cx.notify();
    }

    /// Re-index if the built index is older than `STALE_AFTER` (called on
    /// re-open / focus). No-op while a walk is already in flight.
    fn reindex_if_stale(&mut self, cx: &mut Context<Self>) {
        if self.indexing {
            return;
        }
        let stale = self
            .built_at
            .is_none_or(|t| t.elapsed() > STALE_AFTER);
        if stale {
            self.start_index(cx);
        }
    }

    pub(super) fn set_root(&mut self, root: VaultRoot, cx: &mut Context<Self>) {
        if self.root != root {
            self.root = root;
            cx.notify();
        }
    }

    pub(super) fn set_view(&mut self, view: VaultView, cx: &mut Context<Self>) {
        if self.view != view {
            self.view = view;
            cx.notify();
        }
    }

    // ── header accessors (read by panel_header.rs, the split-out chrome) ──

    pub(super) fn index(&self) -> Option<&VaultIndex> {
        self.index.as_ref()
    }

    pub(super) fn root(&self) -> VaultRoot {
        self.root
    }

    pub(super) fn view(&self) -> VaultView {
        self.view
    }

    pub(super) fn is_indexing(&self) -> bool {
        self.indexing
    }

    pub(super) fn filter_editor(&self) -> Entity<Editor> {
        self.filter_editor.clone()
    }

    // ── interaction plumbing (list/graph rows call these via the weak entity) ──

    /// Open a vault md file in a center editor tab. DEFERRED out of the click
    /// listener (§11: `open_abs_path` is a layout mutation reached
    /// synchronously from a row's `on_click`, which runs inside this panel's
    /// update — defer the workspace routing one cycle).
    pub fn request_open(&mut self, abs_path: PathBuf, window: &Window, cx: &mut Context<Self>) {
        let workspace = self.workspace.clone();
        cx.defer_in(window, move |_, window, cx| {
            workspace
                .update(cx, |workspace, cx| {
                    workspace
                        .open_abs_path(
                            abs_path,
                            OpenOptions {
                                visible: Some(OpenVisible::None),
                                ..Default::default()
                            },
                            window,
                            cx,
                        )
                        .detach_and_log_err(cx);
                })
                .ok();
        });
    }

    /// First promote click → arm (show Confirm/Cancel). Pure state flip.
    pub fn arm_promote(&mut self, key: String, cx: &mut Context<Self>) {
        self.promote.set(key, BundleState::Armed);
        cx.notify();
    }

    /// Dismiss the armed affordance. Pure state flip.
    pub fn cancel_promote(&mut self, key: String, cx: &mut Context<Self>) {
        self.promote.set(key, BundleState::Rest);
        cx.notify();
    }

    /// Confirm → run the fs move on the background executor, re-index on
    /// success. The move is a filesystem mutation reached from a click; mark
    /// in-flight synchronously (immediate feedback), spawn the move (already
    /// off the listener — `cx.spawn` schedules, does not re-enter).
    pub fn confirm_promote(
        &mut self,
        key: String,
        bundle_dir: PathBuf,
        cx: &mut Context<Self>,
    ) {
        self.promote.set(key.clone(), BundleState::InFlight);
        cx.notify();
        let fs = self.fs.clone();
        cx.spawn(async move |this, cx| {
            let outcome = cx
                .background_spawn(async move { promote::promote_bundle(fs, bundle_dir).await })
                .await;
            this.update(cx, |this, cx| {
                match outcome {
                    Ok(o) => this.promote.set(
                        key,
                        BundleState::Promoted {
                            collided: o.renamed_for_collision,
                        },
                    ),
                    Err(e) => this
                        .promote
                        .set(key, BundleState::Failed(short_error(&e.to_string()))),
                }
                // Rebuild so the bundle leaves staging + appears in the vault.
                this.start_index(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

/// Trim an fs error to a short inline string (drop the noisy path suffix).
fn short_error(msg: &str) -> String {
    msg.lines().next().unwrap_or(msg).chars().take(48).collect()
}

impl Render for VaultBrowserPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let panel_bg = cx.theme().colors().panel_background;
        let header = self.render_header(cx);
        let body = self.render_body_element(cx);

        v_flex()
            .key_context("VaultBrowserPanel")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(panel_bg)
            .child(header)
            .child(div().flex_1().min_h_0().child(body))
    }
}

impl VaultBrowserPanel {
    /// Build the body element: the indexing placeholder until the first walk
    /// lands, else the list/graph for the active root.
    fn render_body_element(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let filter = self.filter_editor.read(cx).text(cx);
        // Take the index out to sever the `&self.index` borrow across the
        // `&mut self` body-building call, then put it back (no clone of the
        // doc vec — a cheap Option swap).
        let Some(index) = self.index.take() else {
            return super::style::empty_state(
                IconName::Sparkle,
                "Indexing the vault…",
                "Walking the live vault and import staging.",
                cx,
            )
            .into_any_element();
        };
        let body = self.render_body(&index, &filter, cx);
        self.index = Some(index);
        body
    }

    /// The list/graph body for the active root, plus the honest empty states
    /// when a root's directory is missing (spec §6).
    fn render_body(
        &self,
        index: &VaultIndex,
        filter: &str,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let present = match self.root {
            VaultRoot::Vault => index.vault_present,
            VaultRoot::Staging => index.staging_present,
        };
        if !present {
            let (headline, copy) = match self.root {
                VaultRoot::Vault => (
                    "Vault not found",
                    "Looked for the live vault at L:\\Projects\\atlas-vault.",
                ),
                VaultRoot::Staging => (
                    "No staged sessions",
                    "Looked in vault-import-staging — run `import run` to stage sessions.",
                ),
            };
            return super::style::empty_state(IconName::FolderOpen, headline, copy, cx)
                .into_any_element();
        }

        let docs = index.docs_for(self.root);
        // Staging with zero docs → the "run import run" hint (spec §6).
        if docs.is_empty() && self.root == VaultRoot::Staging {
            return super::style::empty_state(
                IconName::Envelope,
                "No staged sessions",
                "Run `import run` to stage sessions here for review.",
                cx,
            )
            .into_any_element();
        }

        let weak = cx.weak_entity();
        match self.view {
            VaultView::List => {
                super::list::list_view(&docs, self.root, filter, &self.promote, weak, cx)
            }
            VaultView::Graph => {
                super::graph::graph_view(&docs, &self.graph_scroll, weak, cx)
            }
        }
    }
}

impl Focusable for VaultBrowserPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<PanelEvent> for VaultBrowserPanel {}

impl Panel for VaultBrowserPanel {
    fn persistent_name() -> &'static str {
        "VaultBrowserPanel"
    }

    fn panel_key() -> &'static str {
        "VaultBrowserPanel"
    }

    fn position(&self, _window: &Window, _cx: &App) -> DockPosition {
        DockPosition::Left
    }

    fn position_is_valid(&self, position: DockPosition) -> bool {
        matches!(position, DockPosition::Left | DockPosition::Right)
    }

    fn set_position(&mut self, _position: DockPosition, _: &mut Window, _cx: &mut Context<Self>) {
        // Left-dock native surface; position is fixed (modes never touch the
        // left dock, but the user may drag it — no persisted state in v1).
    }

    fn default_size(&self, _window: &Window, _cx: &App) -> Pixels {
        px(340.)
    }

    fn set_active(&mut self, active: bool, _window: &mut Window, cx: &mut Context<Self>) {
        // On re-open (becoming active), re-index if the last build is stale
        // (>60s) — spec §5. No fs watcher in v1.
        if active {
            self.reindex_if_stale(cx);
        }
    }

    fn icon(&self, _window: &Window, _cx: &App) -> Option<IconName> {
        Some(IconName::FileTree)
    }

    fn icon_tooltip(&self, _window: &Window, _cx: &App) -> Option<&'static str> {
        Some("OKF vault browser")
    }

    fn toggle_action(&self) -> Box<dyn Action> {
        Box::new(ToggleFocus)
    }

    fn activation_priority(&self) -> u32 {
        // The fork's 14-20 band is taken; 21 is the next free slot (spec §1).
        21
    }
}
