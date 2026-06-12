//! The Adversary panel (PARITY_SPEC §4.5), ported from the approved web
//! reference render (`bridge/ui/adversary.js` + `panels.css`): a composer
//! deck that broadcasts one prompt to all three models (`POST /adversary`),
//! three vendor columns in an `h_flex` that shimmer while the job runs and
//! fill with Markdown when it lands, and the synthesis card
//! (AGREEMENTS / DISAGREEMENTS / SYNTHESIS) beneath.
//!
//! Data path: [`crate::bridge::AdversaryJobs`] — the broadcast POST and the
//! 2.5s job poll run on background executors through the bridge connection
//! idiom (RUST_PORT_NOTES §4.5: never blocking the UI thread), and the poll
//! is watch-gated on [`Panel::set_active`] exactly like the orchestrator's
//! TranscriptWatch: hidden panel, dead poll.

use gpui::{
    Action, Animation, AnimationExt as _, AnyElement, App, Context, Entity, EventEmitter,
    FocusHandle, Focusable, FontWeight, SharedString, Subscription, TextStyleRefinement, Window,
    actions, pulsating_between, relative,
};
use markdown::{Markdown, MarkdownElement, MarkdownFont, MarkdownStyle};
use settings::Settings as _;
use ui::prelude::*;
use workspace::dock::{DockPosition, Panel, PanelEvent};

use crate::agent_accents::{ACCENT, ACCENT_FILL, STATUS_BLOCKED, STATUS_RUNNING};
use crate::bridge::{
    AdversaryJobs, AdversaryPhase, AdversaryResult, AdversaryWatch, VENDOR_COLUMNS,
    parse_synthesis_sections,
};
use crate::task_board::motion::{DECEL, STATE_FADE, StateFades, mix};
use crate::task_board::style::{HAIRLINE_HI, SURFACE_1, SURFACE_2, agent_chip};

actions!(
    adversary_panel,
    [
        /// Toggles focus on the adversary panel.
        ToggleFocus,
        /// Broadcasts the composer's prompt to all three models (Enter via
        /// the `AdversaryComposer > Editor` keymap binding).
        Broadcast
    ]
);

/// `.adv-col` / `.task-card` entrance (`animation: spring-in .26s
/// var(--decel-curve)`).
const SPRING_IN: std::time::Duration = std::time::Duration::from_millis(260);

/// The landed result as the panel renders it: one Markdown entity per
/// vendor column (built once per job completion, never per frame) + the
/// synthesis card's parsed blocks.
struct DoneView {
    columns: Vec<(&'static str, Entity<Markdown>)>,
    agreements: Option<Entity<Markdown>>,
    disagreements: Option<Entity<Markdown>>,
    synthesis: Option<Entity<Markdown>>,
}

pub struct AdversaryPanel {
    focus_handle: FocusHandle,
    position: DockPosition,
    jobs: Entity<AdversaryJobs>,
    editor: Entity<editor::Editor>,
    done: Option<DoneView>,
    /// Change gate for the jobs observer.
    last_phase: AdversaryPhase,
    /// Held only while the dock shows this panel — its presence keeps the
    /// job poll alive ([`Panel::set_active`]).
    watch: Option<AdversaryWatch>,
    fades: StateFades,
    _jobs_subscription: Subscription,
}

impl AdversaryPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let jobs = cx.new(|_| AdversaryJobs::default());
        let _jobs_subscription =
            cx.observe(&jobs, |this: &mut Self, _, cx| this.sync_from_jobs(cx));
        let editor = cx.new(|cx| {
            let mut editor = editor::Editor::auto_height(1, 6, window, cx);
            editor.set_placeholder_text("Ask all three models the same question…", window, cx);
            editor.set_soft_wrap();
            editor.set_show_indent_guides(false, cx);
            editor
        });
        Self {
            focus_handle: cx.focus_handle(),
            position: DockPosition::Left,
            jobs,
            editor,
            done: None,
            last_phase: AdversaryPhase::Idle,
            watch: None,
            fades: StateFades::new(),
            _jobs_subscription,
        }
    }

    /// A jobs notify landed: rebuild the Markdown entities only when the
    /// phase actually changed (Done builds once per landed job).
    fn sync_from_jobs(&mut self, cx: &mut Context<Self>) {
        let phase = self.jobs.read(cx).phase.clone();
        if phase == self.last_phase {
            return;
        }
        self.done = match &phase {
            AdversaryPhase::Done(result) => Some(build_done_view(result, cx)),
            _ => None,
        };
        self.last_phase = phase;
        cx.notify();
    }

    /// Broadcast the composer's prompt (the web's `broadcast()`): trimmed,
    /// empty is a no-op, and the box keeps its text (web parity — the
    /// prompt stays editable for a follow-up volley).
    fn broadcast(&mut self, cx: &mut Context<Self>) {
        let text = self.editor.read(cx).text(cx);
        let text = text.trim().to_string();
        if text.is_empty() {
            return;
        }
        self.jobs.update(cx, |jobs, cx| jobs.start(text, cx));
    }

    /// `.panel-head`: "Adversary" 18px/500 + the sub-line.
    fn render_header(&self, cx: &App) -> Div {
        let colors = cx.theme().colors();
        h_flex()
            .flex_none()
            .items_baseline()
            .gap(px(12.))
            .px(px(28.))
            .pt(px(18.))
            .pb(px(14.))
            .border_b_1()
            .border_color(colors.border)
            .child(
                div()
                    .text_size(px(18.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(colors.text)
                    .child("Adversary"),
            )
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(colors.text_placeholder)
                    .child("one prompt, three independent models, one synthesis"),
            )
    }

    /// `.adv-composer`: surface-2 r16 deck — editor + the always-visible
    /// broadcast circle (`.adv-send { opacity: 1 }`, unlike the chat
    /// composer's text-gated reveal).
    fn render_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors();
        let focused = self.editor.read(cx).focus_handle(cx).is_focused(window);
        let focus_id = ElementId::Name("adv-composer-focus".into());
        self.fades.set(focus_id.clone(), focused, STATE_FADE);
        let focus_t = self.fades.t(&focus_id);

        let editor_style = {
            let settings = theme_settings::ThemeSettings::get_global(cx);
            editor::EditorStyle {
                background: gpui::transparent_black(),
                local_player: cx.theme().players().local(),
                syntax: cx.theme().syntax().clone(),
                text: gpui::TextStyle {
                    color: colors.text,
                    font_family: settings.ui_font.family.clone(),
                    font_features: settings.ui_font.features.clone(),
                    font_size: px(15.).into(),
                    line_height: relative(1.5).into(),
                    ..Default::default()
                },
                ..Default::default()
            }
        };

        h_flex()
            .key_context("AdversaryComposer")
            .on_action(cx.listener(|this, _: &Broadcast, _, cx| this.broadcast(cx)))
            .w_full()
            .max_w(px(920.))
            .items_end()
            .gap(px(10.))
            .rounded(px(16.))
            .bg(SURFACE_2)
            .border_1()
            .border_color(mix(colors.border, HAIRLINE_HI.into(), focus_t))
            .pl(px(16.))
            .pr(px(12.))
            .py(px(12.))
            .mb(px(22.))
            .child(
                div()
                    .flex_1()
                    .child(editor::EditorElement::new(&self.editor, editor_style)),
            )
            .child(
                // The broadcast circle — 40px `--accent-fill`, white glyph
                // (the web's `swords` glyph has no IconName analog; Send is
                // the nearest broadcast affordance).
                div()
                    .id("adv-send")
                    .flex_none()
                    .size(px(40.))
                    .rounded_full()
                    .bg(gpui::Hsla::from(ACCENT_FILL))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| this.broadcast(cx)))
                    .child(
                        Icon::new(IconName::Send)
                            .size(IconSize::Custom(rems_from_px(20.)))
                            .color(Color::Custom(gpui::white())),
                    ),
            )
            .into_any_element()
    }

    /// `.adv-cols`: three equal columns in an `h_flex`, gap 14, max 1100 —
    /// shimmering while pending, Markdown bodies once landed.
    fn render_columns(&self, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let generation = self.jobs.read(cx).generation;
        let columns: Vec<AnyElement> = match (&self.last_phase, &self.done) {
            (AdversaryPhase::Pending, _) => VENDOR_COLUMNS
                .iter()
                .map(|vendor| self.render_column(vendor, generation, None, window, cx))
                .collect(),
            (AdversaryPhase::Done(_), Some(done)) => done
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
            // `.shimmer-label` — pulsate stands in for the gradient sweep
            // (the §4.1 thread_item idiom ruling).
            None => div()
                .text_size(px(13.))
                .text_color(cx.theme().colors().text_placeholder)
                .child("Thinking…")
                .with_animation(
                    ElementId::Name(format!("adv-shimmer-{vendor}").into()),
                    Animation::new(std::time::Duration::from_millis(1400))
                        .repeat()
                        .with_easing(pulsating_between(0.4, 0.92)),
                    |label, value| label.opacity(value),
                )
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
                ElementId::Name(
                    format!("adv-col-{vendor}-{generation}-{}", landed as u8).into(),
                ),
                Animation::new(SPRING_IN).with_easing(DECEL.easing()),
                |column, t| column.opacity(t).mt(px(4. * (1. - t))),
            )
            .into_any_element()
    }

    /// `.synth-card`: hairline-hi card with the merge head + tinted section
    /// blocks. Always shown once a result lands (even an empty synthesis
    /// renders the bare head — web parity); blocks skip when empty.
    fn render_synthesis(&self, window: &Window, cx: &App) -> Option<AnyElement> {
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

    /// The web's `renderError`: the `.empty-state` block in the columns'
    /// slot (icon · "Broadcast failed" · the error text).
    fn render_error(&self, error: &str, cx: &App) -> AnyElement {
        let colors = cx.theme().colors();
        v_flex()
            .w_full()
            .items_center()
            .text_center()
            .gap(px(10.))
            .p(px(40.))
            .child(
                Icon::new(IconName::XCircle)
                    .size(IconSize::Custom(rems_from_px(34.)))
                    .color(Color::Custom(colors.text_placeholder.opacity(0.8))),
            )
            .child(
                div()
                    .text_size(px(20.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(colors.text_muted)
                    .child("Broadcast failed"),
            )
            .child(
                div()
                    .max_w(px(380.))
                    .text_size(px(13.))
                    .text_color(colors.text_placeholder)
                    .child(SharedString::from(error.to_string())),
            )
            .into_any_element()
    }
}

/// Build the per-vendor Markdown entities + parsed synthesis blocks for a
/// landed result. Missing vendors render the web's `"(no answer)"`.
fn build_done_view(result: &AdversaryResult, cx: &mut App) -> DoneView {
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
    let section = |text: &str, cx: &mut App| {
        (!text.is_empty()).then(|| markdown(text, cx))
    };
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
    style.inline_code.background_color =
        Some(crate::task_board::style::INLINE_CODE_BG.into());
    style.inline_code.font_size = Some(px(12.).into());
    style
}

impl Render for AdversaryPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let composer = self.render_composer(window, cx);
        let columns = self.render_columns(window, cx);
        let synthesis = self.render_synthesis(window, cx);
        let colors = cx.theme().colors();
        let error = match &self.last_phase {
            AdversaryPhase::Failed(error) => Some(self.render_error(&error.clone(), cx)),
            _ => None,
        };
        let panel = v_flex()
            .key_context("AdversaryPanel")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(colors.panel_background)
            .child(self.render_header(cx))
            .child(
                // `.adv-scroll`: 20/28/32 padding.
                v_flex()
                    .id("adv-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(px(28.))
                    .pt(px(20.))
                    .pb(px(32.))
                    .items_start()
                    .child(composer)
                    .children(columns)
                    .children(error)
                    .children(synthesis),
            );
        // The §8.7a frame pump: focus crossfade reads `current()` per frame.
        if self.fades.any_animating() {
            window.request_animation_frame();
        }
        panel
    }
}

impl Focusable for AdversaryPanel {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        // Focusing the panel drops the caret into the prompt box.
        self.editor.read(cx).focus_handle(cx)
    }
}

impl EventEmitter<PanelEvent> for AdversaryPanel {}

impl Panel for AdversaryPanel {
    fn persistent_name() -> &'static str {
        "AdversaryPanel"
    }

    fn panel_key() -> &'static str {
        "AdversaryPanel"
    }

    fn position(&self, _window: &Window, _cx: &App) -> DockPosition {
        self.position
    }

    fn position_is_valid(&self, position: DockPosition) -> bool {
        matches!(position, DockPosition::Left | DockPosition::Right)
    }

    fn set_position(&mut self, position: DockPosition, _: &mut Window, cx: &mut Context<Self>) {
        // Runtime-only, mirroring the task board (Z1).
        self.position = position;
        cx.notify();
    }

    fn set_active(&mut self, active: bool, _window: &mut Window, cx: &mut Context<Self>) {
        // The native route mount/unmount — the job poll lives exactly as
        // long as the dock shows the panel (TranscriptWatch pattern).
        if active {
            if self.watch.is_none() {
                self.watch = Some(self.jobs.update(cx, |jobs, cx| jobs.watch(cx)));
            }
        } else {
            self.watch = None;
        }
    }

    fn default_size(&self, _window: &Window, _cx: &App) -> Pixels {
        px(560.)
    }

    fn icon(&self, _window: &Window, _cx: &App) -> Option<IconName> {
        Some(IconName::UserGroup)
    }

    fn icon_tooltip(&self, _window: &Window, _cx: &App) -> Option<&'static str> {
        Some("Adversary")
    }

    fn toggle_action(&self) -> Box<dyn Action> {
        Box::new(ToggleFocus)
    }

    fn activation_priority(&self) -> u32 {
        7
    }
}
