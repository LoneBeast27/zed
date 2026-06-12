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
use crate::task_board::motion::{DECEL, EFFECTS};
use crate::task_board::style::{HAIRLINE_HI, SURFACE_1, ago_now, chip_reason, rel, tabular_nums};
use markdown::MarkdownElement;

use super::panel::OrchestratorPanel;
use super::transcript::{FRESH_WINDOW, MessageView, prose_style};

/// User-card rise (web `.msg { animation: rise .3s var(--decel-curve) }`).
const RISE: Duration = Duration::from_millis(300);
/// The newest agent block's one-shot streaming fade (the idiom ruling).
const STREAM_FADE: Duration = Duration::from_millis(200);

/// One transcript list item. `live` rides only on a trailing agent reply
/// while busy (the rolling worked-for tick — its label renders
/// `worked_s + seen_at.elapsed()` on the panel's 1s ticker).
pub(super) fn render_message(
    view: &MessageView,
    ix: usize,
    live: bool,
    hover: (bool, bool),
    window: &mut Window,
    cx: &mut Context<OrchestratorPanel>,
) -> AnyElement {
    let body = if view.user {
        render_user_card(view, window, cx)
    } else {
        let live_extra_s = live.then(|| view.seen_at.elapsed().as_secs_f64());
        render_agent_block(view, ix, live_extra_s, hover, window, cx)
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
    hover: (bool, bool),
    window: &mut Window,
    cx: &mut Context<OrchestratorPanel>,
) -> AnyElement {
    let worked = render_worked_for(view, ix, live_extra_s, cx);
    let meta = render_meta_row(view, ix, hover, cx);
    v_flex()
        .id(ElementId::NamedInteger("msg-agent".into(), ix as u64))
        .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
            this.set_message_hover(ix, *hovered, cx);
        }))
        .children(worked)
        .child(MarkdownElement::new(
            view.markdown.clone(),
            prose_style(window, cx),
        ))
        .child(meta)
        .into_any_element()
}

/// The "Worked for Ns · N agent(s)" collapsible (§4.1) — `ui::Disclosure`
/// chevron in a hairline pill; expand is INSTANT (accepted ruling: GPUI has
/// no animated-height idiom; VSCode-instant chrome is on-spec here). The
/// label's counter rolls on the panel's 1s busy ticker.
fn render_worked_for(
    view: &MessageView,
    ix: usize,
    live_extra_s: Option<f64>,
    cx: &mut Context<OrchestratorPanel>,
) -> Option<AnyElement> {
    if view.runs.is_empty() && view.worked_s <= 3.0 {
        return None;
    }
    let colors = cx.theme().colors();
    let open = view.worked_open;
    let ticking = live_extra_s.is_some();
    let shown_s = view.worked_s + live_extra_s.unwrap_or(0.0);
    let n = view.runs.len();
    let label = format!(
        "Worked for {}{}",
        rel(shown_s),
        if n > 0 {
            format!(" · {n} agent{}", if n > 1 { "s" } else { "" })
        } else {
            String::new()
        }
    );

    let summary = h_flex()
        .id(ElementId::NamedInteger("worked-summary".into(), ix as u64))
        .items_center()
        .gap(px(6.))
        .pl(px(8.))
        .pr(px(12.))
        .py(px(if open { 7. } else { 5. }))
        .border_1()
        .border_color(if open { HAIRLINE_HI.into() } else { colors.border })
        .when(open, |this| {
            this.rounded_t(px(12.))
                .bg(SURFACE_1)
        })
        .when(!open, |this| this.rounded_full())
        .cursor_pointer()
        .on_click(cx.listener(move |this, _, _, cx| this.toggle_worked(ix, cx)))
        .child(
            ui::Disclosure::new(
                ElementId::NamedInteger("worked-chev".into(), ix as u64),
                open,
            )
            .on_click({
                let weak = cx.weak_entity();
                move |_, _, cx| {
                    weak.update(cx, |this, cx| this.toggle_worked(ix, cx)).ok();
                }
            }),
        )
        .child(
            h_flex()
                .gap(px(4.))
                .text_size(px(13.))
                .text_color(colors.text_muted)
                .font_features(tabular_nums())
                .when(ticking, |this| this.text_color(colors.text))
                .child(SharedString::from(label)),
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
                .map(|(row_ix, run)| render_step_row(run, ix, row_ix, cx))
                .collect()
        };
        // `.step-timeline`: card body under the relaxed pill, 1px left rail.
        div()
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
    h_flex()
        .id(ElementId::NamedInteger(
            format!("step-{msg_ix}").into(),
            row_ix as u64,
        ))
        .w_full()
        .items_center()
        .gap(px(9.))
        .px(px(4.))
        .py(px(6.))
        .rounded(px(8.))
        .cursor_pointer()
        .hover(|row| row.bg(cx.theme().colors().element_hover))
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
    (hovered, fresh): (bool, bool),
    cx: &mut Context<OrchestratorPanel>,
) -> AnyElement {
    let colors = cx.theme().colors();
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

    let action = |id: &'static str, icon: IconName, ix: usize| {
        div()
            .id(ElementId::NamedInteger(id.into(), ix as u64))
            .size(px(26.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(8.))
            .text_color(cx.theme().colors().text_placeholder)
            .hover(|x| {
                x.bg(cx.theme().colors().element_hover)
                    .text_color(cx.theme().colors().text)
            })
            .cursor_pointer()
            .child(Icon::new(icon).size(IconSize::Small).color(Color::Muted))
    };
    let copy_text = view.text.clone();
    let trio = h_flex()
        .gap(px(4.))
        .child(
            action("msg-copy", IconName::Copy, ix).on_click(cx.listener(move |_, _, _, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(copy_text.to_string()));
            })),
        )
        // Feedback buttons are anatomy-only (the web ships them without
        // handlers — wiring is banked).
        .child(action("msg-up", IconName::ThumbsUp, ix))
        .child(action("msg-down", IconName::ThumbsDown, ix));

    // Tracked-hover reveal: settled states render at 0/1; a flip animates
    // through the 150ms crossfade window.
    let target = if hovered { 1.0 } else { 0.0 };
    let trio: AnyElement = if fresh {
        let (from, to) = (1.0 - target, target);
        trio.with_animation(
            ElementId::NamedInteger("meta-trio-fade".into(), ix as u64),
            Animation::new(Duration::from_millis(150)).with_easing(EFFECTS.easing()),
            move |trio, t| trio.opacity(from + (to - from) * t),
        )
        .into_any_element()
    } else {
        trio.opacity(target).into_any_element()
    };

    h_flex()
        .mt(px(8.))
        .items_center()
        .justify_between()
        .child(
            div()
                .text_size(px(13.))
                .text_color(colors.text_muted)
                .child(SharedString::from(ts_label)),
        )
        .child(trio)
        .into_any_element()
}
