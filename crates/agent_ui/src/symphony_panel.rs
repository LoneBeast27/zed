//! The Symphony panel (PARITY_SPEC §4.3) — the plan/wave FSM score, ported
//! from the approved web reference render (`bridge/ui/symphony.js` +
//! `panels.css`): orchestrator `escalate_plan` results render as wave bands
//! (a `v_flex` of bordered cards, top→bottom = execution order, grouped by
//! `depends_on` topological depth), each wave holding task cards (agent
//! chip + status pill + clamped title + reason); the first wave carries the
//! active accent stripe; empty state explains the panel.
//!
//! Data is the web's truth-source feed: `GET /events?conv=…&since=0` polled
//! at the 2s cadence — and ONLY while the dock shows the panel (the task is
//! dropped on [`Panel::set_active`]`(false)`, the TranscriptWatch lifetime
//! law; `/events` rides neither SSE nor the BridgeStore today). The web's
//! cards are static "Queued" pills — the bridge wires no plan-task ↔ run
//! linkage yet, so live check-off has no data to ride; the §9 approval gate
//! says mirror the shipped render, so check-off stays banked bridge-side.

use std::time::Duration;

use gpui::{
    Action, Animation, AnimationExt as _, AnyElement, App, Context, Entity, EventEmitter,
    FocusHandle, Focusable, FontWeight, SharedString, Task, Window, actions,
};
use serde::Deserialize;
use ui::prelude::*;
use workspace::dock::{DockPosition, Panel, PanelEvent};

use crate::agent_accents::ACCENT;
use crate::bridge::{self, BRIDGE_BASE_URL, BridgeStore, fetch_json};
use crate::task_board::motion::DECEL;
use crate::task_board::style::{HAIRLINE_HI, SURFACE_1, agent_chip, empty_state, queued_pill};

actions!(
    symphony_panel,
    [
        /// Toggles focus on the symphony panel.
        ToggleFocus
    ]
);

/// `/events` cadence (symphony.js `setInterval(loop, 2000)`).
const EVENTS_POLL_INTERVAL: Duration = Duration::from_millis(2000);
/// `.wave { animation: rise .3s var(--decel-curve) }`.
const WAVE_RISE: Duration = Duration::from_millis(300);
/// `.task-card { animation: spring-in .26s var(--decel-curve) }`.
const CARD_SPRING_IN: Duration = Duration::from_millis(260);

/// One task off an `escalate_plan` result (`judgment.py` plan shape).
/// Liberal like the bridge protocol — every field defaults.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct PlanTask {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub task: String,
    #[serde(default)]
    pub agent: String,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub depends_on: Vec<String>,
}

/// One rendered plan (`extractPlans`): the judgment summary + its tasks.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Plan {
    pub summary: String,
    pub tasks: Vec<PlanTask>,
}

/// `extractPlans(events)` — `kind == "tool" && name == "escalate_plan"`
/// events whose result carries a plan ARRAY (error results have no `plan`
/// key and are skipped, exactly like the web filter).
pub fn extract_plans(events: &[serde_json::Value]) -> Vec<Plan> {
    events
        .iter()
        .filter_map(|event| {
            let object = event.as_object()?;
            if object.get("kind").and_then(|v| v.as_str()) != Some("tool")
                || object.get("name").and_then(|v| v.as_str()) != Some("escalate_plan")
            {
                return None;
            }
            let result = object.get("result")?.as_object()?;
            let plan = result.get("plan")?.as_array()?;
            Some(Plan {
                summary: result
                    .get("summary")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                tasks: plan
                    .iter()
                    .map(|task| serde_json::from_value(task.clone()).unwrap_or_default())
                    .collect(),
            })
        })
        .collect()
}

/// `toWaves(tasks)` — group tasks into bands by dependency depth
/// (topological band; unknown dep ids don't count, and the JS `seen`-set
/// cycle guard ports 1:1 so a dependency cycle settles instead of
/// recursing forever). Insertion order is kept within a wave.
pub fn to_waves(tasks: &[PlanTask]) -> Vec<Vec<PlanTask>> {
    use std::collections::{BTreeMap, HashMap, HashSet};
    let by_id: HashMap<&str, &PlanTask> = tasks.iter().map(|t| (t.id.as_str(), t)).collect();
    fn depth(
        task: &PlanTask,
        by_id: &HashMap<&str, &PlanTask>,
        seen: &HashSet<String>,
    ) -> usize {
        let deps: Vec<&PlanTask> = task
            .depends_on
            .iter()
            .filter(|dep| !seen.contains(*dep))
            .filter_map(|dep| by_id.get(dep.as_str()).copied())
            .collect();
        if deps.is_empty() {
            return 0;
        }
        let mut seen = seen.clone();
        seen.insert(task.id.clone());
        1 + deps
            .iter()
            .map(|dep| depth(dep, by_id, &seen))
            .max()
            .unwrap_or(0)
    }
    let mut waves: BTreeMap<usize, Vec<PlanTask>> = BTreeMap::new();
    for task in tasks {
        waves
            .entry(depth(task, &by_id, &HashSet::new()))
            .or_default()
            .push(task.clone());
    }
    waves.into_values().collect()
}

/// `GET /events` reply.
#[derive(Default, Deserialize)]
struct EventsResponse {
    #[serde(default)]
    events: Vec<serde_json::Value>,
}

pub struct SymphonyPanel {
    focus_handle: FocusHandle,
    position: DockPosition,
    /// Read for the followed conversation (`state.conv`) each poll cycle.
    store: Entity<BridgeStore>,
    plans: Vec<Plan>,
    /// The `/events` poll — `Some` only while the dock shows the panel
    /// ([`Panel::set_active`]); dropping it cancels the loop mid-sleep.
    poll: Option<Task<()>>,
}

impl SymphonyPanel {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            position: DockPosition::Left,
            store: bridge::global_store(cx),
            plans: Vec::new(),
            poll: None,
        }
    }

    /// The dock's visibility signal gates the poll (web `mountSymphony`
    /// starts the interval / `unmountSymphony` clears it). The loop's first
    /// iteration fetches immediately, so re-activation is the fresh fetch.
    fn set_poll_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if !active {
            self.poll = None;
            return;
        }
        if self.poll.is_some() {
            return;
        }
        let http_client = cx.http_client();
        self.poll = Some(cx.spawn(async move |this, cx| {
            loop {
                let Ok(conv) = this.read_with(cx, |this, cx| {
                    this.store
                        .read(cx)
                        .transcript_conv
                        .clone()
                        .unwrap_or_default()
                }) else {
                    return; // panel dropped
                };
                let client = http_client.clone();
                let fetched = cx
                    .background_spawn(async move {
                        let url = format!("{BRIDGE_BASE_URL}/events?conv={conv}&since=0");
                        let raw = fetch_json(client.as_ref(), &url).await?;
                        let response: EventsResponse = serde_json::from_str(&raw)?;
                        anyhow::Ok(extract_plans(&response.events))
                    })
                    .await;
                // Fetch failure = the web's silent `catch (e) { /* idle */ }`.
                if let Ok(plans) = fetched
                    && this
                        .update(cx, |this, cx| this.apply_plans(plans, cx))
                        .is_err()
                {
                    return;
                }
                cx.background_executor().timer(EVENTS_POLL_INTERVAL).await;
            }
        }));
    }

    /// Change-gated apply (the web's `lastSig` task-id signature, made
    /// strict: any plan/task field change re-renders).
    fn apply_plans(&mut self, plans: Vec<Plan>, cx: &mut Context<Self>) {
        if self.plans != plans {
            self.plans = plans;
            cx.notify();
        }
    }

    /// `.panel-head`: "Symphony" 18px/500 + the sub-line.
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
                    .child("Symphony"),
            )
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(colors.text_placeholder)
                    .child("plan waves from the judgment tier"),
            )
    }

    /// One `.score`: summary line over the wave bands (a `v_flex` of
    /// bordered cards — RUST_PORT_NOTES §4.3).
    fn render_score(&self, plan_ix: usize, plan: &Plan, cx: &App) -> AnyElement {
        let colors = cx.theme().colors();
        let summary = if plan.summary.is_empty() {
            "plan".to_string()
        } else {
            plan.summary.clone()
        };
        let waves: Vec<AnyElement> = to_waves(&plan.tasks)
            .into_iter()
            .enumerate()
            .map(|(wave_ix, wave)| self.render_wave(plan_ix, wave_ix, &wave, cx))
            .collect();
        v_flex()
            .w_full()
            .max_w(px(920.))
            .mb(px(28.))
            .child(
                div()
                    .mb(px(14.))
                    .text_size(px(14.))
                    .text_color(colors.text_muted)
                    .child(SharedString::from(summary)),
            )
            .children(waves)
            .into_any_element()
    }

    /// One `.wave` band: label + the card grid; the FIRST wave is `.active`
    /// (hairline-hi border + the 2px accent stripe inset 12px).
    fn render_wave(
        &self,
        plan_ix: usize,
        wave_ix: usize,
        wave: &[PlanTask],
        cx: &App,
    ) -> AnyElement {
        let colors = cx.theme().colors();
        let active = wave_ix == 0;
        let cards: Vec<AnyElement> = wave
            .iter()
            .enumerate()
            .map(|(card_ix, task)| self.render_card(plan_ix, wave_ix, card_ix, task, cx))
            .collect();
        let band = div()
            .relative()
            .w_full()
            .rounded(px(12.))
            .border_1()
            .border_color(if active {
                HAIRLINE_HI.into()
            } else {
                colors.border
            })
            .px(px(14.))
            .py(px(12.))
            .mb(px(12.))
            .when(active, |this| {
                // `.wave.active::before` — the accent stripe.
                this.child(
                    div()
                        .absolute()
                        .left_0()
                        .top(px(12.))
                        .bottom(px(12.))
                        .w(px(2.))
                        .rounded(px(2.))
                        .bg(gpui::Hsla::from(ACCENT)),
                )
            })
            .child(
                div()
                    .mb(px(10.))
                    .text_size(px(12.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(colors.text_placeholder)
                    .child(SharedString::from(format!("Wave {}", wave_ix + 1))),
            )
            .child(
                // `.wave-cards`: auto-fill minmax(240px, 1fr) emulated as
                // wrap + grow from a 240px basis (the usage pool-grid idiom).
                h_flex().flex_wrap().gap(px(10.)).children(cards),
            );
        // `rise .3s var(--decel-curve)` — one-shot per wave identity.
        band.with_animation(
            ElementId::Name(format!("wave-{plan_ix}-{wave_ix}").into()),
            Animation::new(WAVE_RISE).with_easing(DECEL.easing()),
            |band, t| band.opacity(t).mt(px(6. * (1. - t))).mb(px(12. - 6. * (1. - t))),
        )
        .into_any_element()
    }

    /// One `.task-card`: agent chip + the "Queued" pill over the clamped
    /// title (and the routing reason when present).
    fn render_card(
        &self,
        plan_ix: usize,
        wave_ix: usize,
        card_ix: usize,
        task: &PlanTask,
        cx: &App,
    ) -> AnyElement {
        let colors = cx.theme().colors();
        // `t.task.slice(0, 120)` then the 3-line clamp.
        let title: String = task.task.chars().take(120).collect();
        let key = format!("task-card-{plan_ix}-{wave_ix}-{card_ix}");
        let card = v_flex()
            .flex_grow()
            .flex_basis(px(240.))
            .rounded(px(12.))
            .bg(SURFACE_1)
            .border_1()
            .border_color(colors.border)
            .px(px(13.))
            .py(px(12.))
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.))
                    .mb(px(9.))
                    .child(agent_chip(&task.agent, cx))
                    .child(queued_pill(ElementId::Name(format!("{key}-pill").into()), cx)),
            )
            .child(
                div()
                    .text_size(px(13.))
                    .line_height(relative(1.5))
                    .text_color(colors.text)
                    .line_clamp(3)
                    .child(SharedString::from(title)),
            )
            .children(task.reason.clone().filter(|r| !r.is_empty()).map(|reason| {
                div()
                    .mt(px(7.))
                    .text_size(px(12.))
                    .text_color(colors.text_placeholder)
                    .child(SharedString::from(reason))
            }));
        // `spring-in .26s var(--decel-curve)` — one-shot per card identity.
        card.with_animation(
            ElementId::Name(format!("{key}-in").into()),
            Animation::new(CARD_SPRING_IN).with_easing(DECEL.easing()),
            |card, t| card.opacity(t).mt(px(4. * (1. - t))),
        )
        .into_any_element()
    }
}

impl Render for SymphonyPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body: AnyElement = if self.plans.is_empty() {
            empty_state(
                IconName::AudioOn,
                "No plans yet",
                "Plans from the judgment tier render here as waves — top-to-bottom \
                 execution order, cards checking off live as agents complete.",
                cx,
            )
            .into_any_element()
        } else {
            let plans = self.plans.clone();
            let scores: Vec<AnyElement> = plans
                .iter()
                .enumerate()
                .map(|(plan_ix, plan)| self.render_score(plan_ix, plan, cx))
                .collect();
            v_flex()
                .id("symphony-scroll")
                .size_full()
                .overflow_y_scroll()
                .px(px(28.))
                .pt(px(20.))
                .pb(px(32.))
                .items_start()
                .children(scores)
                .into_any_element()
        };
        let colors = cx.theme().colors();
        v_flex()
            .key_context("SymphonyPanel")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(colors.panel_background)
            .child(self.render_header(cx))
            .child(div().flex_1().min_h_0().child(body))
    }
}

impl Focusable for SymphonyPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<PanelEvent> for SymphonyPanel {}

impl Panel for SymphonyPanel {
    fn persistent_name() -> &'static str {
        "SymphonyPanel"
    }

    fn panel_key() -> &'static str {
        "SymphonyPanel"
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
        // The native route mount/unmount — `/events` polls only while the
        // dock shows the panel.
        self.set_poll_active(active, cx);
    }

    fn default_size(&self, _window: &Window, _cx: &App) -> Pixels {
        px(560.)
    }

    fn icon(&self, _window: &Window, _cx: &App) -> Option<IconName> {
        Some(IconName::AudioOn)
    }

    fn icon_tooltip(&self, _window: &Window, _cx: &App) -> Option<&'static str> {
        Some("Symphony")
    }

    fn toggle_action(&self) -> Box<dyn Action> {
        Box::new(ToggleFocus)
    }

    fn activation_priority(&self) -> u32 {
        8
    }
}

#[cfg(test)]
#[path = "symphony_panel_tests.rs"]
mod tests;
