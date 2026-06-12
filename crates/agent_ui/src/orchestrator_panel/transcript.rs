//! The orchestrator transcript (PARITY_SPEC §4.1): keyed message views over
//! a virtualized `list(ListState)` (thread_view idiom) — tonal user cards /
//! bare agent prose through `crates/markdown`, worked-for collapsibles with
//! the live rolling tick, the busy shimmer item, and the empty-state
//! greeting.
//!
//! Streaming idiom (RUST_PORT_NOTES §4.1 ruling): the web's 3-word
//! span-splitting chunk-fade is a poll-simulation hack — natively a reply
//! streams into the markdown element and the newest block gets ONE one-shot
//! 200ms effects fade. The span-splitting is deliberately NOT ported.

use std::time::{Duration, Instant};

use gpui::{
    Animation, AnimationExt as _, AnyElement, App, AppContext as _, Entity, FontWeight,
    ListAlignment, ListState, SharedString, TextStyleRefinement, Window, pulsating_between,
};
use markdown::{Markdown, MarkdownFont, MarkdownStyle};
use settings::Settings as _;
use ui::prelude::*;

use crate::bridge::{TranscriptMessage, TranscriptRun, TranscriptSnapshot};
use crate::task_board::style::SURFACE_2B;

/// How long a freshly-appended message keeps its one-shot entrance wrapper
/// attached (covers the 300ms rise with margin; settled messages render
/// bare — §8 idle cost).
pub(super) const FRESH_WINDOW: Duration = Duration::from_millis(450);

/// One transcript message as the panel renders it — markdown entity +
/// worked-for state, keyed by transcript index (§5.6: views persist across
/// poll ticks; only growth appends).
pub(super) struct MessageView {
    pub user: bool,
    pub markdown: Entity<Markdown>,
    /// Raw text (the copy action's payload).
    pub text: SharedString,
    pub ts: f64,
    pub brain: Option<String>,
    pub worked_s: f64,
    pub runs: Vec<TranscriptRun>,
    pub worked_open: bool,
    /// First-sight clock — entrance animations attach only inside
    /// [`FRESH_WINDOW`] so scroll-culling can never replay them.
    pub seen_at: Instant,
}

/// The transcript's view state: keyed message views + the virtualized list.
pub(super) struct TranscriptView {
    pub list_state: ListState,
    views: Vec<MessageView>,
    conv_id: String,
    /// Whether the trailing shimmer item ("Orchestrating…") is listed.
    shimmer: bool,
    /// Items currently spliced into `list_state`.
    listed: usize,
}

impl TranscriptView {
    pub fn new() -> Self {
        Self {
            // Bottom alignment keeps the view pinned to the newest message
            // (the web's scrollTop = scrollHeight on every render) while
            // respecting a user scroll-back.
            list_state: ListState::new(0, ListAlignment::Bottom, px(1024.)),
            views: Vec::new(),
            conv_id: String::new(),
            shimmer: false,
            listed: 0,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.views.is_empty()
    }

    pub fn message_count(&self) -> usize {
        self.views.len()
    }

    pub fn message(&self, ix: usize) -> Option<&MessageView> {
        self.views.get(ix)
    }

    pub fn toggle_worked(&mut self, ix: usize) {
        if let Some(view) = self.views.get_mut(ix) {
            view.worked_open = !view.worked_open;
        }
    }

    /// The index whose worked-for label rolls live while busy: ONLY the
    /// final transcript message, and only when it's an agent reply (a
    /// progressively-streamed reply mid-generation). While the last message
    /// is the user's — the typical busy state — nothing rolls: the previous
    /// turn's frozen counter must never inflate and snap back. (Matches the
    /// web's visible behavior: chat.js's tick selector dead-ends on the
    /// shimmer node, which is always last while busy.)
    pub fn live_agent_ix(&self) -> Option<usize> {
        match self.views.last() {
            Some(view) if !view.user => Some(self.views.len() - 1),
            _ => None,
        }
    }

    /// Keyed reconciliation (§5.6 / chat.js `renderTranscript`): a
    /// conversation switch rebuilds; growth appends new views and never
    /// touches the ones on screen; the shimmer item tracks `busy`. Mirrors
    /// the web's `id:len:busy` signature — including its accepted edge at
    /// the server's 60-message cap, where a sliding window keeps len
    /// constant (the bridge caps at 60; both clients re-key on switch only).
    pub fn sync(&mut self, snapshot: &TranscriptSnapshot, cx: &mut App) {
        let switched = snapshot.id != self.conv_id;
        self.conv_id = snapshot.id.clone();
        // Bounded transcript (PARITY_SPEC §8.4): last 60 — the server
        // already caps there; this is the defensive twin.
        let skip = snapshot.messages.len().saturating_sub(60);
        let messages = &snapshot.messages[skip..];

        if switched || messages.len() < self.views.len() {
            self.views = messages
                .iter()
                .map(|message| build_view(message, cx))
                .collect();
            self.shimmer = snapshot.busy;
            self.listed = self.views.len() + self.shimmer as usize;
            self.list_state.reset(self.listed);
            return;
        }

        let stable = self.views.len();
        for message in &messages[stable..] {
            self.views.push(build_view(message, cx));
        }
        self.shimmer = snapshot.busy;
        let new_count = self.views.len() + self.shimmer as usize;
        if new_count != self.listed || stable != self.views.len() {
            // Re-splice everything past the stable prefix (the shimmer item
            // changes identity when a reply lands in its slot).
            self.list_state.splice(stable..self.listed, new_count - stable);
            self.listed = new_count;
        }
    }
}

fn build_view(message: &TranscriptMessage, cx: &mut App) -> MessageView {
    let text = SharedString::from(message.text.clone());
    MessageView {
        user: message.is_user(),
        markdown: cx.new(|cx| Markdown::new(text.clone(), None, None, cx)),
        text,
        ts: message.ts,
        brain: message.brain.clone(),
        worked_s: message.worked_s,
        runs: message.runs.clone(),
        worked_open: false,
        seen_at: Instant::now(),
    }
}

/// The §4.1 prose ramp over the themed agent style: 15px/1.6 body, the
/// web's `pre`/`code` fills (`--surface-2b` blocks, `#1a1a1a` inline).
pub(super) fn prose_style(window: &Window, cx: &App) -> MarkdownStyle {
    let mut style = MarkdownStyle::themed(MarkdownFont::Agent, window, cx);
    style.base_text_style.refine(&TextStyleRefinement {
        font_size: Some(px(15.).into()),
        line_height: Some(relative(1.6).into()),
        ..Default::default()
    });
    style.code_block.background = Some(gpui::Hsla::from(SURFACE_2B).into());
    style.inline_code.background_color =
        Some(crate::agent_accents::rgba_hex(0x1a1a1aff).into());
    style
}

/// The trailing "Orchestrating…" shimmer item while busy — opacity pulse
/// via `pulsating_between` (the `ui` thread_item idiom; a true gradient
/// text-sweep needs a custom canvas paint and stays banked).
pub(super) fn render_shimmer(cx: &App) -> AnyElement {
    div()
        .mb(px(26.))
        .text_size(px(15.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(cx.theme().colors().text_placeholder)
        .child("Orchestrating…")
        .with_animation(
            ElementId::Name("orchestrating-shimmer".into()),
            Animation::new(Duration::from_millis(1400))
                .repeat()
                .with_easing(pulsating_between(0.4, 0.92)),
            |label, value| label.opacity(value),
        )
        .into_any_element()
}

/// The empty-state greeting (`.chat-greeting`): Outfit headline, serif
/// sub-line, mono force-hints, and the accent bloom (a real BoxShadow —
/// RUST_PORT_NOTES §1 allows glow-shadows where the spec calls for bloom;
/// gpui has no radial-gradient fill). Negative display tracking is a noted
/// divergence — gpui has no letter-spacing.
pub(super) fn render_greeting(cx: &App) -> AnyElement {
    let colors = cx.theme().colors();
    let mono = theme_settings::ThemeSettings::get_global(cx)
        .buffer_font
        .family
        .clone();
    let hint_token = |label: &'static str| {
        div()
            .font_family(mono.clone())
            .text_size(px(12.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(colors.text_muted)
            .child(label)
    };
    v_flex()
        .size_full()
        .items_center()
        .justify_center()
        .text_center()
        .gap(px(10.))
        .child(
            // The radial glow, approximated as an accent bloom shadow.
            div().size(px(1.)).shadow(vec![gpui::BoxShadow {
                color: gpui::Rgba {
                    r: 66. / 255.,
                    g: 133. / 255.,
                    b: 244. / 255.,
                    a: 0.07,
                }
                .into(),
                offset: gpui::point(px(0.), px(40.)),
                blur_radius: px(140.),
                spread_radius: px(120.),
            }]),
        )
        .child(
            div()
                .font_family("Outfit")
                .text_size(px(30.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(colors.text)
                .child("Delegate something."),
        )
        .child(
            div()
                .font_family("Source Serif 4")
                .text_size(px(17.))
                .text_color(colors.text_muted)
                .child("The orchestrator routes your task to the best-fit agent."),
        )
        .child(
            h_flex()
                .mt(px(6.))
                .max_w(px(420.))
                .flex_wrap()
                .items_center()
                .justify_center()
                .gap(px(6.))
                .text_size(px(13.))
                .text_color(colors.text_placeholder)
                .child("Force a target with")
                .child(hint_token("@claude"))
                .child(hint_token("@codex"))
                .child(hint_token("@agy"))
                .child(hint_token("@gemini"))
                .child(", or open a tri-model debate with")
                .child(hint_token("/adversary")),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(role: &str, text: &str) -> TranscriptMessage {
        TranscriptMessage {
            role: role.into(),
            text: text.into(),
            ..Default::default()
        }
    }

    fn snapshot(id: &str, busy: bool, messages: Vec<TranscriptMessage>) -> TranscriptSnapshot {
        TranscriptSnapshot {
            id: id.into(),
            title: "t".into(),
            busy,
            brain: None,
            messages,
        }
    }

    #[gpui::test]
    fn growth_appends_and_keeps_existing_view_identity(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let mut transcript = TranscriptView::new();
            transcript.sync(
                &snapshot("c-1", false, vec![message("user", "hi")]),
                cx,
            );
            assert_eq!(transcript.message_count(), 1);
            assert_eq!(transcript.list_state.item_count(), 1);
            let first_id = transcript.message(0).unwrap().markdown.entity_id();

            // Growth: the existing view (its markdown entity) is untouched.
            transcript.sync(
                &snapshot(
                    "c-1",
                    false,
                    vec![message("user", "hi"), message("orchestrator", "reply")],
                ),
                cx,
            );
            assert_eq!(transcript.message_count(), 2);
            assert_eq!(transcript.list_state.item_count(), 2);
            assert_eq!(
                transcript.message(0).unwrap().markdown.entity_id(),
                first_id,
                "growth must never rebuild on-screen views (§5.6)"
            );
            assert!(!transcript.message(1).unwrap().user);
        });
    }

    #[gpui::test]
    fn conversation_switch_rebuilds(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let mut transcript = TranscriptView::new();
            transcript.sync(
                &snapshot("c-1", false, vec![message("user", "hi")]),
                cx,
            );
            let first_id = transcript.message(0).unwrap().markdown.entity_id();
            transcript.sync(
                &snapshot("c-2", false, vec![message("user", "other")]),
                cx,
            );
            assert_eq!(transcript.message_count(), 1);
            assert_ne!(
                transcript.message(0).unwrap().markdown.entity_id(),
                first_id,
                "a conversation switch re-keys everything"
            );
        });
    }

    #[gpui::test]
    fn shimmer_item_tracks_busy(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let mut transcript = TranscriptView::new();
            // Busy without growth: the shimmer item is listed past the
            // messages.
            transcript.sync(&snapshot("c-1", true, vec![message("user", "go")]), cx);
            assert_eq!(transcript.message_count(), 1);
            assert_eq!(transcript.list_state.item_count(), 2, "messages + shimmer");

            // Reply lands, busy drops: the shimmer slot becomes the reply.
            transcript.sync(
                &snapshot(
                    "c-1",
                    false,
                    vec![message("user", "go"), message("orchestrator", "done")],
                ),
                cx,
            );
            assert_eq!(transcript.message_count(), 2);
            assert_eq!(transcript.list_state.item_count(), 2);

            // Busy flips again without growth: shimmer returns.
            transcript.sync(
                &snapshot(
                    "c-1",
                    true,
                    vec![message("user", "go"), message("orchestrator", "done")],
                ),
                cx,
            );
            assert_eq!(transcript.list_state.item_count(), 3);
        });
    }

    #[gpui::test]
    fn transcript_is_bounded_to_60(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let mut transcript = TranscriptView::new();
            let messages: Vec<_> = (0..70)
                .map(|i| message("user", &format!("m{i}")))
                .collect();
            transcript.sync(&snapshot("c-1", false, messages), cx);
            assert_eq!(transcript.message_count(), 60, "defensive 60-cap (§8.4)");
            assert_eq!(transcript.message(0).unwrap().text.as_ref(), "m10");
        });
    }

    #[gpui::test]
    fn live_tick_only_rides_a_trailing_agent_reply(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let mut transcript = TranscriptView::new();
            assert_eq!(transcript.live_agent_ix(), None);

            // Typical busy state: the user message is last — the previous
            // turn's frozen counter must NOT roll.
            transcript.sync(
                &snapshot(
                    "c-1",
                    true,
                    vec![
                        message("user", "q1"),
                        message("orchestrator", "a1"),
                        message("user", "q2"),
                    ],
                ),
                cx,
            );
            assert_eq!(transcript.live_agent_ix(), None);

            // A progressively-streamed reply IS last — that one rolls.
            transcript.sync(
                &snapshot(
                    "c-1",
                    true,
                    vec![
                        message("user", "q1"),
                        message("orchestrator", "a1"),
                        message("user", "q2"),
                        message("orchestrator", "a2 (streaming)"),
                    ],
                ),
                cx,
            );
            assert_eq!(transcript.live_agent_ix(), Some(3));
        });
    }
}
