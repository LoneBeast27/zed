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
pub const ACCENT_CLAUDE: Rgba = rgba_hex(0xd97757ff);
pub const ACCENT_CODEX: Rgba = rgba_hex(0x10a37fff);
pub const ACCENT_AGY: Rgba = rgba_hex(0x8ab4f8ff);
pub const ACCENT_GEMINI: Rgba = rgba_hex(0xa78bfaff);

// ── Run-status colors (Google dark palette, per the web token block) ──
pub const STATUS_RUNNING: Rgba = rgba_hex(0x81c995ff);
pub const STATUS_BLOCKED: Rgba = rgba_hex(0xfdd663ff);
pub const STATUS_ERROR: Rgba = rgba_hex(0xf28b82ff);
pub const STATUS_DONE: Rgba = rgba_hex(0x8ab4f8ff);
/// `rgba(255, 255, 255, 0.45)` — the idle/neutral dot.
pub const STATUS_IDLE: Rgba = rgba_hex(0xffffff73);

/// Vendor accent for an agent name (case-insensitive, substring-tolerant:
/// `"claude-fable"`, `"OpenAI Codex"`, `"gemini-3"` all resolve). Unknown
/// agents fall back to the chrome accent family ([`ACCENT_AGY`]).
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
        ACCENT_AGY
    };
    rgba.into()
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
        // Unknown vendor falls back to the chrome accent family.
        assert_eq!(accent_for_agent("mystery-agent"), ACCENT_AGY.into());
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
    fn rgba_hex_decodes_channels() {
        let c = rgba_hex(0xd97757ff);
        assert!((c.r - 0xd9 as f32 / 255.0).abs() < f32::EPSILON);
        assert!((c.g - 0x77 as f32 / 255.0).abs() < f32::EPSILON);
        assert!((c.b - 0x57 as f32 / 255.0).abs() < f32::EPSILON);
        assert!((c.a - 1.0).abs() < f32::EPSILON);
    }
}
