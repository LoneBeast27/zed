//! THE design system — the single source of truth for the fork's entire UI.
//!
//! Governing rule ("if Zed has it, we own it"): every themeable value —
//! colors, text ramp, borders, radii, spacing, motion, type scale — is
//! defined HERE and nowhere else. Editing a token in this file re-flows the
//! whole product: the override bridge ([`crate::design_bridge`]) drives Zed's
//! ~150 `ThemeColors` fields from these tokens, and our custom widgets import
//! `theme::design::*` directly. Nothing reads a Zed default. That is what
//! stops any single un-retheme'd surface from standing out — the nightmare we
//! are deliberately avoiding.
//!
//! Provenance of the values:
//!   • Brand palette — `bridge/ui/app.css :root` (PARITY_SPEC §1, the web mirror).
//!   • OLED refinement — the agy/Gemini design pass (near-black `SURFACE_BASE`
//!     to avoid AMOLED pixel smear; off-white `TEXT` to kill halation; the
//!     500-weight `label` requirement). Two Claude/systems calls layered on top:
//!     the opaque elevation ramp is adopted from agy; the run-status palette is
//!     kept on the documented brand (Google-dark) for continuity — agy's
//!     Tailwind-tuned alternatives are a one-line swap.

use gpui::Rgba;

/// `0xRRGGBBAA` → [`Rgba`] in const context (gpui's `rgba()` is not `const fn`).
pub const fn rgba_hex(hex: u32) -> Rgba {
    Rgba {
        r: ((hex >> 24) & 0xFF) as f32 / 255.0,
        g: ((hex >> 16) & 0xFF) as f32 / 255.0,
        b: ((hex >> 8) & 0xFF) as f32 / 255.0,
        a: (hex & 0xFF) as f32 / 255.0,
    }
}

/// Every color token. Stored as [`Rgba`] (const-constructible); call `.into()`
/// at the use site for the `Hsla` that `ThemeColors` and `Styled` expect.
pub mod color {
    use super::rgba_hex;
    use gpui::Rgba;

    // ── Surfaces: opaque elevation ramp (agy OLED, near-black base) ──
    /// Near-black app background. NOT pure `#000` — avoids AMOLED pixel smear.
    pub const SURFACE_BASE: Rgba = rgba_hex(0x09090bff);
    /// Panels, sidebars.
    pub const SURFACE_RAISED: Rgba = rgba_hex(0x111114ff);
    /// Dropdowns, popovers.
    pub const SURFACE_OVERLAY: Rgba = rgba_hex(0x18181bff);
    /// Modals, command palette (floating layers).
    pub const SURFACE_FLOATING: Rgba = rgba_hex(0x1f1f23ff);
    /// Tooltips, highest z-layer.
    pub const SURFACE_TOP: Rgba = rgba_hex(0x27272aff);

    // ── Translucent overlays: tints that ride ON a surface (web app.css) ──
    /// `--surface-1` white·0.05 — idle-pill fill, seg-toggle active.
    pub const OVERLAY_SOFT: Rgba = rgba_hex(0xffffff0d);
    /// `--hover` white·0.06 — hover wash on chrome.
    pub const HOVER: Rgba = rgba_hex(0xffffff0f);

    // ── Borders / hairlines ──
    pub const BORDER: Rgba = rgba_hex(0x27272aff);
    pub const BORDER_SUBTLE: Rgba = rgba_hex(0x1c1c1fff);
    pub const BORDER_STRONG: Rgba = rgba_hex(0x3f3f46ff);
    /// Keyboard-focus ring (doubles as `--accent`).
    pub const BORDER_FOCUS: Rgba = rgba_hex(0x8ab4f8ff);
    /// `--hairline` white·0.08 — where two surfaces meet.
    pub const HAIRLINE: Rgba = rgba_hex(0xffffff14);
    /// `--hairline-hi` white·0.12 — graph node borders, edges, scrollbar thumb.
    pub const HAIRLINE_HI: Rgba = rgba_hex(0xffffff1f);

    // ── Text ramp (agy — opaque zinc; off-white primary kills halation) ──
    /// 15.8:1 on `SURFACE_BASE` (WCAG AAA). Never pure white on OLED.
    pub const TEXT: Rgba = rgba_hex(0xececefff);
    pub const TEXT_MUTED: Rgba = rgba_hex(0xa1a1aaff);
    pub const TEXT_PLACEHOLDER: Rgba = rgba_hex(0x71717aff);
    pub const TEXT_DISABLED: Rgba = rgba_hex(0x52525bff);
    /// Accent text — render at weight 500 (see [`super::type_scale::LABEL`]).
    pub const TEXT_ACCENT: Rgba = rgba_hex(0x8ab4f8ff);
    /// Foreground on an accent fill.
    pub const ON_ACCENT: Rgba = rgba_hex(0x09090bff);

    // ── Accent chrome (single accent) ──
    /// `--accent` — the one chrome accent (focus rings, selected, links).
    pub const ACCENT: Rgba = rgba_hex(0x8ab4f8ff);
    /// `--accent-fill` — deeper filled-action blue (composer send circle).
    pub const ACCENT_FILL: Rgba = rgba_hex(0x1a73e8ff);

    // ── Run-status (brand Google-dark; agy Tailwind alt in the doc header) ──
    pub const STATUS_RUNNING: Rgba = rgba_hex(0x81c995ff);
    pub const STATUS_BLOCKED: Rgba = rgba_hex(0xfdd663ff);
    pub const STATUS_ERROR: Rgba = rgba_hex(0xf28b82ff);
    pub const STATUS_DONE: Rgba = rgba_hex(0x8ab4f8ff);
    /// white·0.45 — idle/neutral dot (degrades visibly, never vanishes).
    pub const STATUS_IDLE: Rgba = rgba_hex(0xffffff73);
    pub const STATUS_INFO: Rgba = rgba_hex(0x8ab4f8ff);
    pub const STATUS_SUCCESS: Rgba = rgba_hex(0x81c995ff);
    pub const STATUS_WARNING: Rgba = rgba_hex(0xfdd663ff);

    // ── Vendor identity (chips / nodes / meters only — never chrome) ──
    pub const VENDOR_CLAUDE: Rgba = rgba_hex(0xd97757ff);
    pub const VENDOR_CODEX: Rgba = rgba_hex(0x10a37fff);
    pub const VENDOR_AGY: Rgba = rgba_hex(0x8ab4f8ff);
    pub const VENDOR_GEMINI: Rgba = rgba_hex(0xa78bfaff);
    /// 10% vendor tints — badge / chip fills.
    pub const VENDOR_CLAUDE_TINT: Rgba = rgba_hex(0xd977571a);
    pub const VENDOR_CODEX_TINT: Rgba = rgba_hex(0x10a37f1a);
    pub const VENDOR_AGY_TINT: Rgba = rgba_hex(0x8ab4f81a);
    pub const VENDOR_GEMINI_TINT: Rgba = rgba_hex(0xa78bfa1a);
}

/// Corner radii (px). Kills the ad-hoc `px(7)`/`px(12)` scattered in widgets —
/// `XL` (12) preserves the existing card radius so migration is value-neutral.
pub mod radius {
    pub const SM: f32 = 4.0;
    pub const MD: f32 = 6.0;
    pub const LG: f32 = 8.0;
    pub const XL: f32 = 12.0;
    pub const PILL: f32 = 9999.0;
}

/// Spacing scale (px) on a 4px base grid.
pub mod space {
    pub const XS: f32 = 4.0;
    pub const SM: f32 = 8.0;
    pub const MD: f32 = 12.0;
    pub const LG: f32 = 16.0;
    pub const XL: f32 = 24.0;
    pub const XXL: f32 = 32.0;
}

/// Motion tokens. Native GPUI cubic-beziers (no spring physics — GPU-rasterized
/// frames only). Geometry rides the longer curve; opacity/color the shorter.
pub mod motion {
    pub const FAST_MS: u64 = 100;
    pub const BASE_MS: u64 = 200;
    pub const SLOW_MS: u64 = 350;
    /// Default UI transition (panels, fades, transforms).
    pub const EASE_OUT_EXPO: [f32; 4] = [0.16, 1.0, 0.3, 1.0];
    /// Reserved for `SLOW_MS` (modals, page-level transitions).
    pub const EASE_OUT_QUINT: [f32; 4] = [0.22, 1.0, 0.36, 1.0];
}

/// Type scale. UI = IBM Plex Sans (weights 400/500/600/700 must be bundled);
/// mono = Lilex. `LABEL` at 12px REQUIRES weight 500 for OLED legibility.
pub mod type_scale {
    use gpui::FontWeight;

    /// A type role: pixel size, weight, and unit-less line-height multiplier.
    #[derive(Debug, Clone, Copy)]
    pub struct Role {
        pub size: f32,
        pub weight: FontWeight,
        pub line_height: f32,
    }

    pub const DISPLAY: Role = Role { size: 32.0, weight: FontWeight::BOLD, line_height: 1.15 };
    pub const TITLE: Role = Role { size: 20.0, weight: FontWeight::SEMIBOLD, line_height: 1.25 };
    pub const HEADING: Role = Role { size: 16.0, weight: FontWeight::SEMIBOLD, line_height: 1.35 };
    pub const BODY: Role = Role { size: 14.0, weight: FontWeight::NORMAL, line_height: 1.5 };
    /// 12px — the one role that needs the 500 weight (halation floor on OLED).
    pub const LABEL: Role = Role { size: 12.0, weight: FontWeight::MEDIUM, line_height: 1.35 };
    pub const CAPTION: Role = Role { size: 11.0, weight: FontWeight::NORMAL, line_height: 1.35 };
    pub const CODE: Role = Role { size: 13.0, weight: FontWeight::NORMAL, line_height: 1.55 };
}
