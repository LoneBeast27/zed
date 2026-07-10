//! The Usage panel's HISTORY section — the bridge run ledger, rendered as a
//! SECTION below the vendor pool cards (zero-new-rail-slot law). Wire types +
//! the render fold live here so [`crate::usage_panel::UsagePanel`] keeps its
//! lifecycle concern and stays under the 500-line ceiling.
//!
//! Field names are PINNED to the live bridge responses (probed 2026-07-10):
//! - `GET /usage-history?by=vendor&limit=50` →
//!   `{by: ["vendor"], count, summary: [{vendor, runs, ok, failed, input,
//!   output, cached, cost_usd, first_ts, last_ts, avg_*}]}`
//! - `GET /usage-history/delta?by=harness_sha&limit=20` →
//!   `{by: "harness_sha", delta: [{harness_sha, runs, ok, failed, input,
//!   output, cached, cost_usd, d_output_pct, d_cost_pct}]}` — the group key is
//!   named after the `by` param, and the delta fields are `d_output_pct` /
//!   `d_cost_pct` (both null on the baseline row).
//!
//! Degrade law (§4.4 never-vanish): a failed fetch keeps the last-known rows
//! and flips the stale flag; a bridge that was never reachable renders the
//! honest "unavailable" line — the section itself never disappears.

use gpui::{AnyElement, App, FontWeight, SharedString};
use serde::Deserialize;
use settings::Settings as _;
use theme_settings::ThemeSettings;
use ui::prelude::*;

use crate::agent_accents::{STATUS_BLOCKED, STATUS_ERROR, accent_for_agent};
use crate::task_board::style::{SURFACE_1, ago_now, tabular_nums};
use crate::usage_panel_groups::vendor_label;

/// One `summary` row from `GET /usage-history?by=vendor` (live field names).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct VendorSummaryRow {
    #[serde(default)]
    pub vendor: String,
    #[serde(default)]
    pub runs: u64,
    #[serde(default)]
    pub ok: u64,
    #[serde(default)]
    pub failed: u64,
    #[serde(default)]
    pub input: u64,
    #[serde(default)]
    pub output: u64,
    #[serde(default)]
    pub cached: u64,
    /// Nullable on the wire (spec'd `cost_usd?`) — a missing cost reads "—".
    #[serde(default)]
    pub cost_usd: Option<f64>,
    #[serde(default)]
    pub last_ts: Option<f64>,
}

/// One `delta` row from `GET /usage-history/delta?by=harness_sha` (live field
/// names — NOT the sketch's `group`/`delta_tokens`/`delta_cost`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct HarnessDeltaRow {
    #[serde(default)]
    pub harness_sha: String,
    #[serde(default)]
    pub runs: u64,
    #[serde(default)]
    pub failed: u64,
    #[serde(default)]
    pub output: u64,
    #[serde(default)]
    pub cost_usd: Option<f64>,
    /// Output-tokens % change vs the previous harness build; null = baseline.
    #[serde(default)]
    pub d_output_pct: Option<f64>,
    /// Cost % change vs the previous harness build; null = baseline.
    #[serde(default)]
    pub d_cost_pct: Option<f64>,
}

/// The panel's history-fetch state. Last-known rows survive a failed refetch
/// (degrade to stale, never blank).
#[derive(Default)]
pub struct HistoryState {
    pub summary: Vec<VendorSummaryRow>,
    /// Total ledger runs behind the summary (the response's `count`).
    pub count: u64,
    pub delta: Vec<HarnessDeltaRow>,
    /// At least one fetch attempt has completed (gates "loading…").
    pub attempted: bool,
    /// The LAST fetch attempt failed — rows (if any) are last-known/stale.
    pub stale: bool,
}

impl HistoryState {
    pub fn is_empty(&self) -> bool {
        self.summary.is_empty() && self.delta.is_empty()
    }
}

/// Parse the `/usage-history` response → (total run count, summary rows).
pub fn parse_summary(raw: &str) -> anyhow::Result<(u64, Vec<VendorSummaryRow>)> {
    #[derive(Default, Deserialize)]
    struct SummaryResponse {
        #[serde(default)]
        count: u64,
        #[serde(default)]
        summary: Vec<VendorSummaryRow>,
    }
    let parsed: SummaryResponse = serde_json::from_str(raw)?;
    Ok((parsed.count, parsed.summary))
}

/// Parse the `/usage-history/delta` response → delta rows.
pub fn parse_delta(raw: &str) -> anyhow::Result<Vec<HarnessDeltaRow>> {
    #[derive(Default, Deserialize)]
    struct DeltaResponse {
        #[serde(default)]
        delta: Vec<HarnessDeltaRow>,
    }
    let parsed: DeltaResponse = serde_json::from_str(raw)?;
    Ok(parsed.delta)
}

/// Compact token count ("7.1k", "1.2M", "37") — mono-cell friendly.
fn fmt_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1e6)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1e3)
    } else {
        n.to_string()
    }
}

/// "$0.74" / "$0.07"; a null wire cost reads the honest "—".
fn fmt_cost(cost: Option<f64>) -> String {
    match cost {
        Some(cost) => format!("${cost:.2}"),
        None => "—".to_string(),
    }
}

/// Signed percent ("+18900%", "−99.9%") for the harness delta columns.
fn fmt_delta_pct(pct: f64) -> String {
    if pct.abs() >= 100.0 {
        format!("{:+.0}%", pct)
    } else {
        format!("{:+.1}%", pct)
    }
}

/// The History SECTION card: summary rows + the compact harness-delta list.
/// Always returns an element — offline/empty states render honest copy in
/// place of rows (never a silent vanish).
///
/// NOT a `uniform_list`: `by=vendor` bounds the summary to vendor cardinality
/// (~5 rows structurally, never near the ~20 virtualization line) and the
/// delta leg is `limit=20`-capped server-side; both ride the panel's existing
/// scroll column.
pub(crate) fn render_history_section(history: &HistoryState, cx: &App) -> AnyElement {
    let colors = cx.theme().colors();
    let mono = ThemeSettings::get_global(cx).buffer_font.family.clone();

    let sub = if history.count > 0 {
        format!("{} runs · by vendor", history.count)
    } else {
        "run ledger · by vendor".to_string()
    };
    let header = h_flex()
        .items_baseline()
        .gap(px(10.))
        .child(
            div()
                .text_size(px(15.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(colors.text)
                .child("History"),
        )
        .child(
            div()
                .ml_auto()
                .text_size(px(12.))
                .font_family(mono.clone())
                .font_features(tabular_nums())
                .text_color(colors.text_placeholder)
                .child(SharedString::from(sub)),
        );

    let mut section = v_flex()
        .w_full()
        .rounded(px(12.))
        .bg(SURFACE_1)
        .border_1()
        .border_color(colors.border)
        .px(px(16.))
        .py(px(14.))
        .child(header);

    // Degrade notes — honest, in the amber warning tone (semantic, kept).
    if history.stale && !history.is_empty() {
        section = section.child(
            div()
                .mt(px(8.))
                .text_size(px(11.))
                .font_family(mono.clone())
                .text_color(STATUS_BLOCKED)
                .child("bridge unreachable — showing last-known ledger"),
        );
    }

    let body: AnyElement = if !history.attempted {
        status_line("loading history…", &mono, cx)
    } else if history.is_empty() && history.stale {
        status_line("history unavailable — bridge unreachable", &mono, cx)
    } else if history.is_empty() {
        status_line("no ledger rows yet", &mono, cx)
    } else {
        let rows: Vec<AnyElement> = history
            .summary
            .iter()
            .map(|row| render_summary_row(row, &mono, cx))
            .collect();
        v_flex()
            .mt(px(12.))
            .gap(px(12.))
            .children(rows)
            .children(render_delta_list(&history.delta, &mono, cx))
            .into_any_element()
    };

    section.child(body).into_any_element()
}

/// A single mono status line inside the section (loading / unavailable /
/// no-rows-yet) — the section's honest non-row bodies.
fn status_line(text: &'static str, mono: &SharedString, cx: &App) -> AnyElement {
    let colors = cx.theme().colors();
    div()
        .mt(px(10.))
        .text_size(px(12.))
        .font_family(mono.clone())
        .italic()
        .text_color(colors.text_placeholder)
        .child(text)
        .into_any_element()
}

/// One vendor summary row: accent dot + vendor name over the token/cost
/// breakdown; runs/ok right-aligned, failures in the error tone when present.
fn render_summary_row(row: &VendorSummaryRow, mono: &SharedString, cx: &App) -> AnyElement {
    let colors = cx.theme().colors();
    let accent = accent_for_agent(&row.vendor);
    let age = row
        .last_ts
        .map(|ts| format!("  ·  last {}", ago_now(ts)))
        .unwrap_or_default();
    let detail = format!(
        "in {} · out {} · cached {} · {}{age}",
        fmt_tokens(row.input),
        fmt_tokens(row.output),
        fmt_tokens(row.cached),
        fmt_cost(row.cost_usd),
    );
    v_flex()
        .w_full()
        .gap(px(4.))
        .child(
            h_flex()
                .items_baseline()
                .gap(px(8.))
                .child(div().size(px(7.)).rounded_full().bg(accent).flex_none())
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(14.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(colors.text)
                        .truncate()
                        .child(SharedString::from(vendor_label(&row.vendor).to_string())),
                )
                .child(
                    div()
                        .flex_none()
                        .text_size(px(12.))
                        .font_family(mono.clone())
                        .font_features(tabular_nums())
                        .text_color(colors.text_muted)
                        .child(SharedString::from(format!(
                            "{} runs · {} ok",
                            row.runs, row.ok
                        ))),
                )
                .when(row.failed > 0, |this| {
                    this.child(
                        div()
                            .flex_none()
                            .text_size(px(12.))
                            .font_family(mono.clone())
                            .font_features(tabular_nums())
                            .text_color(STATUS_ERROR)
                            .child(SharedString::from(format!("· {} failed", row.failed))),
                    )
                }),
        )
        .child(
            div()
                .pl(px(15.))
                .text_size(px(11.))
                .font_family(mono.clone())
                .font_features(tabular_nums())
                .text_color(colors.text_placeholder)
                .child(SharedString::from(detail)),
        )
        .into_any_element()
}

/// The compact harness-delta list under the vendor rows: one mono line per
/// harness build ("sha · runs · out · cost · Δout · Δcost"; nulls read
/// "baseline"). `None` when the delta leg has no rows.
fn render_delta_list(
    delta: &[HarnessDeltaRow],
    mono: &SharedString,
    cx: &App,
) -> Option<AnyElement> {
    if delta.is_empty() {
        return None;
    }
    let colors = cx.theme().colors();
    let rows: Vec<AnyElement> = delta
        .iter()
        .map(|row| {
            let deltas = match (row.d_output_pct, row.d_cost_pct) {
                (None, None) => "baseline".to_string(),
                (out_pct, cost_pct) => format!(
                    "Δout {} · Δcost {}",
                    out_pct.map(fmt_delta_pct).unwrap_or_else(|| "—".into()),
                    cost_pct.map(fmt_delta_pct).unwrap_or_else(|| "—".into()),
                ),
            };
            let failed = if row.failed > 0 {
                format!(" · {} failed", row.failed)
            } else {
                String::new()
            };
            h_flex()
                .items_baseline()
                .gap(px(8.))
                .child(
                    div()
                        .flex_none()
                        .text_size(px(11.))
                        .font_family(mono.clone())
                        .text_color(colors.text_muted)
                        .child(SharedString::from(row.harness_sha.clone())),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(11.))
                        .font_family(mono.clone())
                        .font_features(tabular_nums())
                        .text_color(colors.text_placeholder)
                        .truncate()
                        .child(SharedString::from(format!(
                            "{} runs{failed} · out {} · {} · {deltas}",
                            row.runs,
                            fmt_tokens(row.output),
                            fmt_cost(row.cost_usd),
                        ))),
                )
                .into_any_element()
        })
        .collect();
    Some(
        v_flex()
            .mt(px(4.))
            .pt(px(10.))
            .border_t_1()
            .border_color(colors.border)
            .gap(px(6.))
            .child(
                div()
                    .text_size(px(12.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(colors.text_muted)
                    .child("Harness deltas"),
            )
            .children(rows)
            .into_any_element(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // Verbatim slices of the LIVE bridge responses (probed 2026-07-10) — the
    // wire pins. If the bridge renames a field these fail before the UI lies.
    const LIVE_SUMMARY: &str = r#"{"by": ["vendor"], "count": 28, "summary": [
        {"vendor": "claude", "runs": 11, "ok": 11, "failed": 0, "input": 7090,
         "output": 19012, "cached": 539843, "first_ts": 1783373621.8877583,
         "last_ts": 1783641566.2180357, "cost_usd": 0.742874,
         "avg_output_per_run": 1728.4, "avg_cost_per_run": 0.067534,
         "avg_duration_s": 34.25}]}"#;
    const LIVE_DELTA: &str = r#"{"by": "harness_sha", "vendor": null, "delta": [
        {"harness_sha": "0d94b5f", "runs": 7, "ok": 7, "failed": 0,
         "input": 19124, "output": 14793, "cached": 357378,
         "first_ts": 1783429843.1, "last_ts": 1783430210.1,
         "cost_usd": 0.23356, "avg_output_per_run": 2113.3,
         "avg_cost_per_run": 0.033366, "avg_duration_s": 28.24,
         "d_output_pct": null, "d_cost_pct": null},
        {"harness_sha": "f65bb1b", "runs": 12, "ok": 9, "failed": 2,
         "input": 18972, "output": 20, "cached": 47518,
         "first_ts": 1783434466.1, "last_ts": 1783470778.8,
         "cost_usd": 0.069818, "avg_output_per_run": 1.7,
         "avg_cost_per_run": 0.005818, "avg_duration_s": 14.76,
         "d_output_pct": -99.9, "d_cost_pct": -82.6}]}"#;

    #[test]
    fn parses_the_live_summary_shape() {
        let (count, rows) = parse_summary(LIVE_SUMMARY).unwrap();
        assert_eq!(count, 28);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].vendor, "claude");
        assert_eq!(rows[0].runs, 11);
        assert_eq!(rows[0].cached, 539_843);
        assert_eq!(rows[0].cost_usd, Some(0.742874));
    }

    #[test]
    fn parses_the_live_delta_shape() {
        let rows = parse_delta(LIVE_DELTA).unwrap();
        assert_eq!(rows.len(), 2);
        // The baseline row carries null deltas.
        assert_eq!(rows[0].harness_sha, "0d94b5f");
        assert_eq!(rows[0].d_output_pct, None);
        assert_eq!(rows[1].d_output_pct, Some(-99.9));
        assert_eq!(rows[1].d_cost_pct, Some(-82.6));
        assert_eq!(rows[1].failed, 2);
    }

    #[test]
    fn token_and_cost_formatting() {
        assert_eq!(fmt_tokens(37), "37");
        assert_eq!(fmt_tokens(7090), "7.1k");
        assert_eq!(fmt_tokens(539_843), "539.8k");
        assert_eq!(fmt_tokens(1_200_000), "1.2M");
        assert_eq!(fmt_cost(Some(0.742874)), "$0.74");
        assert_eq!(fmt_cost(None), "—");
        assert_eq!(fmt_delta_pct(-99.9), "-99.9%");
        assert_eq!(fmt_delta_pct(18900.0), "+18900%");
    }
}
