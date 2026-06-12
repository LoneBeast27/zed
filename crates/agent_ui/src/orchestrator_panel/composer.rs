//! The two-layer composer deck (PARITY_SPEC §4.1, the Antigravity
//! signature — `chat.css .composer`): auto-height editor over the controls
//! row ([+ ghost] [brain pill] [spacer] [send circle]) over the darker
//! `--surface-2b` hint footer. The send⇄stop island morph lives in
//! [`super::send_circle`].
//!
//! Composer idiom (RUST_PORT_NOTES §4.1): auto-height `Editor` entity
//! (message_editor pattern), Enter sends / Shift+Enter newlines via the
//! `OrchestratorComposer > Editor` keymap binding to [`Send`]
//! (`assets/keymaps/workspace_modes.json` — loaded only with the
//! workspace-modes flag, like the panel itself).

use editor::{Editor, EditorElement, EditorEvent, EditorStyle};
use gpui::{
    AnyElement, Entity, Focusable as _, SharedString, Subscription, Task, TextStyle, Window,
};
use settings::Settings as _;
use theme_settings::ThemeSettings;
use ui::prelude::*;

use crate::bridge::{BRIDGE_BASE_URL, post_json};
use crate::task_board::style::{HAIRLINE_HI, SURFACE_2, SURFACE_2B};

use super::panel::{OrchestratorPanel, Send};
use super::send_circle::SendCircle;

/// The hint strip's force tokens: (label, inserted prefix).
const HINTS: [(&str, &str); 5] = [
    ("@claude", "@claude "),
    ("@codex", "@codex "),
    ("@agy", "@agy "),
    ("@gemini", "@gemini "),
    ("/adversary", "/adversary "),
];

/// The composer deck's view state — owned by the panel (TranscriptView
/// pattern), not a separate entity.
pub(super) struct Composer {
    pub editor: Entity<Editor>,
    /// The send⇄stop morph state.
    send_circle: SendCircle,
    /// In-flight `POST /message` (the web awaits sends serially).
    send_task: Option<Task<()>>,
    _editor_subscription: Subscription,
}

impl Composer {
    pub fn new(window: &mut Window, cx: &mut gpui::Context<OrchestratorPanel>) -> Self {
        let editor = cx.new(|cx| {
            let mut editor = Editor::auto_height(1, 8, window, cx);
            editor.set_placeholder_text(
                "Describe a task…  @ to force an agent, / for adversary",
                window,
                cx,
            );
            editor.set_soft_wrap();
            editor.set_show_indent_guides(false, cx);
            editor
        });
        // Repaint on edits so the reveal retargets off the live has-text
        // state (the web's `input` listener toggling `.has-text`).
        let _editor_subscription =
            cx.subscribe(&editor, |_, _, event: &EditorEvent, cx| {
                if matches!(event, EditorEvent::BufferEdited) {
                    cx.notify();
                }
            });
        Self {
            editor,
            send_circle: SendCircle::new(),
            send_task: None,
            _editor_subscription,
        }
    }
}

/// `box.value.replace(/^(@\w+\s|\/adversary\s)/, "")` — strip one leading
/// force prefix so hint clicks swap the target instead of stacking them.
fn strip_force_prefix(text: &str) -> &str {
    if let Some(rest) = text.strip_prefix("/adversary")
        && let Some(rest) = rest.strip_prefix(|c: char| c.is_whitespace())
    {
        return rest;
    }
    if let Some(stripped) = text.strip_prefix('@') {
        let word_len = stripped
            .find(|c: char| !(c.is_alphanumeric() || c == '_'))
            .unwrap_or(stripped.len());
        if word_len > 0
            && let Some(rest) = stripped[word_len..].strip_prefix(|c: char| c.is_whitespace())
        {
            return rest;
        }
    }
    text
}

/// The bridge's `POST /message` reply (`{"conv": id, …}`).
#[derive(Default, serde::Deserialize)]
struct MessageResponse {
    #[serde(default)]
    conv: String,
}

impl OrchestratorPanel {
    /// Enter / send-circle click: POST the trimmed text on the background
    /// executor, then canonicalize the conversation + refetch the
    /// transcript (the web's `send()` + immediate `loop()`).
    pub(super) fn send_message(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        if self.busy {
            return; // stop is a no-op until a real abort endpoint lands (banked)
        }
        let text = self.composer.editor.read(cx).text(cx);
        let text = text.trim().to_string();
        if text.is_empty() {
            return;
        }
        self.composer
            .editor
            .update(cx, |editor, cx| editor.clear(window, cx));

        let conv = self.store.read(cx).transcript_conv.clone().unwrap_or_default();
        let http_client = cx.http_client();
        let store = self.store.clone();
        self.composer.send_task = Some(cx.spawn(async move |_, cx| {
            let posted = cx
                .background_spawn(async move {
                    let body = serde_json::json!({ "text": text, "conv": conv }).to_string();
                    let raw =
                        post_json(http_client.as_ref(), &format!("{BRIDGE_BASE_URL}/message"), body)
                            .await?;
                    anyhow::Ok(serde_json::from_str::<MessageResponse>(&raw).unwrap_or_default())
                })
                .await;
            store.update(cx, |store, cx| {
                if let Ok(response) = posted
                    && !response.conv.is_empty()
                {
                    store.set_conversation(Some(response.conv), cx);
                }
                // Refetch even on a failed POST — the bridge may have
                // taken the message before the response broke.
                store.refetch_transcript_soon(cx);
            });
        }));
        cx.notify();
    }

    /// Hint click: swap any existing force prefix for the clicked one and
    /// refocus the editor.
    fn insert_hint(&mut self, ins: &'static str, window: &mut Window, cx: &mut gpui::Context<Self>) {
        self.composer.editor.update(cx, |editor, cx| {
            let text = editor.text(cx);
            let rest = strip_force_prefix(&text).to_string();
            editor.set_text(format!("{ins}{rest}"), window, cx);
        });
        window.focus(&self.composer.editor.read(cx).focus_handle(cx), cx);
    }

    /// The whole composer block: deck + footer inside the centered
    /// `.chat-col` (`.composer-wrap { padding: 6px 9px 20px 0 }`; the
    /// scroll-fade gradient is dropped — the panel surface is already
    /// opaque).
    pub(super) fn render_composer(
        &mut self,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        let has_text = !self.composer.editor.read(cx).is_empty(cx);
        let busy = self.busy;
        self.composer.send_circle.update_motion(busy, has_text);

        let colors = cx.theme().colors();
        let focused = self
            .composer
            .editor
            .read(cx)
            .focus_handle(cx)
            .is_focused(window);

        let deck = v_flex()
            .key_context("OrchestratorComposer")
            .on_action(cx.listener(|this, _: &Send, window, cx| this.send_message(window, cx)))
            .w_full()
            .rounded(px(16.))
            .bg(SURFACE_2)
            .border_1()
            // `:focus-within { border-color: var(--hairline-hi) }`.
            .border_color(if focused { HAIRLINE_HI.into() } else { colors.border })
            .overflow_hidden()
            .child(
                // textarea: padding 15px 17px 4px, 15px/1.55 UI font.
                div()
                    .pt(px(15.))
                    .px(px(17.))
                    .pb(px(4.))
                    .child(self.render_editor(cx)),
            )
            .child(self.render_controls_row(has_text, busy, cx))
            .child(self.render_foot(cx));

        h_flex()
            .w_full()
            .justify_center()
            .pt(px(6.))
            .pb(px(20.))
            .child(
                v_flex()
                    .w_full()
                    .max_w(px(780.))
                    .px(px(32.))
                    // §4.9 composer anchor slot: the running-tasks island
                    // (Task 4) emerges ABOVE the typing pill, in flow.
                    .children(self.render_tasks_island_slot(cx))
                    .child(deck),
            )
            .into_any_element()
    }

    fn render_editor(&self, cx: &mut gpui::Context<Self>) -> AnyElement {
        let settings = ThemeSettings::get_global(cx);
        let text_style = TextStyle {
            color: cx.theme().colors().text,
            font_family: settings.ui_font.family.clone(),
            font_features: settings.ui_font.features.clone(),
            font_size: px(15.).into(),
            line_height: relative(1.55).into(),
            ..Default::default()
        };
        EditorElement::new(
            &self.composer.editor,
            EditorStyle {
                background: gpui::transparent_black(),
                local_player: cx.theme().players().local(),
                syntax: cx.theme().syntax().clone(),
                text: text_style,
                ..Default::default()
            },
        )
        .into_any_element()
    }

    /// `.composer-row`: [+ ghost 32] [brain pill] [spacer] [send 40].
    fn render_controls_row(
        &self,
        has_text: bool,
        busy: bool,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        let colors = cx.theme().colors();
        let brain = self
            .store
            .read(cx)
            .transcript
            .as_ref()
            .and_then(|snapshot| snapshot.brain.clone())
            .unwrap_or_else(|| "idle".to_string());

        h_flex()
            .items_center()
            .gap(px(8.))
            .pt(px(6.))
            .pr(px(10.))
            .pb(px(8.))
            .pl(px(12.))
            .child(
                // `.ghost-btn` — attach affordance (anatomy-only, like the
                // web: no handler ships in this pass).
                div()
                    .id("composer-attach")
                    .size(px(32.))
                    .rounded_full()
                    .border_1()
                    .border_color(colors.border)
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(colors.text_placeholder)
                    .hover(|this| this.text_color(colors.text_muted).border_color(HAIRLINE_HI))
                    .cursor_pointer()
                    .child(Icon::new(IconName::Plus).size(IconSize::Small).color(Color::Muted)),
            )
            .child(
                // `.brain-pill` — store.brain or "idle" + chevron
                // (anatomy-only; the web ships no click handler either).
                h_flex()
                    .id("composer-brain")
                    .items_center()
                    .gap(px(4.))
                    .pl(px(12.))
                    .pr(px(8.))
                    .py(px(6.))
                    .rounded_full()
                    .border_1()
                    .border_color(colors.border)
                    .text_size(px(13.))
                    .text_color(colors.text_muted)
                    .hover(|this| this.text_color(colors.text).border_color(HAIRLINE_HI))
                    .cursor_pointer()
                    .child(SharedString::from(brain))
                    .child(
                        Icon::new(IconName::ChevronDown)
                            .size(IconSize::Small)
                            .color(Color::Placeholder),
                    ),
            )
            .child(div().flex_1())
            .child(self.composer.send_circle.render(has_text || busy, cx))
            .into_any_element()
    }

    /// `.composer-foot` — the darker sub-deck env strip with the clickable
    /// force hints.
    fn render_foot(&self, cx: &mut gpui::Context<Self>) -> AnyElement {
        // Copy the Hsla tokens out so the hint closure can take `cx`
        // mutably (Hsla is Copy; the theme borrow must not outlive this).
        let (placeholder, muted, hover_bg) = {
            let colors = cx.theme().colors();
            (
                colors.text_placeholder,
                colors.text_muted,
                colors.element_hover,
            )
        };
        let mono = ThemeSettings::get_global(cx).buffer_font.family.clone();
        let hint = |label: &'static str, ins: &'static str, cx: &mut gpui::Context<Self>| {
            div()
                .id(ElementId::Name(format!("hint-{label}").into()))
                .px(px(4.))
                .py(px(1.))
                .rounded(px(6.))
                .font_family(mono.clone())
                .text_size(px(12.))
                .text_color(placeholder)
                .hover(move |this| this.text_color(muted).bg(hover_bg))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.insert_hint(ins, window, cx);
                }))
                .child(label)
        };

        let mut foot = h_flex()
            .items_center()
            .flex_wrap()
            .gap(px(8.))
            .px(px(14.))
            .py(px(8.))
            .bg(SURFACE_2B)
            .text_size(px(13.))
            .text_color(placeholder);
        for (label, ins) in &HINTS[..4] {
            foot = foot.child(hint(label, ins, cx));
        }
        foot.child(div().opacity(0.7).child("to force"))
            .child(hint(HINTS[4].0, HINTS[4].1, cx))
            .into_any_element()
    }

    /// The §4.9 composer anchor: the running-tasks island occupies normal
    /// flow here while visible (its height displaces the deck downward —
    /// the choreography), and is absent from the tree otherwise.
    fn render_tasks_island_slot(&mut self, cx: &mut gpui::Context<Self>) -> Option<AnyElement> {
        self.tasks_island
            .visible()
            .then(|| crate::islands::tasks_island::render_tasks_island(&self.tasks_island, cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn force_prefix_swap_matches_the_web_regex() {
        // `^(@\w+\s|\/adversary\s)` swapped for the clicked hint.
        assert_eq!(strip_force_prefix("@codex do the thing"), "do the thing");
        assert_eq!(strip_force_prefix("@claude x"), "x");
        assert_eq!(strip_force_prefix("/adversary debate this"), "debate this");
        // No prefix → untouched.
        assert_eq!(strip_force_prefix("plain task"), "plain task");
        // Mid-text mentions are not prefixes.
        assert_eq!(strip_force_prefix("ask @codex later"), "ask @codex later");
        // A bare @ or missing trailing whitespace is not a prefix (regex
        // requires \w+ then \s).
        assert_eq!(strip_force_prefix("@"), "@");
        assert_eq!(strip_force_prefix("@codex"), "@codex");
        assert_eq!(strip_force_prefix("/adversary"), "/adversary");
        // Newline counts as \s, like the web.
        assert_eq!(strip_force_prefix("@agy\nplan"), "plan");
    }
}
