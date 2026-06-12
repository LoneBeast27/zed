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
use gpui::{Animation, AnimationExt as _, AnyElement, App, SharedString, Task};
use ui::CommonAnimationExt as _;
use ui::prelude::*;

use crate::agent_accents::STATUS_RUNNING;
use crate::bridge::RunRow;
use crate::orchestrator_panel::OrchestratorPanel;
use crate::task_board::motion::{AnimatedValue, DECEL, EFFECTS, MotionCurve, SPATIAL};
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
    /// The pending leave-flow clock while retracting.
    hide_task: Option<Task<()>>,
    /// (run id, row label) for the running set, in board order.
    rows: Vec<(SharedString, SharedString)>,
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
            hide_task: None,
            rows: Vec::new(),
        }
    }

    pub fn visible(&self) -> bool {
        self.visible
    }

    /// Ingest the latest board: derive the running rows and drive the
    /// emerge/retract lifecycle. Returns whether a repaint is needed.
    pub fn sync(
        &mut self,
        board: &[RunRow],
        cx: &mut gpui::Context<OrchestratorPanel>,
    ) -> bool {
        let rows = running_rows(board);
        let mut changed = rows != self.rows;
        if changed {
            self.rows = rows;
        }

        if !self.rows.is_empty() {
            // N runs → emerge from the anchor. A mid-retract arrival cancels
            // the hide and re-emerges from the current offset.
            if self.hide_task.take().is_some() {
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
        } else if self.visible && self.hide_task.is_none() {
            // 0 runs → retract into the anchor (sharper exit), then hide.
            self.emerge.retarget_with(1., MotionCurve::Exit, RETRACT);
            self.fade.retarget(0.);
            self.hide_task = Some(cx.spawn(async move |panel, cx| {
                cx.background_executor().timer(RETRACT).await;
                panel
                    .update(cx, |panel, cx| {
                        let island = &mut panel.tasks_island;
                        if island.rows.is_empty() && island.hide_task.is_some() {
                            island.visible = false;
                            island.hide_task = None;
                            cx.notify();
                        }
                    })
                    .ok();
            }));
            changed = true;
        }
        changed
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
fn running_rows(board: &[RunRow]) -> Vec<(SharedString, SharedString)> {
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
pub fn render_tasks_island(
    island: &TasksIsland,
    cx: &mut gpui::Context<OrchestratorPanel>,
) -> AnyElement {
    let colors = cx.theme().colors();
    let n = island.rows.len();
    let head_label = format!(
        "{n} subagent{}/tasks running",
        if n > 1 { "s" } else { "" }
    );

    // ── head: spinner + count + chevron (down⇄up stacked-glyph crossfade
    //    — the web rotates 180°; see the `chev` field note) ──
    let chevron: AnyElement = {
        let stacked = |flip: f32| {
            div()
                .relative()
                .size(px(16.))
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
                                .size(IconSize::Small)
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
                                .size(IconSize::Small)
                                .color(Color::Placeholder),
                        ),
                )
        };
        if island.chev.animating() {
            let chev = island.chev.clone();
            div()
                .with_animation(
                    ElementId::NamedInteger("ti-chev".into(), island.chev.generation() as u64),
                    Animation::new(CHEV_MORPH),
                    move |wrap, _| wrap.child(stacked(chev.current().clamp(0., 1.))),
                )
                .into_any_element()
        } else {
            stacked(island.chev.target()).into_any_element()
        }
    };
    let head = h_flex()
        .id("ti-head")
        .w_full()
        .items_center()
        .gap(px(9.))
        .px(px(12.))
        .py(px(9.))
        .cursor_pointer()
        .hover(|head| head.bg(cx.theme().colors().element_hover))
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
        h_flex()
            .id(ElementId::Name(format!("ti-row-{run_id}").into()))
            .w_full()
            .items_center()
            .gap(px(9.))
            .px(px(12.))
            .py(px(7.))
            .border_t_1()
            .border_color(cx.theme().colors().border)
            .cursor_pointer()
            .text_size(px(12.5))
            .text_color(cx.theme().colors().text_muted)
            .hover(|row| {
                row.bg(cx.theme().colors().element_hover)
                    .text_color(cx.theme().colors().text)
            })
            .on_click(cx.listener(move |panel, _, window, cx| {
                panel.open_run(open_id.clone(), window, cx);
            }))
            .child(spinner(ElementId::Name(format!("ti-spin-{run_id}").into())))
            .child(div().min_w_0().truncate().child(label.clone()))
    });
    let rows_open = island.open;
    let rows_body: AnyElement = if island.rows_reveal.animating() {
        let reveal = island.rows_reveal.clone();
        v_flex()
            .overflow_hidden()
            .children(rows)
            .with_animation(
                ElementId::NamedInteger(
                    "ti-rows-morph".into(),
                    island.rows_reveal.generation() as u64,
                ),
                Animation::new(ROWS_MORPH),
                move |body, _| body.max_h(px((ROWS_MAX_H * reveal.current()).max(0.))),
            )
            .into_any_element()
    } else if rows_open {
        v_flex()
            .id("ti-rows")
            .max_h(px(ROWS_MAX_H))
            .overflow_y_scroll()
            .children(rows)
            .into_any_element()
    } else {
        gpui::Empty.into_any_element()
    };

    // ── container + the §4.9 emergence wrapper ──
    let container = v_flex()
        .w_full()
        .rounded(px(14.))
        .bg(SURFACE_FLOAT)
        .border_1()
        .border_color(HAIRLINE_HI)
        .overflow_hidden()
        .child(head)
        .child(rows_body);

    if island.emerge.animating() || island.fade.animating() {
        let (emerge, fade) = (island.emerge.clone(), island.fade.clone());
        let generation =
            (island.emerge.generation() as u64) << 8 ^ island.fade.generation() as u64;
        // Frame pump over the Instant-clocked pair: translate toward/away
        // from the composer anchor below while opacity crossfades
        // concurrently (geometry + opacity from frame one, §4.9).
        div()
            .child(container)
            .with_animation(
                ElementId::NamedInteger("ti-emerge".into(), generation),
                Animation::new(EMERGE),
                move |wrap, _| {
                    let offset = TUCK * emerge.current();
                    wrap.mt(px(offset))
                        .mb(px(ANCHOR_GAP - offset))
                        .opacity(fade.current().clamp(0., 1.))
                },
            )
            .into_any_element()
    } else {
        let offset = TUCK * island.emerge.target();
        div()
            .mt(px(offset))
            .mb(px(ANCHOR_GAP - offset))
            .opacity(island.fade.target())
            .child(container)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(id: &str, status: &str, task: Option<&str>, agent: &str) -> RunRow {
        RunRow {
            run_id: id.to_string(),
            status: status.to_string(),
            task: task.map(str::to_string),
            agent: agent.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn rows_filter_running_and_fall_back_task_agent_task() {
        let board = vec![
            run("r-1", "running", Some("port wave Z3"), "claude"),
            run("r-2", "completed", Some("done thing"), "codex"),
            run("r-3", "running", None, "codex"),
            run("r-4", "running", Some(""), "gemini"),
            run("r-5", "running", None, ""),
            run("r-6", "pending", None, "agy"),
        ];
        let rows = running_rows(&board);
        let shaped: Vec<(&str, &str)> = rows
            .iter()
            .map(|(id, label)| (id.as_ref(), label.as_ref()))
            .collect();
        assert_eq!(
            shaped,
            [
                ("r-1", "port wave Z3"), // task wins
                ("r-3", "codex"),        // no task -> agent
                ("r-4", "gemini"),       // empty task -> agent (JS falsy "")
                ("r-5", "task"),         // nothing -> "task"
            ],
            "running-only, board order, task || agent || 'task' labels"
        );
    }
}
