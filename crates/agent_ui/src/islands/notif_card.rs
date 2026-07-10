//! The toast ELEMENT (PARITY_SPEC §4.9) — card anatomy
//! (`notifications.css`: dot + title + agent chip + dismiss ×), the slot
//! wrapper, and the emerge/retract motion. The stack entity
//! ([`super::notif_stack`]) owns lifecycle/state; this module owns what one
//! toast looks like and how it moves.
//!
//! Motion law (§4.9, §8.7): emergence slides translate from past the right
//! edge (100% + 5px overscan) on the decel-in entrance curve (500ms) while
//! opacity crossfades CONCURRENTLY on effects (200ms). Retract retargets
//! the SAME pair onto the sharper asymmetric exit (~400ms on the Caelestia
//! emphasized spline) with the 3-beat slot collapse: slide-out + fade + the
//! slot height/gap collapsing on SPATIAL (320ms) so siblings below RISE to
//! fill the slot — all three property groups start the same frame. The
//! collapse animates the toast's MEASURED height (canvas-captured), an
//! exactness upgrade over the web's non-interpolable `max-height: none→0`.

use std::time::{Duration, Instant};

use gpui::{
    Animation, AnimationExt as _, AnyElement, Context, FontWeight, SharedString, Task, canvas,
};
use settings::Settings as _;
use theme_settings::ThemeSettings;
use ui::prelude::*;

use crate::agent_accents::{ACCENT_FILL, STATUS_BLOCKED, STATUS_ERROR, accent_for_agent};
use crate::task_board::motion::{AnimatedValue, SPATIAL};
use crate::task_board::style::{HAIRLINE_HI, SURFACE_1, tabular_nums};

use super::notif_logic::{ApprovalMeta, ToastKind, expiry_countdown, unix_now};
use super::notif_stack::NotifStack;
use super::usage_island::SURFACE_FLOAT;

/// Column gap, carried per-slot so the retract collapse can swallow it.
const SLOT_GAP: f32 = 10.;
/// Emergence slide (decel-in entrance class).
pub(super) const EMERGE: Duration = Duration::from_millis(500);
/// Concurrent opacity crossfade (effects class).
pub(super) const FADE: Duration = Duration::from_millis(200);
/// The retract envelope: slide-out 400ms (exit curve) + fade 200ms + slot
/// collapse 320ms (spatial) all start together; the slot is removed when
/// the longest beat settles (web: `setTimeout(remove, 420)`).
pub(super) const RETRACT: Duration = Duration::from_millis(420);
pub(super) const RETRACT_SLIDE: Duration = Duration::from_millis(400);
const RETRACT_COLLAPSE_MS: f32 = 320.;
/// Slide overscan past the anchor edge (§4.9 emergence primitive).
const OVERSCAN: f32 = 5.;
/// How far the retracting slot's clip viewport extends past its right edge
/// so the slide-out is masked at the WORKSPACE edge, not the stack's own
/// 14px inset (web: the viewport is the clip plane for both directions):
/// the 14px corner inset + the 5px overscan.
const EDGE_OVERHANG: f32 = 19.;
/// Dismiss-× hover reveal (web `.ct-x { transition: opacity .14s }`).
pub(super) const X_REVEAL: Duration = Duration::from_millis(140);
/// Pre-measure fallback slot height (one-line toast); the canvas capture
/// replaces it on the first painted frame.
pub(super) const FALLBACK_H: f32 = 64.;

pub(super) struct Toast {
    pub(super) id: SharedString,
    pub(super) kind: ToastKind,
    pub(super) title: SharedString,
    pub(super) agent: String,
    pub(super) run_id: Option<SharedString>,
    /// Present only on held [`ToastKind::Approval`] toasts — the inline
    /// Allow/Deny + countdown render from it (Phase-2 §5.2).
    pub(super) approval: Option<ApprovalMeta>,
    /// An allow/deny POST is in flight (or landed and the retiring frame
    /// hasn't arrived yet) — collapses the buttons, one POST at a time.
    pub(super) deciding: bool,
    /// The last decision POST's refusal — the bridge's `{"error"}` VERBATIM
    /// (a 409 "already resolved: deny" reads as a refusal, not offline).
    pub(super) decide_error: Option<SharedString>,
    /// The §4.9 emergence scalar (web `--emerge`: 1 = tucked past the right
    /// edge, 0 = fully out), ONE retargetable value reused by both
    /// directions — DECEL/500ms in, exit-spline/400ms out — so a mid-emerge
    /// dismiss continues from the CURRENT offset instead of snapping to
    /// fully-emerged first (§8.7a).
    pub(super) slide: AnimatedValue,
    /// Concurrent opacity crossfade (EFFECTS/200ms both directions),
    /// retargetable for the same interrupt continuity.
    pub(super) fade: AnimatedValue,
    /// Hover-pausable expiry bookkeeping + the live timer task.
    pub(super) expire: super::notif_logic::ExpireTimer,
    pub(super) expire_task: Option<Task<()>>,
    /// `Some(started)` while retracting (slot collapse + removal clock).
    pub(super) retracting: Option<Instant>,
    /// Height frozen at retract start — the collapse animates it to 0.
    pub(super) retract_from_h: f32,
    /// Canvas-captured painted height (kept current every frame).
    pub(super) measured_h: f32,
    pub(super) hovered: bool,
    /// Dismiss-× reveal opacity — retargetable so a quick hover-out
    /// reverses from the current opacity, not the opposite endpoint.
    pub(super) x_reveal: AnimatedValue,
}

/// One slot in the stack: the card plus the measuring canvas, swapped onto
/// the collapsing wrapper while retracting.
pub(super) fn render_toast(toast: &Toast, width: f32, cx: &mut Context<NotifStack>) -> AnyElement {
    let id = toast.id.clone();
    let card = render_card(toast, width, cx);

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
            .w(px(width))
            .mb(px(SLOT_GAP))
            .child(measure)
            .child(card)
            .into_any_element(),
        Some(_) => {
            // Beats 1+2+3, one clock: slide-out on the exit spline, fade on
            // effects, slot height + gap collapse on spatial — siblings
            // below rise as the slot shrinks. The height/gap animate on a
            // NON-clipping outer wrapper; the clip lives on an inner
            // viewport that extends EDGE_OVERHANG past the slot's right
            // edge (GPUI's overflow mask clips both axes, so the viewport
            // must be the union plane: collapsing height vertically, the
            // workspace edge horizontally — the sliding card escapes the
            // stack inset like the web's viewport-clipped transform).
            let from_h = toast.retract_from_h;
            div()
                .relative()
                .w(px(width))
                .child(
                    div()
                        .w(px(width + EDGE_OVERHANG))
                        .h_full()
                        .overflow_hidden()
                        .child(card),
                )
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

/// The held-approval anatomy under the title (§5.2): matched rule + cwd
/// meta, then the decision row — inline Allow / Deny (in-flight collapse) +
/// the TTL countdown — and the last refusal VERBATIM. Buttons stop
/// propagation so the card's click-through (open the run drawer) stays a
/// separate gesture; NOTHING here touches focus (the no-focus-theft law).
fn approval_rows(
    toast: &Toast,
    meta: &ApprovalMeta,
    cx: &Context<NotifStack>,
) -> AnyElement {
    let colors = cx.theme().colors();
    let mono = ThemeSettings::get_global(cx).buffer_font.family.clone();
    let countdown: SharedString =
        format!("expires {}", expiry_countdown(meta.expires_ts, unix_now())).into();

    let decision_button = |label: &'static str, allow: bool, cx: &Context<NotifStack>| {
        let id = toast.id.clone();
        div()
            .id(ElementId::Name(format!("toast-{label}-{}", toast.id).into()))
            .px(px(10.))
            .py(px(3.))
            .rounded_full()
            .text_size(px(11.))
            .font_weight(FontWeight::MEDIUM)
            .cursor_pointer()
            .when(allow, |x| {
                // Allow = the filled action (ACCENT_FILL, dark glyph — the
                // send-circle grammar).
                x.bg(gpui::Hsla::from(ACCENT_FILL)).text_color(gpui::black())
            })
            .when(!allow, |x| {
                x.border_1()
                    .border_color(colors.border)
                    .text_color(colors.text_muted)
                    .hover(|x| x.bg(colors.element_hover))
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.decide(&id, allow, cx);
            }))
            .child(label)
    };

    let mut rows = v_flex().gap(px(6.));
    if !meta.rule.is_empty() {
        rows = rows.child(
            div()
                .text_size(px(11.5))
                .text_color(colors.text_muted)
                .line_clamp(1)
                .child(SharedString::from(meta.rule.clone())),
        );
    }
    if !meta.cwd.is_empty() {
        rows = rows.child(
            div()
                .font_family(mono.clone())
                .text_size(px(11.))
                .text_color(colors.text_placeholder)
                .truncate()
                .child(SharedString::from(meta.cwd.clone())),
        );
    }
    let decision_row = h_flex()
        .items_center()
        .gap(px(8.))
        .when(!toast.deciding, |row| {
            row.child(decision_button("Allow", true, cx))
                .child(decision_button("Deny", false, cx))
        })
        .when(toast.deciding, |row| {
            // In-flight collapse — one POST at a time; the pending frame
            // retires the toast on success.
            row.child(
                div()
                    .text_size(px(11.5))
                    .text_color(colors.text_placeholder)
                    .child("deciding…"),
            )
        })
        .child(div().flex_1())
        .child(
            // The TTL countdown, ticking on the stack's 1s held ticker.
            div()
                .font_family(mono)
                .text_size(px(11.))
                .font_features(tabular_nums())
                .text_color(gpui::Hsla::from(STATUS_BLOCKED).opacity(0.9))
                .child(countdown),
        );
    rows = rows.child(decision_row);
    if let Some(error) = &toast.decide_error {
        rows = rows.child(
            div()
                .text_size(px(11.5))
                .text_color(gpui::Hsla::from(STATUS_ERROR))
                .child(error.clone()),
        );
    }
    rows.into_any_element()
}

fn render_card(toast: &Toast, width: f32, cx: &mut Context<NotifStack>) -> AnyElement {
    let approval_section = toast
        .approval
        .clone()
        .map(|meta| approval_rows(toast, &meta, cx));
    let colors = cx.theme().colors();
    let id = toast.id.clone();
    let border = match toast.kind {
        ToastKind::Error => gpui::Hsla::from(STATUS_ERROR).opacity(0.34),
        ToastKind::Stale | ToastKind::Approval => {
            gpui::Hsla::from(STATUS_BLOCKED).opacity(0.30)
        }
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
            // `.ct-x { margin: -2px -2px 0 0 }` — the 22px hit circle
            // optically tucks into the card corner.
            .mt(px(-2.))
            .mr(px(-2.))
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
        // Hover-reveal ×: 140ms effects fade (Instant-clocked — the wrapper
        // is a frame pump), settled states bare.
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
        .w(px(width))
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
                    // `.ct-title` font 500 13px/1.35 — explicit line-height
                    // so the 2-line clamp height matches the web (gpui's
                    // default phi ≈ 1.618 would add ~7px per wrapped toast
                    // and shift the measured slot height the retract
                    // collapse uses).
                    div()
                        .text_size(px(13.))
                        .line_height(relative(1.35))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(colors.text)
                        .line_clamp(2)
                        .child(toast.title.clone()),
                )
                .children(chip)
                .children(approval_section),
        )
        .child(dismiss);

    // §4.9 emergence: translate from past the right edge (100% + overscan)
    // on decel-in while opacity rises on effects — concurrent from frame
    // one. Retract retargets the SAME pair onto the sharper exit, so either
    // direction continues from wherever the card is right now (mid-emerge
    // dismiss included — §8.7a). The wrapper is a frame pump over the
    // Instant-clocked values; its restarts re-base nothing.
    if toast.slide.animating() || toast.fade.animating() {
        let (slide, fade) = (toast.slide.clone(), toast.fade.clone());
        let generation =
            (toast.slide.generation() as u64) << 8 ^ (toast.fade.generation() as u64);
        card.with_animation(
            ElementId::NamedInteger(format!("toast-motion-{id}").into(), generation),
            Animation::new(EMERGE),
            move |card, _| {
                let offset = slide.current() * (width + OVERSCAN);
                card.ml(px(offset))
                    .mr(px(-offset))
                    .opacity(fade.current().clamp(0., 1.))
            },
        )
        .into_any_element()
    } else {
        let offset = toast.slide.target() * (width + OVERSCAN);
        card.ml(px(offset))
            .mr(px(-offset))
            .opacity(toast.fade.target())
            .into_any_element()
    }
}
