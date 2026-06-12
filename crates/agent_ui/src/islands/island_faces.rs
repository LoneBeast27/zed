//! The usage island's four representations as DATA (PARITY_SPEC §4.8) —
//! `buildRest`/`buildNotify`/`buildCard` from usage-island.js, plus the
//! deterministic geometry the morph animates between. Faces are compared by
//! value to detect content changes (the web's same-innerHTML guard), built
//! into static elements (the entity owns all animation wrappers), and
//! measured exactly: text runs through the window's text system — the same
//! shaper that paints them — while every other extent is a fixed constant,
//! so morph targets are frame-exact with no ghost-measure pass.

use gpui::{App, FontWeight, SharedString, TextRun, Window};
use settings::Settings as _;
use theme_settings::ThemeSettings;
use ui::prelude::*;

use crate::agent_accents::Tone;
use crate::task_board::style::tabular_nums;

/// Pill (rest/notify) height — web `.isl-rest/.isl-notify { height: 28px }`.
pub const PILL_H: f32 = 28.;
/// Pill horizontal padding — web `#usage-island { padding: 0 12px }`.
const PAD_X: f32 = 12.;
/// Pool dot diameter / gap — web `.isl-dot` 6px, `.isl-dots { gap: 5px }`.
const DOT: f32 = 6.;
const DOT_GAP: f32 = 5.;
/// Gap between dot cluster and text — web `.isl-rest { gap: 8px }`.
const TEXT_GAP: f32 = 8.;
/// Expanded card width — web `.isl-card { width: 248px }`.
pub const CARD_W: f32 = 248.;
const CARD_PAD_TOP: f32 = 10.;
const CARD_PAD_BOTTOM: f32 = 6.;
const CARD_PAD_X: f32 = 8.;
/// Card head: 11px label + 2px top / 8px bottom padding.
const CARD_HEAD_H: f32 = 23.;
/// Card row: 7px padding × 2 + 16px content line, fixed for exact math.
const CARD_ROW_H: f32 = 30.;
const CARD_ROW_GAP: f32 = 2.;
/// Card foot: 4px margin + 1px rule + 8px pad + 14px text + 2px pad.
const CARD_FOOT_H: f32 = 29.;
/// Pill radius = height/2 (the EFFECTIVE radius of the web's 9999px after
/// clamping — animating the raw 9999 would hold the morph capsule-shaped
/// until the very end); card radius — web `.expanded { border-radius: 16px }`.
pub const PILL_R: f32 = PILL_H / 2.;
pub const CARD_R: f32 = 16.;

/// Geometry the container morphs between (§4.9 spatial class).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FaceMetrics {
    pub w: f32,
    pub h: f32,
    pub r: f32,
}

/// One row of the expanded mini-usage card.
#[derive(Debug, Clone, PartialEq)]
pub struct CardRow {
    /// Raw pool key (the row-click payload).
    pub pool: SharedString,
    /// `SHORT` display name ("claude", "codex", …).
    pub short: SharedString,
    pub tone: Tone,
    /// Fill fraction 0–100; `None` renders the dim full-width unknown fill.
    pub used: Option<f32>,
    /// "82%" / "—".
    pub pct: SharedString,
}

/// A representation of the island — the islands flex between these by
/// morphing in place, never by remounting (§0 Island Principle).
#[derive(Debug, Clone, PartialEq)]
pub enum Face {
    /// Compact pill: pool dots + the tightest pool's %.
    Rest {
        dots: Vec<Tone>,
        pct: Option<SharedString>,
    },
    /// Extended pill: `claude 5h · 82% · resets 14:32` tinted in the pool's
    /// status color. Held criticals reuse this face with the error tint on
    /// the container.
    Notify { tone: Tone, text: SharedString },
    /// The five-row mini-usage card.
    Card { rows: Vec<CardRow> },
}

/// `SHORT` from usage-island.js — compact pool names for the pill/card.
pub fn short_pool(name: &str) -> &str {
    match name {
        "claude_sdk_credit" => "claude",
        "codex_plan" => "codex",
        "antigravity_weekly" => "agy",
        "gemini_free_rpd" => "gemini",
        other => other,
    }
}

/// The island's 12px/500 tabular UI font — measurement MUST match paint, so
/// both go through this.
fn island_font(cx: &App) -> gpui::Font {
    let mut font = ThemeSettings::get_global(cx).ui_font.clone();
    font.weight = FontWeight::MEDIUM;
    font.features = tabular_nums();
    font
}

/// Exact painted width of a 12px island text run.
fn text_width(text: &SharedString, window: &Window, cx: &App) -> f32 {
    if text.is_empty() {
        return 0.;
    }
    let run = TextRun {
        len: text.len(),
        font: island_font(cx),
        color: gpui::white(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    window
        .text_system()
        .shape_line(text.clone(), px(12.), &[run], None)
        .width
        .into()
}

/// Card height for `n` rows — pure math, unit-tested.
pub fn card_height(rows: usize) -> f32 {
    let rows_h = rows as f32 * CARD_ROW_H + rows.saturating_sub(1) as f32 * CARD_ROW_GAP;
    CARD_PAD_TOP + CARD_HEAD_H + rows_h + CARD_FOOT_H + CARD_PAD_BOTTOM
}

/// The geometry this face morphs the container to.
pub fn measure(face: &Face, window: &Window, cx: &App) -> FaceMetrics {
    match face {
        Face::Rest { dots, pct } => {
            let n = dots.len() as f32;
            let dots_w = if dots.is_empty() {
                0.
            } else {
                n * DOT + (n - 1.) * DOT_GAP
            };
            let text_w = pct
                .as_ref()
                .map(|pct| TEXT_GAP + text_width(pct, window, cx))
                .unwrap_or(0.);
            FaceMetrics {
                w: PAD_X * 2. + dots_w + text_w,
                h: PILL_H,
                r: PILL_R,
            }
        }
        Face::Notify { text, .. } => FaceMetrics {
            w: PAD_X * 2. + DOT + TEXT_GAP + text_width(text, window, cx),
            h: PILL_H,
            r: PILL_R,
        },
        Face::Card { rows } => FaceMetrics {
            w: CARD_W,
            h: card_height(rows.len()),
            r: CARD_R,
        },
    }
}

fn dot(tone: Tone) -> Div {
    div()
        .flex_none()
        .size(px(DOT))
        .rounded_full()
        .bg(tone.color())
}

fn island_text(text: SharedString, color: gpui::Hsla) -> Div {
    div()
        .text_size(px(12.))
        .font_weight(FontWeight::MEDIUM)
        .font_features(tabular_nums())
        .text_color(color)
        .whitespace_nowrap()
        .child(text)
}

/// Build a face's static content. Card rows route clicks through `on_row`
/// (the entity contracts + switches to the usage mode).
pub fn build_face(
    face: &Face,
    on_row: impl Fn(SharedString, &mut Window, &mut App) + Clone + 'static,
    cx: &App,
) -> AnyElement {
    let colors = cx.theme().colors();
    match face {
        Face::Rest { dots, pct } => h_flex()
            .h(px(PILL_H))
            .px(px(PAD_X))
            .items_center()
            .gap(px(TEXT_GAP))
            .child(
                h_flex()
                    .items_center()
                    .gap(px(DOT_GAP))
                    .children(dots.iter().map(|tone| dot(*tone))),
            )
            .children(
                pct.as_ref()
                    .map(|pct| island_text(pct.clone(), colors.text)),
            )
            .into_any_element(),
        Face::Notify { tone, text } => h_flex()
            .h(px(PILL_H))
            .px(px(PAD_X))
            .items_center()
            .gap(px(TEXT_GAP))
            .child(dot(*tone))
            // Pool-status tint: the whole extended label takes the pool's
            // status color (§4.8).
            .child(island_text(text.clone(), tone.color()))
            .into_any_element(),
        Face::Card { rows } => v_flex()
            .w(px(CARD_W))
            .pt(px(CARD_PAD_TOP))
            .pb(px(CARD_PAD_BOTTOM))
            .px(px(CARD_PAD_X))
            .child(
                // `.isl-card-head` — 11px uppercase label.
                div()
                    .h(px(CARD_HEAD_H))
                    .px(px(10.))
                    .pt(px(2.))
                    .text_size(px(11.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(colors.text_placeholder)
                    .child("USAGE"),
            )
            .child(
                v_flex().gap(px(CARD_ROW_GAP)).children(
                    rows.iter()
                        .enumerate()
                        .map(|(ix, row)| card_row(ix, row, on_row.clone(), cx)),
                ),
            )
            .child(
                // `.isl-card-foot`.
                div()
                    .mt(px(4.))
                    .pt(px(8.))
                    .pb(px(2.))
                    .px(px(10.))
                    .border_t_1()
                    // `--hairline` → colors.border (RUST_PORT_NOTES §1).
                    .border_color(colors.border)
                    .font_family(ThemeSettings::get_global(cx).buffer_font.family.clone())
                    .text_size(px(11.))
                    .text_color(colors.text_placeholder)
                    .whitespace_nowrap()
                    .child("self-metered · open full usage →"),
            )
            .into_any_element(),
    }
}

/// One `.isl-row`: short name · 56px mini meter · right-aligned %.
fn card_row(
    ix: usize,
    row: &CardRow,
    on_row: impl Fn(SharedString, &mut Window, &mut App) + 'static,
    cx: &App,
) -> AnyElement {
    let colors = cx.theme().colors();
    let pool = row.pool.clone();
    let unknown = row.used.is_none();
    let fill_w = row.used.unwrap_or(100.).clamp(0., 100.) / 100.;
    h_flex()
        .id(("isl-row", ix))
        .h(px(CARD_ROW_H))
        .px(px(10.))
        .items_center()
        .gap(px(10.))
        .rounded(px(8.))
        // Inset-grouped-list feel (§0): a whisper of tonal fill. The 0.04
        // fill mirrors a raw rgba() in the web CSS (correctly literal);
        // `:hover` is the `--hover` THEME token (RUST_PORT_NOTES §1).
        .bg(gpui::white().opacity(0.04))
        .hover({
            let hover = colors.element_hover;
            move |row| row.bg(hover)
        })
        .cursor_pointer()
        .on_click(move |_, window, cx| {
            cx.stop_propagation();
            on_row(pool.clone(), window, cx);
        })
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(px(12.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(colors.text)
                .truncate()
                .child(row.short.clone()),
        )
        .child(
            div()
                .flex_none()
                .w(px(56.))
                .h(px(4.))
                .rounded(px(2.))
                .bg(gpui::white().opacity(0.10))
                .overflow_hidden()
                .child(
                    div()
                        .h_full()
                        .rounded(px(2.))
                        .w(relative(fill_w))
                        .bg(row.tone.color())
                        .when(unknown, |fill| fill.opacity(0.35)),
                ),
        )
        .child(
            div()
                .flex_none()
                .text_size(px(12.))
                .font_weight(FontWeight::MEDIUM)
                .font_features(tabular_nums())
                .text_color(colors.text_muted)
                .child(row.pct.clone()),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_pool_matches_usage_island_js() {
        assert_eq!(short_pool("claude_sdk_credit"), "claude");
        assert_eq!(short_pool("codex_plan"), "codex");
        assert_eq!(short_pool("antigravity_weekly"), "agy");
        assert_eq!(short_pool("gemini_free_rpd"), "gemini");
        assert_eq!(short_pool("mystery"), "mystery");
    }

    #[test]
    fn card_height_is_exact_row_math() {
        // pads 16 + head 23 + foot 29 = 68 fixed.
        assert_eq!(card_height(0), 68.);
        assert_eq!(card_height(1), 68. + 30.);
        assert_eq!(card_height(4), 68. + 4. * 30. + 3. * 2.);
    }

    #[test]
    fn faces_compare_by_content() {
        // The same-content render guard (web: className + innerHTML check).
        let a = Face::Rest {
            dots: vec![Tone::Ok, Tone::Warn],
            pct: Some("82%".into()),
        };
        let b = Face::Rest {
            dots: vec![Tone::Ok, Tone::Warn],
            pct: Some("82%".into()),
        };
        assert_eq!(a, b);
        let c = Face::Rest {
            dots: vec![Tone::Ok, Tone::Crit],
            pct: Some("91%".into()),
        };
        assert_ne!(a, c);
    }
}
