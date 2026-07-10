//! S5 google command execution (parity gap #9): fire one LIVE google-lane
//! registry row at the bridge's execute endpoint and render the outcome
//! HONESTLY in the conversation surface.
//!
//! - Selecting an executable row (`Mechanism::GoogleExec`) in the `/`
//!   typeahead POSTs `/commands/<vendor>/<name>` `{"args": …}` — the row's
//!   OWN vendor, since forcing either google token scopes both vendors'
//!   rows (one lane).
//! - Success renders as a system row above the composer: the relay's
//!   `output` text when the response carries one, else the whole response
//!   JSON pretty-printed (deferred polyfill descriptors, session listings —
//!   shown as-is, never summarized into fiction).
//! - Refusals (404 unknown · 405 N/A-by-design · 501 not-built · 502 relay
//!   failure) render the bridge's `{"error": …}` reason VERBATIM —
//!   [`crate::bridge::post_json`] already extracts it (the error-body law).
//!
//! The panel stays thin: fetch/render/POST only. Classification and spend
//! policy live bridge-side (its agy quota guards are server-side; nothing
//! here spawns a vendor process).

use gpui::{AppContext as _, SharedString};
use settings::Settings as _;
use ui::prelude::*;

use crate::bridge::{BRIDGE_BASE_URL, post_json};
use crate::commands::CommandEntry;
use crate::task_board::style::SURFACE_1;

use super::panel::OrchestratorPanel;

/// One executed-command outcome, rendered as a system row in the chat column
/// until dismissed or replaced by the next execution.
pub(super) struct CommandNote {
    /// `/name · vendor` — the row that fired.
    pub title: SharedString,
    /// The honest payload: relay output / pretty response JSON / the
    /// bridge's verbatim refusal / the in-flight marker.
    pub body: SharedString,
    /// Refusal styling (blocked accent) vs settled output.
    pub error: bool,
}

/// Fold a 2xx execute response into display text: the relay's `output`
/// string when present (the human-readable surface), else the whole JSON
/// pretty-printed (deferred descriptors, MCP listings, session maps — all
/// shown verbatim). Pure — unit-tested directly.
pub(super) fn note_body(raw: &str) -> String {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return raw.to_string();
    };
    if let Some(output) = value.get("output").and_then(|v| v.as_str()) {
        return output.to_string();
    }
    serde_json::to_string_pretty(&value).unwrap_or_else(|_| raw.to_string())
}

impl OrchestratorPanel {
    /// Execute one live google-lane row: POST the bridge's execute endpoint
    /// and land the outcome in [`Self::command_note`]. The in-flight state
    /// names the exact wire call (instrument honesty — never a bare
    /// spinner); the reply overwrites it, success or refusal alike.
    pub(super) fn execute_google_command(
        &mut self,
        row: &CommandEntry,
        args: String,
        cx: &mut gpui::Context<Self>,
    ) {
        let Some(vendor) = row.vendor else {
            return; // google rows always carry a vendor; a bare row is not ours
        };
        let vendor = vendor.badge();
        let name = row.name.to_string();
        let title: SharedString = format!("/{name} · {vendor}").into();
        let url = format!("{BRIDGE_BASE_URL}/commands/{vendor}/{name}");
        self.command_note = Some(CommandNote {
            title: title.clone(),
            body: format!("POST /commands/{vendor}/{name} — in flight…").into(),
            error: false,
        });
        cx.notify();
        let http_client = cx.http_client();
        cx.spawn(async move |this, cx| {
            let outcome = cx
                .background_spawn(async move {
                    let body = serde_json::json!({ "args": args }).to_string();
                    post_json(http_client.as_ref(), &url, body).await
                })
                .await;
            this.update(cx, |this, cx| {
                this.command_note = Some(match outcome {
                    Ok(raw) => CommandNote {
                        title,
                        body: note_body(&raw).into(),
                        error: false,
                    },
                    // The bridge's refusal VERBATIM (404/405/501/502 —
                    // post_json extracts the {"error"} body), or the honest
                    // transport error when nothing answered.
                    Err(error) => CommandNote {
                        title,
                        body: error.to_string().into(),
                        error: true,
                    },
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The system row for the last executed command — rendered in the chat
    /// column between the transcript and the composer deck. `None` when
    /// nothing has run (or the note was dismissed).
    pub(super) fn render_command_note(
        &mut self,
        cx: &mut gpui::Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let note = self.command_note.as_ref()?;
        let colors = cx.theme().colors();
        let mono = theme_settings::ThemeSettings::get_global(cx)
            .buffer_font
            .family
            .clone();
        let body_color = if note.error {
            gpui::Hsla::from(crate::agent_accents::color_for_status("blocked"))
        } else {
            colors.text_muted
        };
        let card = v_flex()
            .w_full()
            .rounded(px(12.))
            .bg(SURFACE_1)
            .border_1()
            .border_color(colors.border)
            .px(px(14.))
            .py(px(10.))
            .gap(px(6.))
            .child(
                h_flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .font_family(mono.clone())
                            .text_size(px(12.))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(colors.text)
                            .child(note.title.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(colors.text_placeholder)
                            .child("google lane"),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("command-note-dismiss")
                            .px(px(6.))
                            .rounded(px(5.))
                            .text_size(px(12.))
                            .text_color(colors.text_placeholder)
                            .cursor_pointer()
                            .hover(|s| s.bg(colors.element_hover))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.command_note = None;
                                cx.notify();
                            }))
                            .child("dismiss"),
                    ),
            )
            .child(
                div()
                    .id("command-note-body")
                    .max_h(px(260.))
                    .overflow_y_scroll()
                    .font_family(mono)
                    .text_size(px(12.5))
                    .text_color(body_color)
                    .child(note.body.clone()),
            );
        Some(
            h_flex()
                .w_full()
                .justify_center()
                .pb(px(4.))
                .child(div().w_full().max_w(px(780.)).px(px(32.)).child(card))
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relay_output_is_shown_as_the_human_surface() {
        // agy-subcommand relays return {"output": …} — that text IS the row.
        let raw = r#"{"ok": true, "command": "/models", "vendor": "agy",
                      "relayed": ["agy", "models"],
                      "output": "gemini-3-pro\ngemini-3-flash"}"#;
        assert_eq!(note_body(raw), "gemini-3-pro\ngemini-3-flash");
    }

    #[test]
    fn outputless_responses_render_the_whole_json_verbatim() {
        // Deferred polyfill descriptors / config reads have no `output` —
        // the honest render is the full body, pretty-printed.
        let raw = r#"{"ok": true, "command": "/usage", "vendor": "agy",
                      "deferred": {"kind": "orch-endpoint", "target": "/usage"},
                      "module": "usage console (GET /usage)"}"#;
        let body = note_body(raw);
        assert!(body.contains("\"deferred\""));
        assert!(body.contains("usage console (GET /usage)"));
        assert!(body.contains('\n'), "pretty-printed, not a one-line blob");
    }

    #[test]
    fn non_json_bodies_pass_through_untouched() {
        assert_eq!(note_body("plain text"), "plain text");
    }
}
