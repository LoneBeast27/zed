//! The PERMISSIONS panel (Phase-2 permissions design §5.1, wave 3b) — the
//! center-pane mode surface for the bridge's spawn-gate policy engine:
//!
//! 1. the effective-mode card (Read Only / Auto Edit / Full Access — POSTs
//!    `/permissions/mode`, session scope with a followed conversation, user
//!    scope without one, provenance chain rendered),
//! 2. the per-vendor ENFORCEMENT honesty card (grafted from Design B — the
//!    trust surface: what the effective mode ACTUALLY binds on each lane,
//!    rendered VERBATIM, amber when `degraded:`/`pending:`),
//! 3. the rules card (read-only rows + source badges; `config_errors`
//!    verbatim in amber; jump-to-json — Zed philosophy, config lives in the
//!    file, no rule CRUD here),
//! 4. the pending-approvals inbox (consumes [`BridgeStore::pending_approvals`]
//!    + a held [`ApprovalWatch`] while visible; Allow / Deny via the wave-3a
//!    helpers, deny prompts for an optional note),
//! 5. the audit tail (the last 20 ledger decisions).
//!
//! All Phase-1 panel laws apply (`routines_view`/`writeback` recipe):
//! `snapshot: Option<T>` three honest states, stale-under-amber, one-shot
//! visibility-gated fetch (never free-polled), in-flight collapse, client-side
//! re-guard before I/O, refetch-server-truth on 2xx, `{"error"}` verbatim on
//! refusal, "bridge unreachable" on transport failure.
//!
//! Concern split (CLAUDE.md modularity, the routines 3-file precedent, each
//! render half re-split at the 500-line ceiling): [`data`] wire types + pure
//! predicates, [`view`] the render shell + shared builders, [`policy_cards`]
//! the mode/enforcement/rules cards, [`inbox`] the approvals inbox + audit
//! tail, [`actions`] fetch/POST, [`demo`] the staged agentic-demo snapshot.

mod actions;
pub mod data;
mod demo;
mod inbox;
mod policy_cards;
mod view;

use std::collections::{HashMap, HashSet};

use editor::Editor;
use gpui::{
    App, Context, Entity, FocusHandle, Focusable, SharedString, Subscription, Task, WeakEntity,
    actions,
};
use ui::prelude::*;
use workspace::Workspace;

use crate::bridge::{self, ApprovalWatch, BridgeStore};

use data::PermissionsSnapshot;

actions!(
    permissions_panel,
    [
        /// Toggles focus on the permissions panel.
        ToggleFocus
    ]
);

/// The in-flight/notes key for the mode picker (one mode POST at a time;
/// approval rows key on their approval id).
const MODE_KEY: &str = "mode";

/// The 1s roll for TTL countdowns + audit "ago" labels while visible.
const AGE_TICK: std::time::Duration = std::time::Duration::from_secs(1);

pub struct PermissionsPanel {
    focus_handle: FocusHandle,
    /// For the jump-to-json buttons (`open_abs_path` routed through the
    /// workspace, deferred out of listeners — the open_doc idiom).
    workspace: WeakEntity<Workspace>,
    store: Entity<BridgeStore>,
    /// `None` until the first `GET /permissions` lands (loading /
    /// unreachable states); kept last-known under the stale flag after.
    snapshot: Option<PermissionsSnapshot>,
    /// The last fetch (or a POST's transport leg) failed — amber stale note.
    stale: bool,
    /// A fetch is in flight (first open shows "Loading…", never a blank).
    loading: bool,
    /// Keys with a POST in flight ([`MODE_KEY`] or an approval id) —
    /// buttons collapse to "working…" so a double-click can't double-POST.
    in_flight: HashSet<String>,
    /// Last action outcome per key, VERBATIM (a 409 refusal reads as a
    /// refusal, not offline).
    notes: HashMap<String, String>,
    /// The approval id whose deny-note input row is open, if any.
    deny_prompt: Option<String>,
    /// Lazily-created single-line note editor (needs a window at open time).
    note_editor: Option<Entity<Editor>>,
    /// Held only while the center tab shows this panel — its presence keeps
    /// the store's `/approvals` poll alive (`ModeSurface::set_surface_active`,
    /// the TranscriptWatch pattern).
    watch: Option<ApprovalWatch>,
    /// The 1s ticker for TTL countdowns — visible-only (Lightness law).
    ticker: Option<Task<()>>,
    /// The in-flight fetch (kept so it isn't dropped/cancelled).
    fetch_task: Option<Task<()>>,
    _store_subscription: Subscription,
}

impl PermissionsPanel {
    pub fn new(workspace: WeakEntity<Workspace>, cx: &mut Context<Self>) -> Self {
        let store = bridge::global_store(cx);
        // The store change-gates its own notifies (pending set replace-on-
        // frame, connected flips); this panel renders cheap text rows, so a
        // plain re-render per store notify is in budget.
        let _store_subscription = cx.observe(&store, |_, _, cx| cx.notify());
        // Agentic-demo gate: seed the staged policy state so the panel
        // reviews without a live bridge (POSTs are not offered in demo).
        let snapshot = bridge::is_agentic_demo().then(demo::demo_snapshot);
        Self {
            focus_handle: cx.focus_handle(),
            workspace,
            store,
            snapshot,
            stale: false,
            loading: false,
            in_flight: HashSet::new(),
            notes: HashMap::new(),
            deny_prompt: None,
            note_editor: None,
            watch: None,
            ticker: None,
            fetch_task: None,
            _store_subscription,
        }
    }

    /// The pending rows the inbox renders: the staged demo rows under the
    /// demo gate, else the store's live set (SSE `permission` frames + the
    /// watch-gated `/approvals` poll — two feeds, never disagreeing).
    fn pending_rows(&self, cx: &App) -> Vec<bridge::ApprovalRow> {
        if bridge::is_agentic_demo() {
            demo::demo_pending()
        } else {
            self.store.read(cx).pending_approvals.clone()
        }
    }
}

impl Focusable for PermissionsPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl crate::mode_item::ModeSurface for PermissionsPanel {
    fn fallback_tab_title() -> SharedString {
        "Permissions".into()
    }

    fn fallback_tab_icon() -> IconName {
        IconName::LockOutlined
    }

    fn set_surface_active(&mut self, active: bool, cx: &mut Context<Self>) {
        // The native route mount/unmount: the `/approvals` poll watch, the
        // one-shot snapshot fetch, and the 1s TTL ticker live exactly as
        // long as the center tab shows this panel. Demo mode holds no watch
        // and fetches nothing — the staged snapshot is already seeded.
        if active {
            if !bridge::is_agentic_demo() {
                if self.watch.is_none() {
                    self.watch = Some(self.store.update(cx, |store, cx| store.watch_approvals(cx)));
                }
                self.fetch_permissions(cx);
            }
            if self.ticker.is_none() {
                self.ticker = Some(cx.spawn(async move |this, cx| {
                    loop {
                        cx.background_executor().timer(AGE_TICK).await;
                        if this.update(cx, |_, cx| cx.notify()).is_err() {
                            return;
                        }
                    }
                }));
            }
        } else {
            self.watch = None;
            self.ticker = None;
        }
    }
}
