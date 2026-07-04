//! Tests for the settings status panel's pure label helpers.

use super::*;
use crate::bridge::ScrapeMeta;

#[test]
fn transport_labels_cover_every_variant() {
    assert_eq!(transport_label(Transport::Sse), "SSE push");
    assert_eq!(transport_label(Transport::Polling), "polling fallback");
    assert_eq!(transport_label(Transport::None), "—");
}

#[test]
fn freshness_degrades_visibly_never_vanishes() {
    // §4.4 lesson applied to §4.6: unknown/stale stays readable.
    assert_eq!(freshness_line(&UsageMeta::default()), "self-metered");
    let fresh = UsageMeta {
        source: None,
        scraped: Some(ScrapeMeta {
            stale: false,
            age_h: None,
            age_min: Some(12.0),
            reset_phrase: Some("14:32".to_string()),
        }),
        ..Default::default()
    };
    assert_eq!(freshness_line(&fresh), "scraped 12m ago");
    let stale = UsageMeta {
        source: None,
        scraped: Some(ScrapeMeta {
            stale: true,
            age_h: Some(36.2),
            age_min: None,
            reset_phrase: None,
        }),
        ..Default::default()
    };
    assert_eq!(freshness_line(&stale), "stale — scraped 36.2h ago");
}
