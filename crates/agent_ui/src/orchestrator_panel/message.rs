//! Per-message transcript rendering (PARITY_SPEC §4.1 message anatomy):
//! tonal user cards / bare agent prose, the worked-for collapsible with its
//! step timeline, and the hover-revealed meta trio. View state lives in
//! [`super::transcript`]; this module owns what one message looks like.

use std::time::Duration;

use gpui::{
    Animation, AnimationExt as _, AnyElement, ClipboardItem, Context, FontWeight, SharedString,
    Window,
};
use ui::prelude::*;

use crate::agent_accents::accent_for_agent;
use crate::bridge::TranscriptRun;
use crate::task_board::motion::{DECEL, EFFECTS, STATE_FADE, StateFades, mix};
use crate::task_board::style::{HAIRLINE_HI, SURFACE_1, ago_now, chip_reason, rel, tabular_nums};
use markdown::MarkdownElement;

use super::panel::OrchestratorPanel;
use super::transcript::{FRESH_WINDOW, MessageView, prose_style};

/// User-card rise (web `.msg { animation: rise .3s var(--decel-curve) }`).
const RISE: Duration = Duration::from_millis(300);
/// The newest agent block's one-shot streaming fade (the idiom ruling).
const STREAM_FADE: Duration = Duration::from_millis(200);

/// The tracked-hover key for a message's meta-trio reveal (also the agent
/// block's element id).
fn msg_hover_id(ix: usize) -> ElementId {
    ElementId::NamedInteger("msg-agent".into(), ix as u64)
}

/// One transcript list item. `live` rides only on a trailing agent reply
/// while busy (the rolling worked-for tick — its label renders
/// `worked_s + seen_at.elapsed()` on the panel's 1s ticker).
pub(super) fn render_message(
    view: &MessageView,
    ix: usize,
    live: bool,
    fades: &StateFades,
    window: &mut Window,
    cx: &mut Context<OrchestratorPanel>,
) -> AnyElement {
    let body = if view.user {
        render_user_card(view, window, cx)
    } else {
        let live_extra_s = live.then(|| view.seen_at.elapsed().as_secs_f64());
        render_agent_block(view, ix, live_extra_s, fades, window, cx)
    };

    // Entrance: user cards rise 4px on decel (web `rise`); agent replies get
    // the single one-shot streaming fade (idiom ruling — NOT span chunks).
    let fresh = view.seen_at.elapsed() < FRESH_WINDOW;
    let body = div().mb(px(26.)).child(body);
    if !fresh {
        return body.into_any_element();
    }
    if view.user {
        // `rise` keyframes (app.css): translateY(6px) → 0 with the opacity
        // fade (the .985 scale beat is dropped — gpui has no scale styling).
        body.with_animation(
            ElementId::NamedInteger("msg-rise".into(), ix as u64),
            Animation::new(RISE).with_easing(DECEL.easing()),
            |card, t| card.opacity(t).mt(px(6. * (1. - t))).mb(px(26. - 6. * (1. - t))),
        )
        .into_any_element()
    } else {
        body.with_animation(
            ElementId::NamedInteger("msg-stream-fade".into(), ix as u64),
            Animation::new(STREAM_FADE).with_easing(EFFECTS.easing()),
            |block, t| block.opacity(t),
        )
        .into_any_element()
    }
}

/// `.user-card` — full-width tonal card (`--surface-1`, hairline, r12,
/// p14×16), NOT a right-aligned bubble; the card differentiates the speaker.
fn render_user_card(
    view: &MessageView,
    window: &mut Window,
    cx: &mut Context<OrchestratorPanel>,
) -> AnyElement {
    let colors = cx.theme().colors();
    div()
        .rounded(px(12.))
        .bg(SURFACE_1)
        .border_1()
        .border_color(colors.border)
        .px(px(16.))
        .py(px(14.))
        .child(MarkdownElement::new(
            view.markdown.clone(),
            prose_style(window, cx),
        ))
        .into_any_element()
}

/// `.msg.agent` — worked-for collapsible above bare prose, hover-revealed
/// meta trio beneath.
fn render_agent_block(
    view: &MessageView,
    ix: usize,
    live_extra_s: Option<f64>,
    fades: &StateFades,
    window: &mut Window,
    cx: &mut Context<OrchestratorPanel>,
) -> AnyElement {
    let worked = render_worked_for(view, ix, live_extra_s, fades, cx);
    let meta = render_meta_row(view, ix, fades, cx);
    v_flex()
        .id(msg_hover_id(ix))
        .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
            this.set_fade(msg_hover_id(ix), *hovered, STATE_FADE, cx);
        }))
        .children(worked)
        .child(MarkdownElement::new(
            view.markdown.clone(),
            prose_style(window, cx),
        ))
        .child(meta)
        .into_any_element()
}

/// The "Worked for Ns · N agent(s)" collapsible (§4.1) — chevron + label in
/// a hairline pill; expand is INSTANT (accepted ruling: GPUI has no
/// animated-height idiom; VSCode-instant chrome is on-spec here). The
/// label's counter rolls on the panel's 1s busy ticker, and ONLY the
/// counter renders bright (`.wtick { color: var(--text) }`,
/// unconditionally) — the chrome words stay muted, settled or live.
fn render_worked_for(
    view: &MessageView,
    ix: usize,
    live_extra_s: Option<f64>,
    fades: &StateFades,
    cx: &mut Context<OrchestratorPanel>,
) -> Option<AnyElement> {
    if view.runs.is_empty() && view.worked_s <= 3.0 {
        return None;
    }
    let colors = cx.theme().colors();
    let open = view.worked_open;
    let shown_s = view.worked_s + live_extra_s.unwrap_or(0.0);
    let n = view.runs.len();

    // Hover crossfade (`summary { transition: color/border-color .15s
    // var(--effects-curve) }` + `summary:hover { color: var(--text);
    // border-color: var(--hairline-hi) }`).
    let summary_id = ElementId::NamedInteger("worked-summary".into(), ix as u64);
    let hover_t = fades.t(&summary_id);
    let chrome_color = mix(colors.text_muted, colors.text, hover_t);
    let border_color = if open {
        HAIRLINE_HI.into()
    } else {
        mix(colors.border, HAIRLINE_HI.into(), hover_t)
    };

    let summary = h_flex()
        .id(summary_id.clone())
        .items_center()
        .gap(px(6.))
        // Open relaxes the pill into a card header: pl 8→9, py 5→7 (+1px
        // bottom standing in for the dropped transparent border's height).
        .pl(px(if open { 9. } else { 8. }))
        .pr(px(12.))
        .pt(px(if open { 7. } else { 5. }))
        .pb(px(if open { 8. } else { 5. }))
        .border_color(border_color)
        .when(open, |this| {
            // `border-bottom-color: transparent` — the header flows
            // seamlessly into the timeline card (per-side widths stand in
            // for CSS per-side colors).
            this.border_t_1()
                .border_l_1()
                .border_r_1()
                .rounded_t(px(12.))
                .bg(SURFACE_1)
        })
        .when(!open, |this| this.border_1().rounded_full())
        .cursor_pointer()
        .on_hover(cx.listener({
            let summary_id = summary_id.clone();
            move |this, hovered: &bool, _, cx| {
                this.set_fade(summary_id.clone(), *hovered, STATE_FADE, cx);
            }
        }))
        .on_click(cx.listener(move |this, _, _, cx| this.toggle_worked(ix, cx)))
        .child(
            // `.chev` — 16px, rides the summary's color ramp (the web
            // rotates 90°; the down-glyph swap is the in-tree idiom).
            Icon::new(if open {
                IconName::ChevronDown
            } else {
                IconName::ChevronRight
            })
            .size(IconSize::Medium)
            .color(Color::Custom(chrome_color)),
        )
        .child(
            h_flex()
                .gap(px(4.))
                .text_size(px(13.))
                .text_color(chrome_color)
                .child("Worked for")
                .child(
                    // `.wtick`: the ticking number alone, always at full
                    // `--text`, tabular so rolls never reflow.
                    div()
                        .text_color(colors.text)
                        .font_features(tabular_nums())
                        .child(SharedString::from(rel(shown_s))),
                )
                .children((n > 0).then(|| {
                    SharedString::from(format!(
                        "· {n} agent{}",
                        if n > 1 { "s" } else { "" }
                    ))
                })),
        );

    let body = open.then(|| {
        let rows: Vec<AnyElement> = if view.runs.is_empty() {
            vec![
                div()
                    .px(px(4.))
                    .py(px(6.))
                    .text_size(px(13.))
                    .italic()
                    .text_color(cx.theme().colors().text_placeholder)
                    .child("no spawns")
                    .into_any_element(),
            ]
        } else {
            view.runs
                .iter()
                .enumerate()
                .map(|(row_ix, run)| render_step_row(run, ix, row_ix, fades, cx))
                .collect()
        };
        // `.step-timeline`: card body under the relaxed pill, 1px left
        // rail, spanning the full message column (the web's block-level
        // grid inside the full-width `.msg`; only the summary is
        // inline-flex).
        div()
            .w_full()
            .border_1()
            .border_t_0()
            .border_color(HAIRLINE_HI)
            .rounded_b(px(12.))
            .child(
                v_flex()
                    .ml(px(9.))
                    .pl(px(12.))
                    .pt(px(10.))
                    .pb(px(8.))
                    .border_l_1()
                    .border_color(HAIRLINE_HI)
                    .children(rows),
            )
    });

    Some(
        v_flex()
            .mb(px(12.))
            .items_start()
            .child(summary)
            .children(body)
            .into_any_element(),
    )
}

/// One `.step-row`: vendor dot · agent name · routing reason; click routes
/// to the task board and opens the run drawer (the Z2 toast pattern).
fn render_step_row(
    run: &TranscriptRun,
    msg_ix: usize,
    row_ix: usize,
    fades: &StateFades,
    cx: &mut Context<OrchestratorPanel>,
) -> AnyElement {
    let colors = cx.theme().colors();
    let reason = chip_reason(run.chip.as_deref(), &run.agent);
    let reason = if reason.is_empty() {
        "spawned".to_string()
    } else {
        reason
    };
    let run_id = SharedString::from(run.run_id.clone());
    let row_id = ElementId::NamedInteger(format!("step-{msg_ix}").into(), row_ix as u64);
    // `.step-row { transition: background .15s var(--effects-curve) }` —
    // the hover wash crossfades (the color flip in chat.css:118 is dead CSS
    // in the reference too; refuted finding #2).
    let hover_t = fades.t(&row_id);
    h_flex()
        .id(row_id.clone())
        .w_full()
        .items_center()
        .gap(px(9.))
        .px(px(4.))
        .py(px(6.))
        .rounded(px(8.))
        .cursor_pointer()
        .bg(colors.element_hover.opacity(hover_t))
        .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
            this.set_fade(row_id.clone(), *hovered, STATE_FADE, cx);
        }))
        .on_click(cx.listener(move |this, _, window, cx| {
            this.open_run(run_id.clone(), window, cx);
        }))
        .child(
            div()
                .flex_none()
                .size(px(7.))
                .rounded_full()
                .bg(accent_for_agent(&run.agent)),
        )
        .child(
            div()
                .flex_none()
                .text_size(px(13.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(colors.text)
                .child(SharedString::from(run.agent.clone())),
        )
        .child(
            div()
                .min_w_0()
                .text_size(px(13.))
                .text_color(colors.text_placeholder)
                .truncate()
                .child(SharedString::from(reason)),
        )
        .into_any_element()
}

/// `.msg-meta`: timestamp (+ brain) left, hover-revealed copy/👍/👎 trio
/// right — the reveal rides the panel's tracked hover (150ms effects fade,
/// §0 "hovers are gentle fades", same idiom as the board rows).
fn render_meta_row(
    view: &MessageView,
    ix: usize,
    fades: &StateFades,
    cx: &mut Context<OrchestratorPanel>,
) -> AnyElement {
    // Copy the Hsla tokens out so the action builder can take `cx` mutably
    // (the theme borrow must not outlive this).
    let (placeholder, text, text_muted, hover_bg) = {
        let colors = cx.theme().colors();
        (
            colors.text_placeholder,
            colors.text,
            colors.text_muted,
            colors.element_hover,
        )
    };
    let mut ts_label = if view.ts > 0.0 {
        ago_now(view.ts)
    } else {
        String::new()
    };
    if let Some(brain) = &view.brain {
        if !ts_label.is_empty() {
            ts_label.push_str(" · ");
        }
        ts_label.push_str(brain);
    }
    // `.meta-actions .ic { transition: color/background .15s }`: 16px glyph
    // at --text-3 rest → --text + hover wash, each button fading
    // independently (per-element CSS transitions).
    let action = |id: &'static str, icon: IconName, cx: &mut Context<OrchestratorPanel>| {
        let button_id = ElementId::NamedInteger(id.into(), ix as u64);
        let hover_t = fades.t(&button_id);
        let hover_id = button_id.clone();
        div()
            .id(button_id)
            .size(px(26.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(8.))
            .bg(hover_bg.opacity(hover_t))
            .cursor_pointer()
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                this.set_fade(hover_id.clone(), *hovered, STATE_FADE, cx);
            }))
            .child(
                Icon::new(icon)
                    .size(IconSize::Medium)
                    .color(Color::Custom(mix(placeholder, text, hover_t))),
            )
    };
    let copy_text = view.text.clone();
    let trio = h_flex()
        .gap(px(4.))
        .child(
            action("msg-copy", IconName::Copy, cx).on_click(cx.listener(move |_, _, _, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(copy_text.to_string()));
            })),
        )
        // Feedback buttons are anatomy-only (the web ships them without
        // handlers — wiring is banked).
        .child(action("msg-up", IconName::ThumbsUp, cx))
        .child(action("msg-down", IconName::ThumbsDown, cx));

    // Tracked-hover reveal (`.msg.agent:hover .meta-actions { opacity: 1 }`
    // on the .15s effects transition): the scalar is read per frame off the
    // panel's fade map — mid-fade flips re-base continuously, and sweeping
    // across messages fades every departed trio concurrently.
    let trio = trio.opacity(fades.t(&msg_hover_id(ix)));

    h_flex()
        .mt(px(8.))
        .items_center()
        .justify_between()
        .child(
            div()
                .text_size(px(13.))
                .text_color(text_muted)
                .child(SharedString::from(ts_label)),
        )
        .child(trio)
        .into_any_element()
}
