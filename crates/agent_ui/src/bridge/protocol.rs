//! Bridge wire types. The `/sse` stream emits `event: X` + `data: {"type":"X",…}`
//! frames; the event type is INSIDE the JSON payload (contract pinned in
//! COMPLEMENTARY_SOLUTIONS verdict A), so only the data line is parsed and a
//! single `#[serde(tag = "type")]` enum types the whole feed. Everything is
//! liberal in what it accepts: fields default so bridge-side schema drift
//! never breaks the panels.

use serde::Deserialize;

/// One run row from `GET /board` or an SSE `board` event.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct RunRow {
    #[serde(default)]
    pub run_id: String,
    #[serde(default)]
    pub conv: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub elapsed_s: f64,
    #[serde(default)]
    pub task: Option<String>,
    #[serde(default)]
    pub agent: String,
    #[serde(default)]
    pub chip: Option<String>,
}

/// One pool row extracted from the usage object
/// (`pool-name -> { headroom_pct, window }`; `_`-prefixed keys are metadata).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PoolRow {
    pub name: String,
    pub headroom_pct: Option<f64>,
    pub window: Option<String>,
}

/// A typed event off the `/sse` stream. The bridge writes the discriminant
/// both as the SSE `event:` field and as `"type"` inside the data payload —
/// we parse the payload only.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum BridgeEvent {
    Board {
        #[serde(default)]
        board: Vec<RunRow>,
    },
    /// The usage payload carries arbitrary pool keys at the top level next to
    /// `"type"` — flatten captures them all; [`pools_from_object`] shapes them.
    Usage {
        #[serde(flatten)]
        fields: serde_json::Map<String, serde_json::Value>,
    },
    /// Forward compat: unknown event types deserialize (and are dropped by
    /// the client) instead of erroring the stream.
    #[serde(other)]
    Unknown,
}

/// Shape a usage JSON object (from `GET /usage` or a flattened `usage` event)
/// into sorted pool rows, skipping `_`-prefixed metadata keys.
pub fn pools_from_object(object: &serde_json::Map<String, serde_json::Value>) -> Vec<PoolRow> {
    let mut rows = Vec::new();
    for (name, entry) in object {
        if name.starts_with('_') {
            continue; // bridge metadata keys
        }
        rows.push(PoolRow {
            name: name.clone(),
            headroom_pct: entry.get("headroom_pct").and_then(|v| v.as_f64()),
            window: entry
                .get("window")
                .and_then(|v| v.as_str())
                .map(str::to_string),
        });
    }
    // Deterministic render order regardless of JSON map ordering.
    rows.sort_by(|a, b| a.name.cmp(&b.name));
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn board_event_tag_deserializes() {
        // Fixture truth: live `/sse` frame shape, 2026-06-12.
        let event: BridgeEvent = serde_json::from_str(
            r#"{"type": "board", "board": [
                {"run_id": "r-123", "conv": "conv-9", "status": "running",
                 "elapsed_s": 42.5, "task": "port wave Z1", "agent": "claude",
                 "chip": "skc3", "unknown_future_field": true},
                {"run_id": "r-456", "conv": "conv-2", "status": "completed",
                 "elapsed_s": 901, "agent": "codex"}
            ]}"#,
        )
        .unwrap();
        let BridgeEvent::Board { board } = event else {
            panic!("expected Board, got {event:?}");
        };
        assert_eq!(board.len(), 2);
        assert_eq!(board[0].run_id, "r-123");
        assert_eq!(board[0].status, "running");
        assert_eq!(board[0].elapsed_s, 42.5);
        assert_eq!(board[0].task.as_deref(), Some("port wave Z1"));
        // Optional fields absent are tolerated.
        assert_eq!(board[1].task, None);
        assert_eq!(board[1].chip, None);
    }

    #[test]
    fn board_event_missing_key_is_empty() {
        let event: BridgeEvent = serde_json::from_str(r#"{"type": "board"}"#).unwrap();
        let BridgeEvent::Board { board } = event else {
            panic!("expected Board");
        };
        assert!(board.is_empty());
    }

    #[test]
    fn usage_event_tag_deserializes_and_shapes_pools() {
        // Trimmed from the live `/sse` usage frame, 2026-06-12.
        let event: BridgeEvent = serde_json::from_str(
            r#"{"type": "usage",
                "claude_sdk_credit": {"used_in_window": 7, "window": "month",
                                      "soft_cap": 2000, "headroom_pct": 100},
                "codex_plan": {"headroom_pct": null},
                "gemini_free_rpd": {"headroom_pct": 99.5, "window": "day"},
                "_source": "self-metering + vendor-page scrape",
                "_scraped": {"stale": true, "age_h": 36.2}}"#,
        )
        .unwrap();
        let BridgeEvent::Usage { fields } = event else {
            panic!("expected Usage, got {event:?}");
        };
        let pools = pools_from_object(&fields);
        // Sorted by name, "_"-metadata filtered, "type" consumed by the tag.
        assert_eq!(pools.len(), 3);
        assert_eq!(pools[0].name, "claude_sdk_credit");
        assert_eq!(pools[0].headroom_pct, Some(100.0));
        assert_eq!(pools[0].window.as_deref(), Some("month"));
        assert_eq!(pools[1].name, "codex_plan");
        assert_eq!(pools[1].headroom_pct, None); // explicit null
        assert_eq!(pools[1].window, None); // missing key
        assert_eq!(pools[2].name, "gemini_free_rpd");
        assert_eq!(pools[2].headroom_pct, Some(99.5));
    }

    #[test]
    fn unknown_event_type_is_tolerated() {
        let event: BridgeEvent =
            serde_json::from_str(r#"{"type": "transcript", "lines": []}"#).unwrap();
        assert!(matches!(event, BridgeEvent::Unknown));
    }
}
