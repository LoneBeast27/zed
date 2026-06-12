//! The run drawer's tab bodies (web `drawer.js` `summary()`/`result()`/
//! `logs()`): Summary fields (Routing/Task/Error/Usage/Run), Result text
//! (markdown), and the mono kind/payload log tail.

use gpui::{
    AnyElement, App, Entity, FontWeight, SharedString, TextStyleRefinement, UnderlineStyle,
    Window, relative,
};
use markdown::{Markdown, MarkdownElement, MarkdownStyle};
use settings::Settings as _;
use theme_settings::ThemeSettings;
use ui::prelude::*;

use super::run_detail::{DrawerTab, RunDetail};
use super::style::{SURFACE_2B, rel};

/// Board-local markdown base style — `kind` picks the web type spec for the
/// surface: Summary Task = 13px UI `--text-2` at 1.5 (board.css:205 `.fv`),
/// Result = 12.5px mono full-strength `--text` at 1.6 (board.css:207-211
/// `.field pre`). Replaces the borrowed context-server-modal style (10px
/// XSmall muted), which matched neither.
enum BodyMarkdown {
    Task,
    Result,
}

fn body_markdown_style(kind: BodyMarkdown, window: &Window, cx: &App) -> MarkdownStyle {
    let theme_settings = ThemeSettings::get_global(cx);
    let colors = cx.theme().colors();
    let mut text_style = window.text_style();
    let refinement = match kind {
        BodyMarkdown::Task => TextStyleRefinement {
            font_family: Some(theme_settings.ui_font.family.clone()),
            font_fallbacks: theme_settings.ui_font.fallbacks.clone(),
            font_features: Some(theme_settings.ui_font.features.clone()),
            font_size: Some(px(13.).into()),
            line_height: Some(relative(1.5)),
            color: Some(colors.text_muted),
            ..Default::default()
        },
        BodyMarkdown::Result => TextStyleRefinement {
            font_family: Some(theme_settings.buffer_font.family.clone()),
            font_fallbacks: theme_settings.buffer_font.fallbacks.clone(),
            font_features: Some(theme_settings.buffer_font.features.clone()),
            font_size: Some(px(12.5).into()),
            line_height: Some(relative(1.6)),
            color: Some(colors.text),
            ..Default::default()
        },
    };
    text_style.refine(&refinement);
    MarkdownStyle {
        base_text_style: text_style,
        selection_background_color: colors.element_selection_background,
        link: TextStyleRefinement {
            background_color: Some(colors.editor_foreground.opacity(0.025)),
            underline: Some(UnderlineStyle {
                color: Some(colors.text_accent.opacity(0.5)),
                thickness: px(1.),
                ..Default::default()
            }),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// Renders the active tab's body content.
pub(super) fn render_body(
    tab: DrawerTab,
    detail: Option<&RunDetail>,
    task_md: Option<&Entity<Markdown>>,
    result_md: Option<&Entity<Markdown>>,
    window: &Window,
    cx: &App,
) -> AnyElement {
    match tab {
        DrawerTab::Summary => render_summary(detail, task_md, window, cx),
        DrawerTab::Result => render_result(detail, result_md, window, cx),
        DrawerTab::Logs => render_logs(detail, cx),
    }
}

/// `.field`: uppercase 11px key over the value.
fn field(key: &'static str, value: AnyElement, cx: &App) -> Div {
    let colors = cx.theme().colors();
    v_flex()
        .mb(px(16.))
        .child(
            div()
                .mb(px(5.))
                .text_size(px(11.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(colors.text_placeholder)
                .child(SharedString::from(key.to_uppercase())),
        )
        .child(value)
}

fn mono_value(text: String, cx: &App) -> AnyElement {
    let colors = cx.theme().colors();
    let mono = ThemeSettings::get_global(cx).buffer_font.family.clone();
    div()
        .font_family(mono)
        .text_size(px(12.))
        .text_color(colors.text_muted)
        .child(SharedString::from(text))
        .into_any_element()
}

/// `.field pre`: surface-2b rounded mono block.
fn pre_block(text: String, cx: &App) -> AnyElement {
    let colors = cx.theme().colors();
    let mono = ThemeSettings::get_global(cx).buffer_font.family.clone();
    div()
        .rounded(px(12.))
        .border_1()
        .border_color(colors.border)
        .bg(SURFACE_2B)
        .px(px(16.))
        .py(px(14.))
        .font_family(mono)
        .text_size(px(12.5))
        .text_color(colors.text)
        .child(SharedString::from(text))
        .into_any_element()
}

fn render_summary(
    detail: Option<&RunDetail>,
    task_md: Option<&Entity<Markdown>>,
    window: &Window,
    cx: &App,
) -> AnyElement {
    let colors = cx.theme().colors();
    let Some(detail) = detail else {
        return mono_value("Loading…".into(), cx);
    };
    let text_value = |s: String| {
        div()
            .text_size(px(13.))
            .text_color(colors.text_muted)
            .child(SharedString::from(s))
            .into_any_element()
    };
    let usage_line = detail.usage.as_ref().map(|usage| {
        usage
            .iter()
            .filter(|(_, v)| !v.is_null())
            .map(|(k, v)| format!("{k}: {v}"))
            .collect::<Vec<_>>()
            .join("  ·  ")
    });
    let task_body: AnyElement = match task_md {
        Some(md) => MarkdownElement::new(md.clone(), body_markdown_style(BodyMarkdown::Task, window, cx))
            .into_any_element(),
        None => text_value(String::new()),
    };
    v_flex()
        .child(field(
            "Routing",
            text_value(detail.chip.clone().unwrap_or_default()),
            cx,
        ))
        .child(field("Task", task_body, cx))
        .children(
            detail
                .error
                .clone()
                .map(|error| field("Error", pre_block(error, cx), cx)),
        )
        .children(
            usage_line
                .filter(|line| !line.is_empty())
                .map(|line| field("Usage", mono_value(line, cx), cx)),
        )
        .child(field(
            "Run",
            mono_value(format!("{} · {}", detail.run_id, rel(detail.elapsed_s)), cx),
            cx,
        ))
        .into_any_element()
}

fn render_result(
    detail: Option<&RunDetail>,
    result_md: Option<&Entity<Markdown>>,
    window: &Window,
    cx: &App,
) -> AnyElement {
    let colors = cx.theme().colors();
    match (result_md, detail) {
        (Some(md), _) => div()
            .rounded(px(12.))
            .border_1()
            .border_color(colors.border)
            .bg(SURFACE_2B)
            .px(px(16.))
            .py(px(14.))
            .child(MarkdownElement::new(
                md.clone(),
                body_markdown_style(BodyMarkdown::Result, window, cx),
            ))
            .into_any_element(),
        (None, Some(detail)) => {
            let hint = if detail.status == "running" {
                "No result text yet — still running."
            } else {
                "No result text."
            };
            div()
                .text_size(px(13.))
                .text_color(colors.text_muted)
                .child(hint)
                .into_any_element()
        }
        (None, None) => mono_value("Loading…".into(), cx),
    }
}

fn render_logs(detail: Option<&RunDetail>, cx: &App) -> AnyElement {
    let colors = cx.theme().colors();
    let mono = ThemeSettings::get_global(cx).buffer_font.family.clone();
    let events = detail.map(|d| d.events.clone()).unwrap_or_default();
    if events.is_empty() {
        return div()
            .text_size(px(13.))
            .text_color(colors.text_muted)
            .child("No events recorded.")
            .into_any_element();
    }
    let rows = events.into_iter().enumerate().map(|(ix, event)| {
        let payload: String = event.payload.to_string().chars().take(400).collect();
        h_flex()
            .id(ElementId::NamedInteger("log-row".into(), ix as u64))
            .items_start()
            .gap(px(10.))
            .py(px(7.))
            .border_b_1()
            .border_color(colors.border)
            .font_family(mono.clone())
            .text_size(px(12.))
            .child(
                div()
                    .flex_none()
                    .min_w(px(52.))
                    .text_color(colors.text_placeholder)
                    .child(SharedString::from(event.kind)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_color(colors.text_muted)
                    .child(SharedString::from(payload)),
            )
    });
    v_flex().children(rows).into_any_element()
}
