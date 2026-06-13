//! The adversary panel's result rendering (`.adv-cols` + `.synth-card`):
//! three vendor columns in an `h_flex` (agent-chip head, pulsate shimmer
//! while pending, 13px/1.6 Markdown once landed) and the synthesis card
//! with the tinted AGREEMENTS / DISAGREEMENTS / SYNTHESIS blocks.

use gpui::{
    Animation, AnimationExt as _, AnyElement, App, Context, Entity, FontWeight, SharedString,
    TextStyleRefinement, Window, relative,
};
use markdown::{Markdown, MarkdownElement, MarkdownFont, MarkdownStyle};
use ui::prelude::*;

use crate::agent_accents::{ACCENT, STATUS_BLOCKED, STATUS_RUNNING};
use crate::bridge::{AdversaryPhase, AdversaryResult, VENDOR_COLUMNS, parse_synthesis_sections};
use crate::task_board::motion::DECEL;
use crate::task_board::style::{HAIRLINE_HI, INLINE_CODE_BG, SURFACE_1, agent_chip};

use super::panel::AdversaryPanel;

/// `.adv-col` entrance (`animation: spring-in .26s var(--decel-curve)`).
const SPRING_IN: std::time::Duration = std::time::Duration::from_millis(260);

/// The landed result as the panel renders it: one Markdown entity per
/// vendor column (built once per job completion, never per frame) + the
/// synthesis card's parsed blocks.
pub(super) struct DoneView {
    columns: Vec<(&'static str, Entity<Markdown>)>,
    agreements: Option<Entity<Markdown>>,
    disagreements: Option<Entity<Markdown>>,
    synthesis: Option<Entity<Markdown>>,
}

/// Build the per-vendor Markdown entities + parsed synthesis blocks for a
/// landed result. Missing vendors render the web's `"(no answer)"`.
pub(super) fn build_done_view(result: &AdversaryResult, cx: &mut App) -> DoneView {
    let markdown = |text: &str, cx: &mut App| {
        let text = SharedString::from(text.to_string());
        cx.new(|cx| Markdown::new(text, None, None, cx))
    };
    let columns = VENDOR_COLUMNS
        .iter()
        .map(|vendor| {
            let answer = result
                .answers
                .get(*vendor)
                .map(String::as_str)
                .unwrap_or("(no answer)");
            (*vendor, markdown(answer, cx))
        })
        .collect();
    let sections = parse_synthesis_sections(result.synthesis.as_deref().unwrap_or(""));
    let section = |text: &str, cx: &mut App| (!text.is_empty()).then(|| markdown(text, cx));
    DoneView {
        columns,
        agreements: section(&sections.agreements, cx),
        disagreements: section(&sections.disagreements, cx),
        synthesis: section(&sections.synthesis, cx),
    }
}

/// `.adv-col-body` / `.synth-block-body`: 13px/1.6 UI prose with the
/// `#1a1a1a` 12px inline code.
fn column_prose_style(window: &Window, cx: &App) -> MarkdownStyle {
    let mut style = MarkdownStyle::themed(MarkdownFont::Agent, window, cx);
    style.base_text_style.refine(&TextStyleRefinement {
        font_size: Some(px(13.).into()),
        line_height: Some(relative(1.6).into()),
        ..Default::default()
    });
    style.inline_code.background_color = Some(INLINE_CODE_BG.into());
    style.inline_code.font_size = Some(px(12.).into());
    style
}

impl AdversaryPanel {
    /// `.adv-cols`: three equal columns in an `h_flex`, gap 14, max 1100 —
    /// shimmering while pending, Markdown bodies once landed.
    pub(super) fn render_columns(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let generation = self.jobs.read(cx).generation;
        let columns: Vec<AnyElement> = match (&self.last_phase, &self.done) {
            (AdversaryPhase::Pending, _) => VENDOR_COLUMNS
                .iter()
                .map(|vendor| self.render_column(vendor, generation, None, window, cx))
                .collect(),
            // Done OR Cancelled (P4): both render the landed columns; Cancelled
            // additionally shows the aborted banner (render_aborted_banner).
            (AdversaryPhase::Done(_) | AdversaryPhase::Cancelled(_), Some(done)) => done
                .columns
                .iter()
                .map(|(vendor, markdown)| {
                    self.render_column(vendor, generation, Some(markdown.clone()), window, cx)
                })
                .collect(),
            _ => return None,
        };
        Some(
            h_flex()
                .w_full()
                .max_w(px(1100.))
                .items_start()
                .gap(px(14.))
                .mb(px(20.))
                .children(columns)
                .into_any_element(),
        )
    }

    /// One `.adv-col`: agent-chip header (vendor dot + name) over the body —
    /// pulsate shimmer while that column is pending, prose once landed. The
    /// spring-in entrance keys off (vendor, generation, landed) so the
    /// web's re-rendered cols replay it per state, never per frame.
    fn render_column(
        &self,
        vendor: &'static str,
        generation: u64,
        body: Option<Entity<Markdown>>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let landed = body.is_some();
        let content: AnyElement = match body {
            Some(markdown) => div()
                .text_size(px(13.))
                .child(MarkdownElement::new(markdown, column_prose_style(window, cx)))
                .into_any_element(),
            // `.shimmer-label` — the gradient text-sweep (web `chat.css`);
            // a highlight band travels through "Thinking…" while the column
            // is pending, stopping when the result lands. Pulsate is the
            // reduced-motion fallback (`motion::ShimmerMode::Pulsate`).
            None => div()
                .text_size(px(13.))
                .child(crate::task_board::motion::shimmer(
                    ElementId::Name(format!("adv-shimmer-{vendor}").into()),
                    "Thinking…",
                    px(13.),
                    FontWeight::default(),
                    cx.theme().colors().text_placeholder,
                    crate::agent_accents::SHIMMER_HIGHLIGHT.into(),
                    crate::task_board::motion::ShimmerMode::Sweep,
                ))
                .into_any_element(),
        };
        let column = v_flex()
            .flex_1()
            .min_w_0()
            .rounded(px(12.))
            .bg(SURFACE_1)
            .border_1()
            .border_color(cx.theme().colors().border)
            .px(px(15.))
            .py(px(14.))
            .min_h(px(120.))
            .items_start()
            .child(div().mb(px(12.)).child(agent_chip(vendor, cx)))
            .child(div().w_full().child(content));
        // spring-in .26s on the decel curve (opacity + 4px rise; the web's
        // scale beat is dropped — gpui has no element scale).
        column
            .with_animation(
                ElementId::Name(format!("adv-col-{vendor}-{generation}-{}", landed as u8).into()),
                Animation::new(SPRING_IN).with_easing(DECEL.easing()),
                |column, t| column.opacity(t).mt(px(4. * (1. - t))),
            )
            .into_any_element()
    }

    /// The P4 aborted banner: shown ONLY in `Cancelled` phase, above the
    /// (partial) columns, so an aborted broadcast is visibly distinct from a
    /// normal completed Done. Amber-tinted (the blocked bucket) with an
    /// explanation that the legs were stopped mid-flight.
    pub(super) fn render_aborted_banner(&self, cx: &App) -> Option<AnyElement> {
        if !matches!(self.last_phase, AdversaryPhase::Cancelled(_)) {
            return None;
        }
        let colors = cx.theme().colors();
        let amber: gpui::Hsla = STATUS_BLOCKED.into();
        Some(
            h_flex()
                .w_full()
                .max_w(px(1100.))
                .gap(px(8.))
                .items_center()
                .rounded(px(10.))
                .border_1()
                .border_color(amber.opacity(0.4))
                .bg(amber.opacity(0.1))
                .px(px(14.))
                .py(px(10.))
                .mb(px(16.))
                .child(
                    Icon::new(IconName::Stop)
                        .size(IconSize::Custom(rems_from_px(16.)))
                        .color(Color::Custom(amber)),
                )
                .child(
                    div()
                        .text_size(px(13.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(colors.text_muted)
                        .child("Broadcast aborted — legs stopped mid-flight; answers below are partial."),
                )
                .into_any_element(),
        )
    }

    /// `.synth-card`: hairline-hi card with the merge head + tinted section
    /// blocks. Always shown once a result lands (even an empty synthesis
    /// renders the bare head — web parity); blocks skip when empty.
    pub(super) fn render_synthesis(&self, window: &Window, cx: &App) -> Option<AnyElement> {
        let done = self.done.as_ref()?;
        let colors = cx.theme().colors();
        let block = |title: &'static str,
                     body: &Option<Entity<Markdown>>,
                     tint: gpui::Hsla|
         -> Option<AnyElement> {
            let markdown = body.clone()?;
            Some(
                v_flex()
                    .mb(px(14.))
                    .child(
                        div()
                            .mb(px(6.))
                            .text_size(px(12.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(tint)
                            .child(SharedString::from(title.to_uppercase())),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .child(MarkdownElement::new(
                                markdown,
                                column_prose_style(window, cx),
                            )),
                    )
                    .into_any_element(),
            )
        };
        Some(
            v_flex()
                .w_full()
                .max_w(px(1100.))
                .rounded(px(12.))
                .bg(SURFACE_1)
                .border_1()
                .border_color(HAIRLINE_HI)
                .px(px(18.))
                .py(px(16.))
                .child(
                    h_flex()
                        .items_center()
                        .gap(px(8.))
                        .mb(px(14.))
                        .child(
                            // `.synth-head .ms { font-size: 18px; color:
                            // var(--accent) }` — `merge` glyph's nearest
                            // IconName analog.
                            Icon::new(IconName::PullRequest)
                                .size(IconSize::Custom(rems_from_px(18.)))
                                .color(Color::Custom(ACCENT.into())),
                        )
                        .child(
                            div()
                                .text_size(px(14.))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(colors.text)
                                .child("Synthesis"),
                        ),
                )
                .children(block("Agreements", &done.agreements, STATUS_RUNNING.into()))
                .children(block(
                    "Disagreements",
                    &done.disagreements,
                    STATUS_BLOCKED.into(),
                ))
                .children(block("Synthesis", &done.synthesis, colors.text_placeholder))
                .into_any_element(),
        )
    }
}
