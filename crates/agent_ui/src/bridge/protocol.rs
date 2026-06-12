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

/// Scrape metadata off the usage object's `_scraped` key.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ScrapeMeta {
    /// The scrape outlived its freshness window — meters degrade to
    /// "unknown/stale", never silently vanish (PARITY_SPEC §4.4).
    pub stale: bool,
    pub age_h: Option<f64>,
    pub age_min: Option<f64>,
    /// First vendor reset phrase ("resets 14:32"). `None` when stale or
    /// absent — mirrors the web's `resetPhrase()` (usage-island.js), which
    /// refuses a reset time it can no longer trust.
    pub reset_phrase: Option<String>,
}

/// The `_`-prefixed metadata keys that ride next to the pools in every
/// usage payload.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UsageMeta {
    /// `_source` — "self-metering + vendor-page scrape".
    pub source: Option<String>,
    /// `_scraped` — present only when a vendor-page scrape feeds the pools.
    pub scraped: Option<ScrapeMeta>,
}

impl UsageMeta {
    /// Whether the scrape (if any) has gone stale.
    pub fn stale(&self) -> bool {
        self.scraped.as_ref().is_some_and(|scrape| scrape.stale)
    }
}

/// Extract [`UsageMeta`] from a usage JSON object. Liberal like the rest of
/// the protocol: missing/odd-shaped metadata degrades to defaults.
pub fn usage_meta_from_object(object: &serde_json::Map<String, serde_json::Value>) -> UsageMeta {
    let source = object
        .get("_source")
        .and_then(|value| value.as_str())
        .map(str::to_string);
    let scraped = object
        .get("_scraped")
        .and_then(|value| value.as_object())
        .map(|scraped| {
            let stale = scraped
                .get("stale")
                .and_then(|value| value.as_bool())
                .unwrap_or(false);
            ScrapeMeta {
                stale,
                age_h: scraped.get("age_h").and_then(|value| value.as_f64()),
                age_min: scraped.get("age_min").and_then(|value| value.as_f64()),
                reset_phrase: if stale {
                    None
                } else {
                    first_reset_phrase(scraped)
                },
            }
        });
    UsageMeta { source, scraped }
}

/// `resetPhrase()` from usage-island.js — the first non-empty
/// `reset_phrases[0]` across `_scraped.vendors`. (The workspace builds
/// serde_json with `preserve_order`, so vendor iteration matches JS
/// `Object.values` insertion order exactly.)
fn first_reset_phrase(scraped: &serde_json::Map<String, serde_json::Value>) -> Option<String> {
    let vendors = scraped.get("vendors")?.as_object()?;
    for vendor in vendors.values() {
        if let Some(phrase) = vendor
            .get("reset_phrases")
            .and_then(|phrases| phrases.as_array())
            .and_then(|phrases| phrases.first())
            .and_then(|phrase| phrase.as_str())
        {
            return Some(phrase.to_string());
        }
    }
    None
}

/// Shape a usage JSON object (from `GET /usage` or a flattened `usage` event)
/// into pool rows in WIRE ORDER, skipping `_`-prefixed metadata keys. The
/// workspace builds serde_json with `preserve_order`, so iteration matches
/// the server's insertion order (orchestrator/usage.py: claude, codex, agy,
/// gemini) exactly as JS `Object.entries` does — dots, card rows, and panel
/// cards all render in the approved order.
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
        // Wire order preserved, "_"-metadata filtered, "type" consumed by
        // the tag.
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
    fn pools_render_in_wire_order_not_alphabetical() {
        // The JSON order authority is the server's insertion order
        // (orchestrator/usage.py: claude, codex, agy, gemini — NOT
        // alphabetical, which would put antigravity first). preserve_order
        // carries it through, exactly like Python json → JS Object.entries.
        let event: BridgeEvent = serde_json::from_str(
            r#"{"type": "usage",
                "claude_sdk_credit": {"headroom_pct": 18},
                "codex_plan": {"headroom_pct": 50},
                "antigravity_weekly": {"headroom_pct": 80},
                "gemini_free_rpd": {"headroom_pct": 99}}"#,
        )
        .unwrap();
        let BridgeEvent::Usage { fields } = event else {
            panic!("expected Usage");
        };
        let pools = pools_from_object(&fields);
        let names: Vec<&str> = pools.iter().map(|pool| pool.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "claude_sdk_credit",
                "codex_plan",
                "antigravity_weekly",
                "gemini_free_rpd"
            ],
            "pool order must match the wire, not a sort"
        );
    }

    #[test]
    fn unknown_event_type_is_tolerated() {
        let event: BridgeEvent =
            serde_json::from_str(r#"{"type": "transcript", "lines": []}"#).unwrap();
        assert!(matches!(event, BridgeEvent::Unknown));
    }

    fn usage_object(json: &str) -> serde_json::Map<String, serde_json::Value> {
        serde_json::from_str::<serde_json::Value>(json)
            .unwrap()
            .as_object()
            .unwrap()
            .clone()
    }

    #[test]
    fn usage_meta_extracts_source_and_fresh_scrape() {
        let meta = usage_meta_from_object(&usage_object(
            r#"{
                "_source": "self-metering + vendor-page scrape",
                "_scraped": {
                    "stale": false, "age_min": 12.0,
                    "vendors": {
                        "claude": {"reset_phrases": ["14:32"]},
                        "codex": {"reset_phrases": []}
                    }
                },
                "claude_sdk_credit": {"headroom_pct": 18}
            }"#,
        ));
        assert_eq!(
            meta.source.as_deref(),
            Some("self-metering + vendor-page scrape")
        );
        assert!(!meta.stale());
        let scraped = meta.scraped.unwrap();
        assert_eq!(scraped.age_min, Some(12.0));
        assert_eq!(scraped.reset_phrase.as_deref(), Some("14:32"));
    }

    #[test]
    fn usage_meta_stale_scrape_withholds_reset_phrase() {
        // resetPhrase() returns null when stale — a reset time it can no
        // longer trust is worse than none.
        let meta = usage_meta_from_object(&usage_object(
            r#"{"_scraped": {"stale": true, "age_h": 36.2,
                "vendors": {"claude": {"reset_phrases": ["14:32"]}}}}"#,
        ));
        assert!(meta.stale());
        let scraped = meta.scraped.unwrap();
        assert_eq!(scraped.age_h, Some(36.2));
        assert_eq!(scraped.reset_phrase, None);
    }

    #[test]
    fn usage_meta_degrades_on_missing_or_odd_metadata() {
        let meta = usage_meta_from_object(&usage_object(
            r#"{"claude_sdk_credit": {"headroom_pct": 50}}"#,
        ));
        assert_eq!(meta, UsageMeta::default());
        assert!(!meta.stale());
        // Wrong-typed metadata degrades instead of erroring.
        let meta = usage_meta_from_object(&usage_object(
            r#"{"_source": 7, "_scraped": "yes"}"#,
        ));
        assert_eq!(meta, UsageMeta::default());
    }
}
