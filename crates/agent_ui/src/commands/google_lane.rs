//! The bridge's S5 google command lane, folded into the fork registry
//! (parity gap #9, `COMMAND_SURFACE_PARITY.md` §4).
//!
//! The bridge serves the gemini + agy rows (`bridge/features/commands/`):
//!
//! - `GET /commands/help` → `{"rows": […], "markdown", "counts"}` — the FULL
//!   documented lane: live passthrough/polyfill rows, honest-unbuilt rows
//!   (`"disabled": true` + `"tooltip"`), and N/A-by-design rows with their
//!   reasons. This module fetches THIS endpoint (not the menu-filtered
//!   `GET /commands?vendor=`) because the fork's own [`super::registry`]
//!   fold already applies the same law (N/A never in the menu, disabled rows
//!   stay greyed) — one fetch feeds both the typeahead and `/help`.
//! - `POST /commands/<vendor>/<name>` `{args?}` — execution, user-invoked
//!   only. Honest errors: 404 unknown vendor/command · 405 N/A-by-design
//!   (reason) · 501 polyfill-not-built (tooltip) · 502 relay failure — all
//!   as `{"error": …}` bodies the fork renders VERBATIM
//!   ([`crate::bridge::post_json`]'s error-body law). Fired from
//!   `orchestrator_panel::command_note`, never from here.
//!
//! Parsing is PURE (serde over the wire shape, unit-tested on fixtures);
//! the background fetch lives in [`super::registry::CommandRegistry`].
//! Rows the fork can't place (an unknown vendor from a newer bridge) are
//! skipped — honest degrade, never a panic.

use gpui::SharedString;

use super::types::{Classification, CommandEntry, CommandKind, Mechanism, Vendor};

/// One row of the bridge's `GET /commands/help` response (`types.py
/// CommandEntry.row()`).
#[derive(Debug, Default, serde::Deserialize)]
struct WireRow {
    #[serde(default)]
    name: String,
    #[serde(default)]
    vendor: String,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    classification: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    disabled: bool,
    #[serde(default)]
    mechanism: WireMechanism,
    /// Polyfill module name (the law: the row names its polyfill).
    #[serde(default)]
    module: String,
    /// N/A reason / unbuilt tooltip source.
    #[serde(default)]
    reason: String,
    /// The disabled row's hover text (bridge `disabled_tooltip()`).
    #[serde(default)]
    tooltip: String,
}

#[derive(Debug, Default, serde::Deserialize)]
struct WireMechanism {
    #[serde(default)]
    kind: String,
}

#[derive(Debug, Default, serde::Deserialize)]
struct WireResponse {
    #[serde(default)]
    rows: Vec<WireRow>,
}

/// Parse the bridge's `/commands/help` body into fork registry rows.
/// Undecodable JSON → empty (the caller keeps the static offline stubs —
/// never a silent half-lane).
pub fn parse_rows(raw: &str) -> Vec<CommandEntry> {
    let Ok(response) = serde_json::from_str::<WireResponse>(raw) else {
        return Vec::new();
    };
    response.rows.into_iter().filter_map(entry).collect()
}

/// Map one wire row. `None` when the row can't be placed truthfully
/// (no name, or a vendor this fork doesn't know).
fn entry(row: WireRow) -> Option<CommandEntry> {
    if row.name.is_empty() {
        return None;
    }
    let vendor = match Vendor::from_word(&row.vendor) {
        Some(vendor @ (Vendor::Gemini | Vendor::Agy)) => vendor,
        // A non-google vendor on the GOOGLE lane endpoint would be a bridge
        // bug — refuse the row rather than mislabel it.
        _ => return None,
    };
    let kind = match row.kind.as_str() {
        "custom" => CommandKind::Custom,
        _ => CommandKind::Builtin,
    };
    let classification = match row.classification.as_str() {
        "passthrough" => Classification::Passthrough,
        "polyfill" => Classification::polyfill(non_empty(&row.module, &row.reason)),
        "na-by-design" => Classification::na(non_empty(&row.reason, &row.description)),
        _ => return None,
    };
    let mechanism = if row.disabled {
        // Bridge-side honest-unbuilt (and N/A) rows: greyed-with-tooltip /
        // help-only, exactly as the bridge classified them.
        Mechanism::unbuilt(non_empty(&row.tooltip, &row.reason))
    } else {
        Mechanism::GoogleExec {
            mech: SharedString::from(row.mechanism.kind),
        }
    };
    Some(CommandEntry {
        name: row.name.into(),
        vendor: Some(vendor),
        kind,
        classification,
        mechanism,
        description: row.description.into(),
    })
}

/// First non-empty of two wire fields, with a last-resort honest fallback.
fn non_empty(first: &str, second: &str) -> SharedString {
    if !first.is_empty() {
        first.to_string().into()
    } else if !second.is_empty() {
        second.to_string().into()
    } else {
        "not built yet".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A trimmed real `/commands/help` shape: one live passthrough, one live
    /// polyfill, one honest-unbuilt, one N/A-by-design, one custom expansion.
    const FIXTURE: &str = r#"{
        "rows": [
            {"name": "models", "slash": "/models", "vendor": "agy",
             "kind": "builtin", "classification": "passthrough",
             "mechanism": {"kind": "agy-subcommand", "relay": "models"},
             "description": "List agy's available models.",
             "badge": "agy", "disabled": false},
            {"name": "stats", "slash": "/stats", "vendor": "gemini",
             "kind": "builtin", "classification": "polyfill",
             "mechanism": {"kind": "orch-endpoint", "target": "/usage"},
             "description": "Session/model usage stats.",
             "badge": "gemini", "disabled": false,
             "module": "usage console (GET /usage)"},
            {"name": "goal", "slash": "/goal", "vendor": "agy",
             "kind": "builtin", "classification": "polyfill",
             "mechanism": {"kind": "unbuilt", "reason": "Goal-mode polyfill not built."},
             "description": "Long-running goal mode.",
             "badge": "agy", "disabled": true,
             "module": "goal-mode (AFK run)",
             "reason": "Goal-mode polyfill not built.",
             "tooltip": "Goal-mode polyfill not built."},
            {"name": "credits", "slash": "/credits", "vendor": "agy",
             "kind": "builtin", "classification": "na-by-design",
             "mechanism": {"kind": "unbuilt", "reason": "Vendor billing."},
             "description": "G1-credit panel.",
             "badge": "agy", "disabled": true,
             "reason": "Vendor billing."},
            {"name": "ship-it", "slash": "/ship-it", "vendor": "gemini",
             "kind": "custom", "classification": "passthrough",
             "mechanism": {"kind": "expand-template"},
             "description": "Custom command (ship-it.toml).",
             "badge": "custom", "disabled": false,
             "source": "~/.gemini/commands/ship-it.toml"}
        ],
        "markdown": "…", "counts": {"total": 5}
    }"#;

    #[test]
    fn live_rows_carry_google_exec_with_the_bridge_mech_kind() {
        let rows = parse_rows(FIXTURE);
        let models = rows.iter().find(|r| r.name == "models").unwrap();
        assert_eq!(models.vendor, Some(Vendor::Agy));
        assert_eq!(models.kind, CommandKind::Builtin);
        assert_eq!(models.classification, Classification::Passthrough);
        assert!(!models.is_disabled(), "a live bridge row is enabled");
        assert_eq!(
            models.mechanism,
            Mechanism::GoogleExec { mech: "agy-subcommand".into() }
        );

        let stats = rows.iter().find(|r| r.name == "stats").unwrap();
        assert_eq!(stats.vendor, Some(Vendor::Gemini));
        assert_eq!(
            stats.classification,
            Classification::polyfill("usage console (GET /usage)"),
            "the polyfill row NAMES its module (the law)"
        );
        assert!(matches!(stats.mechanism, Mechanism::GoogleExec { .. }));
    }

    #[test]
    fn unbuilt_rows_stay_disabled_with_the_bridge_tooltip() {
        let rows = parse_rows(FIXTURE);
        let goal = rows.iter().find(|r| r.name == "goal").unwrap();
        assert!(goal.is_disabled(), "bridge-unbuilt renders greyed, never live");
        assert_eq!(goal.disabled_tooltip(), Some("Goal-mode polyfill not built."));
    }

    #[test]
    fn na_rows_carry_the_reason_and_never_reach_the_menu() {
        let rows = parse_rows(FIXTURE);
        let credits = rows.iter().find(|r| r.name == "credits").unwrap();
        assert_eq!(credits.classification, Classification::na("Vendor billing."));
        // The registry fold excludes NaByDesign from visible() — prove the
        // parsed row hits that arm.
        let visible = crate::commands::registry::visible(&rows, "", Some(Vendor::Agy));
        assert!(!visible.iter().any(|r| r.name == "credits"));
        // …while the live + unbuilt rows DO show under the google force
        // (either vendor token joins both — one lane).
        assert!(visible.iter().any(|r| r.name == "models"));
        assert!(visible.iter().any(|r| r.name == "goal"));
        assert!(visible.iter().any(|r| r.name == "stats"));
    }

    #[test]
    fn custom_rows_keep_their_kind() {
        let rows = parse_rows(FIXTURE);
        let custom = rows.iter().find(|r| r.name == "ship-it").unwrap();
        assert_eq!(custom.kind, CommandKind::Custom);
        assert_eq!(custom.source_badge(), Some("gemini"));
    }

    #[test]
    fn unplaceable_rows_are_skipped_not_mislabeled() {
        let raw = r#"{"rows": [
            {"name": "x", "vendor": "claude", "kind": "builtin",
             "classification": "passthrough", "mechanism": {"kind": "relay"},
             "description": "wrong lane", "disabled": false},
            {"name": "", "vendor": "agy", "kind": "builtin",
             "classification": "passthrough", "mechanism": {"kind": "relay"},
             "description": "no name", "disabled": false},
            {"name": "y", "vendor": "agy", "kind": "builtin",
             "classification": "quantum", "mechanism": {"kind": "relay"},
             "description": "unknown classification", "disabled": false}
        ]}"#;
        assert!(parse_rows(raw).is_empty());
    }

    #[test]
    fn undecodable_body_folds_to_empty() {
        assert!(parse_rows("not json").is_empty());
        assert!(parse_rows("42").is_empty());
        assert!(parse_rows("{}").is_empty());
    }
}
