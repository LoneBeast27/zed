//! The Briefing panel: a compact mission-control surface assembled from the
//! pure [`crate::briefing::build_brief`] data builder.

use gpui::{
    AnyElement, App, Context, Div, Entity, FocusHandle, Focusable, FontWeight, SharedString,
    Subscription,
};
use ui::prelude::*;

use crate::bridge::{self, BridgeStore, PoolRow, RunRow};
use crate::task_board::style::SURFACE_1;

pub struct BriefingPanel {
    focus_handle: FocusHandle,
    store: Entity<BridgeStore>,
    last_seen: Option<(Vec<RunRow>, Vec<PoolRow>)>,
    _store_subscription: Subscription,
}

impl BriefingPanel {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let store = bridge::global_store(cx);
        let _store_subscription = cx.observe(&store, |this: &mut Self, store, cx| {
            let store = store.read(cx);
            let snapshot = (store.board.clone(), store.usage.clone());
            if this.last_seen.as_ref() != Some(&snapshot) {
                this.last_seen = Some(snapshot);
                cx.notify();
            }
        });
        Self {
            focus_handle: cx.focus_handle(),
            store,
            last_seen: None,
            _store_subscription,
        }
    }

    fn today_utc() -> String {
        time::OffsetDateTime::now_utc()
            .format(time::macros::format_description!(
                "[year]-[month]-[day]"
            ))
            .unwrap_or_else(|_| time::OffsetDateTime::now_utc().date().to_string())
    }

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
                    .child("Briefing"),
            )
    }

    fn render_section(section: &crate::briefing::Section, cx: &App) -> AnyElement {
        let colors = cx.theme().colors();
        let rows = section.items.iter().map(|item| {
            h_flex()
                .w_full()
                .items_start()
                .child(
                    div()
                        .text_size(px(13.))
                        .font_weight(if item.kind == "pricing" {
                            FontWeight::MEDIUM
                        } else {
                            FontWeight::NORMAL
                        })
                        .text_color(if item.kind == "pricing" {
                            colors.text_muted
                        } else {
                            colors.text_placeholder
                        })
                        .child(SharedString::from(item.text.clone())),
                )
                .into_any_element()
        });
        v_flex()
            .w_full()
            .rounded(px(12.))
            .bg(SURFACE_1)
            .border_1()
            .border_color(colors.border)
            .px(px(16.))
            .py(px(14.))
            .child(
                div()
                    .text_size(px(15.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(colors.text)
                    .child(section.label),
            )
            .child(v_flex().mt(px(12.)).gap(px(10.)).children(rows))
            .into_any_element()
    }
}

impl Render for BriefingPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (board, usage) = if crate::bridge::is_agentic_demo() {
            (
                crate::constellation::demo::demo_board(30.),
                crate::usage_panel_demo::demo_pools(),
            )
        } else {
            let store = self.store.read(cx);
            (store.board.clone(), store.usage.clone())
        };
        let today = Self::today_utc();
        let brief = crate::briefing::build_brief(&board, &usage, &today);

        let body: AnyElement = if brief.sections.is_empty() {
            crate::task_board::style::empty_state(
                IconName::ListTodo,
                "No briefing yet",
                "Briefing sections appear when board or usage data is available.",
                cx,
            )
            .into_any_element()
        } else {
            let sections: Vec<AnyElement> = brief
                .sections
                .iter()
                .map(|section| Self::render_section(section, cx))
                .collect();
            div()
                .id("briefing-sections")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .px(px(28.))
                .pt(px(20.))
                .pb(px(32.))
                .child(
                    h_flex().w_full().justify_center().child(
                        v_flex()
                            .w_full()
                            .max_w(px(720.))
                            .gap(px(16.))
                            .children(sections),
                    ),
                )
                .into_any_element()
        };

        let colors = cx.theme().colors();
        v_flex()
            .key_context("BriefingPanel")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(colors.panel_background)
            .child(self.render_header(cx))
            .child(body)
    }
}

impl Focusable for BriefingPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl crate::mode_item::ModeSurface for BriefingPanel {
    fn fallback_tab_title() -> SharedString {
        "Briefing".into()
    }

    fn fallback_tab_icon() -> IconName {
        IconName::ListTodo
    }
}
