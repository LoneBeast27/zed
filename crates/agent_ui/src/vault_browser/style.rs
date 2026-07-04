//! Shared visual vocabulary for the vault browser. Re-uses the task-board's
//! out-of-chrome tokens + empty-state builder (one source of truth — PARITY
//! tokens live in `task_board::style`) and adds the vault-specific type/vendor
//! chips + the age formatter.

use gpui::{FontWeight, Rgba, SharedString};
use ui::prelude::*;

use crate::agent_accents::{accent_for_agent, rgba_hex};

// Re-export the board tokens + empty state so vault modules import from one
// place (task_board::style is the canonical PARITY token source).
pub(super) use crate::task_board::style::{HAIRLINE_HI, SURFACE_1, ago_now, empty_state};

/// `--surface-2: #141414` — chip fill for the type/vendor pills.
const CHIP_BG: Rgba = rgba_hex(0x1a1a1aff);

/// A small type chip (the OKF `type:` value): mono 11px on the inline-code
/// fill, rounded. Empty type → nothing.
pub(super) fn type_chip(doc_type: &str, cx: &App) -> Option<Div> {
    if doc_type.is_empty() {
        return None;
    }
    let colors = cx.theme().colors();
    Some(
        div()
            .flex_none()
            .px(px(7.))
            .py(px(2.))
            .rounded(px(6.))
            .bg(CHIP_BG)
            .text_size(px(11.))
            .text_color(colors.text_muted)
            .child(SharedString::from(doc_type.to_string())),
    )
}

/// A vendor chip: vendor-accent dot + name, matching the agent-chip idiom but
/// keyed off the OKF `vendor:` field. Empty vendor → nothing.
pub(super) fn vendor_chip(vendor: &str, cx: &App) -> Option<Div> {
    if vendor.is_empty() {
        return None;
    }
    let colors = cx.theme().colors();
    let accent = accent_for_agent(vendor);
    Some(
        h_flex()
            .flex_none()
            .items_center()
            .gap(px(5.))
            .px(px(7.))
            .py(px(2.))
            .rounded(px(6.))
            .bg(CHIP_BG)
            .text_size(px(11.))
            .text_color(colors.text_muted)
            .child(div().size(px(6.)).rounded_full().bg(accent))
            .child(SharedString::from(vendor.to_string())),
    )
}

/// A redactions-count badge for a session (only rendered when > 0): the count
/// in the blocked/amber tone, signalling secrets were scrubbed.
pub(super) fn redactions_badge(count: u64, cx: &App) -> Option<Div> {
    if count == 0 {
        return None;
    }
    let _ = cx;
    Some(
        div()
            .flex_none()
            .text_size(px(11.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(crate::agent_accents::STATUS_BLOCKED)
            .child(SharedString::from(format!("{count} redacted"))),
    )
}

/// Format an ISO-8601 `updated` timestamp into a compact relative age
/// ("3h", "2d"). Falls back to the raw string's date part if it can't parse.
pub(super) fn updated_age(iso: &str) -> String {
    if iso.is_empty() {
        return String::new();
    }
    match parse_iso_epoch(iso) {
        Some(epoch) => ago_now(epoch),
        None => iso.split('T').next().unwrap_or(iso).to_string(),
    }
}

/// Minimal ISO-8601 → unix-seconds parser for the `YYYY-MM-DDTHH:MM:SS(.fff)Z`
/// shape the importer writes (FORMATS.md). Avoids a chrono round-trip in the
/// hot list path; returns None on any deviation (caller shows the date part).
fn parse_iso_epoch(iso: &str) -> Option<f64> {
    let iso = iso.trim().trim_matches('"');
    let (date, rest) = iso.split_once('T')?;
    let mut d = date.split('-');
    let year: i64 = d.next()?.parse().ok()?;
    let month: i64 = d.next()?.parse().ok()?;
    let day: i64 = d.next()?.parse().ok()?;
    let time = rest.trim_end_matches('Z');
    let time = time.split('.').next().unwrap_or(time);
    let mut t = time.split(':');
    let hour: i64 = t.next()?.parse().ok()?;
    let min: i64 = t.next().unwrap_or("0").parse().ok()?;
    let sec: i64 = t.next().unwrap_or("0").parse().ok()?;
    // Days since the Unix epoch via a civil-calendar algorithm (Howard Hinnant).
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some((days * 86400 + hour * 3600 + min * 60 + sec) as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_parses_the_importer_shape() {
        // 2026-06-30T11:18:47.122Z — the real session.md updated stamp.
        let epoch = parse_iso_epoch("2026-06-30T11:18:47.122Z").unwrap();
        // Sanity: within the expected 2026 range (1.7e9..1.8e9 unix seconds).
        assert!(epoch > 1_780_000_000.0 && epoch < 1_800_000_000.0);
    }

    #[test]
    fn iso_handles_quotes_and_no_millis() {
        assert!(parse_iso_epoch("\"2026-05-15T20:40:26Z\"").is_some());
    }

    #[test]
    fn updated_age_falls_back_to_date_on_garbage() {
        assert_eq!(updated_age("not-a-date"), "not-a-date");
        assert_eq!(updated_age(""), "");
    }
}
