//! The run drawer (PARITY_SPEC §4.2, Codex anatomy / web `drawer.js`):
//! right slide-over with head (vendor swatch · title · status pill · close),
//! worked-for strip, flat text tabs Summary | Result | Logs, body fetched
//! from `GET /run/<id>` (tail-polled 1s while running). Esc or scrim-click
//! closes; entrance/exit slide on the decel curve with a concurrent scrim
//! crossfade (§4.9 spatial/effects split).

use std::sync::Arc;
use std::time::Duration;

use gpui::{
    Animation, AnimationExt as _, AnyElement, App, Context, Entity, EventEmitter, FocusHandle,
    Focusable, FontWeight, KeyDownEvent, SharedString, Task, Window, relative,
};
use http_client::HttpClient;
use markdown::{Markdown, MarkdownElement};
use serde::Deserialize;
use settings::Settings as _;
use theme_settings::ThemeSettings;
use ui::prelude::*;

use crate::agent_accents::accent_for_agent;
use crate::agent_configuration::configure_context_server_modal::default_markdown_style;
use crate::bridge::{BRIDGE_BASE_URL, fetch_json};

use super::motion::{DECEL, EFFECTS};
use super::style::{HAIRLINE_HI, SURFACE_2B, chip_reason, rel, status_pill};

/// Logs tail-poll cadence while the run is live (web: 1s).
const LOG_POLL: Duration = Duration::from_secs(1);
/// Slide-over duration (web: transform .4s decel).
const SLIDE: Duration = Duration::from_millis(400);
/// Scrim crossfade (web: opacity .22s effects).
const SCRIM_FADE: Duration = Duration::from_millis(220);

/// `GET /run/<id>` payload. Liberal: everything defaults.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RunDetail {
    #[serde(default)]
    pub run_id: String,
    #[serde(default)]
    pub agent: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub elapsed_s: f64,
    #[serde(default)]
    pub task: Option<String>,
    #[serde(default)]
    pub chip: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub usage: Option<serde_json::Map<String, serde_json::Value>>,
    #[serde(default)]
    pub events: Vec<RunEvent>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct RunEvent {
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DrawerTab {
    Summary,
    Result,
    Logs,
}

/// Emitted when the drawer has finished closing — the panel drops it.
#[derive(Debug, Clone)]
pub struct DismissDrawer;

pub struct RunDrawer {
    run_id: SharedString,
    detail: Option<RunDetail>,
    tab: DrawerTab,
    /// Markdown entities for the summary task + result body, rebuilt when
    /// the underlying text changes.
    task_md: Option<Entity<Markdown>>,
    result_md: Option<Entity<Markdown>>,
    focus_handle: FocusHandle,
    needs_focus: bool,
    closing: bool,
    _poll: Task<()>,
}

impl RunDrawer {
    pub fn new(run_id: SharedString, cx: &mut Context<Self>) -> Self {
        let http_client: Arc<dyn HttpClient> = cx.http_client();
        let poll_run_id = run_id.clone();
        let poll = cx.spawn(async move |this, cx| {
            loop {
                let client = http_client.clone();
                let url = format!("{BRIDGE_BASE_URL}/run/{poll_run_id}");
                let fetched = cx
                    .background_spawn(async move {
                        let raw = fetch_json(client.as_ref(), &url).await?;
                        anyhow::Ok(serde_json::from_str::<RunDetail>(&raw)?)
                    })
                    .await;
                let running = match fetched {
                    Ok(detail) => {
                        let running = detail.status == "running";
                        if this
                            .update(cx, |this, cx| this.set_detail(detail, cx))
                            .is_err()
                        {
                            return;
                        }
                        running
                    }
                    // Bridge hiccup — keep the last detail, stop tailing.
                    Err(_) => false,
                };
                if !running {
                    return;
                }
                cx.background_executor().timer(LOG_POLL).await;
            }
        });
        Self {
            run_id,
            detail: None,
            tab: DrawerTab::Summary,
            task_md: None,
            result_md: None,
            focus_handle: cx.focus_handle(),
            needs_focus: true,
            closing: false,
            _poll: poll,
        }
    }

    fn set_detail(&mut self, detail: RunDetail, cx: &mut Context<Self>) {
        let task_changed =
            self.detail.as_ref().map(|d| d.task.clone()) != Some(detail.task.clone());
        let text_changed =
            self.detail.as_ref().map(|d| d.text.clone()) != Some(detail.text.clone());
        if task_changed || self.task_md.is_none() {
            self.task_md = detail.task.clone().map(|task| {
                cx.new(|cx| Markdown::new(SharedString::from(task), None, None, cx))
            });
        }
        if text_changed || self.result_md.is_none() {
            self.result_md = detail.text.clone().map(|text| {
                cx.new(|cx| Markdown::new(SharedString::from(text), None, None, cx))
            });
        }
        self.detail = Some(detail);
        cx.notify();
    }

    /// Begin the close: run the exit slide, then tell the panel to drop us.
    pub fn dismiss(&mut self, cx: &mut Context<Self>) {
        if self.closing {
            return;
        }
        self.closing = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SLIDE).await;
            this.update(cx, |_, cx| cx.emit(DismissDrawer)).ok();
        })
        .detach();
    }

    fn set_tab(&mut self, tab: DrawerTab, cx: &mut Context<Self>) {
        if self.tab != tab {
            self.tab = tab;
            cx.notify();
        }
    }

    // ── render pieces ──────────────────────────────────────────────────────

    fn render_head(&self, cx: &mut Context<Self>) -> Div {
        let colors = cx.theme().colors();
        let mono = ThemeSettings::get_global(cx).buffer_font.family.clone();
        let detail = self.detail.clone().unwrap_or_default();
        let agent = if detail.agent.is_empty() {
            "agent".to_string()
        } else {
            detail.agent.clone()
        };
        let reason = {
            let r = chip_reason(detail.chip.as_deref(), &agent);
            if r.is_empty() { "routed".to_string() } else { r }
        };
        h_flex()
            .flex_none()
            .items_center()
            .gap(px(11.))
            .px(px(22.))
            .pt(px(18.))
            .child(
                div()
                    .flex_none()
                    .size(px(9.))
                    .rounded_full()
                    .bg(accent_for_agent(&agent)),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_size(px(15.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(colors.text)
                            .child(SharedString::from(format!("{agent} run"))),
                    )
                    .child(
                        div()
                            .mt(px(3.))
                            .font_family(mono)
                            .text_size(px(12.5))
                            .text_color(colors.text_placeholder)
                            .truncate()
                            .child(SharedString::from(format!(
                                "{agent} · {reason} · {}",
                                rel(detail.elapsed_s)
                            ))),
                    ),
            )
            .child(status_pill("drawer-pill", &detail.status, None, cx))
            .child(
                IconButton::new("drawer-close", IconName::Close)
                    .icon_color(Color::Muted)
                    .on_click(cx.listener(|this, _, _, cx| this.dismiss(cx))),
            )
    }

    fn render_worked(&self, cx: &Context<Self>) -> Option<Div> {
        let colors = cx.theme().colors();
        let detail = self.detail.as_ref()?;
        let still = if detail.status == "running" {
            " · still running"
        } else {
            ""
        };
        Some(
            h_flex()
                .flex_none()
                .items_center()
                .gap(px(7.))
                .px(px(22.))
                .pt(px(12.))
                .text_size(px(13.))
                .text_color(colors.text_muted)
                .child(
                    Icon::new(IconName::Clock)
                        .size(IconSize::Small)
                        .color(Color::Muted),
                )
                .child("Worked for")
                .child(
                    div()
                        .text_color(colors.text)
                        .font_weight(FontWeight::MEDIUM)
                        .child(SharedString::from(rel(detail.elapsed_s))),
                )
                .child(SharedString::from(still.to_string())),
        )
    }

    /// Flat text tabs (13px/500, active = full text + 2px accent underline,
    /// no boxes — board.css `.drawer-tabs`).
    fn render_tabs(&self, cx: &mut Context<Self>) -> Div {
        let colors = cx.theme().colors();
        let tab = |label: &'static str, value: DrawerTab, this: &Self| {
            let on = this.tab == value;
            div()
                .id(ElementId::Name(format!("drawer-tab-{label}").into()))
                .px(px(13.))
                .pt(px(8.))
                .pb(px(10.))
                .text_size(px(13.))
                .font_weight(FontWeight::MEDIUM)
                .cursor_pointer()
                .border_b_2()
                .when(on, |t| t.text_color(colors.text).border_color(colors.text_accent))
                .when(!on, |t| {
                    t.text_color(colors.text_placeholder)
                        .border_color(gpui::transparent_black())
                })
                .child(label)
        };
        h_flex()
            .flex_none()
            .gap(px(2.))
            .px(px(22.))
            .pt(px(14.))
            .border_b_1()
            .border_color(colors.border)
            .child(tab("Summary", DrawerTab::Summary, self).on_click(
                cx.listener(|this, _, _, cx| this.set_tab(DrawerTab::Summary, cx)),
            ))
            .child(tab("Result", DrawerTab::Result, self).on_click(
                cx.listener(|this, _, _, cx| this.set_tab(DrawerTab::Result, cx)),
            ))
            .child(tab("Logs", DrawerTab::Logs, self).on_click(
                cx.listener(|this, _, _, cx| this.set_tab(DrawerTab::Logs, cx)),
            ))
    }

    fn render_body(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let body = match self.tab {
            DrawerTab::Summary => self.render_summary(window, cx),
            DrawerTab::Result => self.render_result(window, cx),
            DrawerTab::Logs => self.render_logs(cx),
        };
        div()
            .id("drawer-body")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px(px(22.))
            .pt(px(18.))
            .pb(px(28.))
            .child(body)
            .into_any_element()
    }

    fn field(&self, key: &'static str, value: AnyElement, cx: &App) -> Div {
        let colors = cx.theme().colors();
        v_flex()
            .mb(px(16.))
            .child(
                div()
                    .mb(px(5.))
                    .text_size(px(11.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(colors.text_placeholder)
                    .child(SharedString::from(key.to_uppercase())),
            )
            .child(value)
    }

    fn mono_value(&self, text: String, cx: &App) -> AnyElement {
        let colors = cx.theme().colors();
        let mono = ThemeSettings::get_global(cx).buffer_font.family.clone();
        div()
            .font_family(mono)
            .text_size(px(12.))
            .text_color(colors.text_muted)
            .child(SharedString::from(text))
            .into_any_element()
    }

    fn pre_block(&self, text: String, cx: &App) -> AnyElement {
        let colors = cx.theme().colors();
        let mono = ThemeSettings::get_global(cx).buffer_font.family.clone();
        div()
            .rounded(px(12.))
            .border_1()
            .border_color(colors.border)
            .bg(SURFACE_2B)
            .px(px(16.))
            .py(px(14.))
            .font_family(mono)
            .text_size(px(12.5))
            .text_color(colors.text)
            .child(SharedString::from(text))
            .into_any_element()
    }

    fn render_summary(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors();
        let Some(detail) = self.detail.clone() else {
            return self.mono_value("Loading…".into(), cx);
        };
        let text_value = |s: String| {
            div()
                .text_size(px(13.))
                .text_color(colors.text_muted)
                .child(SharedString::from(s))
                .into_any_element()
        };
        let usage_line = detail.usage.as_ref().map(|usage| {
            usage
                .iter()
                .filter(|(_, v)| !v.is_null())
                .map(|(k, v)| format!("{k}: {v}"))
                .collect::<Vec<_>>()
                .join("  ·  ")
        });
        let task_body: AnyElement = match &self.task_md {
            Some(md) => MarkdownElement::new(md.clone(), default_markdown_style(window, cx))
                .into_any_element(),
            None => text_value(String::new()),
        };
        v_flex()
            .child(self.field(
                "Routing",
                text_value(detail.chip.clone().unwrap_or_default()),
                cx,
            ))
            .child(self.field("Task", task_body, cx))
            .children(
                detail
                    .error
                    .clone()
                    .map(|error| self.field("Error", self.pre_block(error, cx), cx)),
            )
            .children(
                usage_line
                    .filter(|line| !line.is_empty())
                    .map(|line| self.field("Usage", self.mono_value(line, cx), cx)),
            )
            .child(self.field(
                "Run",
                self.mono_value(
                    format!("{} · {}", detail.run_id, rel(detail.elapsed_s)),
                    cx,
                ),
                cx,
            ))
            .into_any_element()
    }

    fn render_result(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors();
        match (&self.result_md, &self.detail) {
            (Some(md), _) => div()
                .rounded(px(12.))
                .border_1()
                .border_color(colors.border)
                .bg(SURFACE_2B)
                .px(px(16.))
                .py(px(14.))
                .child(MarkdownElement::new(
                    md.clone(),
                    default_markdown_style(window, cx),
                ))
                .into_any_element(),
            (None, Some(detail)) => {
                let hint = if detail.status == "running" {
                    "No result text yet — still running."
                } else {
                    "No result text."
                };
                div()
                    .text_size(px(13.))
                    .text_color(colors.text_muted)
                    .child(hint)
                    .into_any_element()
            }
            (None, None) => self.mono_value("Loading…".into(), cx),
        }
    }

    fn render_logs(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors();
        let mono = ThemeSettings::get_global(cx).buffer_font.family.clone();
        let events = self
            .detail
            .as_ref()
            .map(|d| d.events.clone())
            .unwrap_or_default();
        if events.is_empty() {
            return div()
                .text_size(px(13.))
                .text_color(colors.text_muted)
                .child("No events recorded.")
                .into_any_element();
        }
        let rows = events.into_iter().enumerate().map(|(ix, event)| {
            let payload: String = event
                .payload
                .to_string()
                .chars()
                .take(400)
                .collect();
            h_flex()
                .id(ElementId::NamedInteger("log-row".into(), ix as u64))
                .items_start()
                .gap(px(10.))
                .py(px(7.))
                .border_b_1()
                .border_color(colors.border)
                .font_family(mono.clone())
                .text_size(px(12.))
                .child(
                    div()
                        .flex_none()
                        .min_w(px(52.))
                        .text_color(colors.text_placeholder)
                        .child(SharedString::from(event.kind)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_color(colors.text_muted)
                        .child(SharedString::from(payload)),
                )
        });
        v_flex().children(rows).into_any_element()
    }
}

impl Render for RunDrawer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.needs_focus && !self.closing {
            self.needs_focus = false;
            window.focus(&self.focus_handle, cx);
        }
        let colors = cx.theme().colors();
        let closing = self.closing;

        // Scrim: solid rgba fallback per the web's non-backdrop-filter path;
        // fades on the effects curve, concurrent with the slide (§4.9).
        let scrim = div()
            .id("drawer-scrim")
            .absolute()
            .inset_0()
            .bg(gpui::black().opacity(0.5))
            .occlude()
            .on_click(cx.listener(|this, _, _, cx| this.dismiss(cx)))
            .with_animation(
                ElementId::Name(
                    if closing { "drawer-scrim-out" } else { "drawer-scrim-in" }.into(),
                ),
                Animation::new(SCRIM_FADE).with_easing(EFFECTS.easing()),
                move |scrim, t| scrim.opacity(if closing { 1.0 - t } else { t }),
            );

        let sheet = v_flex()
            .id("drawer-sheet")
            .absolute()
            .top_0()
            .bottom_0()
            .right_0()
            .w(relative(0.92))
            .max_w(px(560.))
            .bg(colors.panel_background)
            .border_l_1()
            .border_color(HAIRLINE_HI)
            .occlude()
            .child(self.render_head(cx))
            .children(self.render_worked(cx))
            .child(self.render_tabs(cx))
            .child(self.render_body(window, cx))
            .with_animation(
                ElementId::Name(
                    if closing { "drawer-slide-out" } else { "drawer-slide-in" }.into(),
                ),
                Animation::new(SLIDE).with_easing(DECEL.easing()),
                move |sheet, t| {
                    // Slide across the full drawer width: offscreen at 1.
                    let out = if closing { t } else { 1.0 - t };
                    sheet.right(relative(-0.92 * out))
                },
            );

        div()
            .key_context("RunDrawer")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    this.dismiss(cx);
                }
            }))
            .absolute()
            .inset_0()
            .child(scrim)
            .child(sheet)
    }
}

impl Focusable for RunDrawer {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<DismissDrawer> for RunDrawer {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_detail_deserializes_liberally() {
        let detail: RunDetail = serde_json::from_str(
            r#"{
                "run_id": "r-9", "agent": "claude", "status": "running",
                "elapsed_s": 12.5, "task": "do the thing",
                "chip": "claude · code-heavy · 92%",
                "usage": {"tokens": 1234, "cost": null},
                "events": [
                    {"kind": "spawn", "payload": {"a": 1}},
                    {"kind": "log", "payload": "line"}
                ],
                "unknown_field": true
            }"#,
        )
        .unwrap();
        assert_eq!(detail.run_id, "r-9");
        assert_eq!(detail.status, "running");
        assert_eq!(detail.events.len(), 2);
        assert_eq!(detail.events[0].kind, "spawn");
        assert_eq!(detail.text, None);
        assert_eq!(detail.error, None);

        // Minimal payload also parses.
        let minimal: RunDetail = serde_json::from_str("{}").unwrap();
        assert_eq!(minimal.events.len(), 0);
    }
}
