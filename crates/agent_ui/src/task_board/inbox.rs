//! Inbox (grid view) — the Antigravity run list, one row per run
//! (PARITY_SPEC §4.2 / web `board.js renderInbox` + `board.css .inbox-row`).
//!
//! Rendered through `uniform_list` (rows are fixed-height) with rows keyed by
//! run id, so a poll/SSE tick morphs existing rows in place instead of
//! rebuilding the list — the native expression of the web's keyed DOM
//! reconciliation (§5.6): hover state survives ticks and the running pill's
//! pulse never restarts.

use std::sync::Arc;

use gpui::{
    Animation, AnimationExt as _, AnyElement, App, ElementId, FontWeight, SharedString,
    WeakEntity, uniform_list,
};
use settings::Settings as _;
use theme_settings::ThemeSettings;
use ui::prelude::*;

use crate::agent_accents::{STATUS_BLOCKED, STATUS_DONE};
use crate::bridge::RunRow;

use super::panel::TaskBoardPanel;
use super::style::{chip_reason, rel, status_pill};

/// Fixed inbox row height (web: 14px padding × 2 + title 14px + 5px gap +
/// sub-line 13px ≈ 72px), required uniform for `uniform_list`.
const ROW_HEIGHT: f32 = 72.;

/// Builds the virtualized inbox list. `rows` arrive newest-activity-first
/// (the caller reverses the board, mirroring `board.js`).
pub fn inbox_list(
    rows: Arc<Vec<RunRow>>,
    panel: WeakEntity<TaskBoardPanel>,
    cx: &App,
) -> AnyElement {
    if rows.is_empty() {
        return empty_inbox(cx).into_any_element();
    }
    uniform_list(
        "task-board-inbox",
        rows.len(),
        move |range, _window, cx| {
            range
                .map(|ix| inbox_row(&rows[ix], panel.clone(), cx))
                .collect()
        },
    )
    .size_full()
    .px(px(16.))
    .pt(px(8.))
    .into_any_element()
}

/// One `.inbox-row`: status pill (elapsed reveals inside on hover) · title +
/// agent chip/reason sub-line · badge slot + relative time.
fn inbox_row(run: &RunRow, panel: WeakEntity<TaskBoardPanel>, cx: &mut App) -> AnyElement {
    let colors = cx.theme().colors();
    let border = colors.border;
    let hover_bg = colors.element_hover;
    let text = colors.text;
    let text_2 = colors.text_muted;
    let text_3 = colors.text_placeholder;
    let mono = ThemeSettings::get_global(cx).buffer_font.family.clone();

    let run_id = SharedString::from(run.run_id.clone());
    let title = SharedString::from(
        run.task
            .clone()
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| "untitled task".to_string()),
    );
    let reason = chip_reason(run.chip.as_deref(), &run.agent);
    let when = SharedString::from(rel(run.elapsed_s));
    let status = run.status.as_str();

    // Badge slot: bell when blocked, blue done-dot when completed.
    let badge: Option<AnyElement> = match status {
        "failed" | "killed" => Some(
            Icon::new(IconName::Bell)
                .size(IconSize::Small)
                .color(Color::Custom(STATUS_BLOCKED.into()))
                .into_any_element(),
        ),
        "completed" => Some(
            div()
                .size(px(8.))
                .rounded_full()
                .bg(STATUS_DONE)
                .into_any_element(),
        ),
        _ => None,
    };

    let on_click_panel = panel.clone();
    let click_run_id = run_id.clone();
    let row = h_flex()
        .id(ElementId::Name(run_id.clone()))
        .group("inbox-row")
        .w_full()
        .h(px(ROW_HEIGHT))
        .items_center()
        .gap(px(14.))
        .px(px(12.))
        .border_b_1()
        .border_color(border)
        .hover(move |style| style.bg(hover_bg))
        .cursor_pointer()
        .on_click(move |_, _, cx| {
            on_click_panel
                .update(cx, |panel, cx| panel.open_run(click_run_id.clone(), cx))
                .ok();
        })
        // Status pill; the elapsed segment inside reveals on row hover (the
        // web's compact↔extended island morph — see Z1_REPORT divergences).
        .child(
            status_pill(
                ElementId::Name(format!("pill-{run_id}").into()),
                status,
                Some(rel(run.elapsed_s)),
                cx,
            )
            .min_w(px(72.)),
        )
        // Main column: title + agent chip / routing reason.
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap(px(5.))
                .child(
                    div()
                        .text_size(px(14.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(text)
                        .truncate()
                        .child(title),
                )
                .child(
                    h_flex()
                        .items_center()
                        .gap(px(10.))
                        .text_size(px(13.))
                        .text_color(text_2)
                        .child(super::style::agent_chip(&run.agent, cx))
                        .when(!reason.is_empty(), |this| {
                            this.child(
                                div()
                                    .text_color(text_3)
                                    .truncate()
                                    .child(SharedString::from(reason)),
                            )
                        }),
                ),
        )
        // Right rail: badge slot + relative time.
        .child(
            h_flex()
                .flex_none()
                .items_center()
                .gap(px(10.))
                .children(badge)
                .child(
                    div()
                        .font_family(mono)
                        .text_size(px(13.))
                        .text_color(text_3)
                        .child(when),
                ),
        );

    // `.rise` entrance — one-shot per run identity (keyed by run id), so a
    // tick never restarts it (web `seenRuns`/keyed-DOM equivalent).
    row.with_animation(
        ElementId::Name(format!("rise-{run_id}").into()),
        Animation::new(std::time::Duration::from_millis(300))
            .with_easing(gpui::ease_out_quint()),
        |row, t| row.opacity(t),
    )
    .into_any_element()
}

/// Web empty-state copy, verbatim.
fn empty_inbox(cx: &App) -> impl IntoElement {
    super::style::empty_state(
        IconName::Envelope,
        "No runs yet",
        "Delegated tasks land here as an inbox — status, agent, and elapsed time, re-sorted live.",
        cx,
    )
}
