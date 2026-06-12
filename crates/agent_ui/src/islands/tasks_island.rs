//! The composer-anchored running-tasks island (PARITY_SPEC §4.9 anchor
//! catalog: "Composer-anchored — emerges above the typing pill";
//! `tasks-island.{js,css}` 1:1). Docked IN NORMAL FLOW directly above the
//! orchestrator composer — growing it displaces the composer's
//! neighborhood (the reflow IS the choreography, Caelestia sibling rule) —
//! never an overlay.
//!
//! Lifecycle: hidden at zero running runs; 0→N EMERGES from the composer
//! anchor (translateY-from-anchor on decel-in 500ms + concurrent opacity on
//! effects 200ms — the §4.9 split); N→0 retracts onto the sharper exit
//! spline (400ms) and then leaves the flow. Collapsed head (spinner ring +
//! "N subagents/tasks running" + chevron) ⇄ expanded spinner rows on the
//! 320ms spatial max-height morph; a row click routes to the task board and
//! opens that run's drawer. The open state persists across sessions via
//! `db::kvp` (the web's localStorage `ti-open`).
//!
//! Data rides the panel's existing store observer (SSE board push) — no
//! poll, no timer beyond the retract clock (the web polls `/board` at 2.5s;
//! natively the push feed already carries the running set).

use std::time::Duration;

use db::kvp::KeyValueStore;
use gpui::{AnyElement, App, SharedString, Task};
use ui::CommonAnimationExt as _;
use ui::prelude::*;

use crate::agent_accents::STATUS_RUNNING;
use crate::bridge::RunRow;
use crate::orchestrator_panel::OrchestratorPanel;
use crate::task_board::motion::{AnimatedValue, DECEL, EFFECTS, MotionCurve, SPATIAL, StateFades, mix};
use crate::task_board::style::{HAIRLINE_HI, tabular_nums};

use super::usage_island::SURFACE_FLOAT;

/// Emergence slide from the composer anchor (decel-in entrance class).
const EMERGE: Duration = Duration::from_millis(500);
/// Concurrent opacity crossfade (effects class).
const FADE: Duration = Duration::from_millis(200);
/// Retract envelope: the sharper asymmetric exit, then leave the flow.
const RETRACT: Duration = Duration::from_millis(400);
/// Rows expand/collapse (`.ti-rows { transition: max-height .32s
/// var(--spatial-curve) }`).
const ROWS_MORPH: Duration = Duration::from_millis(320);
/// Chevron rotate (`.ti-chev { transition: transform .26s }`).
const CHEV_MORPH: Duration = Duration::from_millis(260);
/// Tuck distance toward the anchor below (`transform: translateY(10px)`).
const TUCK: f32 = 10.;
/// Gap to the composer beneath (`margin: 0 0 8px`).
const ANCHOR_GAP: f32 = 8.;
/// Expanded rows viewport cap (`.tasks-island.open .ti-rows { max-height:
/// 176px }`).
const ROWS_MAX_H: f32 = 176.;

/// The persisted open-state key (the web's localStorage `ti-open`).
const OPEN_KEY: &str = "orchestrator_tasks_island_open";

/// Island chrome hover crossfade (`.ti-head`/`.ti-row { transition: … .12s
/// var(--effects-curve) }` — the island runs 120ms where chat chrome runs
/// 150ms).
const ISLAND_FADE: Duration = Duration::from_millis(120);

/// The island's view state — owned by the orchestrator panel (TranscriptView
/// pattern); this module owns its lifecycle and what it looks like.
pub struct TasksIsland {
    /// Collapsed head ⇄ expanded rows (persisted).
    open: bool,
    /// Whether the island occupies flow (the web's `hidden` attribute).
    visible: bool,
    /// §4.9 emergence scalar: 1 = tucked into the composer anchor, 0 = out.
    /// One retargetable value for both directions (DECEL/500 in,
    /// exit-spline/400 out) — a mid-emerge retract continues from the
    /// current offset (§8.7a).
    emerge: AnimatedValue,
    /// Concurrent opacity (EFFECTS/200 both directions).
    fade: AnimatedValue,
    /// Rows reveal 0..1 → max-height 0..176 (SPATIAL/320).
    rows_reveal: AnimatedValue,
    /// Chevron flip 0 (down) ⇄ 1 (up) — a stacked-glyph crossfade standing
    /// in for the web's 180° rotate (`ui::Transformable` is crate-private;
    /// rotating would mean patching stock `ui`).
    chev: AnimatedValue,
    /// Mid-retract (the N→0 exit is running; the LAST snapshot stays
    /// rendered until [`Self::commit_hide`]). State twin of `hide_task` —
    /// kept separate so the lifecycle is testable without a panel entity.
    retracting: bool,
    /// The pending leave-flow clock while retracting.
    hide_task: Option<Task<()>>,
    /// (run id, row label) for the running set, in board order. While
    /// retracting this intentionally holds the LAST non-empty set — the
    /// web's `paint()` early-returns on zero rows without touching the DOM,
    /// so the head count + spinner rows stay frozen through the 400ms exit
    /// (§4.9 retract-the-last-representation; never a "0 running" flash).
    rows: Vec<(SharedString, SharedString)>,
}

/// What a board ingest decided (the pure half of [`TasksIsland::sync`]).
struct IngestOutcome {
    /// Repaint needed.
    changed: bool,
    /// The N→0 transition just began — the caller must arm the 400ms
    /// leave-flow clock.
    start_retract: bool,
}

impl TasksIsland {
    pub fn new(cx: &App) -> Self {
        let open = KeyValueStore::global(cx)
            .read_kvp(OPEN_KEY)
            .ok()
            .flatten()
            .is_some_and(|value| value == "1");
        Self {
            open,
            visible: false,
            emerge: AnimatedValue::settled(1., DECEL, EMERGE),
            fade: AnimatedValue::settled(0., EFFECTS, FADE),
            rows_reveal: AnimatedValue::settled(if open { 1. } else { 0. }, SPATIAL, ROWS_MORPH),
            chev: AnimatedValue::settled(if open { 1. } else { 0. }, SPATIAL, CHEV_MORPH),
            retracting: false,
            hide_task: None,
            rows: Vec::new(),
        }
    }

    pub fn visible(&self) -> bool {
        self.visible
    }

    /// Whether any §4.9 motion value is mid-flight — the panel's frame-pump
    /// signal.
    pub fn any_animating(&self) -> bool {
        self.emerge.animating()
            || self.fade.animating()
            || self.rows_reveal.animating()
            || self.chev.animating()
    }

    /// Ingest the latest running rows (derived via [`running_rows`] under
    /// the caller's store read borrow) and drive the emerge/retract
    /// lifecycle. Returns whether a repaint is needed.
    pub fn sync(
        &mut self,
        rows: Vec<(SharedString, SharedString)>,
        cx: &mut gpui::Context<OrchestratorPanel>,
    ) -> bool {
        let outcome = self.ingest(rows);
        if outcome.start_retract {
            // The leave-flow clock. Dropping the task (a mid-retract
            // arrival in `ingest`) cancels it before the commit runs.
            self.hide_task = Some(cx.spawn(async move |panel, cx| {
                cx.background_executor().timer(RETRACT).await;
                panel
                    .update(cx, |panel, cx| {
                        if panel.tasks_island.commit_hide() {
                            cx.notify();
                        }
                    })
                    .ok();
            }));
        } else if !self.retracting && self.hide_task.is_some() {
            self.hide_task = None; // cancelled by a mid-retract arrival
        }
        outcome.changed
    }

    /// The pure lifecycle step (split from [`Self::sync`] so the retract
    /// choreography is testable without a panel entity).
    fn ingest(&mut self, rows: Vec<(SharedString, SharedString)>) -> IngestOutcome {
        if !rows.is_empty() {
            // N runs → emerge from the anchor. A mid-retract arrival cancels
            // the hide and re-emerges from the current offset.
            let mut changed = rows != self.rows;
            if changed {
                self.rows = rows;
            }
            if self.retracting {
                self.retracting = false;
                changed = true;
            }
            if !self.visible {
                self.visible = true;
                // Pre-emergence pose: tucked toward the composer, transparent.
                self.emerge.jump(1.);
                self.fade.jump(0.);
                changed = true;
            }
            self.emerge.retarget_with(0., DECEL, EMERGE);
            self.fade.retarget(1.);
            IngestOutcome { changed, start_retract: false }
        } else if self.visible && !self.retracting {
            // 0 runs → retract into the anchor (sharper exit), then leave
            // the flow. `self.rows` is deliberately NOT cleared here — the
            // last representation stays frozen through the 400ms exit; the
            // clock's `commit_hide` drops it.
            self.retracting = true;
            self.emerge.retarget_with(1., MotionCurve::Exit, RETRACT);
            self.fade.retarget(0.);
            IngestOutcome { changed: true, start_retract: true }
        } else {
            IngestOutcome { changed: false, start_retract: false }
        }
    }

    /// The retract clock landed: leave the flow and drop the frozen
    /// snapshot (cleared only NOW, never at retract start). Returns false
    /// when a mid-retract arrival already cancelled the hide.
    fn commit_hide(&mut self) -> bool {
        if !self.retracting {
            return false;
        }
        self.retracting = false;
        self.visible = false;
        self.hide_task = None;
        self.rows.clear();
        true
    }

    /// Head click: flip and persist (the web's localStorage write).
    pub fn toggle_open(&mut self, cx: &mut gpui::Context<OrchestratorPanel>) {
        self.open = !self.open;
        self.rows_reveal
            .retarget_with(if self.open { 1. } else { 0. }, SPATIAL, ROWS_MORPH);
        self.chev
            .retarget_with(if self.open { 1. } else { 0. }, SPATIAL, CHEV_MORPH);
        let value = if self.open { "1" } else { "0" }.to_string();
        let kvp = KeyValueStore::global(cx);
        db::write_and_log(cx, move || async move {
            kvp.write_kvp(OPEN_KEY.to_string(), value).await
        });
        cx.notify();
    }
}

/// `running.map(r => r.task || r.agent || "task")` keyed by run id — the
/// island's row set, in board order (tasks-island.js `paint()`).
/// `pub(crate)`: the panel derives rows under its store read borrow and
/// hands them to [`TasksIsland::sync`] (no board clone per notify).
pub(crate) fn running_rows(board: &[RunRow]) -> Vec<(SharedString, SharedString)> {
    board
        .iter()
        .filter(|run| run.status == "running")
        .map(|run| {
            let label = run
                .task
                .clone()
                .filter(|task| !task.is_empty())
                .unwrap_or_else(|| {
                    if run.agent.is_empty() {
                        "task".to_string()
                    } else {
                        run.agent.clone()
                    }
                });
            (
                SharedString::from(run.run_id.clone()),
                SharedString::from(label),
            )
        })
        .collect()
}

/// The 13px open-circle spinner (web `.ti-spin`, approximated with the
/// in-tree rotating-icon idiom — gpui has no per-side border colors for a
/// true arc ring; 1s vs the web's 0.9s).
fn spinner(id: impl Into<ElementId>) -> AnyElement {
    Icon::new(IconName::ArrowCircle)
        .size(IconSize::XSmall)
        .color(Color::Custom(STATUS_RUNNING.into()))
        .with_keyed_rotate_animation(id, 1)
        .into_any_element()
}

/// The island element: head + rows inside the emergence wrapper. Rendered
/// by the orchestrator panel inside the composer column, above the deck.
///
/// Identity rule (§8.7a / Z3 fix #4): the element tree here is the SAME
/// SHAPE whether settled or mid-morph — every animated style reads its
/// Instant-clocked value's `current()` directly (settled values return
/// their target), and the frame pump is the panel's single
/// `request_animation_frame` (armed off [`TasksIsland::any_animating`]).
/// gpui keys element state (incl. each spinner's rotate-start Instant) by
/// the FULL ancestor-id path, so generation-keyed animation wrappers that
/// appear/disappear around the rows or the island would snap every spinner
/// phase to 0° on each toggle/settle — the web's infinite CSS animation on
/// persistent DOM nodes never resets.
pub fn render_tasks_island(
    island: &TasksIsland,
    fades: &StateFades,
    cx: &mut gpui::Context<OrchestratorPanel>,
) -> AnyElement {
    let colors = cx.theme().colors();
    let n = island.rows.len();
    let head_label = format!(
        "{n} subagent{}/tasks running",
        if n > 1 { "s" } else { "" }
    );

    // ── head: spinner + count + chevron (down⇄up stacked-glyph crossfade
    //    — the web rotates 180°; see the `chev` field note). The flip
    //    scalar is read per frame; no wrapper, no identity churn. ──
    let chevron = {
        let flip = island.chev.current().clamp(0., 1.);
        div()
            .relative()
            .size(px(18.))
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .opacity(1. - flip)
                    .child(
                        Icon::new(IconName::ChevronDown)
                            .size(IconSize::Custom(rems_from_px(18.)))
                            .color(Color::Placeholder),
                    ),
            )
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .opacity(flip)
                    .child(
                        Icon::new(IconName::ChevronUp)
                            .size(IconSize::Custom(rems_from_px(18.)))
                            .color(Color::Placeholder),
                    ),
            )
    };
    let head_id = ElementId::Name("ti-head".into());
    let head_t = fades.t(&head_id);
    let head = h_flex()
        .id(head_id.clone())
        .w_full()
        .items_center()
        .gap(px(9.))
        .px(px(12.))
        .py(px(9.))
        .cursor_pointer()
        .bg(colors.element_hover.opacity(head_t))
        .on_hover(cx.listener(move |panel, hovered: &bool, _, cx| {
            panel.set_fade(head_id.clone(), *hovered, ISLAND_FADE, cx);
        }))
        .on_click(cx.listener(|panel, _, _, cx| panel.tasks_island.toggle_open(cx)))
        .child(spinner("ti-head-spin"))
        .child(
            div()
                .text_size(px(13.))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(colors.text)
                .font_features(tabular_nums())
                .child(SharedString::from(head_label)),
        )
        .child(div().flex_1())
        .child(chevron);

    // ── rows: keyed by run id (spinners never restart on a board tick —
    //    the element id is stable across frames, §5.6) ──
    let rows = island.rows.iter().map(|(run_id, label)| {
        let open_id = run_id.clone();
        let row_id = ElementId::Name(format!("ti-row-{run_id}").into());
        let row_t = fades.t(&row_id);
        let hover_id = row_id.clone();
        h_flex()
            .id(row_id)
            .w_full()
            .items_center()
            .gap(px(9.))
            .px(px(12.))
            .py(px(7.))
            .border_t_1()
            .border_color(cx.theme().colors().border)
            .cursor_pointer()
            .text_size(px(12.5))
            .text_color(mix(
                cx.theme().colors().text_muted,
                cx.theme().colors().text,
                row_t,
            ))
            .bg(cx.theme().colors().element_hover.opacity(row_t))
            .on_hover(cx.listener(move |panel, hovered: &bool, _, cx| {
                panel.set_fade(hover_id.clone(), *hovered, ISLAND_FADE, cx);
            }))
            .on_click(cx.listener(move |panel, _, window, cx| {
                panel.open_run(open_id.clone(), window, cx);
            }))
            .child(spinner(ElementId::Name(format!("ti-spin-{run_id}").into())))
            .child(div().min_w_0().truncate().child(label.clone()))
    });
    // ONE stable id ("ti-rows") across the morph and the settled-open
    // state, so each row spinner's rotate phase survives the expand/
    // collapse morph and its settle. Rows still leave the tree entirely
    // when settled-closed (§8 idle cost — ledgered divergence #12).
    let rows_body: AnyElement = if island.rows_reveal.animating() {
        v_flex()
            .id("ti-rows")
            .overflow_hidden()
            .max_h(px((ROWS_MAX_H * island.rows_reveal.current()).max(0.)))
            .children(rows)
            .into_any_element()
    } else if island.open {
        v_flex()
            .id("ti-rows")
            .max_h(px(ROWS_MAX_H))
            .overflow_y_scroll()
            .children(rows)
            .into_any_element()
    } else {
        gpui::Empty.into_any_element()
    };

    // ── container + the §4.9 emergence pose, read per frame: translate
    //    toward/away from the composer anchor below while opacity
    //    crossfades concurrently (geometry + opacity from frame one) ──
    let offset = TUCK * island.emerge.current();
    div()
        .mt(px(offset))
        .mb(px(ANCHOR_GAP - offset))
        .opacity(island.fade.current().clamp(0., 1.))
        .child(
            v_flex()
                .w_full()
                .rounded(px(14.))
                .bg(SURFACE_FLOAT)
                .border_1()
                .border_color(HAIRLINE_HI)
                .overflow_hidden()
                .child(head)
                .child(rows_body),
        )
        .into_any_element()
}

#[cfg(test)]
#[path = "tasks_island_tests.rs"]
mod tests;
