//! The adversary panel's lifecycle + composer deck: the center-surface
//! lifecycle with the watch-gated job-poll lifetime, the `.adv-composer`
//! (editor + the always-visible broadcast circle), the panel head, and the
//! error state. Result rendering lives in [`super::columns`].

use gpui::{
    App, AnyElement, Context, Entity, FocusHandle, Focusable, FontWeight, SharedString,
    Subscription, Window, actions, relative,
};
use settings::Settings as _;
use ui::prelude::*;

use crate::agent_accents::ACCENT_FILL;
use crate::bridge::{AdversaryJobs, AdversaryPhase, AdversaryWatch};
use crate::task_board::motion::{STATE_FADE, StateFades, mix};
use crate::task_board::style::{HAIRLINE_HI, SURFACE_2, empty_state};

use super::columns::{DoneView, build_done_view};

actions!(
    adversary_panel,
    [
        /// Toggles focus on the adversary panel.
        ToggleFocus,
        /// Broadcasts the composer's prompt to all three models (Enter via
        /// the `AdversaryComposer > Editor` keymap binding).
        Broadcast,
        /// Aborts the in-flight broadcast — kills the three legs' children.
        Abort
    ]
);

pub struct AdversaryPanel {
    focus_handle: FocusHandle,
    pub(super) jobs: Entity<AdversaryJobs>,
    editor: Entity<editor::Editor>,
    pub(super) done: Option<DoneView>,
    /// Change gate for the jobs observer.
    pub(super) last_phase: AdversaryPhase,
    /// Held only while the center tab shows this panel — its presence keeps
    /// the job poll alive (`ModeSurface::set_surface_active`).
    watch: Option<AdversaryWatch>,
    fades: StateFades,
    _jobs_subscription: Subscription,
}

impl AdversaryPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let jobs = cx.new(|_| {
            let mut jobs = AdversaryJobs::default();
            // Agentic-demo gate: seed a staged completed broadcast so the
            // three-column + synthesis surface reviews without a live volley
            // (crate::adversary_panel_demo). `sync_from_jobs` (below) builds
            // the DoneView from this phase on the first observer tick.
            if crate::bridge::is_agentic_demo() {
                jobs.phase = crate::adversary_panel_demo::demo_phase();
            }
            jobs
        });
        let _jobs_subscription =
            cx.observe(&jobs, |this: &mut Self, _, cx| this.sync_from_jobs(cx));
        let editor = cx.new(|cx| {
            // Web parity (adversary.js:16 `<textarea rows="2">`): the adversary
            // composer opens two lines tall — the reference deliberately gives
            // this deck a taller resting height than the generic rows="1" house
            // idiom — and caps at ~160px ≈ 7 lines (panels.css:97).
            let mut editor = editor::Editor::auto_height(2, 7, window, cx);
            editor.set_placeholder_text("Ask all three models the same question…", window, cx);
            editor.set_soft_wrap();
            editor.set_show_indent_guides(false, cx);
            editor
        });
        // Build the initial done view from the jobs' phase (the demo seed sets
        // a Done phase; the live path leaves it Idle). The observer only fires
        // on later notifies, so a seeded phase must be materialized here.
        let seed_phase = jobs.read(cx).phase.clone();
        let done = match &seed_phase {
            AdversaryPhase::Done(result) | AdversaryPhase::Cancelled(result) => {
                Some(build_done_view(result, cx))
            }
            _ => None,
        };
        Self {
            focus_handle: cx.focus_handle(),
            jobs,
            editor,
            done,
            last_phase: seed_phase,
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
            // Cancelled carries the partial result — build the same column view
            // so the (aborted) legs render under the aborted banner (P4).
            AdversaryPhase::Done(result) | AdversaryPhase::Cancelled(result) => {
                Some(build_done_view(result, cx))
            }
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

    /// Broadcast a prompt supplied externally (the orchestrator's `/adversary
    /// <text>` command, S4): seed the composer so the text is visible + editable
    /// for a follow-up, then fire. Empty text opens the surface without firing.
    pub fn broadcast_text(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        let text = text.trim().to_string();
        if text.is_empty() {
            return;
        }
        self.editor
            .update(cx, |editor, cx| editor.set_text(text.clone(), window, cx));
        self.jobs.update(cx, |jobs, cx| jobs.start(text, cx));
    }

    /// Abort the in-flight broadcast (the abort affordance) — POSTs
    /// `/adversary/<job>/abort` via the jobs entity; the live poll lands the
    /// aborted result. A no-op unless a broadcast is pending.
    fn abort(&mut self, cx: &mut Context<Self>) {
        self.jobs.update(cx, |jobs, cx| jobs.abort(cx));
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

        // While a broadcast is in flight the circle becomes an abort affordance
        // (Stop glyph) — POSTs the abort endpoint; otherwise it's the
        // broadcast circle.
        let pending = matches!(self.last_phase, AdversaryPhase::Pending);
        let circle = if pending {
            div()
                .id("adv-abort")
                .on_click(cx.listener(|this, _, _, cx| this.abort(cx)))
                .child(
                    Icon::new(IconName::Stop)
                        .size(IconSize::Custom(rems_from_px(20.)))
                        .color(Color::Custom(gpui::white())),
                )
        } else {
            // The broadcast circle — the web's `swords` glyph has no IconName
            // analog; Send is the nearest broadcast affordance.
            div()
                .id("adv-send")
                .on_click(cx.listener(|this, _, _, cx| this.broadcast(cx)))
                .child(
                    Icon::new(IconName::Send)
                        .size(IconSize::Custom(rems_from_px(20.)))
                        .color(Color::Custom(gpui::white())),
                )
        };

        h_flex()
            .key_context("AdversaryComposer")
            .on_action(cx.listener(|this, _: &Broadcast, _, cx| this.broadcast(cx)))
            .on_action(cx.listener(|this, _: &Abort, _, cx| this.abort(cx)))
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
                // 40px `--accent-fill` circle, white glyph (broadcast or abort).
                circle
                    .flex_none()
                    .size(px(40.))
                    .rounded_full()
                    .bg(gpui::Hsla::from(ACCENT_FILL))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer(),
            )
            .into_any_element()
    }

    /// The web's `renderError`: the `.empty-state` block in the columns'
    /// slot (icon · "Broadcast failed" · the error text).
    fn render_error(&self, error: &str, cx: &App) -> AnyElement {
        empty_state(
            IconName::XCircle,
            "Broadcast failed",
            SharedString::from(error.to_string()),
            cx,
        )
        .into_any_element()
    }
}

impl Render for AdversaryPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let composer = self.render_composer(window, cx);
        let aborted_banner = self.render_aborted_banner(cx);
        let columns = self.render_columns(window, cx);
        let synthesis = self.render_synthesis(window, cx);
        let error = match &self.last_phase {
            AdversaryPhase::Failed(error) => Some(self.render_error(&error.clone(), cx)),
            _ => None,
        };
        let colors = cx.theme().colors();
        let panel = v_flex()
            .key_context("AdversaryPanel")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(colors.panel_background)
            .child(self.render_header(cx))
            .child(
                // `.adv-scroll`: results area, CENTERED column (Amendment
                // 2026-07-04 (4) item 3). Composer is NOT here — user ruling
                // (live review, "adversary still on top instead of bottom"):
                // the composer pins to the pane FOOT like the orchestrator;
                // results/banner/synthesis scroll above it.
                v_flex()
                    .id("adv-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(px(28.))
                    .pt(px(20.))
                    .pb(px(12.))
                    .items_center()
                    .children(aborted_banner)
                    .children(columns)
                    .children(error)
                    .children(synthesis),
            )
            .child(
                // Composer deck pinned at the bottom (orchestrator anatomy),
                // centered in the full-bleed pane.
                v_flex()
                    .flex_none()
                    .w_full()
                    .items_center()
                    .px(px(28.))
                    .pt(px(8.))
                    .pb(px(32.))
                    .child(composer),
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

impl crate::mode_item::ModeSurface for AdversaryPanel {
    // Center-pane surface (Amendment 2026-07-04 (2)) — the dock `Panel`
    // era is retired.
    fn fallback_tab_title() -> SharedString {
        "Adversary".into()
    }

    fn fallback_tab_icon() -> IconName {
        IconName::UserGroup
    }

    fn set_surface_active(&mut self, active: bool, cx: &mut Context<Self>) {
        // The native route mount/unmount — the job poll lives exactly as
        // long as the center tab shows the panel (TranscriptWatch pattern).
        if active {
            if self.watch.is_none() {
                self.watch = Some(self.jobs.update(cx, |jobs, cx| jobs.watch(cx)));
            }
        } else {
            self.watch = None;
        }
    }
}
