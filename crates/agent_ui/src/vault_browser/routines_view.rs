//! The ROUTINES view — the render half (`routines.rs` owns the data,
//! `routines_actions.rs` the buttons + POSTs; split at the 500-line
//! ceiling). One card per routine: title (greyed when disabled) · schedule
//! chip · ping liveness dot · the bridge's health word, over the last-fire
//! outcome line and the row actions (Fire now / Enable–Disable / the AMBER
//! Consent affordance).
//!
//! Degrade law (the axioms contract): loading / unavailable / honest-empty
//! states via the shared `empty_state`; a stale snapshot renders under the
//! amber stale note; NO fake rows on connection failure, ever.

use gpui::{AnyElement, Context, Div, FontWeight, Rgba, SharedString};
use settings::Settings as _;
use theme_settings::ThemeSettings;
use ui::prelude::*;

use crate::agent_accents::{
    STATUS_BLOCKED, STATUS_DONE, STATUS_ERROR, STATUS_IDLE, STATUS_RUNNING,
};
use crate::task_board::style::ago;

use super::panel::VaultBrowserPanel;
use super::routines::{LastFire, PingResult, RoutineRow};
use super::style::{SURFACE_1, empty_state};

/// What the most recent sidecar event was (drives the outcome line's color).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LastOutcome {
    Never,
    Ok,
    Error,
    Skipped,
}

/// Fold the sidecar ledger into the row's last-fire line: the MOST RECENT of
/// (fire, skip) wins; a fire with an error reads error + the text verbatim.
pub(super) fn last_outcome_line(last: &LastFire, now: f64) -> (LastOutcome, String) {
    let fired = last.at.unwrap_or(f64::NEG_INFINITY);
    let skipped = last.skipped.unwrap_or(f64::NEG_INFINITY);
    if last.at.is_none() && last.skipped.is_none() {
        return (LastOutcome::Never, "never fired".to_string());
    }
    if skipped > fired {
        let when = ago(skipped, now);
        let reason = last.skip_reason.as_deref().unwrap_or("");
        return if reason.is_empty() {
            (LastOutcome::Skipped, format!("skipped · {when}"))
        } else {
            (LastOutcome::Skipped, format!("skipped · {when} — {reason}"))
        };
    }
    let when = ago(fired, now);
    match last.error.as_deref() {
        Some(error) if !error.is_empty() => {
            (LastOutcome::Error, format!("error · {when} — {error}"))
        }
        _ => (LastOutcome::Ok, format!("ok · {when}")),
    }
}

impl LastOutcome {
    fn color(self) -> Rgba {
        match self {
            LastOutcome::Never | LastOutcome::Ok => STATUS_DONE,
            LastOutcome::Error => STATUS_ERROR,
            LastOutcome::Skipped => STATUS_BLOCKED,
        }
    }
}

/// The health word's color — safety states keep hue (MONO ruling), calm
/// states go neutral. Unknown health words read idle, never invented-green.
pub(super) fn health_color(health: &str) -> Rgba {
    match health {
        "ok" => STATUS_DONE,
        "firing" | "continuous" => STATUS_RUNNING,
        "late" | "warning" => STATUS_BLOCKED,
        "failed" | "invalid" | "banned" => STATUS_ERROR,
        _ => STATUS_IDLE, // disabled | banked | manual | anything new
    }
}

/// The ping liveness dot: green only on a recorded ok; any failure shape
/// (error / timeout / 429) reads red; no result yet reads idle-neutral.
pub(super) fn ping_dot_color(last: Option<&PingResult>) -> Rgba {
    match last {
        Some(result) if result.status == "ok" => STATUS_RUNNING,
        Some(_) => STATUS_ERROR,
        None => STATUS_IDLE,
    }
}

fn now_unix() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|epoch| epoch.as_secs_f64())
        .unwrap_or(0.0)
}

impl VaultBrowserPanel {
    /// The ROUTINES body. Bridge-fed, independent of the fs index (same
    /// contract as the axioms view) — renders before the index gate.
    pub(super) fn routines_view(&self, cx: &Context<Self>) -> AnyElement {
        let Some(snapshot) = self.routines.snapshot.clone() else {
            return if self.routines.stale {
                empty_state(
                    IconName::XCircle,
                    "Routines unavailable",
                    "Could not reach the bridge at 127.0.0.1:4530 — routines will appear when it is back.",
                    cx,
                )
                .into_any_element()
            } else {
                empty_state(
                    IconName::ArrowCircle,
                    "Loading routines…",
                    "Fetching the routine table from the bridge.",
                    cx,
                )
                .into_any_element()
            };
        };

        let colors = cx.theme().colors();
        let mut column = v_flex().w_full().gap(px(10.));

        if self.routines.stale {
            column = column.child(
                div()
                    .text_size(px(11.))
                    .text_color(STATUS_BLOCKED)
                    .child("bridge unreachable — showing last-known routines"),
            );
        }

        if snapshot.routines.is_empty() {
            column = column.child(div().w_full().h(px(220.)).child(empty_state(
                IconName::Clock,
                "No routines",
                "Definitions live in the vault (projects/*/routines/*.md, global/routines/) — rows appear when the engine loads one.",
                cx,
            )));
        } else {
            let active = snapshot
                .routines
                .iter()
                .filter(|row| row.valid && row.status == "active")
                .count();
            column = column.child(
                div()
                    .text_size(px(11.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(colors.text_placeholder)
                    .child(SharedString::from(format!(
                        "{} routines · {active} active",
                        snapshot.routines.len()
                    ))),
            );
            let now = now_unix();
            for (ix, row) in snapshot.routines.iter().enumerate() {
                column = column.child(self.routine_row(ix, row, now, cx));
            }
        }

        div()
            .id("vault-routines")
            .size_full()
            .overflow_y_scroll()
            .p(px(12.))
            .child(column)
            .into_any_element()
    }

    /// One routine card: title + health word, meta chips, last-fire line,
    /// problems (invalid rows), the action row, and the verbatim action note.
    fn routine_row(&self, ix: usize, row: &RoutineRow, now: f64, cx: &Context<Self>) -> Div {
        let colors = cx.theme().colors();
        let mono = ThemeSettings::get_global(cx).buffer_font.family.clone();
        let disabled = row.status != "active";

        // ── title + the bridge's health word ──
        let title_row = h_flex()
            .w_full()
            .items_center()
            .gap(px(8.))
            .child(
                div()
                    .flex_1()
                    .text_size(px(13.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(if disabled {
                        colors.text_placeholder
                    } else {
                        colors.text
                    })
                    .child(SharedString::from(row.title.clone())),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(px(11.))
                    .font_family(mono.clone())
                    .text_color(health_color(&row.health))
                    .child(SharedString::from(row.health.clone())),
            );

        // ── meta: schedule chip · ping dot · next-fire words ──
        let mut meta = h_flex().w_full().items_center().gap(px(8.));
        if !row.when.is_empty() {
            meta = meta.child(
                div()
                    .flex_none()
                    .px(px(7.))
                    .py(px(2.))
                    .rounded(px(6.))
                    .bg(SURFACE_1)
                    .text_size(px(11.))
                    .font_family(mono.clone())
                    .text_color(colors.text_muted)
                    .child(SharedString::from(row.when.clone())),
            );
        }
        if let Some(ping) = row.hack.as_ref().and_then(|hack| hack.ping.as_ref()) {
            let label = match ping.last.as_ref() {
                Some(last) if last.status != "ok" => {
                    format!("{} · {}", ping.vendor, last.status)
                }
                _ => ping.vendor.clone(),
            };
            meta = meta.child(
                h_flex()
                    .flex_none()
                    .items_center()
                    .gap(px(5.))
                    .child(
                        div()
                            .size(px(6.))
                            .rounded_full()
                            .bg(ping_dot_color(ping.last.as_ref())),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .font_family(mono.clone())
                            .text_color(colors.text_muted)
                            .child(SharedString::from(label)),
                    ),
            );
        }
        if let Some(next) = row.next_fire_in.clone() {
            meta = meta.child(
                div()
                    .text_size(px(11.))
                    .text_color(colors.text_placeholder)
                    .child(SharedString::from(next)),
            );
        }

        // ── the last-fire outcome line ──
        let (outcome, line) = last_outcome_line(&row.last, now);
        let outcome_line = div()
            .w_full()
            .text_size(px(11.))
            .font_family(mono.clone())
            .text_color(outcome.color())
            .child(SharedString::from(line));

        let mut card = v_flex()
            .w_full()
            .gap(px(6.))
            .p(px(10.))
            .rounded(px(10.))
            .bg(SURFACE_1)
            .border_1()
            .border_color(colors.border)
            .child(title_row)
            .child(meta)
            .child(outcome_line);

        if !row.valid {
            // Invalid: every problem verbatim, NO actions (bridge 409s them).
            for problem in &row.problems {
                card = card.child(
                    div()
                        .w_full()
                        .text_size(px(11.))
                        .font_family(mono.clone())
                        .text_color(STATUS_ERROR)
                        .child(SharedString::from(problem.clone())),
                );
            }
            return card;
        }

        card = card.child(self.routine_actions(ix, row, &mono, cx));
        if let Some(note) = self.routines.notes.get(&row.id) {
            card = card.child(
                div()
                    .w_full()
                    .text_size(px(11.))
                    .font_family(mono)
                    .text_color(colors.text_muted)
                    .child(SharedString::from(note.clone())),
            );
        }
        card
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    fn last(
        at: Option<f64>,
        error: Option<&str>,
        skipped: Option<f64>,
        skip_reason: Option<&str>,
    ) -> LastFire {
        LastFire {
            at,
            error: error.map(str::to_string),
            skipped,
            skip_reason: skip_reason.map(str::to_string),
            count: 0,
        }
    }

    #[test]
    fn outcome_line_never_fired() {
        let (outcome, line) = last_outcome_line(&last(None, None, None, None), 1_000_000.0);
        assert_eq!(outcome, LastOutcome::Never);
        assert_eq!(line, "never fired");
    }

    #[test]
    fn outcome_line_ok_and_error() {
        let now = 1_783_684_804.0 + 7200.0;
        let (outcome, line) =
            last_outcome_line(&last(Some(1_783_684_804.0), None, None, None), now);
        assert_eq!(outcome, LastOutcome::Ok);
        assert_eq!(line, "ok · 2h");

        let (outcome, line) = last_outcome_line(
            &last(Some(1_783_684_804.0), Some("ping executor: OSError: boom"), None, None),
            now,
        );
        assert_eq!(outcome, LastOutcome::Error);
        assert_eq!(line, "error · 2h — ping executor: OSError: boom");
    }

    #[test]
    fn outcome_line_most_recent_event_wins() {
        let now = 1_783_690_000.0;
        // Live h20 shape: an OLD skip behind a NEWER clean fire → ok wins.
        let ledger = last(
            Some(1_783_684_804.0),
            None,
            Some(1_783_661_400.0),
            Some("headroom_gt: pool unreadable (fail closed)"),
        );
        assert_eq!(last_outcome_line(&ledger, now).0, LastOutcome::Ok);

        // Skip NEWER than the last fire → the skip (verbatim reason) wins.
        let ledger = last(
            Some(1_783_661_400.0),
            None,
            Some(1_783_684_804.0),
            Some("min_interval: queue-1 (a hack can never burst)"),
        );
        let (outcome, line) = last_outcome_line(&ledger, now);
        assert_eq!(outcome, LastOutcome::Skipped);
        assert!(line.ends_with("— min_interval: queue-1 (a hack can never burst)"));

        // Skip with no recorded fire at all (missed-fire policy skip).
        let (outcome, line) =
            last_outcome_line(&last(None, None, Some(1_783_684_804.0), None), now);
        assert_eq!(outcome, LastOutcome::Skipped);
        assert_eq!(line, "skipped · 87m");
    }

    #[test]
    fn health_words_map_to_safety_buckets() {
        assert_eq!(health_color("ok"), STATUS_DONE);
        assert_eq!(health_color("firing"), STATUS_RUNNING);
        assert_eq!(health_color("continuous"), STATUS_RUNNING);
        assert_eq!(health_color("late"), STATUS_BLOCKED);
        assert_eq!(health_color("warning"), STATUS_BLOCKED);
        assert_eq!(health_color("failed"), STATUS_ERROR);
        assert_eq!(health_color("invalid"), STATUS_ERROR);
        assert_eq!(health_color("banned"), STATUS_ERROR);
        assert_eq!(health_color("disabled"), STATUS_IDLE);
        assert_eq!(health_color("banked"), STATUS_IDLE);
        assert_eq!(health_color("manual"), STATUS_IDLE);
        // Unknown future health words degrade to idle, never invented-green.
        assert_eq!(health_color("someday-new"), STATUS_IDLE);
    }

    #[test]
    fn ping_dot_is_green_only_on_ok() {
        let result = |status: &str| PingResult {
            status: status.to_string(),
            latency_ms: Some(100.0),
        };
        assert_eq!(ping_dot_color(Some(&result("ok"))), STATUS_RUNNING);
        assert_eq!(ping_dot_color(Some(&result("error"))), STATUS_ERROR);
        assert_eq!(ping_dot_color(Some(&result("timeout"))), STATUS_ERROR);
        assert_eq!(ping_dot_color(Some(&result("429"))), STATUS_ERROR);
        assert_eq!(ping_dot_color(None), STATUS_IDLE);
    }
}
