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
    Focusable, FontWeight, KeyDownEvent, Pixels, SharedString, Task, Window, canvas, relative,
};
use http_client::HttpClient;
use markdown::Markdown;
use serde::Deserialize;
use settings::Settings as _;
use theme_settings::ThemeSettings;
use ui::prelude::*;

use crate::agent_accents::accent_for_agent;
use crate::bridge::{BRIDGE_BASE_URL, fetch_json};

use super::motion::{DECEL, EFFECTS, STATE_FADE, StateFade, mix};
use super::style::{HAIRLINE_HI, chip_reason, rel, status_pill};

/// Logs tail-poll cadence while the run is live (web: 1s).
const LOG_POLL: Duration = Duration::from_secs(1);
/// Tail-poll backoff cap while the bridge is erroring — the poll never stops
/// on a transient fetch error (web drawer.js skips the tick and the interval
/// retries); it just slows down until the bridge answers again.
const LOG_POLL_ERROR_CAP: Duration = Duration::from_secs(5);
/// Slide-over duration (web: transform .4s decel).
const SLIDE: Duration = Duration::from_millis(400);
/// Scrim crossfade (web: opacity .22s effects).
const SCRIM_FADE: Duration = Duration::from_millis(220);
/// Sheet width (web: `width: min(560px, 92vw)` — board.css:164).
const SHEET_FRACTION: f32 = 0.92;
const SHEET_MAX_WIDTH: f32 = 560.;

/// Drawer slide travel in SHEET-width terms: CSS `translateX(102%)` resolves
/// against the element's OWN width (board.css:167), so the offscreen offset
/// is −(resolved sheet width) × 1.02 — never a fraction of the panel, which
/// over-travels on wide panels (entrance dead-zone + ~2× apparent velocity,
/// the §8.7(b) gate failure).
fn slide_offset(panel_width: f32, out: f32) -> f32 {
    -(panel_width * SHEET_FRACTION).min(SHEET_MAX_WIDTH) * 1.02 * out
}

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
pub(super) enum DrawerTab {
    Summary,
    Result,
    Logs,
}

/// Emitted when the drawer has finished closing — the panel drops it.
#[derive(Debug, Clone)]
pub struct DismissDrawer;

pub struct RunDrawer {
    detail: Option<RunDetail>,
    tab: DrawerTab,
    /// The previously active tab — the 150ms tab crossfade eases both the
    /// leaving and the arriving tab (web `.drawer-tabs button` transition).
    prev_tab: DrawerTab,
    tab_fade: StateFade,
    /// Close-button tracked hover (web `#drawer-close` 150ms transition).
    close_hovered: bool,
    close_fade: StateFade,
    /// Containing-panel width, measured each frame by a layout canvas — the
    /// slide animator converts it to the resolved sheet width so travel
    /// matches the web's `translateX(102%)` at every panel width.
    container_width: Option<Pixels>,
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
        let poll = cx.spawn(async move |this, cx| {
            let mut failures: u32 = 0;
            loop {
                let client = http_client.clone();
                let url = format!("{BRIDGE_BASE_URL}/run/{run_id}");
                let fetched = cx
                    .background_spawn(async move {
                        let raw = fetch_json(client.as_ref(), &url).await?;
                        anyhow::Ok(serde_json::from_str::<RunDetail>(&raw)?)
                    })
                    .await;
                let running = match fetched {
                    Ok(detail) => {
                        failures = 0;
                        let running = detail.status == "running";
                        if this
                            .update(cx, |this, cx| this.set_detail(detail, cx))
                            .is_err()
                        {
                            return;
                        }
                        running
                    }
                    // Bridge hiccup — keep the last detail and keep tailing;
                    // continuation follows the last-known status (web
                    // drawer.js catches, skips the tick, and retries).
                    Err(_) => {
                        failures += 1;
                        match this.update(cx, |this, _| {
                            this.detail
                                .as_ref()
                                .is_none_or(|detail| detail.status == "running")
                        }) {
                            Ok(running) => running,
                            Err(_) => return,
                        }
                    }
                };
                if !running {
                    return;
                }
                // 1s healthy cadence; back off 1s per consecutive failure,
                // capped at 5s, so an offline bridge isn't hammered.
                let delay = (LOG_POLL * (failures + 1)).min(LOG_POLL_ERROR_CAP);
                cx.background_executor().timer(delay).await;
            }
        });
        Self {
            detail: None,
            tab: DrawerTab::Summary,
            prev_tab: DrawerTab::Summary,
            tab_fade: StateFade::default(),
            close_hovered: false,
            close_fade: StateFade::default(),
            container_width: None,
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
            self.prev_tab = self.tab;
            self.tab = tab;
            self.tab_fade.bump();
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
            .child(self.render_close(cx))
    }

    /// `#drawer-close`: 30×30 grid-centered button, 20px glyph, 8px radius;
    /// hover swaps text-3 → text over a hover bg, both eased 150ms effects
    /// (board.css:181-184).
    fn render_close(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors();
        let (off_text, on_text) = (colors.text_placeholder, colors.text);
        let (off_bg, on_bg) = (gpui::transparent_black(), colors.element_hover);
        let hovered = self.close_hovered;
        let icon = move |color: gpui::Hsla| {
            Icon::new(IconName::Close)
                .size(IconSize::Custom(rems_from_px(20.)))
                .color(Color::Custom(color))
        };
        let base = div()
            .id("drawer-close")
            .flex_none()
            .size(px(30.))
            .rounded(px(8.))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if this.close_hovered != *hovered {
                    this.close_hovered = *hovered;
                    this.close_fade.bump();
                    cx.notify();
                }
            }))
            .on_click(cx.listener(|this, _, _, cx| this.dismiss(cx)));
        if self.close_fade.fresh() {
            let (from_text, to_text) = if hovered {
                (off_text, on_text)
            } else {
                (on_text, off_text)
            };
            let (from_bg, to_bg) = if hovered { (off_bg, on_bg) } else { (on_bg, off_bg) };
            base.with_animation(
                ElementId::NamedInteger(
                    "drawer-close-fade".into(),
                    self.close_fade.generation() as u64,
                ),
                Animation::new(STATE_FADE).with_easing(EFFECTS.easing()),
                move |button, t| {
                    button
                        .bg(mix(from_bg, to_bg, t))
                        .child(icon(mix(from_text, to_text, t)))
                },
            )
            .into_any_element()
        } else if hovered {
            base.bg(on_bg).child(icon(on_text)).into_any_element()
        } else {
            base.child(icon(off_text)).into_any_element()
        }
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
    /// no boxes — board.css `.drawer-tabs`). Each tab pulls down 1px so the
    /// active underline OVERLAPS the container hairline (one merged line),
    /// and tab switches crossfade color/underline over 150ms effects.
    fn render_tabs(&self, cx: &mut Context<Self>) -> Div {
        let colors = cx.theme().colors();
        let border = colors.border;
        let on_text = colors.text;
        let off_text = colors.text_placeholder;
        let on_border = colors.text_accent;
        let off_border = gpui::transparent_black();
        let fresh = self.tab_fade.fresh();
        let generation = self.tab_fade.generation() as u64;
        let (current, previous) = (self.tab, self.prev_tab);

        let tab = |label: &'static str, value: DrawerTab, cx: &Context<Self>| -> AnyElement {
            let on = current == value;
            let was_on = previous == value;
            let base = div()
                .id(ElementId::Name(format!("drawer-tab-{label}").into()))
                .px(px(13.))
                .pt(px(8.))
                .pb(px(10.))
                // board.css `.drawer-tabs button { margin-bottom: -1px }` —
                // the 2px accent underline merges with the strip hairline.
                .mb(px(-1.))
                .text_size(px(13.))
                .font_weight(FontWeight::MEDIUM)
                .cursor_pointer()
                .border_b_2()
                .on_click(cx.listener(move |this, _, _, cx| this.set_tab(value, cx)))
                .child(label);
            if fresh && on != was_on {
                let (from_text, to_text) = if on {
                    (off_text, on_text)
                } else {
                    (on_text, off_text)
                };
                let (from_border, to_border) = if on {
                    (off_border, on_border)
                } else {
                    (on_border, off_border)
                };
                base.with_animation(
                    ElementId::NamedInteger(format!("drawer-tab-fade-{label}").into(), generation),
                    Animation::new(STATE_FADE).with_easing(EFFECTS.easing()),
                    move |tab, t| {
                        tab.text_color(mix(from_text, to_text, t))
                            .border_color(mix(from_border, to_border, t))
                    },
                )
                .into_any_element()
            } else if on {
                base.text_color(on_text)
                    .border_color(on_border)
                    .into_any_element()
            } else {
                base.text_color(off_text)
                    .border_color(off_border)
                    .into_any_element()
            }
        };
        h_flex()
            .flex_none()
            .gap(px(2.))
            .px(px(22.))
            .pt(px(14.))
            .border_b_1()
            .border_color(border)
            .child(tab("Summary", DrawerTab::Summary, cx))
            .child(tab("Result", DrawerTab::Result, cx))
            .child(tab("Logs", DrawerTab::Logs, cx))
    }

    fn render_body(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id("drawer-body")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px(px(22.))
            .pt(px(18.))
            .pb(px(28.))
            .child(super::run_detail_body::render_body(
                self.tab,
                self.detail.as_ref(),
                self.task_md.as_ref(),
                self.result_md.as_ref(),
                window,
                cx,
            ))
            .into_any_element()
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

        // Measure the containing panel each frame (prepaint, no notify) —
        // the slide animator reads last frame's width; first frame falls
        // back to the panel-relative offset.
        let measure = {
            let drawer = cx.weak_entity();
            canvas(
                move |bounds, _, cx| {
                    drawer
                        .update(cx, |this, _| this.container_width = Some(bounds.size.width))
                        .ok();
                },
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0()
        };

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

        let panel_width = self.container_width;
        let sheet = v_flex()
            .id("drawer-sheet")
            .absolute()
            .top_0()
            .bottom_0()
            .right_0()
            .w(relative(SHEET_FRACTION))
            .max_w(px(SHEET_MAX_WIDTH))
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
                    // Slide across the SHEET width (+2% clearing the 1px
                    // border), the web's translateX(102%): offscreen at 1.
                    let out = if closing { t } else { 1.0 - t };
                    let offset: gpui::Length = match panel_width {
                        Some(width) => px(slide_offset(width.as_f32(), out)).into(),
                        // Unmeasured first frame: panel-relative fallback.
                        None => relative(-SHEET_FRACTION * out).into(),
                    };
                    sheet.right(offset)
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
            .child(measure)
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
    fn slide_travel_is_sheet_width_terms_at_every_panel_width() {
        // Narrow panel (Z1 default 420px): sheet resolves to 92% = 386.4px,
        // travel = 1.02 × that — the geometry where the old panel-relative
        // math coincidentally agreed (within the 2% border clearance).
        assert!((slide_offset(420., 1.0) - (-394.128)).abs() < 1e-3);
        // Wide panel (1200px): sheet caps at 560px, travel = 571.2px —
        // NOT the old 0.92 × panel = 1104px over-travel (§8.7(b) dead zone).
        assert!((slide_offset(1200., 1.0) - (-571.2)).abs() < 1e-3);
        // Crossover panel width (560/0.92 ≈ 608.7): both formulas agree.
        assert!((slide_offset(608.7, 1.0) - (-571.2)).abs() < 0.1);
        // Settled (out = 0) is exactly in place.
        assert_eq!(slide_offset(1200., 0.0), 0.0);
        // Travel scales linearly with the animator's `out`.
        assert!((slide_offset(1200., 0.5) - (-285.6)).abs() < 1e-3);
    }

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
