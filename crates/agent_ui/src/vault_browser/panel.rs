//! The vault-browser `Panel` (LEFT dock). Owns the built [`VaultIndex`], the
//! filter editor, the folder-tree collapse state, and the root/view toggles.
//! Indexing runs on a background task (`cx.background_spawn`) built on first
//! open; a manual refresh + a re-open-when-stale (>60s) rebuild it.
//!
//! Dispatch law (RUST_PORT_NOTES §11): opening a file (raw or reading view),
//! promoting a bundle, and creating a note are LAYOUT/fs mutations reached
//! from click listeners — they are deferred out of the listener via
//! `window.defer`/spawned continuations (see `open_doc.rs`, `promote.rs`,
//! `new_note.rs`). Fold/filter/root flips are pure entity-state and stay
//! synchronous (immediate visual feedback).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use editor::Editor;
use fs::Fs;
use gpui::{
    Action, App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable,
    MouseButton, Pixels, SharedString, Subscription, Task, WeakEntity, Window, actions,
};
use markdown_preview::markdown_preview_view::MarkdownPreviewView;
use ui::prelude::*;
use workspace::{
    Workspace,
    dock::{DockPosition, Panel, PanelEvent},
};

use super::axioms::AxiomsState;
use super::field::VaultField;
use super::importers_ops::ImportersState;
use super::index::{VaultIndex, VaultRoot, build_index};
use super::new_note::NewNoteState;
use super::panel_graph::GraphDrag;
use super::promote::PromoteState;
use super::routines::RoutinesState;
use super::writeback_state::WritebackState;

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
/// AXIOMS is the third segment (bridge-fed candidate review, not vault fs);
/// ROUTINES is the fourth (the bridge's croniter table — definitions ARE
/// vault files, but the live schedule/consent state lives on the bridge);
/// WRITEBACK is the fifth (run→note candidates + import conflicts, the
/// bridge's promote-on-read staging); IMPORT is the sixth (the importers
/// trigger/progress surface the staging empty-state hint points at — see
/// `importers.rs` for the discovered two-family contract).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VaultView {
    List,
    Graph,
    Axioms,
    Routines,
    Writeback,
    Import,
}

impl VaultView {
    /// The bridge-fed bodies (no fs index, no vault filter box).
    pub(super) fn is_bridge_fed(self) -> bool {
        matches!(
            self,
            VaultView::Axioms | VaultView::Routines | VaultView::Writeback | VaultView::Import
        )
    }
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
    pub(super) workspace: WeakEntity<Workspace>,
    pub(super) fs: Arc<dyn Fs>,
    /// The built index, `None` until the first background walk lands
    /// (pub(super) for `panel_body`'s take/put borrow-sever swap).
    pub(super) index: Option<VaultIndex>,
    /// True while a walk is in flight (drives the header spinner label).
    indexing: bool,
    /// When the current index finished building (staleness gate).
    built_at: Option<Instant>,
    root: VaultRoot,
    view: VaultView,
    edges: EdgeMode,
    filter_editor: Entity<Editor>,
    /// Collapsed tree-section keys (session-local, the Obsidian folder state).
    pub(super) collapsed: HashSet<SharedString>,
    /// Per-bundle promote UI state.
    pub(super) promote: PromoteState,
    /// AXIOMS view state (bridge snapshot + fetch lifecycle + in-flight
    /// approve/reject marks) — owned by `axioms.rs`, stored here.
    pub(super) axioms: AxiomsState,
    /// ROUTINES view state (bridge snapshot + fetch lifecycle + in-flight
    /// action marks + verbatim action notes) — owned by `routines.rs`.
    pub(super) routines: RoutinesState,
    /// WRITEBACK view state (run→note candidates + import conflicts + the
    /// conflict modal) — owned by `writeback_state.rs`, stored here.
    pub(super) writeback: WritebackState,
    /// IMPORT view state (importer snapshot + compose + the watched job) —
    /// owned by `importers_ops.rs`, stored here.
    pub(super) importers: ImportersState,
    /// "+ New note" compose state — owned by `new_note.rs`, stored here.
    pub(super) new_note: NewNoteState,
    /// Open reading-view tabs by absolute path (dedup — a re-click activates
    /// the existing tab instead of stacking; see `open_doc.rs`).
    pub(super) previews: HashMap<PathBuf, WeakEntity<MarkdownPreviewView>>,
    /// GRAPH view scroll position.
    pub(super) graph_scroll: gpui::ScrollHandle,
    /// Field sim-clock origin (ms are measured from here).
    pub(super) field_epoch: Instant,
    /// The GRAPH view's physics field (shared constellation stepper) — built
    /// lazily from the active root's docs, advanced once per frame while the
    /// graph is showing, idle-cost zero once settled.
    pub(super) graph_field: VaultField,
    /// The doc id whose graph hover card is up.
    pub(super) graph_hovered: Option<SharedString>,
    /// An in-flight graph node drag (pointer re-aims the anchor 1:1).
    pub(super) graph_drag: Option<GraphDrag>,
    /// A drag that moved suppresses the click it lands on.
    pub(super) graph_suppress_click: bool,
    /// A pending "reveal in list": a session id to select after the view flips.
    pub(super) graph_reveal: Option<SharedString>,
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
            editor.set_placeholder_text("Filter notes…", window, cx);
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
            edges: EdgeMode::All,
            filter_editor,
            collapsed: HashSet::default(),
            promote: PromoteState::default(),
            axioms: AxiomsState::default(),
            routines: RoutinesState::default(),
            writeback: WritebackState::default(),
            importers: ImportersState::default(),
            new_note: NewNoteState::default(),
            previews: HashMap::default(),
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

    /// Manual refresh (header button, post-promote, post-note-create) —
    /// always rebuilds (and refetches the active bridge-fed view — axioms or
    /// routines — so refresh means refresh there too).
    pub(super) fn refresh(&mut self, cx: &mut Context<Self>) {
        self.start_index(cx);
        match self.view {
            VaultView::Axioms => self.fetch_axioms(cx),
            VaultView::Routines => self.fetch_routines(cx),
            VaultView::Writeback => self.fetch_writeback(cx),
            VaultView::Import => self.fetch_importers(cx),
            VaultView::List | VaultView::Graph => {}
        }
        cx.notify();
    }

    /// Re-index if the built index is older than `STALE_AFTER` (called on
    /// re-open / focus). No-op while a walk is already in flight.
    fn reindex_if_stale(&mut self, cx: &mut Context<Self>) {
        if self.indexing {
            return;
        }
        let stale = self.built_at.is_none_or(|t| t.elapsed() > STALE_AFTER);
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
            // Flip-to-a-bridge-fed-view refreshes its snapshot — a
            // visibility-gated one-shot, same as the briefing panel. §11-safe
            // from the seg listener: both fetches are a state flip + `cx.spawn`
            // (schedules, never re-enters this update).
            match view {
                VaultView::Axioms => self.fetch_axioms(cx),
                VaultView::Routines => self.fetch_routines(cx),
                VaultView::Writeback => self.fetch_writeback(cx),
                VaultView::Import => self.fetch_importers(cx),
                VaultView::List | VaultView::Graph => {}
            }
            cx.notify();
        }
    }

    pub(super) fn set_edges(&mut self, edges: EdgeMode, cx: &mut Context<Self>) {
        if self.edges != edges {
            self.edges = edges;
            cx.notify();
        }
    }

    /// Fold/unfold a tree section (header chevron click). Pure state flip —
    /// §11-safe synchronous; the list re-flattens on the notify.
    pub fn toggle_section(&mut self, key: SharedString, cx: &mut Context<Self>) {
        if !self.collapsed.remove(&key) {
            self.collapsed.insert(key);
        }
        cx.notify();
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

    /// Retain a subscription created by a split-out impl (new_note's title
    /// editor).
    pub(super) fn push_subscription(&mut self, subscription: Subscription) {
        self._subscriptions.push(subscription);
    }
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
        // (>60s) — spec §5. No fs watcher in v1. The bridge-fed views refetch
        // on the same visibility edge (their data lives on the bridge, not
        // the fs).
        if active {
            self.reindex_if_stale(cx);
            match self.view {
                VaultView::Axioms => self.fetch_axioms(cx),
                VaultView::Routines => self.fetch_routines(cx),
                VaultView::Writeback => self.fetch_writeback(cx),
                VaultView::Import => self.fetch_importers(cx),
                VaultView::List | VaultView::Graph => {}
            }
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
