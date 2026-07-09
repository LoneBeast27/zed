//! Agent vendor accents + run-status colors (PARITY_SPEC §1 / RUST_PORT_NOTES §1).
//!
//! These are deliberately NOT `ThemeColors` fields: vendor identity and run-status
//! colors live outside chrome tokens (mirrors the web bridge keeping `--claude` etc.
//! out of the chrome token set). Chips, graph nodes, and meters consume them; the
//! single-accent chrome never does.

use gpui::{Hsla, Rgba};

/// Builds an [`Rgba`] from a `0xRRGGBBAA` literal in const context
/// (gpui's `rgba()` is not `const fn`).
pub(crate) const fn rgba_hex(hex: u32) -> Rgba {
    Rgba {
        r: ((hex >> 24) & 0xFF) as f32 / 255.0,
        g: ((hex >> 16) & 0xFF) as f32 / 255.0,
        b: ((hex >> 8) & 0xFF) as f32 / 255.0,
        a: (hex & 0xFF) as f32 / 255.0,
    }
}

// ── Vendor accents (chips / nodes / meters only — never chrome) ──
// 3-provider theming (user ruling 2026-07-08): orange = Anthropic,
// green = OpenAI, blue = Google. agy + gemini are the SAME provider, so
// both live in the blue family — agy light, gemini deeper — instead of
// gemini's old purple, which read as a phantom fourth provider.
pub const ACCENT_CLAUDE: Rgba = rgba_hex(0xd97757ff);
pub const ACCENT_CODEX: Rgba = rgba_hex(0x10a37fff);
pub const ACCENT_AGY: Rgba = rgba_hex(0x8ab4f8ff);
pub const ACCENT_GEMINI: Rgba = rgba_hex(0x4285f4ff);

/// `--accent-fill` — the filled-action surface (the composer's send circle).
/// MONO ruling 2026-07-06: light fill + dark glyph, no blue.
pub const ACCENT_FILL: Rgba = rgba_hex(0xe5e5e5ff);

// ── Run-status colors. MONO ruling: only safety signals keep hue (running/
//    blocked/error); done is a calm state and goes neutral — a board full of
//    done runs must not tint the app blue. ──
pub const STATUS_RUNNING: Rgba = rgba_hex(0x81c995ff);
pub const STATUS_BLOCKED: Rgba = rgba_hex(0xfdd663ff);
pub const STATUS_ERROR: Rgba = rgba_hex(0xf28b82ff);
pub const STATUS_DONE: Rgba = rgba_hex(0xd4d4d4ff);
/// `rgba(255, 255, 255, 0.45)` — the idle/neutral dot.
pub const STATUS_IDLE: Rgba = rgba_hex(0xffffff73);

/// `rgba(250, 249, 245, 0.85)` — the highlight the in-progress label shimmer
/// sweeps through its glyphs (web `chat.css` `.shimmer-label` gradient stop).
pub const SHIMMER_HIGHLIGHT: Rgba = rgba_hex(0xfaf9f5d9);

/// Vendor accent for an agent name (case-insensitive, substring-tolerant:
/// `"claude-fable"`, `"OpenAI Codex"`, `"gemini-3"` all resolve). Unknown
/// agents keep the neutral `.sw` default ([`TEXT_3`]) — NOT a vendor color,
/// so an unknown vendor's identity dot never impersonates a known one.
pub fn accent_for_agent(name: &str) -> Hsla {
    let name = name.to_ascii_lowercase();
    let rgba = if name.contains("claude") || name.contains("anthropic") {
        ACCENT_CLAUDE
    } else if name.contains("codex") || name.contains("openai") || name.contains("gpt") {
        ACCENT_CODEX
    } else if name.contains("gemini") {
        ACCENT_GEMINI
    } else if name.contains("agy") || name.contains("antigravity") {
        ACCENT_AGY
    } else {
        TEXT_3
    };
    rgba.into()
}

/// Constellation identity color (user ruling 2026-07-08): in the agent
/// constellation, HUE carries WHO (the 3-provider orange/green/blue) and
/// the checkpoint-dot SHAPE carries lifecycle (hollow/arc/solid). Safety
/// stays loud: failed/killed keep the error red regardless of vendor.
pub fn constellation_node_color(agent: &str, status: &str) -> Hsla {
    let status = status.to_ascii_lowercase();
    if matches!(status.as_str(), "error" | "failed" | "dead" | "killed") {
        return STATUS_ERROR.into();
    }
    accent_for_agent(agent)
}

/// `--accent` — the single chrome accent; usage meters in the OK band fill
/// with it ("accent → --blocked ≥75 → --error ≥90"). MONO ruling 2026-07-06:
/// bright grey-white, not blue.
pub const ACCENT: Rgba = rgba_hex(0xe5e5e5ff);
/// Atmosphere — the one fixed radial bloom behind the empty/greeting state.
/// MONO: a soft white bloom (was blue rgba(66,133,244,0.07)).
pub const GREET_BLOOM: Rgba = Rgba {
    r: 1.0,
    g: 1.0,
    b: 1.0,
    a: 0.05,
};
/// `--text-3: rgba(255,255,255,0.38)` — the unknown/stale meter tone.
pub const TEXT_3: Rgba = rgba_hex(0xffffff61);

/// Usage-meter tone (PARITY_SPEC §4.4 / §4.8): the status color band a
/// pool's used-% falls in. `toneFor()` from usage-island.js.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Ok,
    Warn,
    Crit,
    Unknown,
}

impl Tone {
    /// Meter-fill / dot / tinted-text color for this tone.
    pub fn color(self) -> Hsla {
        let rgba = match self {
            Tone::Ok => ACCENT,
            Tone::Warn => STATUS_BLOCKED,
            Tone::Crit => STATUS_ERROR,
            Tone::Unknown => TEXT_3,
        };
        rgba.into()
    }
}

/// Tone for a used-% (`None` = unmetered/unscraped pool → unknown, which
/// degrades visibly instead of vanishing — the Antigravity quota-opacity
/// lesson).
pub fn tone_for_used(used_pct: Option<f64>) -> Tone {
    match used_pct {
        None => Tone::Unknown,
        Some(used) if used >= 90.0 => Tone::Crit,
        Some(used) if used >= 75.0 => Tone::Warn,
        Some(_) => Tone::Ok,
    }
}

/// Used-% for a pool's headroom (`used = 100 − headroom`), the shared
/// ingest math of the usage panel and island.
pub fn used_pct(headroom_pct: Option<f64>) -> Option<f64> {
    headroom_pct.map(|headroom| 100.0 - headroom)
}

/// Status color for a bridge run status string (case-insensitive).
/// Unrecognized statuses render as idle/neutral.
pub fn color_for_status(status: &str) -> Hsla {
    let status = status.to_ascii_lowercase();
    let rgba = match status.as_str() {
        "running" | "active" | "working" => STATUS_RUNNING,
        "blocked" | "warn" | "warning" | "waiting" | "stalled" => STATUS_BLOCKED,
        "error" | "failed" | "dead" | "killed" => STATUS_ERROR,
        "done" | "complete" | "completed" | "success" | "merged" => STATUS_DONE,
        _ => STATUS_IDLE,
    };
    rgba.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vendor_accents_resolve_case_insensitively() {
        assert_eq!(accent_for_agent("Claude Fable 5"), ACCENT_CLAUDE.into());
        assert_eq!(accent_for_agent("CODEX"), ACCENT_CODEX.into());
        assert_eq!(accent_for_agent("gemini-3-pro"), ACCENT_GEMINI.into());
        assert_eq!(accent_for_agent("Antigravity"), ACCENT_AGY.into());
        // Unknown vendor keeps the web's neutral .sw default (--text-3) —
        // never a vendor color (ACCENT_AGY is also --accent/--done).
        assert_eq!(accent_for_agent("mystery-agent"), TEXT_3.into());
    }

    #[test]
    fn constellation_color_is_vendor_hue_except_safety_red() {
        // Identity by hue, lifecycle by shape — but failure is always red.
        assert_eq!(
            constellation_node_color("claude-fable", "running"),
            ACCENT_CLAUDE.into()
        );
        assert_eq!(
            constellation_node_color("codex", "completed"),
            ACCENT_CODEX.into()
        );
        assert_eq!(
            constellation_node_color("gemini-3", "pending"),
            ACCENT_GEMINI.into()
        );
        assert_eq!(
            constellation_node_color("claude", "failed"),
            STATUS_ERROR.into()
        );
        assert_eq!(
            constellation_node_color("agy", "killed"),
            STATUS_ERROR.into()
        );
    }

    #[test]
    fn status_colors_resolve() {
        assert_eq!(color_for_status("running"), STATUS_RUNNING.into());
        assert_eq!(color_for_status("Blocked"), STATUS_BLOCKED.into());
        assert_eq!(color_for_status("ERROR"), STATUS_ERROR.into());
        assert_eq!(color_for_status("done"), STATUS_DONE.into());
        assert_eq!(color_for_status("idle"), STATUS_IDLE.into());
        assert_eq!(color_for_status("???"), STATUS_IDLE.into());
    }

    #[test]
    fn meter_tone_bands_match_the_web_thresholds() {
        // toneFor(): ok < 75 ≤ warn < 90 ≤ crit; null → unknown.
        assert_eq!(tone_for_used(None), Tone::Unknown);
        assert_eq!(tone_for_used(Some(0.0)), Tone::Ok);
        assert_eq!(tone_for_used(Some(74.9)), Tone::Ok);
        assert_eq!(tone_for_used(Some(75.0)), Tone::Warn);
        assert_eq!(tone_for_used(Some(89.9)), Tone::Warn);
        assert_eq!(tone_for_used(Some(90.0)), Tone::Crit);
        assert_eq!(tone_for_used(Some(120.0)), Tone::Crit);
    }

    #[test]
    fn tone_colors_resolve_to_status_tokens() {
        assert_eq!(Tone::Ok.color(), ACCENT.into());
        assert_eq!(Tone::Warn.color(), STATUS_BLOCKED.into());
        assert_eq!(Tone::Crit.color(), STATUS_ERROR.into());
        assert_eq!(Tone::Unknown.color(), TEXT_3.into());
    }

    #[test]
    fn used_pct_inverts_headroom() {
        assert_eq!(used_pct(Some(18.0)), Some(82.0));
        assert_eq!(used_pct(None), None);
    }

    #[test]
    fn rgba_hex_decodes_channels() {
        let c = rgba_hex(0xd97757ff);
        assert!((c.r - 0xd9 as f32 / 255.0).abs() < f32::EPSILON);
        assert!((c.g - 0x77 as f32 / 255.0).abs() < f32::EPSILON);
        assert!((c.b - 0x57 as f32 / 255.0).abs() < f32::EPSILON);
        assert!((c.a - 1.0).abs() < f32::EPSILON);
    }
}
