//! Inbox (grid view) — the Antigravity run list, one row per run
//! (PARITY_SPEC §4.2 / web `board.js renderInbox` + `board.css .inbox-row`).
//!
//! Rendered through `uniform_list` (rows are fixed-height) with rows keyed by
//! run id, so a poll/SSE tick morphs existing rows in place instead of
//! rebuilding the list — the native expression of the web's keyed DOM
//! reconciliation (§5.6): hover state survives ticks and the running pill's
//! pulse never restarts.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    Animation, AnimationExt as _, AnyElement, App, ElementId, FontWeight, SharedString,
    WeakEntity, uniform_list,
};
use settings::Settings as _;
use theme_settings::ThemeSettings;
use ui::prelude::*;

use crate::agent_accents::{STATUS_BLOCKED, STATUS_DONE};
use crate::bridge::RunRow;

use super::motion::{EFFECTS, STATE_FADE, mix};
use super::panel::TaskBoardPanel;
use super::style::{ElapsedReveal, chip_reason, rel, status_pill};

/// Fixed inbox row height (web: 14px padding × 2 + title 14px + 5px gap +
/// sub-line 13px ≈ 72px), required uniform for `uniform_list`.
const ROW_HEIGHT: f32 = 72.;

/// How long a run counts as fresh after first appearing on the board: the
/// 300ms `.rise` plus margin. Outside the window rows render bare, so a row
/// scrolled out of the uniform_list viewport (element state dropped) and
/// back in can never replay its entrance — the web equivalent is the keyed
/// DOM node persisting after `.rise` finished at creation.
const RISE_WINDOW: Duration = Duration::from_millis(600);

/// Snapshot of the panel's tracked row hover, handed to the row builders:
/// `(run_id, crossfade-window-open)` for the hovered row and for the row
/// that most recently lost hover (its 150ms fade-out).
#[derive(Clone, Default)]
pub struct RowHoverState {
    pub hovered: Option<(SharedString, bool)>,
    pub unhovered: Option<(SharedString, bool)>,
}

/// Builds the virtualized inbox list. `rows` arrive newest-activity-first
/// (the caller reverses the board, mirroring `board.js`). `seen` is the
/// panel-owned first-appearance map driving one-shot `.rise` entrances.
pub fn inbox_list(
    rows: Arc<Vec<RunRow>>,
    seen: &mut HashMap<SharedString, Instant>,
    hover: RowHoverState,
    panel: WeakEntity<TaskBoardPanel>,
    cx: &App,
) -> AnyElement {
    if rows.is_empty() {
        seen.clear();
        return empty_inbox(cx).into_any_element();
    }
    // Stamp first sight at board-arrival time (not first viewport render),
    // matching the web where `.rise` fires at node creation; prune the gone.
    let now = Instant::now();
    let fresh: Arc<Vec<bool>> = Arc::new(
        rows.iter()
            .map(|run| {
                let first_seen = *seen
                    .entry(SharedString::from(run.run_id.clone()))
                    .or_insert(now);
                now.duration_since(first_seen) < RISE_WINDOW
            })
            .collect(),
    );
    let live: std::collections::HashSet<&str> =
        rows.iter().map(|run| run.run_id.as_str()).collect();
    seen.retain(|run_id, _| live.contains(run_id.as_ref()));

    uniform_list(
        "task-board-inbox",
        rows.len(),
        move |range, _window, cx| {
            range
                .map(|ix| inbox_row(&rows[ix], fresh[ix], &hover, panel.clone(), cx))
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
fn inbox_row(
    run: &RunRow,
    rise_fresh: bool,
    hover: &RowHoverState,
    panel: WeakEntity<TaskBoardPanel>,
    cx: &mut App,
) -> AnyElement {
    let colors = cx.theme().colors();
    let border = colors.border;
    let hover_bg = colors.element_hover;
    let no_bg = gpui::transparent_black();
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

    // Tracked hover (web `.inbox-row { transition: background .15s }` +
    // the `.pill-elapsed` reveal): is this row hovered, or mid fade-out?
    let (is_hovered, hover_fresh) = match &hover.hovered {
        Some((id, fresh)) if *id == run_id => (true, *fresh),
        _ => (false, false),
    };
    let unhover_fresh = matches!(&hover.unhovered, Some((id, fresh)) if *id == run_id && *fresh);

    let on_click_panel = panel.clone();
    let click_run_id = run_id.clone();
    let hover_panel = panel.clone();
    let hover_run_id = run_id.clone();
    let row = h_flex()
        .id(ElementId::Name(run_id.clone()))
        .w_full()
        .h(px(ROW_HEIGHT))
        .items_center()
        .gap(px(14.))
        .px(px(12.))
        .border_b_1()
        .border_color(border)
        .cursor_pointer()
        .on_hover(move |hovered, _, cx| {
            hover_panel
                .update(cx, |panel, cx| {
                    panel.set_row_hover(hover_run_id.clone(), *hovered, cx)
                })
                .ok();
        })
        .on_click(move |_, _, cx| {
            on_click_panel
                .update(cx, |panel, cx| panel.open_run(click_run_id.clone(), cx))
                .ok();
        })
        // Status pill; the elapsed segment inside reveals on row hover (the
        // web's compact↔extended island morph — collapsed at rest, 120ms
        // effects fade on hover; width-spring lands with Z4).
        .child(
            status_pill(
                ElementId::Name(format!("pill-{run_id}").into()),
                status,
                Some(ElapsedReveal {
                    text: rel(run.elapsed_s),
                    id: run_id.clone(),
                    hovered: is_hovered,
                    fresh: hover_fresh,
                }),
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

    // Row bg: 150ms effects crossfade on hover flips (board.css:38); settled
    // states render bare — no idle animation wrappers.
    let row: AnyElement = if is_hovered {
        if hover_fresh {
            row.with_animation(
                ElementId::Name(format!("rowbg-in-{run_id}").into()),
                Animation::new(STATE_FADE).with_easing(EFFECTS.easing()),
                move |row, t| row.bg(mix(no_bg, hover_bg, t)),
            )
            .into_any_element()
        } else {
            row.bg(hover_bg).into_any_element()
        }
    } else if unhover_fresh {
        row.with_animation(
            ElementId::Name(format!("rowbg-out-{run_id}").into()),
            Animation::new(STATE_FADE).with_easing(EFFECTS.easing()),
            move |row, t| row.bg(mix(hover_bg, no_bg, t)),
        )
        .into_any_element()
    } else {
        row.into_any_element()
    };

    // `.rise` entrance (web: `.3s var(--decel-curve)`) — attached only
    // inside the panel-owned fresh window, so scroll culling (which drops
    // GPUI element state) can never replay it; settled rows render bare.
    // Wraps outside the bg fade so the two animations compose.
    if !rise_fresh {
        return row;
    }
    div()
        .w_full()
        .h(px(ROW_HEIGHT))
        .child(row)
        .with_animation(
            ElementId::Name(format!("rise-{run_id}").into()),
            Animation::new(std::time::Duration::from_millis(300))
                .with_easing(super::motion::DECEL.easing()),
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
