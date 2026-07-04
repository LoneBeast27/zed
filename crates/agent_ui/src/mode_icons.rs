//! Mode-icon resolution for the dock pill (PARITY_SPEC §3).
//!
//! Mode files (`.agents/modes/*.json`) name icons in a Material-Symbols-ish
//! PascalCase vocabulary ("Chat", "ListTodo", "AudioOn", …). The fork's
//! `IconName` enum is `EnumString` over snake_case, so resolution is:
//! camel→snake, exact `FromStr` lookup, then a closest-real-variant table
//! for known vocabulary drift, then (logged) the generic fallback. The six
//! shipped mode icons all resolve exactly — the table exists so a future
//! mode file using a web-side icon name degrades to the nearest in-tree
//! glyph instead of the anonymous fallback.

use std::str::FromStr;

use ui::IconName;

/// Last-resort icon when a mode's `icon` string matches neither a real
/// `IconName` variant nor the closest-variant table. `ZedAgent` is the
/// closest in-tree analog to the spec's "Hub" glyph.
pub(crate) const FALLBACK_ICON: IconName = IconName::ZedAgent;

/// Closest-real-variant table (snake_case keys, post-`camel_to_snake`):
/// Material-Symbols names the web surface uses that have no exact `IconName`
/// twin. Consulted only after the exact lookup misses.
const CLOSEST_VARIANTS: &[(&str, IconName)] = &[
    ("hub", IconName::Blocks),
    ("message_circle", IconName::Chat),
    ("message_square", IconName::Chat),
    ("forum", IconName::Chat),
    ("list_checks", IconName::ListTodo),
    ("check_square", IconName::ListTodo),
    ("checklist", IconName::ListTodo),
    ("audio_lines", IconName::AudioOn),
    ("volume_up", IconName::AudioOn),
    ("graphic_eq", IconName::AudioOn),
    ("sliders_horizontal", IconName::Sliders),
    ("sliders_vertical", IconName::Sliders),
    ("tune", IconName::Sliders),
    ("gear", IconName::Settings),
    ("cog", IconName::Settings),
    ("users", IconName::UserGroup),
    ("people", IconName::UserGroup),
    ("group", IconName::UserGroup),
    ("user", IconName::Person),
];

/// Map a mode's `icon` JSON field to an `IconName` variant.
///
/// The `IconName` enum is `EnumString` with `serialize_all = "snake_case"`,
/// so we lowercase + camel-to-snake the input before lookup. Misses walk the
/// [`CLOSEST_VARIANTS`] table; only a table miss logs and falls back.
pub(crate) fn icon_name_for(spec: &str) -> IconName {
    let snake = camel_to_snake(spec);
    if let Ok(icon) = IconName::from_str(&snake) {
        return icon;
    }
    if let Some((_, icon)) = CLOSEST_VARIANTS.iter().find(|(key, _)| *key == snake) {
        log::info!(
            "mode_icons: icon '{spec}' (snake='{snake}') mapped to closest variant {icon:?}"
        );
        return *icon;
    }
    log::warn!(
        "mode_icons: icon '{spec}' (snake='{snake}') not a known IconName — using fallback"
    );
    FALLBACK_ICON
}

/// Convert a CamelCase or PascalCase string to snake_case. Idempotent on
/// already-snake input ("Hub" → "hub", "AiOpenAi" → "ai_open_ai", "settings"
/// → "settings").
fn camel_to_snake(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    for (i, ch) in s.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if i > 0 && !out.ends_with('_') {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camel_to_snake_handles_known_cases() {
        assert_eq!(camel_to_snake("Hub"), "hub");
        assert_eq!(camel_to_snake("Settings"), "settings");
        assert_eq!(camel_to_snake("ZedAgent"), "zed_agent");
        assert_eq!(camel_to_snake("AiOpenAi"), "ai_open_ai");
        // Already-snake input is preserved.
        assert_eq!(camel_to_snake("zed_agent"), "zed_agent");
    }

    #[test]
    fn every_shipped_mode_icon_resolves_to_a_real_variant() {
        // The six icons in `.agents/modes/*.json` — each MUST hit the exact
        // lookup (no table, no fallback). This is the §3 guard: a rename in
        // the IconName enum that orphans a mode icon fails here first.
        assert_eq!(icon_name_for("Chat"), IconName::Chat); // orchestrator
        assert_eq!(icon_name_for("ListTodo"), IconName::ListTodo); // taskboard
        assert_eq!(icon_name_for("AudioOn"), IconName::AudioOn); // symphony
        assert_eq!(icon_name_for("Sliders"), IconName::Sliders); // usage
        assert_eq!(icon_name_for("UserGroup"), IconName::UserGroup); // adversary
        assert_eq!(icon_name_for("Settings"), IconName::Settings); // settings
    }

    #[test]
    fn misses_walk_the_closest_variant_table_before_the_fallback() {
        // "Hub" is not a real IconName variant — the table maps it to the
        // nearest in-tree glyph instead of the anonymous fallback.
        assert_eq!(icon_name_for("Hub"), IconName::Blocks);
        assert_eq!(icon_name_for("MessageCircle"), IconName::Chat);
        assert_eq!(icon_name_for("SlidersHorizontal"), IconName::Sliders);
        assert_eq!(icon_name_for("Users"), IconName::UserGroup);
        // A genuinely unknown name still falls back (and logs).
        assert_eq!(icon_name_for("NotARealIconEver"), FALLBACK_ICON);
    }

    #[test]
    fn snake_cased_input_resolves_directly() {
        // Forward-compat for hand-authored mode files already in snake_case.
        assert_eq!(icon_name_for("zed_agent"), IconName::ZedAgent);
        assert_eq!(icon_name_for("settings"), IconName::Settings);
    }
}
