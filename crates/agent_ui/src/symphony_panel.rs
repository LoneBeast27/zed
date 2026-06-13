//! The Symphony panel (PARITY_SPEC §4.3) — the plan/wave FSM score, ported
//! from the approved web reference render (`bridge/ui/symphony.js` +
//! `panels.css`): a persisted plan renders as wave bands (a `v_flex` of
//! bordered cards, top→bottom = execution order, grouped by `depends_on`
//! topological depth), each wave holding task cards (agent chip + LIVE status
//! pill + clamped title + reason); the first wave carries the active accent
//! stripe; empty state explains the panel.
//!
//! LIVE CHECK-OFF (the bridge now wires plan↔run linkage): the panel renders
//! from [`BridgeStore::plan`] — a `PlanSnapshot` off `GET /plan` / the SSE
//! `plan` event, where each subtask carries its linked run's rolled-up status
//! (pending/running/done/failed/killed). The cards check off as their runs
//! progress. The store feeds the plan over SSE when connected; a watch-gated
//! `/plan` poll (held only while the dock shows the panel —
//! [`Panel::set_active`], the [`PlanWatch`] lifetime law) covers the
//! polling-fallback window.
//!
//! The bridge OWNS the wave derivation (`bridge/state.py` `plan_view` →
//! `orchestrator/plans.py` `to_waves`) and is the single source of truth: the
//! panel renders the bands it is handed via [`PlanSnapshot::waves`] and never
//! re-derives topology natively. (A panel-local `to_waves`/`extract_plans`
//! port once lived here as a notional `/events`-shape fallback; it was never
//! wired into the render path — dead code masquerading as a live fallback,
//! the B1 drift trap — and was deleted in the fix pass.)

use gpui::{
    Action, Animation, AnimationExt as _, AnyElement, App, Context, Entity, EventEmitter,
    FocusHandle, Focusable, FontWeight, SharedString, Subscription, Window, actions,
};
use ui::prelude::*;
use workspace::dock::{DockPosition, Panel, PanelEvent};

use crate::agent_accents::ACCENT;
use crate::bridge::{self, BridgeStore, PlanSnapshot, PlanSubtask, PlanWatch};
use crate::task_board::motion::DECEL;
use crate::task_board::style::{HAIRLINE_HI, SURFACE_1, agent_chip, empty_state, plan_status_pill};

actions!(
    symphony_panel,
    [
        /// Toggles focus on the symphony panel.
        ToggleFocus
    ]
);

use std::time::Duration;

/// `.wave { animation: rise .3s var(--decel-curve) }`.
const WAVE_RISE: Duration = Duration::from_millis(300);
/// `.task-card { animation: spring-in .26s var(--decel-curve) }`.
const CARD_SPRING_IN: Duration = Duration::from_millis(260);

pub struct SymphonyPanel {
    focus_handle: FocusHandle,
    position: DockPosition,
    /// The shared bridge store — its [`BridgeStore::plan`] is the panel's
    /// truth source (fed by SSE / the watch-gated `/plan` poll). Observed for
    /// `cx.notify()` so check-off updates repaint.
    store: Entity<BridgeStore>,
    /// Held only while the dock shows the panel ([`Panel::set_active`]) — its
    /// presence keeps the `/plan` poll alive (the SSE-fallback path).
    plan_watch: Option<PlanWatch>,
    _store_subscription: Subscription,
}

impl SymphonyPanel {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let store = bridge::global_store(cx);
        let _store_subscription = cx.observe(&store, |_, _, cx| cx.notify());
        Self {
            focus_handle: cx.focus_handle(),
            position: DockPosition::Left,
            store,
            plan_watch: None,
            _store_subscription,
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
    /// bordered cards — RUST_PORT_NOTES §4.3). Waves are bridge-derived
    /// (`PlanSnapshot::waves`); each card checks off from its subtask's live
    /// status.
    fn render_score(&self, plan: &PlanSnapshot, cx: &App) -> AnyElement {
        let plan_ix = 0usize; // one live plan at a time off /plan
        let colors = cx.theme().colors();
        let summary = if plan.summary.is_empty() {
            "plan".to_string()
        } else {
            plan.summary.clone()
        };
        let waves: Vec<AnyElement> = plan
            .waves
            .iter()
            .enumerate()
            .map(|(wave_ix, wave)| self.render_wave(plan_ix, wave_ix, wave, cx))
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
        wave: &[PlanSubtask],
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

    /// One `.task-card`: agent chip + the LIVE status pill (the check-off:
    /// Queued → Running → Done/Blocked/Killed as the linked run progresses)
    /// over the clamped title (and the routing reason when present).
    fn render_card(
        &self,
        plan_ix: usize,
        wave_ix: usize,
        card_ix: usize,
        task: &PlanSubtask,
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
                    .child(plan_status_pill(
                        ElementId::Name(format!("{key}-pill").into()),
                        &task.status,
                        cx,
                    )),
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
        let plan = self.store.read(cx).plan.clone();
        let body: AnyElement = match plan.filter(PlanSnapshot::is_present) {
            None => empty_state(
                IconName::AudioOn,
                "No plans yet",
                "Plans from the judgment tier render here as waves — top-to-bottom \
                 execution order, cards checking off live as agents complete.",
                cx,
            )
            .into_any_element(),
            Some(plan) => v_flex()
                .id("symphony-scroll")
                .size_full()
                .overflow_y_scroll()
                .px(px(28.))
                .pt(px(20.))
                .pb(px(32.))
                .items_start()
                .child(self.render_score(&plan, cx))
                .into_any_element(),
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
        // The native route mount/unmount — the `/plan` poll (the SSE-fallback
        // path) lives exactly as long as the dock shows the panel
        // (PlanWatch / TranscriptWatch pattern). SSE feeds the plan directly
        // when connected; the watch covers the polling-fallback window.
        if active {
            if self.plan_watch.is_none() {
                self.plan_watch = Some(self.store.update(cx, |store, cx| store.watch_plan(cx)));
            }
        } else {
            self.plan_watch = None;
        }
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
