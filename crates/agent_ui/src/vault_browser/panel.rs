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
    Action, App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable,
    MouseButton, Pixels, Point, SharedString, Subscription, Task, WeakEntity, Window, actions,
};
use ui::prelude::*;
use workspace::{
    OpenOptions, OpenVisible, Workspace,
    dock::{DockPosition, Panel, PanelEvent},
};

use super::field::VaultField;
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

/// LIST sort (galaxy backlog 2026-07-08): grouped-by-kind (the OKF section
/// order) or one flat recency stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortMode {
    Kind,
    Updated,
}

/// GRAPH edge filter (galaxy backlog 2026-07-08): declutter by isolating
/// semantic wiki-links or the supersedes chains; Structural spine only ever
/// paints under All.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeMode {
    All,
    Links,
    Supersedes,
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
    sort: SortMode,
    edges: EdgeMode,
    filter_editor: Entity<Editor>,
    /// Per-bundle promote UI state.
    promote: PromoteState,
    /// GRAPH view scroll position.
    graph_scroll: gpui::ScrollHandle,
    /// Field sim-clock origin (ms are measured from here).
    field_epoch: Instant,
    /// The GRAPH view's physics field (shared constellation stepper) — built
    /// lazily from the active root's docs, advanced once per frame while the
    /// graph is showing, idle-cost zero once settled.
    graph_field: VaultField,
    /// The doc id whose graph hover card is up.
    graph_hovered: Option<SharedString>,
    /// An in-flight graph node drag (pointer re-aims the anchor 1:1).
    graph_drag: Option<GraphDrag>,
    /// A drag that moved suppresses the click it lands on.
    graph_suppress_click: bool,
    /// A pending "reveal in list": a session id to select after the view flips.
    graph_reveal: Option<SharedString>,
    /// The in-flight index task (kept so it isn't dropped/cancelled).
    _index_task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

/// An in-flight graph node drag: pointer deltas re-aim the field anchor 1:1
/// (the constellation drag idiom).
struct GraphDrag {
    id: SharedString,
    start_mouse: gpui::Point<Pixels>,
    start_anchor: (f32, f32),
    moved: bool,
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
            sort: SortMode::Kind,
            edges: EdgeMode::All,
            filter_editor,
            promote: PromoteState::default(),
            graph_scroll: gpui::ScrollHandle::new(),
            field_epoch: Instant::now(),
            graph_field: VaultField::default(),
            graph_hovered: None,
            graph_drag: None,
            graph_suppress_click: false,
            graph_reveal: None,
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

    pub(super) fn set_sort(&mut self, sort: SortMode, cx: &mut Context<Self>) {
        if self.sort != sort {
            self.sort = sort;
            cx.notify();
        }
    }

    pub(super) fn set_edges(&mut self, edges: EdgeMode, cx: &mut Context<Self>) {
        if self.edges != edges {
            self.edges = edges;
            cx.notify();
        }
    }

    pub(super) fn sort(&self) -> SortMode {
        self.sort
    }

    pub(super) fn edges(&self) -> EdgeMode {
        self.edges
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

    // ── graph field interaction (graph_render.rs calls these) ──

    /// Begin dragging a graph node — record its current anchor so pointer
    /// deltas re-aim it 1:1 (constellation drag idiom). Pure state flip.
    pub fn begin_graph_drag(
        &mut self,
        id: SharedString,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let Some(anchor) = self
            .graph_field
            .nodes
            .iter()
            .find(|n| n.id == id.as_ref())
            .map(|n| (n.ax, n.ay))
        else {
            return;
        };
        self.graph_field.begin_drag(&id);
        self.graph_drag = Some(GraphDrag {
            id,
            start_mouse: position,
            start_anchor: anchor,
            moved: false,
        });
        cx.notify();
    }

    fn graph_drag_moved(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(drag) = &mut self.graph_drag else {
            return;
        };
        let dx = (position.x - drag.start_mouse.x).as_f32();
        let dy = (position.y - drag.start_mouse.y).as_f32();
        if dx.abs() + dy.abs() > 4. {
            drag.moved = true;
        }
        let id = drag.id.clone();
        let (ax, ay) = (drag.start_anchor.0 + dx, drag.start_anchor.1 + dy);
        self.graph_field.drag_to(&id, ax, ay);
        cx.notify();
    }

    fn graph_drag_ended(&mut self, cx: &mut Context<Self>) {
        if let Some(drag) = self.graph_drag.take() {
            self.graph_suppress_click = drag.moved;
            self.graph_field.end_drag(self.field_now());
            cx.notify();
        }
    }

    /// The field sim clock (ms since the panel was created) — monotonic, so
    /// every field transition is measured against the same origin.
    fn field_now(&self) -> f32 {
        self.field_epoch.elapsed().as_secs_f32() * 1000.
    }

    /// Hover a graph node (raise/lower its card). Pure state flip.
    pub fn set_graph_hover(&mut self, id: SharedString, hovered: bool, cx: &mut Context<Self>) {
        if hovered {
            if self.graph_hovered.as_ref() != Some(&id) {
                self.graph_hovered = Some(id);
                cx.notify();
            }
        } else if self.graph_hovered.as_ref() == Some(&id) {
            self.graph_hovered = None;
            cx.notify();
        }
    }

    /// Double-click a SESSION node → reveal it in the LIST view: flip the view
    /// toggle and stage the id so the list can scroll/select it (integration
    /// hook §3). A drag that moved is not a click.
    pub fn reveal_in_list(&mut self, id: SharedString, cx: &mut Context<Self>) {
        if std::mem::take(&mut self.graph_suppress_click) {
            return;
        }
        self.graph_reveal = Some(id);
        self.view = VaultView::List;
        cx.notify();
    }

    /// The id a "reveal in list" staged (the list highlights it once). Consumed
    /// on read so the highlight is a one-shot.
    pub(super) fn take_graph_reveal(&mut self) -> Option<SharedString> {
        self.graph_reveal.take()
    }

    /// Show a list row's doc in the GRAPH ("show in graph" affordance §3): flip
    /// to the graph view. The field already carries every doc, so the node is
    /// present; a future slice can pan/select it.
    pub fn show_in_graph(&mut self, cx: &mut Context<Self>) {
        self.view = VaultView::Graph;
        cx.notify();
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let panel_bg = cx.theme().colors().panel_background;
        let header = self.render_header(cx);
        let (body, animating, graph_active) = self.render_body_element(cx);

        // Frame pump (§8): the panel schedules the next frame ONLY while the
        // graph field is hot (settling, dragging). A settled field, the list
        // view, or an empty panel pumps nothing — idle cost zero.
        if animating {
            window.request_animation_frame();
        }

        let mut root = v_flex()
            .key_context("VaultBrowserPanel")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(panel_bg);
        // Graph drag: pointer deltas re-aim the anchor while the button holds.
        if graph_active {
            root = root
                .on_mouse_move(cx.listener(|this, event: &gpui::MouseMoveEvent, _, cx| {
                    if this.graph_drag.is_some() {
                        this.graph_drag_moved(event.position, cx);
                    }
                }))
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| this.graph_drag_ended(cx)),
                );
        }
        root.child(header)
            .child(div().flex_1().min_h_0().child(body))
    }
}

impl VaultBrowserPanel {
    /// Build the body element (returns the element, whether a frame pump is
    /// needed, and whether the graph is the active surface — for drag wiring).
    /// The indexing placeholder shows until the first walk lands.
    fn render_body_element(&mut self, cx: &mut Context<Self>) -> (gpui::AnyElement, bool, bool) {
        let filter = self.filter_editor.read(cx).text(cx);
        // Take the index out to sever the `&self.index` borrow across the
        // `&mut self` body-building call, then put it back (a cheap swap).
        let Some(index) = self.index.take() else {
            let el = super::style::empty_state(
                IconName::Sparkle,
                "Indexing the vault…",
                "Walking the live vault and import staging.",
                cx,
            )
            .into_any_element();
            return (el, false, false);
        };
        let out = self.render_body(&index, &filter, cx);
        self.index = Some(index);
        out
    }

    /// The list/graph body for the active root, plus the honest empty states
    /// when a root's directory is missing (spec §6). Returns `(element,
    /// animating, graph_active)`.
    fn render_body(
        &mut self,
        index: &VaultIndex,
        filter: &str,
        cx: &mut Context<Self>,
    ) -> (gpui::AnyElement, bool, bool) {
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
            let el = super::style::empty_state(IconName::FolderOpen, headline, copy, cx)
                .into_any_element();
            return (el, false, false);
        }

        let docs = index.docs_for(self.root);
        // Staging with zero docs → the "run import run" hint (spec §6).
        if docs.is_empty() && self.root == VaultRoot::Staging {
            let el = super::style::empty_state(
                IconName::Envelope,
                "No staged sessions",
                "Run `import run` to stage sessions here for review.",
                cx,
            )
            .into_any_element();
            return (el, false, false);
        }

        match self.view {
            VaultView::List => {
                let reveal = self.take_graph_reveal();
                let weak = cx.weak_entity();
                let el = super::list::list_view(
                    &docs,
                    self.root,
                    filter,
                    self.sort,
                    &self.promote,
                    reveal.as_deref(),
                    weak,
                    cx,
                );
                (el, false, false)
            }
            VaultView::Graph => {
                // Fold: (re)build the field when the doc set (identity) changed
                // — a promote re-indexes and the fold picks it up with no jump
                // (§3); pins + live displacement carry across. Below the field
                // ceiling only (the static fallback needs no field).
                let now = self.field_now();
                let degraded = docs.len() > super::field::FIELD_MAX_NODES;
                let animating = if degraded {
                    false
                } else {
                    if !self.graph_field.matches(&docs) {
                        self.graph_field
                            .rebuild(&docs, super::graph::node_size, now);
                    }
                    self.graph_field.advance(now)
                };
                let hovered = self.graph_hovered.clone();
                let weak = cx.weak_entity();
                let el = super::graph::graph_view(
                    &self.graph_field,
                    &docs,
                    &self.graph_scroll,
                    hovered.as_deref(),
                    self.edges,
                    weak,
                    cx,
                );
                (el, animating, true)
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
