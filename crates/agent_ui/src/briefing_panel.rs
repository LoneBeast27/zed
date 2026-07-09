//! The Briefing panel: a compact mission-control surface assembled from the
//! pure [`crate::briefing::build_brief`] data builder, plus the bridge's
//! Morning Brief (Wave-2 briefs engine — `GET /briefs`, dogfood 2026-07-08):
//! the deterministic routine-generated brief renders as a markdown card
//! above the live sections, with a Generate-now action.

use gpui::{
    AnyElement, App, AppContext as _, Context, Div, Entity, FocusHandle, Focusable, FontWeight,
    SharedString, Subscription, Task,
};
use markdown::{Markdown, MarkdownElement};
use ui::prelude::*;

use crate::bridge::{self, BRIDGE_BASE_URL, BridgeStore, PoolRow, RunRow, fetch_json, post_json};
use crate::task_board::style::SURFACE_1;

/// The bridge project the brief belongs to. The bridge boots with a single
/// "default" project on this deployment; a project picker is a later slice.
const BRIEF_PROJECT: &str = "default";

/// The latest bridge Morning Brief, view-ready (the markdown body carries
/// its own "# Morning brief — …" title heading).
struct BridgeBrief {
    body: Entity<Markdown>,
}

pub struct BriefingPanel {
    focus_handle: FocusHandle,
    store: Entity<BridgeStore>,
    last_seen: Option<(Vec<RunRow>, Vec<PoolRow>)>,
    /// The bridge's newest Morning Brief (`GET /briefs`); `None` until the
    /// first activation fetch lands (or when none has been generated yet).
    bridge_brief: Option<BridgeBrief>,
    generating: bool,
    _brief_fetch: Option<Task<()>>,
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
            bridge_brief: None,
            generating: false,
            _brief_fetch: None,
            _store_subscription,
        }
    }

    /// One-shot `GET /briefs` (visibility-gated: fired on tab activation and
    /// after a generate). A missing/erroring bridge leaves the local live
    /// sections — the panel never blanks on the fetch.
    fn fetch_bridge_brief(&mut self, cx: &mut Context<Self>) {
        let client = cx.http_client();
        self._brief_fetch = Some(cx.spawn(async move |this, cx| {
            let fetched = cx
                .background_spawn(async move {
                    let url = format!("{BRIDGE_BASE_URL}/briefs?project={BRIEF_PROJECT}");
                    let raw = fetch_json(client.as_ref(), &url).await?;
                    anyhow::Ok(serde_json::from_str::<serde_json::Value>(&raw)?)
                })
                .await;
            let Ok(value) = fetched else { return };
            let latest = value.get("latest").cloned().unwrap_or(serde_json::Value::Null);
            let (title, body) = (
                latest.get("title").and_then(|t| t.as_str()).map(str::to_string),
                latest.get("body").and_then(|b| b.as_str()).map(str::to_string),
            );
            let _ = title;
            this.update(cx, |this, cx| {
                this.generating = false;
                if let Some(body) = body {
                    let body = cx.new(|cx| Markdown::new(SharedString::from(body), None, None, cx));
                    this.bridge_brief = Some(BridgeBrief { body });
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// POST `/briefs/generate` (deterministic, zero LLM cost) then refetch.
    fn generate_brief(&mut self, cx: &mut Context<Self>) {
        if self.generating {
            return;
        }
        self.generating = true;
        cx.notify();
        let client = cx.http_client();
        self._brief_fetch = Some(cx.spawn(async move |this, cx| {
            let _ = cx
                .background_spawn(async move {
                    let url = format!("{BRIDGE_BASE_URL}/briefs/generate");
                    let body = format!("{{\"project\": \"{BRIEF_PROJECT}\"}}");
                    post_json(client.as_ref(), &url, body).await
                })
                .await;
            this.update(cx, |this, cx| this.fetch_bridge_brief(cx)).ok();
        }));
    }

    fn today_utc() -> String {
        time::OffsetDateTime::now_utc()
            .format(time::macros::format_description!(
                "[year]-[month]-[day]"
            ))
            .unwrap_or_else(|_| time::OffsetDateTime::now_utc().date().to_string())
    }

    fn render_header(&self, cx: &mut Context<Self>) -> Div {
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
            .child(div().flex_1())
            .child(
                // Generate-now (the Morning Brief routine's mechanism, on
                // demand — deterministic, zero LLM cost).
                div()
                    .id("brief-generate")
                    .px(px(10.))
                    .py(px(4.))
                    .rounded(px(8.))
                    .border_1()
                    .border_color(colors.border)
                    .text_size(px(12.))
                    .text_color(colors.text_muted)
                    .cursor_pointer()
                    .hover(|s| s.bg(cx.theme().colors().element_hover))
                    .on_click(cx.listener(|this, _, _, cx| this.generate_brief(cx)))
                    .child(if self.generating {
                        "Generating…"
                    } else {
                        "Generate now"
                    }),
            )
    }

    /// The bridge Morning Brief card (markdown body under its title).
    fn render_bridge_brief(&self, window: &mut Window, cx: &App) -> Option<AnyElement> {
        let brief = self.bridge_brief.as_ref()?;
        let colors = cx.theme().colors();
        Some(
            v_flex()
                .w_full()
                .rounded(px(12.))
                .bg(SURFACE_1)
                .border_1()
                .border_color(colors.border)
                .px(px(16.))
                .py(px(14.))
                .child(
                    // Full-width wrapper: an unconstrained MarkdownElement
                    // under-measures its wrapped height here and the card's
                    // tail overlapped the next section (sweep 2026-07-08).
                    div().w_full().overflow_hidden().child(MarkdownElement::new(
                        brief.body.clone(),
                        markdown::MarkdownStyle::themed(markdown::MarkdownFont::Agent, window, cx),
                    )),
                )
                .into_any_element(),
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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

        let bridge_card = self.render_bridge_brief(window, cx);
        let body: AnyElement = if brief.sections.is_empty() && bridge_card.is_none() {
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
                            // The bridge Morning Brief leads (routine-
                            // generated markdown); live sections follow.
                            .children(bridge_card)
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

    fn set_surface_active(&mut self, active: bool, cx: &mut Context<Self>) {
        // Visibility-gated one-shot: refresh the bridge Morning Brief each
        // time the tab comes on screen (no poll — briefs change on routine
        // fire or the Generate button, both rare).
        if active {
            self.fetch_bridge_brief(cx);
        }
    }
}
