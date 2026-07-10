//! The PERMISSIONS render shell (design §5.1 `view.rs`, split at the
//! 500-line ceiling): the panel `Render` impl (header · honest pre-snapshot
//! states · the centered card stack) and the shared card/button/note
//! builders. The policy cards (mode / enforcement / rules) live in
//! [`super::policy_cards`], the runtime cards (pending approvals inbox /
//! audit tail) in [`super::inbox`].
//!
//! Degrade law (the axioms contract): loading / "bridge unreachable" /
//! honest-empty via the shared `empty_state`; a stale snapshot renders under
//! the amber last-known note; NO fake rows on connection failure, ever.

use gpui::{AnyElement, App, Context, Div, ElementId, FontWeight, SharedString, Window};
use settings::Settings as _;
use theme_settings::ThemeSettings;
use ui::prelude::*;

use crate::agent_accents::{ACCENT_FILL, STATUS_BLOCKED};
use crate::bridge;
use crate::task_board::style::{SURFACE_1, empty_state};

use super::PermissionsPanel;

pub(super) fn now_unix() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|epoch| epoch.as_secs_f64())
        .unwrap_or(0.0)
}

impl PermissionsPanel {
    /// `.panel-head`: "Permissions" 18px/500 + the policy sub-line.
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
                    .child("Permissions"),
            )
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(colors.text_placeholder)
                    .child("spawn-gate policy — mode · rules · approvals · audit"),
            )
    }

    /// One `.setting-card`: h2 over rows (+ optional hint) — the
    /// settings_status_panel anatomy.
    pub(super) fn card(
        &self,
        title: &'static str,
        rows: Vec<AnyElement>,
        hint: Option<&'static str>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let colors = cx.theme().colors();
        v_flex()
            .w_full()
            .max_w(px(720.))
            .rounded(px(12.))
            .bg(SURFACE_1)
            .border_1()
            .border_color(colors.border)
            .px(px(18.))
            .py(px(16.))
            .mb(px(16.))
            .child(
                div()
                    .mb(px(10.))
                    .text_size(px(15.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(colors.text)
                    .child(title),
            )
            .children(rows)
            .children(hint.map(|hint| {
                div()
                    .mt(px(10.))
                    .text_size(px(12.))
                    .line_height(relative(1.6))
                    .text_color(colors.text_placeholder)
                    .child(hint)
            }))
            .into_any_element()
    }

    /// An action button (the writeback `wb_button` idiom): accent-filled
    /// primary / hairline secondary; disabled renders the honest inert state.
    pub(super) fn button(
        &self,
        id: ElementId,
        label: SharedString,
        primary: bool,
        enabled: bool,
        cx: &Context<Self>,
        on_click: impl Fn(&mut PermissionsPanel, &mut Window, &mut Context<PermissionsPanel>)
        + 'static,
    ) -> AnyElement {
        let colors = cx.theme().colors();
        let base = div()
            .id(id)
            .px(px(9.))
            .py(px(4.))
            .rounded(px(8.))
            .text_size(px(12.))
            .font_weight(FontWeight::MEDIUM);
        if !enabled {
            return base
                .border_1()
                .border_color(colors.border)
                .text_color(colors.text_placeholder)
                .child(label)
                .into_any_element();
        }
        let styled = if primary {
            base.bg(ACCENT_FILL).text_color(gpui::black())
        } else {
            base.border_1()
                .border_color(colors.border)
                .text_color(colors.text_muted)
                .hover(|s| s.border_color(colors.border_variant))
        };
        styled
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, window, cx| on_click(this, window, cx)))
            .child(label)
            .into_any_element()
    }

    /// The in-flight "working…" note (buttons collapse — the axioms idiom).
    pub(super) fn working_note(&self, mono: &SharedString, cx: &Context<Self>) -> AnyElement {
        div()
            .text_size(px(12.))
            .font_family(mono.clone())
            .italic()
            .text_color(cx.theme().colors().text_placeholder)
            .child("working…")
            .into_any_element()
    }

    /// A verbatim action-outcome note under a card/row.
    pub(super) fn note_line(
        &self,
        key: &str,
        mono: &SharedString,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        self.notes.get(key).map(|note| {
            div()
                .w_full()
                .text_size(px(11.))
                .font_family(mono.clone())
                .text_color(cx.theme().colors().text_muted)
                .child(SharedString::from(note.clone()))
                .into_any_element()
        })
    }
}

impl gpui::Render for PermissionsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Owned copy: the card builders need `&mut cx` while the shell still
        // reads colors (the adversary-panel idiom).
        let colors = cx.theme().colors().clone();
        let mono = ThemeSettings::get_global(cx).buffer_font.family.clone();

        let body: AnyElement = if self.snapshot.is_none() {
            // The three honest pre-snapshot states: stale (and no retry in
            // flight) ⇒ unreachable, else loading (a fetch fires on
            // activation and after every action).
            if self.stale && !self.loading {
                empty_state(
                    IconName::XCircle,
                    "Permissions unavailable",
                    "Could not reach the bridge at 127.0.0.1:4530 — the policy \
                     state will appear when it is back.",
                    cx,
                )
                .into_any_element()
            } else {
                empty_state(
                    IconName::ArrowCircle,
                    "Loading permissions…",
                    "Fetching the policy state from the bridge.",
                    cx,
                )
                .into_any_element()
            }
        } else {
            let mut column = v_flex().w_full().max_w(px(720.));
            if self.stale {
                column = column.child(
                    div()
                        .mb(px(10.))
                        .text_size(px(11.))
                        .text_color(STATUS_BLOCKED)
                        .child("bridge unreachable — showing last-known permissions"),
                );
            }
            if bridge::is_agentic_demo() {
                column = column.child(
                    div()
                        .mb(px(10.))
                        .text_size(px(11.))
                        .text_color(colors.text_placeholder)
                        .child("agentic demo — staged policy state, actions disabled"),
                );
            }
            column
                .child(self.render_mode_card(&mono, cx))
                .child(self.render_enforcement_card(&mono, cx))
                .child(self.render_rules_card(&mono, cx))
                .child(self.render_pending_card(&mono, cx))
                .child(self.render_audit_card(&mono, cx))
                .into_any_element()
        };

        v_flex()
            .key_context("PermissionsPanel")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(colors.panel_background)
            .child(self.render_header(cx))
            .child(
                // Centered bounded column (Amendment 2026-07-04 (4) item 3).
                v_flex()
                    .id("permissions-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(px(28.))
                    .pt(px(20.))
                    .pb(px(32.))
                    .items_center()
                    .child(body),
            )
    }
}
