//! The corner notification stack (PARITY_SPEC §4.9) — toasts that EMERGE
//! from the right edge beneath the usage-island head, ported from
//! `notifications.js`/`notifications.css`. Driven push-first off the
//! `BridgeStore`: run lifecycle transitions (completed → done toast,
//! failed/killed → error toast) and the usage scrape's fresh→stale flip.
//!
//! Motion law (§4.9, §8.7): emergence slides translate from past the right
//! edge (100% + 5px overscan) on the decel-in entrance curve (500ms) while
//! opacity crossfades CONCURRENTLY on effects (200ms). Retract is the
//! sharper asymmetric exit (~400ms on the Caelestia emphasized spline) with
//! the 3-beat slot collapse: slide-out + fade + the slot height/gap
//! collapsing on SPATIAL (320ms) so siblings below RISE to fill the slot —
//! all three property groups start the same frame. The collapse animates
//! the toast's MEASURED height (canvas-captured), an exactness upgrade over
//! the web's non-interpolable `max-height: none → 0`.
//!
//! Renders in the same custom corner-cluster layer as the island head —
//! consistency of the corner system over the workspace toast hosts (the
//! host comparison lives in `islands/mod.rs`).

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use gpui::{
    Animation, AnimationExt as _, AnyElement, Context, Entity, FontWeight, SharedString,
    Subscription, Task, WeakEntity, Window, canvas,
};
use ui::prelude::*;
use workspace::Workspace;

use crate::agent_accents::{STATUS_BLOCKED, STATUS_ERROR, accent_for_agent};
use crate::bridge::{self, BridgeStore};
use crate::task_board::TaskBoardPanel;
use crate::task_board::motion::{AnimatedValue, DECEL, EFFECTS, MotionCurve, SPATIAL};
use crate::task_board::style::{HAIRLINE_HI, SURFACE_1};

use super::notif_logic::{
    MAX_VISIBLE, StaleLatch, TOAST_TIMEOUT, ToastKind, ToastPayload, diff_board, stale_payload,
};
use super::usage_island::SURFACE_FLOAT;

/// Stack width — web `#notif-stack { width: 332px }`.
pub const STACK_W: f32 = 332.;
/// Column gap, carried per-slot so the retract collapse can swallow it.
const SLOT_GAP: f32 = 10.;
/// Emergence slide (decel-in entrance class).
const EMERGE: Duration = Duration::from_millis(500);
/// Concurrent opacity crossfade (effects class).
const FADE: Duration = Duration::from_millis(200);
/// The retract envelope: slide-out 400ms (exit curve) + fade 200ms + slot
/// collapse 320ms (spatial) all start together; the slot is removed when
/// the longest beat settles (web: `setTimeout(remove, 420)`).
const RETRACT: Duration = Duration::from_millis(420);
const RETRACT_SLIDE: Duration = Duration::from_millis(400);
const RETRACT_COLLAPSE_MS: f32 = 320.;
/// Slide overscan past the anchor edge (§4.9 emergence primitive).
const OVERSCAN: f32 = 5.;
/// Dismiss-× hover reveal (web `.ct-x { transition: opacity .14s }`).
const X_REVEAL: Duration = Duration::from_millis(140);
/// Pre-measure fallback slot height (one-line toast); the canvas capture
/// replaces it on the first painted frame.
const FALLBACK_H: f32 = 64.;

struct Toast {
    id: SharedString,
    kind: ToastKind,
    title: SharedString,
    agent: String,
    run_id: Option<SharedString>,
    /// The §4.9 emergence scalar (web `--emerge`: 1 = tucked past the right
    /// edge, 0 = fully out), ONE retargetable value reused by both
    /// directions — DECEL/500ms in, exit-spline/400ms out — so a mid-emerge
    /// dismiss continues from the CURRENT offset instead of snapping to
    /// fully-emerged first (§8.7a).
    slide: AnimatedValue,
    /// Concurrent opacity crossfade (EFFECTS/200ms both directions),
    /// retargetable for the same interrupt continuity.
    fade: AnimatedValue,
    /// Hover-pausable expiry bookkeeping + the live timer task.
    expire: super::notif_logic::ExpireTimer,
    expire_task: Option<Task<()>>,
    /// `Some(started)` while retracting (slot collapse + removal clock).
    retracting: Option<Instant>,
    /// Height frozen at retract start — the collapse animates it to 0.
    retract_from_h: f32,
    /// Canvas-captured painted height (kept current every frame).
    measured_h: f32,
    hovered: bool,
    /// Dismiss-× reveal opacity — retargetable so a quick hover-out
    /// reverses from the current opacity, not the opposite endpoint.
    x_reveal: AnimatedValue,
}

pub struct NotifStack {
    store: Entity<BridgeStore>,
    workspace: WeakEntity<Workspace>,
    prev_runs: HashMap<String, String>,
    primed: bool,
    stale_latch: StaleLatch,
    /// Dedupe: stable ids already surfaced never double-fire.
    seen: HashSet<String>,
    /// Newest first (top of the top-right stack).
    toasts: Vec<Toast>,
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
            _store_subscription,
        }
    }

    fn sync_from_store(&mut self, cx: &mut Context<Self>) {
        let store = self.store.read(cx);
        let board = store.board.clone();
        let stale = store.usage_meta.stale();
        let scrape = store.usage_meta.scraped.clone();
        let payloads = diff_board(&mut self.prev_runs, &mut self.primed, &board);
        let mut changed = false;
        for payload in payloads {
            changed |= self.surface(payload, cx);
        }
        if self.stale_latch.flip(stale) {
            changed |= self.surface(stale_payload(scrape.as_ref()), cx);
        }
        // The store notifies every second while a run ticks — only repaint
        // the stack when a toast actually surfaced (§8 idle cost).
        if changed {
            cx.notify();
        }
    }

    /// Surface a payload as a new toast (newest first). Returns whether
    /// anything new appeared (dedupe makes re-fires no-ops).
    fn surface(&mut self, payload: ToastPayload, cx: &mut Context<Self>) -> bool {
        if !self.seen.insert(payload.id.clone()) {
            return false;
        }
        let id = SharedString::from(payload.id);
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
        self.arm_expire(&id, cx);
        // §8 bounded DOM: retract the oldest beyond the cap.
        let beyond: Vec<SharedString> = self
            .toasts
            .iter()
            .filter(|toast| toast.retracting.is_none())
            .skip(MAX_VISIBLE)
            .map(|toast| toast.id.clone())
            .collect();
        for id in beyond {
            self.retract(&id, cx);
        }
        true
    }

    fn toast_mut(&mut self, id: &SharedString) -> Option<&mut Toast> {
        self.toasts.iter_mut().find(|toast| toast.id == *id)
    }

    /// (Re)arm the expiry task for whatever time the pausable clock has
    /// left.
    fn arm_expire(&mut self, id: &SharedString, cx: &mut Context<Self>) {
        let Some(toast) = self.toast_mut(id) else {
            return;
        };
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
    fn retract(&mut self, id: &SharedString, cx: &mut Context<Self>) {
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
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Hover pauses the expiry clock (and reveals the dismiss ×).
    fn set_hover(&mut self, id: &SharedString, hovered: bool, cx: &mut Context<Self>) {
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

    /// Body click → route to the board and open that run's drawer (the
    /// native `location.hash = "#/board"; openDrawer(runId)`), then retract.
    fn clicked(&mut self, id: &SharedString, window: &mut Window, cx: &mut Context<Self>) {
        let Some(toast) = self.toasts.iter().find(|toast| toast.id == *id) else {
            return;
        };
        if toast.retracting.is_some() {
            return;
        }
        if let Some(run_id) = toast.run_id.clone() {
            self.workspace
                .update(cx, |workspace, cx| {
                    workspace.focus_panel::<TaskBoardPanel>(window, cx);
                    if let Some(panel) = workspace.panel::<TaskBoardPanel>(cx) {
                        // Emits TaskBoardEvent::OpenRun + opens the drawer.
                        panel.update(cx, |panel, cx| panel.open_run(run_id.clone(), cx));
                    }
                })
                .ok();
        }
        self.retract(id, cx);
    }

    fn render_toast(&self, toast: &Toast, cx: &mut Context<Self>) -> AnyElement {
        let id = toast.id.clone();
        let card = self.render_card(toast, cx);

        // The measuring canvas keeps the slot's painted height current so a
        // retract can collapse from the exact value (run_detail idiom).
        let measure = {
            let weak = cx.weak_entity();
            let id = id.clone();
            canvas(
                move |bounds, _, cx| {
                    weak.update(cx, |this, _| {
                        if let Some(toast) = this.toast_mut(&id) {
                            toast.measured_h = f32::from(bounds.size.height);
                        }
                    })
                    .ok();
                },
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0()
        };

        match toast.retracting {
            None => div()
                .relative()
                .w(px(STACK_W))
                .mb(px(SLOT_GAP))
                .child(measure)
                .child(card)
                .into_any_element(),
            Some(_) => {
                // Beats 1+2+3, one clock: slide-out on the exit spline,
                // fade on effects, slot height + gap collapse on spatial —
                // siblings below rise as the slot shrinks.
                let from_h = toast.retract_from_h;
                div()
                    .relative()
                    .w(px(STACK_W))
                    .overflow_hidden()
                    .child(card)
                    .with_animation(
                        ElementId::Name(format!("toast-retract-{id}").into()),
                        Animation::new(RETRACT),
                        move |slot, t| {
                            let ms = t * RETRACT.as_millis() as f32;
                            let collapse = SPATIAL.eval((ms / RETRACT_COLLAPSE_MS).min(1.));
                            slot.h(px((from_h * (1. - collapse)).max(0.)))
                                .mb(px(SLOT_GAP * (1. - collapse)))
                        },
                    )
                    .into_any_element()
            }
        }
    }

    fn render_card(&self, toast: &Toast, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors();
        let id = toast.id.clone();
        let border = match toast.kind {
            ToastKind::Error => gpui::Hsla::from(STATUS_ERROR).opacity(0.34),
            ToastKind::Stale => gpui::Hsla::from(STATUS_BLOCKED).opacity(0.30),
            ToastKind::Done => HAIRLINE_HI.into(),
        };
        let retracting = toast.retracting.is_some();

        let chip = (!toast.agent.is_empty()).then(|| {
            h_flex()
                .flex_none()
                .items_center()
                .gap(px(5.))
                .pl(px(5.))
                .pr(px(7.))
                .py(px(1.))
                .rounded_full()
                .bg(SURFACE_1)
                .border_1()
                // `--hairline` → colors.border (RUST_PORT_NOTES §1).
                .border_color(colors.border)
                .text_size(px(10.))
                // `.ct-chip` font 500 10px/1.6.
                .line_height(relative(1.6))
                .font_weight(FontWeight::MEDIUM)
                .text_color(colors.text_muted)
                .child(
                    div()
                        .size(px(7.))
                        .rounded_full()
                        .bg(accent_for_agent(&toast.agent)),
                )
                .child(SharedString::from(toast.agent.clone()))
        });

        let dismiss = {
            let id = id.clone();
            let base = div()
                .id(ElementId::Name(format!("toast-x-{id}").into()))
                .flex_none()
                .size(px(22.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .text_color(colors.text_placeholder)
                .hover(|x| x.bg(colors.element_hover).text_color(colors.text))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.retract(&id, cx);
                }))
                .child(Icon::new(IconName::Close).size(IconSize::Small));
            // Hover-reveal ×: 140ms effects fade (Instant-clocked — the
            // wrapper is a frame pump), settled states bare.
            if toast.x_reveal.animating() {
                let reveal = toast.x_reveal.clone();
                base.with_animation(
                    ElementId::NamedInteger(
                        format!("toast-x-fade-{}", toast.id).into(),
                        toast.x_reveal.generation() as u64,
                    ),
                    Animation::new(X_REVEAL),
                    move |x, _| x.opacity(reveal.current()),
                )
                .into_any_element()
            } else {
                base.opacity(toast.x_reveal.target()).into_any_element()
            }
        };

        let card = h_flex()
            .id(ElementId::Name(format!("toast-{id}").into()))
            .w(px(STACK_W))
            .items_start()
            .gap(px(10.))
            .px(px(12.))
            .py(px(11.))
            .rounded(px(16.))
            .bg(SURFACE_FLOAT)
            .border_1()
            .border_color(border)
            .when(!retracting, |card| {
                let hover_id = id.clone();
                let click_id = id.clone();
                card.cursor_pointer()
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        this.set_hover(&hover_id, *hovered, cx);
                    }))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.clicked(&click_id, window, cx);
                    }))
            })
            .child(
                // Status dot.
                div()
                    .flex_none()
                    .size(px(8.))
                    .mt(px(5.))
                    .rounded_full()
                    .bg(toast.kind.color()),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(6.))
                    .child(
                        // `.ct-title` font 500 13px/1.35 — explicit
                        // line-height so the 2-line clamp height matches
                        // the web (gpui's default phi ≈ 1.618 would add
                        // ~7px per wrapped toast and shift the measured
                        // slot height the retract collapse uses).
                        div()
                            .text_size(px(13.))
                            .line_height(relative(1.35))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(colors.text)
                            .line_clamp(2)
                            .child(toast.title.clone()),
                    )
                    .children(chip),
            )
            .child(dismiss);

        // §4.9 emergence: translate from past the right edge (100% +
        // overscan) on decel-in while opacity rises on effects — concurrent
        // from frame one. Retract retargets the SAME pair onto the sharper
        // exit, so either direction continues from wherever the card is
        // right now (mid-emerge dismiss included — §8.7a). The wrapper is a
        // frame pump over the Instant-clocked values; its restarts re-base
        // nothing.
        if toast.slide.animating() || toast.fade.animating() {
            let (slide, fade) = (toast.slide.clone(), toast.fade.clone());
            let generation =
                (toast.slide.generation() as u64) << 8 ^ (toast.fade.generation() as u64);
            card.with_animation(
                ElementId::NamedInteger(format!("toast-motion-{id}").into(), generation),
                Animation::new(EMERGE),
                move |card, _| {
                    let offset = slide.current() * (STACK_W + OVERSCAN);
                    card.ml(px(offset))
                        .mr(px(-offset))
                        .opacity(fade.current().clamp(0., 1.))
                },
            )
            .into_any_element()
        } else {
            let offset = toast.slide.target() * (STACK_W + OVERSCAN);
            card.ml(px(offset))
                .mr(px(-offset))
                .opacity(toast.fade.target())
                .into_any_element()
        }
    }
}

impl Render for NotifStack {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let toasts: Vec<AnyElement> = self
            .toasts
            .iter()
            .map(|toast| self.render_toast(toast, cx))
            .collect();
        v_flex().w(px(STACK_W)).children(toasts)
    }
}
