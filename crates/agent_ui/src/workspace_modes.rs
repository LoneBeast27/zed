//! Canonical workspace modes — activity-bar use-case switcher.
//!
//! Loads mode definitions from `.agents/modes/*.json` per the spec at
//! `.planning/zed-fork/WORKSPACE_MODES.md`. Each mode is a top-level
//! workspace configuration (Orchestrator, Symphony, Settings, ...) that
//! reconfigures the dock layout to suit that use case.
//!
//! M0 (this module): definition schema + filesystem loader. No GPUI
//! integration yet — that's M1 (activity bar render) + M2 (switcher).

use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

/// A workspace mode definition, loaded from `.agents/modes/{id}.json`.
#[derive(Debug, Deserialize, Clone)]
pub struct WorkspaceMode {
    pub schema_version: u32,
    pub id: String,
    pub display_name: String,
    pub description: String,
    /// Icon name. Should match a Zed `IconName` enum variant for built-in
    /// icons; SVG path support is forward-compat (M0+).
    pub icon: String,
    /// Brand accent color as 6-digit hex (no leading `#`). Drives the
    /// active-mode indicator and any per-mode chrome tints.
    pub accent_color_hex: String,
    pub layout: HashMap<String, LayoutSpec>,
    #[serde(default)]
    pub default_keybinding: Option<String>,
    #[serde(default)]
    pub pinned_position: Option<PinnedPosition>,
    /// URL opened in the system default browser when switching to this mode
    /// (M2). Used by web-surface modes (e.g. `browser` → the task-board /
    /// scraper twin at `http://127.0.0.1:4530`).
    #[serde(default)]
    pub open_url: Option<String>,
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PinnedPosition {
    Top,
    Bottom,
}

/// Per-surface layout spec inside a mode definition. Keys (Amendment
/// 2026-07-04 (2)): `center` (the mode's primary surface, opened as a
/// center-pane workspace item — `size_px`/`fill` ignored, full-bleed),
/// `right_dock` / `bottom_dock` (dock panels, e.g. the constellation).
/// `left_dock` is legacy: a spec naming a center surface is promoted to
/// `center`; modes otherwise leave the left dock under user control.
#[derive(Debug, Deserialize, Clone)]
pub struct LayoutSpec {
    #[serde(default)]
    pub panel: Option<String>,
    #[serde(default)]
    pub size_px: Option<u32>,
    #[serde(default)]
    pub fill: bool,
    #[serde(default = "default_true")]
    pub visible: bool,
}

fn default_true() -> bool {
    true
}

/// Parse every `*.json` (excluding `_*.json`) in `dir` as a `WorkspaceMode`.
/// Returns modes sorted by declared Ctrl+Alt default keybinding first, then by
/// pinned position: `Top` modes first, then unpinned alphabetical, then
/// `Bottom` modes last.
///
/// Files that fail to parse log a warning and are skipped — partial success
/// rather than total failure, so a single malformed mode doesn't break the
/// whole activity bar.
pub fn load_modes_from_dir(dir: &Path) -> Vec<WorkspaceMode> {
    let mut modes = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(err) => {
            log::warn!(
                "workspace_modes: cannot read dir {}: {}",
                dir.display(),
                err
            );
            return modes;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        // Skip underscore-prefixed files (e.g., `_state/` snapshots, future
        // internal-use files).
        if path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.starts_with('_'))
            .unwrap_or(false)
        {
            continue;
        }
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(err) => {
                log::warn!(
                    "workspace_modes: cannot read {}: {}",
                    path.display(),
                    err
                );
                continue;
            }
        };
        match serde_json::from_str::<WorkspaceMode>(&text) {
            Ok(mode) => {
                log::debug!(
                    "workspace_modes: loaded {} ({})",
                    mode.id,
                    mode.display_name
                );
                modes.push(mode);
            }
            Err(err) => log::warn!(
                "workspace_modes: parse error in {}: {}",
                path.display(),
                err
            ),
        }
    }
    sort_modes(&mut modes);
    modes
}

/// Sort modes for activity-bar display order: declared Ctrl+Alt keybinding
/// order first, then pinned top first (alphabetical within the group), then
/// unpinned alphabetical, then pinned bottom last.
fn sort_modes(modes: &mut [WorkspaceMode]) {
    use std::cmp::Ordering;
    modes.sort_by(|a, b| {
        let bucket = |m: &WorkspaceMode| match m.pinned_position {
            Some(PinnedPosition::Top) => 0u8,
            None => 1,
            Some(PinnedPosition::Bottom) => 2,
        };
        match default_keybinding_number(a).cmp(&default_keybinding_number(b)) {
            Ordering::Equal => match bucket(a).cmp(&bucket(b)) {
                Ordering::Equal => a.id.cmp(&b.id),
                other => other,
            },
            other => other,
        }
    });
}

fn default_keybinding_number(mode: &WorkspaceMode) -> Option<u8> {
    let binding = mode.default_keybinding.as_deref()?.to_ascii_lowercase();
    let mut parts = binding.split(['+', '-']);
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some("ctrl"), Some("alt"), Some(number), None) => number.parse().ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_mode(id: &str, pin: Option<PinnedPosition>) -> WorkspaceMode {
        WorkspaceMode {
            schema_version: 1,
            id: id.into(),
            display_name: id.into(),
            description: String::new(),
            icon: "Hub".into(),
            accent_color_hex: "DA7756".into(),
            layout: HashMap::new(),
            default_keybinding: None,
            pinned_position: pin,
            open_url: None,
        }
    }

    fn make_keybound_mode(id: &str, default_keybinding: &str) -> WorkspaceMode {
        let mut mode = make_mode(id, None);
        mode.default_keybinding = Some(default_keybinding.into());
        mode
    }

    #[test]
    fn sort_respects_pinned_position() {
        let mut modes = vec![
            make_mode("zeta", Some(PinnedPosition::Bottom)),
            make_mode("beta", None),
            make_mode("alpha", Some(PinnedPosition::Top)),
            make_mode("gamma", None),
            make_mode("settings", Some(PinnedPosition::Bottom)),
        ];
        sort_modes(&mut modes);
        let ids: Vec<&str> = modes.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["alpha", "beta", "gamma", "settings", "zeta"]);
    }

    #[test]
    fn sort_matches_declared_default_keybinding_positions() {
        let mut modes = vec![
            make_keybound_mode("orchestrator", "Ctrl+Alt+1"),
            make_keybound_mode("adversary", "Ctrl+Alt+5"),
            make_keybound_mode("symphony", "Ctrl+Alt+3"),
            make_keybound_mode("taskboard", "Ctrl+Alt+2"),
            make_keybound_mode("usage", "Ctrl+Alt+4"),
            make_keybound_mode("settings", "Ctrl+Alt+6"),
        ];
        sort_modes(&mut modes);

        let ids: Vec<&str> = modes.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(
            ids,
            vec![
                "orchestrator",
                "taskboard",
                "symphony",
                "usage",
                "adversary",
                "settings"
            ]
        );
        for (index, mode) in modes.iter().enumerate() {
            assert_eq!(
                mode.default_keybinding.as_deref(),
                Some(format!("Ctrl+Alt+{}", index + 1).as_str())
            );
        }
    }

    #[test]
    fn parses_orchestrator_shaped_json() {
        let json = r#"{
            "schema_version": 1,
            "id": "orchestrator",
            "display_name": "Orchestrator",
            "description": "Multi-agent chat",
            "icon": "Hub",
            "accent_color_hex": "DA7756",
            "layout": {
                "left_dock": { "panel": "AgentPanel", "size_px": 280, "visible": true },
                "center_dock": { "panel": "ConversationView", "fill": true }
            },
            "default_keybinding": "Ctrl+Alt+1",
            "pinned_position": "top"
        }"#;
        let mode: WorkspaceMode = serde_json::from_str(json).unwrap();
        assert_eq!(mode.id, "orchestrator");
        assert_eq!(mode.pinned_position, Some(PinnedPosition::Top));
        assert_eq!(mode.layout.get("left_dock").unwrap().size_px, Some(280));
        assert!(mode.layout.get("center_dock").unwrap().fill);
        // visible defaults to true when missing
        let layout = mode.layout.get("center_dock").unwrap();
        assert!(layout.visible);
        // open_url defaults to None when absent.
        assert_eq!(mode.open_url, None);
    }

    #[test]
    fn parses_center_primary_layout() {
        // Amendment 2026-07-04 (2): the shipped mode files now put their
        // primary under `center` (+ optional right_dock, e.g. the
        // orchestrator's constellation).
        let json = r#"{
            "schema_version": 1,
            "id": "orchestrator",
            "display_name": "Orchestrator",
            "description": "The orchestrator conversation",
            "icon": "Chat",
            "accent_color_hex": "d97757",
            "layout": {
                "center": { "panel": "orchestrator" },
                "right_dock": { "panel": "constellation", "size_px": 480 }
            }
        }"#;
        let mode: WorkspaceMode = serde_json::from_str(json).unwrap();
        let center = mode.layout.get("center").unwrap();
        assert_eq!(center.panel.as_deref(), Some("orchestrator"));
        assert!(center.visible);
        assert_eq!(
            mode.layout.get("right_dock").unwrap().size_px,
            Some(480)
        );
    }

    #[test]
    fn parses_open_url_field() {
        // The `browser` mode (PRD_V2 §6) carries an `open_url` that the M2
        // switcher opens in the system default browser on mode switch.
        let json = r#"{
            "schema_version": 1,
            "id": "browser",
            "display_name": "Browser",
            "description": "Web surface",
            "icon": "ToolWeb",
            "accent_color_hex": "4285F4",
            "layout": {},
            "open_url": "http://127.0.0.1:4530"
        }"#;
        let mode: WorkspaceMode = serde_json::from_str(json).unwrap();
        assert_eq!(mode.id, "browser");
        assert_eq!(mode.open_url.as_deref(), Some("http://127.0.0.1:4530"));
    }

    #[test]
    fn unknown_fields_are_silently_accepted_for_forward_compat() {
        // serde Deserialize default behavior ignores unknown fields,
        // which is what we want for forward-compat with future schema
        // versions that may add fields.
        let json = r#"{
            "schema_version": 999,
            "id": "futuristic",
            "display_name": "From The Future",
            "description": "Has fields we don't know about",
            "icon": "Hub",
            "accent_color_hex": "DA7756",
            "layout": {},
            "future_field": "should be ignored",
            "another_future_field": { "nested": true }
        }"#;
        let mode: WorkspaceMode = serde_json::from_str(json).unwrap();
        assert_eq!(mode.id, "futuristic");
        assert_eq!(mode.schema_version, 999);
    }
}
