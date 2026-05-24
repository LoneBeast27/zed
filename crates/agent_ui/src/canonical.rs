//! Canonical agent-UI helpers — per-agent accent colors and shared constants
//! for the `canonical_agent_ui` settings flag.

use gpui::{Hsla, Rgba, rgb};
use ui::Color;

/// Brand-tinted accent color per ACP agent id, used by the canonical_agent_ui
/// code paths to surface "who's at the helm" at a glance.
///
/// - Claude (claude-acp, anything starting with "claude") → terra-cotta orange #DA7756
/// - Codex  (codex-acp, anything starting with "codex")   → OpenAI green       #10A37F
/// - Gemini (gemini, anything starting with "gemini")     → Google blue        #4285F4
/// - Anything else                                        → Apple system gray  #8E8E93
pub fn accent_hex_for_agent(agent_id: impl AsRef<str>) -> u32 {
    let lower = agent_id.as_ref().to_lowercase();
    if lower.contains("claude") {
        0xDA7756
    } else if lower.contains("codex") || lower.contains("openai") {
        0x10A37F
    } else if lower.contains("gemini") || lower.contains("google") {
        0x4285F4
    } else {
        0x8E8E93
    }
}

pub fn accent_hsla_for_agent(agent_id: impl AsRef<str>) -> Hsla {
    Rgba::from(rgb(accent_hex_for_agent(agent_id))).into()
}

pub fn agent_label_color(agent_id: impl AsRef<str>) -> Color {
    Color::Custom(accent_hsla_for_agent(agent_id))
}
