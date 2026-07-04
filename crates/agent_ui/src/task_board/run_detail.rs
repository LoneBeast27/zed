//! The run drawer (PARITY_SPEC §4.2, Codex anatomy / web `drawer.js`):
//! right slide-over with head (vendor swatch · TASK TITLE · status pill ·
//! close), worked-for strip, flat text tabs Summary | Result | Logs. Two
//! feeds: [`RunDrawer::new`] fetches `GET /run/<id>` (tail-polled 1s while
//! running; a first fetch that 404s/errors resolves to an honest error state
//! after a BOUNDED wait — never eternal "Loading…"), and [`RunDrawer::local`]
//! is pushed from a local store (the constellation demo) with zero bridge
//! I/O. Esc or scrim-click closes; entrance/exit slide on the decel curve
//! with a concurrent scrim crossfade (§4.9 spatial/effects split).

use std::sync::Arc;
use std::time::Duration;

use gpui::{
    Animation, AnimationExt as _, AnyElement, App, Context, Entity, EventEmitter, FocusHandle,
    Focusable, KeyDownEvent, Pixels, SharedString, Task, Window, canvas, relative,
};
use http_client::HttpClient;
use markdown::Markdown;
use serde::Deserialize;
use ui::prelude::*;

use crate::bridge::{BRIDGE_BASE_URL, fetch_json, post_json};

use super::motion::{DECEL, EFFECTS, StateFade};
use super::style::HAIRLINE_HI;

/// Logs tail-poll cadence while the run is live (web: 1s).
const LOG_POLL: Duration = Duration::from_secs(1);
/// Tail-poll backoff cap while the bridge is erroring — the poll never stops
/// on a transient fetch error (web drawer.js skips the tick and the interval
/// retries); it just slows down until the bridge answers again.
const LOG_POLL_ERROR_CAP: Duration = Duration::from_secs(5);
/// How many consecutive first-fetch failures before the drawer resolves to
/// an honest error state instead of "Loading…" (bounded: with the 1s-per-
/// failure backoff this is ~6s worst case; a 404 fails fast on the first
/// response). Applies only while NO detail has ever loaded — once data is
/// on screen, transient bridge errors keep the last detail and keep tailing.
const FIRST_FETCH_ATTEMPTS: u32 = 3;
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
    /// TEAMS §6 role ("designer"/"tester"/…) — the title's fallback when the
    /// run has no task title (never the literal "agent run").
    #[serde(default)]
    pub archetype: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub usage: Option<serde_json::Map<String, serde_json::Value>>,
    /// The worker's agent-session id (`GET /run/<id>` `session_id`, 2026-07-04)
    /// — the drawer head surfaces it so a run is traceable back to its worker
    /// session. `None` on an older bridge / a run without a session.
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub events: Vec<RunEvent>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct RunEvent {
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub payload: serde_json::Value,
}

/// Web log row truncation (`JSON.stringify(ev.payload).slice(0, 400)`).
const LOG_PAYLOAD_CAP: usize = 400;

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
    /// The id this drawer was opened for — the honest header while nothing
    /// has loaded (never a placeholder "agent run").
    pub(super) run_id: SharedString,
    pub(super) detail: Option<RunDetail>,
    /// Set when the bounded first fetch gave up (404 / unreachable bridge)
    /// with no detail ever loaded — the body renders this instead of an
    /// eternal "Loading…".
    pub(super) load_failed: Option<SharedString>,
    /// Locally-fed drawer ([`Self::local`]): no bridge behind it, so the
    /// abort affordance (a bridge POST) is meaningless and hidden.
    pub(super) local: bool,
    /// Embedded placement (PARITY_SPEC amendment 2026-07-04): rendered as a
    /// section BELOW the constellation graph on the SAME surface — no scrim,
    /// no absolute sheet, no slide, no border seam. Internal anatomy (head,
    /// worked strip, tabs, body) is identical to the slide-over.
    embedded: bool,
    pub(super) tab: DrawerTab,
    /// The previously active tab — the 150ms tab crossfade eases both the
    /// leaving and the arriving tab (web `.drawer-tabs button` transition).
    pub(super) prev_tab: DrawerTab,
    pub(super) tab_fade: StateFade,
    /// Close-button tracked hover (web `#drawer-close` 150ms transition).
    pub(super) close_hovered: bool,
    pub(super) close_fade: StateFade,
    /// Containing-panel width, measured each frame by a layout canvas — the
    /// slide animator converts it to the resolved sheet width so travel
    /// matches the web's `translateX(102%)` at every panel width.
    container_width: Option<Pixels>,
    /// Markdown entities for the summary task + result body, rebuilt when
    /// the underlying text changes.
    task_md: Option<Entity<Markdown>>,
    result_md: Option<Entity<Markdown>>,
    /// Pre-stringified Logs rows (kind, payload≤400 chars) — serialized once
    /// per poll tick in [`Self::set_detail`], never in render (the drawer
    /// redraws every frame while the pill pulse runs).
    log_rows: Vec<(SharedString, SharedString)>,
    focus_handle: FocusHandle,
    needs_focus: bool,
    closing: bool,
    /// True once an abort has been POSTed — the head's abort button reflects
    /// it (disabled "Aborting…") until the poll lands the killed status.
    pub(super) aborting: bool,
    _poll: Task<()>,
    _abort: Option<Task<()>>,
}

impl RunDrawer {
    pub fn new(run_id: SharedString, cx: &mut Context<Self>) -> Self {
        let http_client: Arc<dyn HttpClient> = cx.http_client();
        let poll_run_id = run_id.clone();
        let poll = cx.spawn(async move |this, cx| {
            let run_id = poll_run_id;
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
                    // Fetch error. If detail is already on screen: keep it
                    // and keep tailing (web drawer.js catches, skips the
                    // tick, retries). If NOTHING ever loaded: the wait is
                    // BOUNDED — a 404 resolves to "not found" immediately,
                    // anything else after FIRST_FETCH_ATTEMPTS — never an
                    // eternal "Loading…".
                    Err(error) => {
                        failures += 1;
                        let not_found = error.to_string().contains("returned 404");
                        let state = this.update(cx, |this, cx| {
                            if this.detail.is_none()
                                && (not_found || failures >= FIRST_FETCH_ATTEMPTS)
                            {
                                this.load_failed = Some(if not_found {
                                    "Run not found on bridge.".into()
                                } else {
                                    "Bridge unreachable — run detail unavailable.".into()
                                });
                                cx.notify();
                                return None;
                            }
                            Some(
                                this.detail
                                    .as_ref()
                                    .is_none_or(|detail| detail.status == "running"),
                            )
                        });
                        match state {
                            Ok(Some(running)) => running,
                            Ok(None) => return, // resolved to the error state
                            Err(_) => return,   // drawer dropped
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
        Self::empty(run_id, poll, false, cx)
    }

    /// A drawer fed from a LOCAL store (the constellation's staged demo —
    /// synthetic runs that don't exist on the bridge, so a fetch would 404
    /// into an error state): zero bridge I/O, detail arrives up front and
    /// the owner pushes refreshes via [`Self::push_local_detail`].
    pub fn local(detail: RunDetail, cx: &mut Context<Self>) -> Self {
        let run_id = SharedString::from(detail.run_id.clone());
        let mut this = Self::empty(run_id, Task::ready(()), true, cx);
        this.set_detail(detail, cx);
        this
    }

    /// Refresh a [`Self::local`] drawer's detail — called by the owner off
    /// its EXISTING frame pump at ~1s granularity (no timer of its own; the
    /// elapsed tick rides the owner's animation clock).
    pub fn push_local_detail(&mut self, detail: RunDetail, cx: &mut Context<Self>) {
        self.set_detail(detail, cx);
    }

    fn empty(run_id: SharedString, poll: Task<()>, local: bool, cx: &mut Context<Self>) -> Self {
        Self {
            run_id,
            detail: None,
            load_failed: None,
            local,
            embedded: false,
            tab: DrawerTab::Summary,
            prev_tab: DrawerTab::Summary,
            tab_fade: StateFade::default(),
            close_hovered: false,
            close_fade: StateFade::default(),
            container_width: None,
            task_md: None,
            result_md: None,
            log_rows: Vec::new(),
            focus_handle: cx.focus_handle(),
            needs_focus: true,
            closing: false,
            aborting: false,
            _poll: poll,
            _abort: None,
        }
    }

    /// Abort this run (the drawer abort affordance): POST `/run/<id>/abort` on
    /// the background executor. The bridge terminates the run's tracked child
    /// process and marks it killed; the drawer's own tail-poll then lands the
    /// killed status (no local detail flip needed). No-op unless running.
    ///
    /// On a POST FAILURE (P2: bridge 500, 404 from a since-reaped run, refused
    /// connection) the button must NOT stick on the disabled "Aborting…"
    /// placeholder forever — the tail-poll only clears `aborting` once the run
    /// reaches a non-running status, which never happens if the abort didn't
    /// land. So a failed POST resets `aborting` and notifies, re-arming the
    /// affordance for a retry.
    pub(super) fn abort(&mut self, cx: &mut Context<Self>) {
        let Some(detail) = self.detail.as_ref() else {
            return;
        };
        if self.local || self.aborting || detail.status != "running" || detail.run_id.is_empty() {
            return;
        }
        self.aborting = true;
        let run_id = detail.run_id.clone();
        let http_client: Arc<dyn HttpClient> = cx.http_client();
        self._abort = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let url = format!("{BRIDGE_BASE_URL}/run/{run_id}/abort");
                    post_json(http_client.as_ref(), &url, "{}".to_string()).await
                })
                .await;
            if result.is_err() {
                // Abort didn't land — re-arm the button so the user can retry
                // (the poll would otherwise never clear `aborting`).
                this.update(cx, |this, cx| {
                    this.aborting = false;
                    cx.notify();
                })
                .ok();
            }
        }));
        cx.notify();
    }

    fn set_detail(&mut self, detail: RunDetail, cx: &mut Context<Self>) {
        let task_changed =
            self.detail.as_ref().map(|d| d.task.clone()) != Some(detail.task.clone());
        let text_changed =
            self.detail.as_ref().map(|d| d.text.clone()) != Some(detail.text.clone());
        let events_changed = self.detail.as_ref().map(|d| &d.events) != Some(&detail.events);
        if events_changed {
            self.log_rows = detail
                .events
                .iter()
                .map(|event| {
                    let payload: String =
                        event.payload.to_string().chars().take(LOG_PAYLOAD_CAP).collect();
                    (
                        SharedString::from(event.kind.clone()),
                        SharedString::from(payload),
                    )
                })
                .collect();
        }
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

    /// Mark this drawer as embedded below the constellation graph
    /// (PARITY_SPEC amendment 2026-07-04 — unified surface).
    pub fn embed(&mut self) {
        self.embedded = true;
    }

    /// Begin the close: run the exit slide, then tell the panel to drop us.
    /// Embedded placement has no slide — the panel drops us immediately.
    pub fn dismiss(&mut self, cx: &mut Context<Self>) {
        if self.closing {
            return;
        }
        self.closing = true;
        cx.notify();
        if self.embedded {
            cx.emit(DismissDrawer);
            return;
        }
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SLIDE).await;
            this.update(cx, |_, cx| cx.emit(DismissDrawer)).ok();
        })
        .detach();
    }

    pub(super) fn set_tab(&mut self, tab: DrawerTab, cx: &mut Context<Self>) {
        if self.tab != tab {
            self.prev_tab = self.tab;
            self.tab = tab;
            self.tab_fade.bump();
            cx.notify();
        }
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
                self.load_failed.as_ref(),
                self.task_md.as_ref(),
                self.result_md.as_ref(),
                &self.log_rows,
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

        // Embedded placement (PARITY_SPEC amendment 2026-07-04): a section
        // below the constellation graph on the SAME surface — the parent's
        // background shows through (no fill, no scrim, no slide, no border
        // seam). Identical internal anatomy.
        if self.embedded {
            return v_flex()
                .key_context("RunDrawer")
                .track_focus(&self.focus_handle)
                .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                    if event.keystroke.key == "escape" {
                        this.dismiss(cx);
                    }
                }))
                .size_full()
                .min_h_0()
                .child(self.render_head(cx))
                .children(self.render_worked(cx))
                .child(self.render_tabs(cx))
                .child(self.render_body(window, cx))
                .into_any_element();
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
            .into_any_element()
    }
}

impl Focusable for RunDrawer {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<DismissDrawer> for RunDrawer {}

#[cfg(test)]
#[path = "run_detail_tests.rs"]
mod tests;
