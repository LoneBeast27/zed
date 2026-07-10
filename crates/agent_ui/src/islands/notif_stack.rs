//! The corner notification stack ENTITY (PARITY_SPEC §4.9) — toasts that
//! EMERGE from the right edge beneath the usage-island head, ported from
//! `notifications.js`/`notifications.css`. Driven push-first off the
//! `BridgeStore`: run lifecycle transitions (completed → done toast,
//! failed/killed → error toast) and the usage scrape's fresh→stale flip.
//! This module owns lifecycle/state (diffing, surfacing, expiry timers,
//! retract scheduling); the toast element + §4.9 motion live in
//! [`super::notif_card`].
//!
//! Renders in the same custom corner-cluster layer as the island head —
//! consistency of the corner system over the workspace toast hosts (the
//! host comparison lives in `islands/mod.rs`).

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, Context, Entity, SharedString, Subscription, Task, WeakEntity, Window,
};
use ui::prelude::*;
use workspace::Workspace;

use crate::bridge::{
    self, ApprovalWatch, BridgeStore, post_approval_allow, post_approval_deny,
};
use crate::task_board::motion::{AnimatedValue, DECEL, EFFECTS, MotionCurve};

use super::notif_card::{
    EMERGE, FADE, FALLBACK_H, RETRACT, RETRACT_SLIDE, Toast, X_REVEAL, render_toast,
};
use super::notif_logic::{
    MAX_VISIBLE, StaleLatch, TOAST_TIMEOUT, ToastPayload, diff_board, stale_payload,
    sync_approvals,
};

/// Countdown repaint cadence while an approval toast is held (the drawer's
/// worked-for tick idiom — alive ONLY while a held toast shows a deadline).
const COUNTDOWN_TICK: Duration = Duration::from_secs(1);

/// Stack width — web `#notif-stack { width: 332px }`.
pub const STACK_W: f32 = 332.;

/// The rendered stack width for a viewport — web `max-width: calc(100vw -
/// 28px)` (notifications.css:18): below ~360px workspace width toasts
/// SHRINK instead of being clipped by the workspace overflow.
pub fn clamped_width(viewport_w: f32) -> f32 {
    (viewport_w - 28.).clamp(0., STACK_W)
}

pub struct NotifStack {
    store: Entity<BridgeStore>,
    workspace: WeakEntity<Workspace>,
    prev_runs: HashMap<String, String>,
    primed: bool,
    stale_latch: StaleLatch,
    /// Dedupe: stable ids already surfaced never double-fire. For approval
    /// toasts this doubles as the dismissed-but-still-pending memory (pruned
    /// by [`sync_approvals`] when an approval resolves or the feed goes
    /// stale).
    seen: HashSet<String>,
    /// Newest first (top of the top-right stack).
    toasts: Vec<Toast>,
    /// Held while ≥1 approval toast is up — keeps the `/approvals` poll
    /// fallback alive so the toast retires even without SSE (§5.2: the poll
    /// runs ONLY while a toast is held or the panel visible).
    approval_watch: Option<ApprovalWatch>,
    /// 1s repaint driver for the held toasts' expiry countdown — `Some` only
    /// while an approval toast is up (no idle timers).
    countdown_ticker: Option<Task<()>>,
    _store_subscription: Subscription,
}

impl NotifStack {
    pub fn new(workspace: WeakEntity<Workspace>, cx: &mut Context<Self>) -> Self {
        let store = bridge::global_store(cx);
        let _store_subscription =
            cx.observe(&store, |this: &mut Self, _, cx| this.sync_from_store(cx));
        Self {
            store,
            workspace,
            prev_runs: HashMap::new(),
            primed: false,
            stale_latch: StaleLatch::default(),
            seen: HashSet::new(),
            toasts: Vec::new(),
            approval_watch: None,
            countdown_ticker: None,
            _store_subscription,
        }
    }

    fn sync_from_store(&mut self, cx: &mut Context<Self>) {
        let store = self.store.read(cx);
        let board = store.board.clone();
        let stale = store.usage_meta.stale();
        let scrape = store.usage_meta.scraped.clone();
        let connected = store.connected;
        let pending = store.pending_approvals.clone();
        let payloads = diff_board(&mut self.prev_runs, &mut self.primed, &board);
        let mut changed = false;
        for payload in payloads {
            changed |= self.surface(payload, cx);
        }
        if self.stale_latch.flip(stale) {
            changed |= self.surface(stale_payload(scrape.as_ref()), cx);
        }
        // Held approval toasts (Phase-2 §5.2): reconcile against the live
        // pending set — replace semantics retire on the empty frame, the
        // fresh→stale latch retires on disconnect.
        let held: Vec<String> = self
            .toasts
            .iter()
            .filter(|toast| toast.kind.held() && toast.retracting.is_none())
            .map(|toast| toast.id.to_string())
            .collect();
        let (surface, retire) = sync_approvals(connected, &pending, &held, &mut self.seen);
        for payload in surface {
            changed |= self.surface(payload, cx);
        }
        for id in retire {
            self.retract(&SharedString::from(id), cx);
            changed = true;
        }
        self.sync_approval_lifelines(cx);
        // The store notifies every second while a run ticks — only repaint
        // the stack when a toast actually surfaced (§8 idle cost).
        if changed {
            cx.notify();
        }
    }

    /// Acquire/drop the `/approvals` poll watch + the 1s countdown repaint
    /// with held-toast presence (Lightness: both live ONLY while a held
    /// approval toast is up).
    fn sync_approval_lifelines(&mut self, cx: &mut Context<Self>) {
        let any_held = self
            .toasts
            .iter()
            .any(|toast| toast.kind.held() && toast.retracting.is_none());
        if any_held {
            if self.approval_watch.is_none() {
                self.approval_watch =
                    Some(self.store.update(cx, |store, cx| store.watch_approvals(cx)));
            }
            if self.countdown_ticker.is_none() {
                self.countdown_ticker = Some(cx.spawn(async move |this, cx| {
                    loop {
                        cx.background_executor().timer(COUNTDOWN_TICK).await;
                        let live = this.update(cx, |this, cx| {
                            let held = this
                                .toasts
                                .iter()
                                .any(|toast| toast.kind.held() && toast.retracting.is_none());
                            if held {
                                cx.notify();
                            } else {
                                this.countdown_ticker = None;
                            }
                            held
                        });
                        if !matches!(live, Ok(true)) {
                            return;
                        }
                    }
                }));
            }
        } else {
            self.approval_watch = None;
            self.countdown_ticker = None;
        }
    }

    /// POST the inline toast decision (§5.2: in-flight collapse, the
    /// bridge's `{"error"}` refusal VERBATIM on failure, and NO local
    /// retirement on success — the pending frame's replace semantics retire
    /// the toast, server truth only).
    pub(super) fn decide(&mut self, id: &SharedString, allow: bool, cx: &mut Context<Self>) {
        let Some(toast) = self.toast_mut(id) else {
            return;
        };
        if toast.deciding || toast.retracting.is_some() {
            return; // in-flight collapse: one POST at a time
        }
        let Some(approval_id) = toast
            .approval
            .as_ref()
            .map(|meta| meta.approval_id.clone())
        else {
            return;
        };
        toast.deciding = true;
        toast.decide_error = None;
        cx.notify();
        let toast_id = id.clone();
        let http_client = cx.http_client();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    if allow {
                        post_approval_allow(http_client.as_ref(), &approval_id, None).await
                    } else {
                        post_approval_deny(http_client.as_ref(), &approval_id, None).await
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                if let Some(toast) = this.toast_mut(&toast_id) {
                    match result {
                        // Success: stay collapsed — the next pending frame
                        // (SSE or the held watch's poll) retires the toast.
                        Ok(_) => {}
                        Err(error) => {
                            // 404/409 refusal or transport error — verbatim,
                            // and the buttons re-arm for a retry.
                            toast.deciding = false;
                            toast.decide_error = Some(format!("{error}").into());
                        }
                    }
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Surface a payload as a new toast (newest first). Returns whether
    /// anything new appeared (dedupe makes re-fires no-ops).
    fn surface(&mut self, payload: ToastPayload, cx: &mut Context<Self>) -> bool {
        if !self.seen.insert(payload.id.clone()) {
            return false;
        }
        let id = SharedString::from(payload.id);
        let held = payload.kind.held();
        // §4.9 emergence: slide in from past the edge (1 → 0) on decel-in
        // while opacity rises on effects — concurrent from frame one.
        let mut slide = AnimatedValue::settled(1.0, DECEL, EMERGE);
        slide.retarget(0.0);
        let mut fade = AnimatedValue::settled(0.0, EFFECTS, FADE);
        fade.retarget(1.0);
        self.toasts.insert(
            0,
            Toast {
                id: id.clone(),
                kind: payload.kind,
                title: SharedString::from(payload.title),
                agent: payload.agent,
                run_id: payload.run_id.map(SharedString::from),
                approval: payload.approval,
                deciding: false,
                decide_error: None,
                slide,
                fade,
                expire: super::notif_logic::ExpireTimer::armed(TOAST_TIMEOUT),
                expire_task: None,
                retracting: None,
                retract_from_h: FALLBACK_H,
                measured_h: FALLBACK_H,
                hovered: false,
                x_reveal: AnimatedValue::settled(0.0, EFFECTS, X_REVEAL),
            },
        );
        // Held toasts (approvals) never arm the auto-retract clock — they
        // retire on decision / stale, not on time (§5.2 "no ExpireTimer").
        if !held {
            self.arm_expire(&id, cx);
        }
        // §8 bounded DOM: retract the oldest beyond the cap. Held approval
        // toasts are exempt in both directions (never evicted by transient
        // toasts — they're a safety surface, and the bridge TTL bounds them).
        let beyond: Vec<SharedString> = self
            .toasts
            .iter()
            .filter(|toast| toast.retracting.is_none() && !toast.kind.held())
            .skip(MAX_VISIBLE)
            .map(|toast| toast.id.clone())
            .collect();
        for id in beyond {
            self.retract(&id, cx);
        }
        true
    }

    pub(super) fn toast_mut(&mut self, id: &SharedString) -> Option<&mut Toast> {
        self.toasts.iter_mut().find(|toast| toast.id == *id)
    }

    /// (Re)arm the expiry task for whatever time the pausable clock has
    /// left. Held toasts never arm (no auto-retract on approvals — a
    /// hover-out on one must not start a clock that was never running).
    fn arm_expire(&mut self, id: &SharedString, cx: &mut Context<Self>) {
        let Some(toast) = self.toast_mut(id) else {
            return;
        };
        if toast.kind.held() {
            return;
        }
        toast.expire.resume();
        let remaining = toast.expire.remaining();
        let id = id.clone();
        toast.expire_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(remaining).await;
            this.update(cx, |this, cx| this.retract(&id, cx)).ok();
        }));
    }

    /// The 3-beat retract: freeze the measured slot height, retarget the
    /// emergence pair onto the sharper exit (continuing from the CURRENT
    /// offset/opacity — a mid-emerge dismiss never snaps to fully-emerged
    /// first, §8.7a), start the slot collapse, and remove the slot when the
    /// envelope settles. All three beats start the same frame.
    pub(super) fn retract(&mut self, id: &SharedString, cx: &mut Context<Self>) {
        let Some(toast) = self.toast_mut(id) else {
            return;
        };
        if toast.retracting.is_some() {
            return;
        }
        toast.retract_from_h = toast.measured_h.max(1.);
        toast.retracting = Some(Instant::now());
        toast
            .slide
            .retarget_with(1.0, MotionCurve::Exit, RETRACT_SLIDE);
        toast.fade.retarget(0.0);
        toast.expire_task = None;
        let id = id.clone();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(RETRACT).await;
            this.update(cx, |this, cx| {
                this.toasts.retain(|toast| toast.id != id);
                // A retracted held toast releases the `/approvals` watch +
                // countdown tick if it was the last one (no idle lifelines).
                this.sync_approval_lifelines(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Hover pauses the expiry clock (and reveals the dismiss ×).
    pub(super) fn set_hover(&mut self, id: &SharedString, hovered: bool, cx: &mut Context<Self>) {
        let id = id.clone();
        let Some(toast) = self.toast_mut(&id) else {
            return;
        };
        if toast.hovered == hovered || toast.retracting.is_some() {
            return;
        }
        toast.hovered = hovered;
        // Retargetable reveal: a quick hover-out mid-fade reverses from the
        // current opacity (web: a plain CSS opacity transition).
        toast.x_reveal.retarget(if hovered { 1.0 } else { 0.0 });
        if hovered {
            toast.expire.pause();
            toast.expire_task = None;
            cx.notify();
        } else {
            self.arm_expire(&id, cx);
            cx.notify();
        }
    }

    /// Body click → route to the board (taskboard mode / center tab) and
    /// open that run's drawer (the native `location.hash = "#/board";
    /// openDrawer(runId)`), then retract.
    pub(super) fn clicked(&mut self, id: &SharedString, window: &mut Window, cx: &mut Context<Self>) {
        let Some(toast) = self.toasts.iter().find(|toast| toast.id == *id) else {
            return;
        };
        if toast.retracting.is_some() {
            return;
        }
        if let Some(run_id) = toast.run_id.clone() {
            // Route to the board one cycle later. open_task_board_run reads the
            // ActivityBar (via switch_to_mode_id → the surfaces registry on the
            // bar), so the layout mutation is deferred out of this listener per
            // the interaction-dispatch law (RUST_PORT_NOTES 2026-07-04). This
            // click runs inside the NotifStack's update, not the bar's — it does
            // not panic today, but the deferred form is the uniform safe idiom.
            let workspace = self.workspace.clone();
            window.defer(cx, move |window, cx| {
                workspace
                    .update(cx, |workspace, cx| {
                        // Emits TaskBoardEvent::OpenRun + opens the drawer.
                        crate::mode_item::open_task_board_run(run_id, workspace, window, cx);
                    })
                    .ok();
            });
        }
        self.retract(id, cx);
    }
}

impl Render for NotifStack {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Web `max-width: calc(100vw - 28px)` — shrink on narrow viewports
        // instead of clipping at the workspace overflow.
        let width = clamped_width(f32::from(window.viewport_size().width));
        let toasts: Vec<AnyElement> = self
            .toasts
            .iter()
            .map(|toast| render_toast(toast, width, cx))
            .collect();
        v_flex().w(px(width)).children(toasts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stack_width_clamps_to_the_viewport() {
        // max-width: calc(100vw - 28px), capped at the web's 332px.
        assert_eq!(clamped_width(1920.), STACK_W);
        assert_eq!(clamped_width(360.), STACK_W);
        assert_eq!(clamped_width(300.), 272.);
        assert_eq!(clamped_width(10.), 0.);
    }
}
