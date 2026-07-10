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
    AnyElement, App, AppContext as _, Entity, FontWeight, ListAlignment, ListState, SharedString,
    TextStyleRefinement, Window,
};
use markdown::{Markdown, MarkdownFont, MarkdownStyle};
use settings::Settings as _;
use ui::prelude::*;

use crate::agent_accents::GREET_BLOOM;
use crate::bridge::{TranscriptMessage, TranscriptRun, TranscriptSnapshot};
use crate::task_board::motion;
use crate::task_board::style::{INLINE_CODE_BG, SURFACE_2B};

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

    /// The index whose worked-for label rolls live: nothing while settled
    /// (`busy == false` — a settled trailing reply's frozen counter must
    /// never tick), and while busy ONLY the final transcript message, and
    /// only when it's an agent reply (a progressively-streamed reply
    /// mid-generation). While the last message is the user's — the typical
    /// busy state — nothing rolls: the previous turn's frozen counter must
    /// never inflate and snap back. (Matches the web's visible behavior:
    /// chat.js's tick loop runs only while busy and its selector dead-ends
    /// on the shimmer node, which is always last while busy.)
    pub fn live_agent_ix(&self, busy: bool) -> Option<usize> {
        if !busy {
            return None;
        }
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

/// The meta line's attribution vendor (§4.1): a FORCED turn credits the
/// agent that actually RAN it (`@claude`) — the routing brain never touched
/// a forced message (`orchestrator/loop.py` `FORCE_RE` bypass), so showing
/// `brain` there misattributes the reply. Routed turns keep the brain.
/// Forced detection reads the run chip's reason segment — the same
/// "forced target" / "user override" markers
/// [`crate::task_board::style::chip_reason`] rewrites.
pub(super) fn meta_vendor(brain: Option<&str>, runs: &[TranscriptRun]) -> Option<String> {
    let forced_agent = runs.iter().find_map(|run| {
        let reason = run.chip.as_deref()?.split('·').nth(1)?.trim().to_ascii_lowercase();
        // The raw bridge chip reads "forced @vendor target (short-circuit)" —
        // NOT the display rewrite "forced target" — so match the stable stem
        // (live-miss caught by the 2026-07-10 playtest: meta rendered bare).
        ((reason.contains("forced") || reason.contains("override"))
            && !run.agent.is_empty())
        .then(|| format!("@{}", run.agent))
    });
    forced_agent.or_else(|| brain.map(str::to_string))
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

/// The §4.1 prose ramp over the themed agent style: 15px/1.6 body plus the
/// full `.agent-prose pre`/`code` anatomy from chat.css — fills, hairline,
/// radius, padding, and the 12.5px/13px code sizes (the themed defaults run
/// the 12px agent buffer size with the near-invisible `border_variant`
/// hairline and r8/p8 geometry).
pub(super) fn prose_style(window: &Window, cx: &App) -> MarkdownStyle {
    let mut style = MarkdownStyle::themed(MarkdownFont::Agent, window, cx);
    style.base_text_style.refine(&TextStyleRefinement {
        font_size: Some(px(15.).into()),
        line_height: Some(relative(1.6).into()),
        ..Default::default()
    });
    // `pre { background: var(--surface-2b); border: 1px solid
    // var(--hairline); border-radius: 12px; padding: 13px 15px }` — the
    // port's hairline convention maps --hairline to `colors.border`.
    style.code_block.background = Some(gpui::Hsla::from(SURFACE_2B).into());
    style.code_block.border_color = Some(cx.theme().colors().border);
    let radius = gpui::AbsoluteLength::Pixels(px(12.));
    style.code_block.corner_radii.top_left = Some(radius);
    style.code_block.corner_radii.top_right = Some(radius);
    style.code_block.corner_radii.bottom_right = Some(radius);
    style.code_block.corner_radii.bottom_left = Some(radius);
    let pad_y = gpui::DefiniteLength::Absolute(gpui::AbsoluteLength::Pixels(px(13.)));
    let pad_x = gpui::DefiniteLength::Absolute(gpui::AbsoluteLength::Pixels(px(15.)));
    style.code_block.padding.top = Some(pad_y);
    style.code_block.padding.bottom = Some(pad_y);
    style.code_block.padding.left = Some(pad_x);
    style.code_block.padding.right = Some(pad_x);
    // `pre code { font-size: 12.5px; line-height: 1.55 }`.
    style.code_block.text.font_size = Some(px(12.5).into());
    style.code_block.text.line_height = Some(relative(1.55).into());
    // `code { font-size: 13px; background: #1a1a1a }`. The web's inline
    // hairline pill-box (1px border + r6 + 1×6 padding) is unreachable —
    // `MarkdownStyle.inline_code` is a TextStyleRefinement with no box
    // fields (ledgered platform gap, Z3_REPORT).
    style.inline_code.background_color = Some(INLINE_CODE_BG.into());
    style.inline_code.font_size = Some(px(13.).into());
    style
}

/// The trailing "Orchestrating…" shimmer item while busy — the gradient
/// text-sweep (`motion::shimmer`), with pulsate as the reduced-motion
/// fallback. Replaces the prior `pulsating_between` placeholder.
pub(super) fn render_shimmer(cx: &App) -> AnyElement {
    // The §4.1 in-progress label as the true gradient text-sweep (web
    // `.shimmer-label`): a highlight band travels through "Orchestrating…"
    // while busy, stopping cleanly when the reply lands (the element only
    // exists while the shimmer item is listed). Pulsate stays the reduced-
    // motion fallback — see `motion::ShimmerMode`.
    div().mb(px(26.)).child(motion::shimmer(
        ElementId::Name("orchestrating-shimmer".into()),
        "Orchestrating…",
        px(15.),
        FontWeight::MEDIUM,
        cx.theme().colors().text_placeholder,
        crate::agent_accents::SHIMMER_HIGHLIGHT.into(),
        motion::ShimmerMode::Sweep,
    ))
    .into_any_element()
}

/// Resolve a greeting display family with a non-mono safety net: gpui's
/// unknown-family fallback stack tries `.ZedMono` → embedded Lilex FIRST
/// (text_system.rs), so a missing display font would silently render the
/// PARITY_SPEC §2 signature lines in MONOSPACE. Outfit + Source Serif 4
/// ship embedded under `assets/fonts/` (both OFL, licenses alongside), so
/// this resolves to the requested family everywhere the loader ran; the
/// guard keeps the degrade path UI-font (Inter class), never mono.
fn greeting_family(name: &'static str, cx: &App) -> SharedString {
    if cx
        .text_system()
        .all_font_names()
        .iter()
        .any(|family| family == name)
    {
        name.into()
    } else {
        theme_settings::ThemeSettings::get_global(cx)
            .ui_font
            .family
            .clone()
    }
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
                color: GREET_BLOOM.into(),
                offset: gpui::point(px(0.), px(40.)),
                blur_radius: px(140.),
                spread_radius: px(120.),
            }]),
        )
        .child(
            div()
                .font_family(greeting_family("Outfit", cx))
                .text_size(px(30.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(colors.text)
                .child("Delegate something."),
        )
        .child(
            div()
                .font_family(greeting_family("Source Serif 4", cx))
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
#[path = "transcript_tests.rs"]
mod tests;
