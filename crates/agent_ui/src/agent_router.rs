//! Rule-based per-message agent router (slice B1).
//!
//! Implements the v1 rule-based classifier described verbatim in
//! `.planning/zed-fork/MULTI_AGENT_ROUTING.md` §3.3. This module is a
//! standalone pure function used by the composer pre-dispatch path; it has no
//! GPUI, ACP, or composer dependencies and is safe to evolve in isolation.
//!
//! Future slices (B2 user-extensible role hints, B3 LLM classifier) build on
//! this surface. The settings-side hint shape mirrors §7 of the same doc.

use serde::{Deserialize, Serialize};

/// Routing target chosen by the classifier.
///
/// See MULTI_AGENT_ROUTING.md §2 (per-model role taxonomy) and §3.3 for the
/// decision table this enum is the output of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AgentTarget {
    /// Multi-file orchestrator (claude-acp).
    Claude,
    /// Single-file bug-fix / optimizer (codex-acp).
    Codex,
    /// MD-file + user-intent governor; multi-modal (gemini CLI).
    Gemini,
    /// Fall through to the thread's primary agent (usually Claude).
    Default,
}

/// Lightweight stand-in for the real mention type used by the composer.
///
/// The real `Mention` lives in `mention_set.rs` and carries more fields than
/// the classifier cares about. We model only the booleans the §3.3 v1 rules
/// inspect so this module stays free of UI deps until B2 wires the real type
/// in. See MULTI_AGENT_ROUTING.md §3.3.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Mention {
    /// True if the mentioned resource is a markdown file (.md / .markdown).
    pub is_markdown: bool,
    /// True if the mentioned resource is a file (vs. symbol / web / thread).
    pub is_file: bool,
}

impl Mention {
    /// Mirror of the real mention type's expected accessor name in §3.3.
    pub fn is_markdown(&self) -> bool {
        self.is_markdown
    }
}

/// Context the classifier inspects beyond the raw message text.
///
/// Mirrors the `ThreadContext` shape referenced in MULTI_AGENT_ROUTING.md §3.3
/// pseudocode — only the fields the v1 rules touch. The borrow is read-only;
/// the classifier never mutates context.
#[derive(Debug, Clone, Copy)]
pub struct RouteContext<'a> {
    /// Files / docs / etc. attached via @-mention in the composer.
    pub mentions: &'a [Mention],
    /// Composer has at least one image attachment (e.g. drag-and-drop screenshot).
    pub has_image_attachment: bool,
    /// Composer has at least one video attachment.
    pub has_video_attachment: bool,
}

impl<'a> RouteContext<'a> {
    /// Build a context with no attachments and the supplied mentions slice.
    pub fn with_mentions(mentions: &'a [Mention]) -> Self {
        Self {
            mentions,
            has_image_attachment: false,
            has_video_attachment: false,
        }
    }
}

/// User-extensible keyword / threshold knobs feeding the rule classifier.
///
/// Mirrors `AgentRoleHints` in MULTI_AGENT_ROUTING.md §7. Values here are the
/// defaults referenced by the doc; B2 will hydrate this struct from
/// `.agents/settings.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRoleHints {
    pub claude_keywords: Vec<String>,
    pub codex_keywords: Vec<String>,
    pub gemini_keywords: Vec<String>,
    /// Mentions count at or below this routes a fix/bug/optimize message to Codex.
    pub max_files_for_codex: usize,
    /// Mentions count at or above this routes any message to Claude (multi-file rule).
    pub min_files_for_claude: usize,
}

impl Default for AgentRoleHints {
    fn default() -> Self {
        // Defaults are pulled verbatim from MULTI_AGENT_ROUTING.md §3.3 / §7.
        // Pseudocode in §3.3 uses lowercase substring matches; keep them lowercase.
        Self {
            claude_keywords: vec![
                "refactor".to_string(),
                "across the codebase".to_string(),
                "implement".to_string(),
            ],
            codex_keywords: vec![
                "fix".to_string(),
                "bug".to_string(),
                "optimize".to_string(),
                "rewrite".to_string(),
            ],
            gemini_keywords: vec![
                "intent".to_string(),
                "review my".to_string(),
                "spiritual".to_string(),
                "frontend feel".to_string(),
            ],
            max_files_for_codex: 1,
            min_files_for_claude: 2,
        }
    }
}

/// Classify `message` to a single [`AgentTarget`] using the §3.3 v1 rules with
/// default role hints.
///
/// Equivalent to [`classify_with_hints`] called with [`AgentRoleHints::default`].
pub fn classify(message: &str, ctx: &RouteContext<'_>) -> AgentTarget {
    classify_with_hints(message, ctx, &AgentRoleHints::default())
}

/// Classify `message` using user-supplied role hints.
///
/// Implements MULTI_AGENT_ROUTING.md §3.3 v1 verbatim, with hints replacing the
/// hard-coded keyword lists / file-count thresholds in the doc's pseudocode.
/// Order of checks matches the doc: multi-file → single-file bug-fix →
/// markdown/intent → multi-modal → default.
pub fn classify_with_hints(
    message: &str,
    ctx: &RouteContext<'_>,
    hints: &AgentRoleHints,
) -> AgentTarget {
    let lower = message.to_lowercase();
    let file_mentions = ctx.mentions.iter().filter(|m| m.is_file).count();

    // 1. Multi-file / orchestrator signals → Claude.
    if file_mentions >= hints.min_files_for_claude
        || contains_any(&lower, &hints.claude_keywords)
    {
        return AgentTarget::Claude;
    }

    // 2. Single-file + bug-fix verb → Codex.
    if file_mentions > 0
        && file_mentions <= hints.max_files_for_codex
        && contains_any(&lower, &hints.codex_keywords)
    {
        return AgentTarget::Codex;
    }

    // 3. Markdown mention or intent / review verbs → Gemini.
    if ctx.mentions.iter().any(|m| m.is_markdown())
        || contains_any(&lower, &hints.gemini_keywords)
    {
        return AgentTarget::Gemini;
    }

    // 4. Image / video attachment → Gemini (multi-modal).
    if ctx.has_image_attachment || ctx.has_video_attachment {
        return AgentTarget::Gemini;
    }

    // 5. Otherwise: thread primary.
    AgentTarget::Default
}

/// Returns true iff `haystack` contains any keyword from `needles`.
///
/// `haystack` MUST be already lowercased — the classifier does this once per
/// message and reuses the lowercased form across all keyword buckets.
fn contains_any(haystack: &str, needles: &[String]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(is_markdown: bool) -> Mention {
        Mention {
            is_markdown,
            is_file: true,
        }
    }

    #[test]
    fn multi_file_mention_routes_to_claude() {
        let mentions = [file(false), file(false)];
        let ctx = RouteContext::with_mentions(&mentions);
        assert_eq!(classify("update both", &ctx), AgentTarget::Claude);
    }

    #[test]
    fn refactor_keyword_routes_to_claude_even_without_mentions() {
        let ctx = RouteContext::with_mentions(&[]);
        assert_eq!(
            classify("Please refactor the auth flow", &ctx),
            AgentTarget::Claude
        );
    }

    #[test]
    fn across_codebase_phrase_routes_to_claude() {
        let ctx = RouteContext::with_mentions(&[]);
        assert_eq!(
            classify("rename this symbol across the codebase", &ctx),
            AgentTarget::Claude
        );
    }

    #[test]
    fn single_file_fix_routes_to_codex() {
        let mentions = [file(false)];
        let ctx = RouteContext::with_mentions(&mentions);
        assert_eq!(classify("fix this", &ctx), AgentTarget::Codex);
    }

    #[test]
    fn single_file_bug_routes_to_codex() {
        let mentions = [file(false)];
        let ctx = RouteContext::with_mentions(&mentions);
        assert_eq!(
            classify("there's a bug in this function", &ctx),
            AgentTarget::Codex
        );
    }

    #[test]
    fn single_file_optimize_routes_to_codex() {
        let mentions = [file(false)];
        let ctx = RouteContext::with_mentions(&mentions);
        assert_eq!(classify("optimize this loop", &ctx), AgentTarget::Codex);
    }

    #[test]
    fn markdown_mention_routes_to_gemini() {
        let mentions = [file(true)];
        let ctx = RouteContext::with_mentions(&mentions);
        assert_eq!(
            classify("take a look at this doc", &ctx),
            AgentTarget::Gemini
        );
    }

    #[test]
    fn intent_keyword_routes_to_gemini() {
        let ctx = RouteContext::with_mentions(&[]);
        assert_eq!(
            classify("what's the intent here?", &ctx),
            AgentTarget::Gemini
        );
    }

    #[test]
    fn review_my_phrase_routes_to_gemini() {
        let ctx = RouteContext::with_mentions(&[]);
        assert_eq!(
            classify("review my PRD before I send it", &ctx),
            AgentTarget::Gemini
        );
    }

    #[test]
    fn image_attachment_routes_to_gemini() {
        let ctx = RouteContext {
            mentions: &[],
            has_image_attachment: true,
            has_video_attachment: false,
        };
        assert_eq!(classify("what is this?", &ctx), AgentTarget::Gemini);
    }

    #[test]
    fn video_attachment_routes_to_gemini() {
        let ctx = RouteContext {
            mentions: &[],
            has_image_attachment: false,
            has_video_attachment: true,
        };
        assert_eq!(classify("summarize", &ctx), AgentTarget::Gemini);
    }

    #[test]
    fn plain_text_with_no_signals_routes_to_default() {
        let ctx = RouteContext::with_mentions(&[]);
        assert_eq!(classify("hello there", &ctx), AgentTarget::Default);
    }

    #[test]
    fn classifier_is_case_insensitive() {
        let ctx = RouteContext::with_mentions(&[]);
        assert_eq!(
            classify("REFACTOR The Auth Module", &ctx),
            AgentTarget::Claude
        );
    }

    #[test]
    fn multi_file_beats_single_file_fix_rule() {
        // Two file mentions + "fix" keyword: §3.3 checks multi-file FIRST,
        // so this MUST route to Claude, not Codex.
        let mentions = [file(false), file(false)];
        let ctx = RouteContext::with_mentions(&mentions);
        assert_eq!(classify("fix these", &ctx), AgentTarget::Claude);
    }

    #[test]
    fn markdown_mention_beats_image_attachment() {
        // §3.3 checks markdown rule before image-attachment rule.
        // Confirms ordering is preserved.
        let mentions = [file(true)];
        let ctx = RouteContext {
            mentions: &mentions,
            has_image_attachment: true,
            has_video_attachment: false,
        };
        assert_eq!(classify("look", &ctx), AgentTarget::Gemini);
    }

    #[test]
    fn user_defined_codex_keyword_overrides_default() {
        // B2 forward-compat: confirm hint-driven path picks up a custom keyword
        // the default list does NOT contain.
        let mentions = [file(false)];
        let ctx = RouteContext::with_mentions(&mentions);
        let mut hints = AgentRoleHints::default();
        hints.codex_keywords.push("yeet".to_string());

        // With default hints, "yeet this" + 1 mention falls through to Default.
        assert_eq!(
            classify("yeet this", &ctx),
            AgentTarget::Default,
            "default hints should not route on 'yeet'"
        );
        // With the custom hint, the same input routes to Codex.
        assert_eq!(
            classify_with_hints("yeet this", &ctx, &hints),
            AgentTarget::Codex
        );
    }

    #[test]
    fn user_defined_claude_keyword_overrides_default() {
        let ctx = RouteContext::with_mentions(&[]);
        let mut hints = AgentRoleHints::default();
        hints.claude_keywords.push("megamerge".to_string());

        assert_eq!(classify("megamerge please", &ctx), AgentTarget::Default);
        assert_eq!(
            classify_with_hints("megamerge please", &ctx, &hints),
            AgentTarget::Claude
        );
    }

    #[test]
    fn user_defined_file_thresholds_take_effect() {
        // Raise min_files_for_claude to 3 — 2 mentions should no longer trip
        // the multi-file rule.
        let mentions = [file(false), file(false)];
        let ctx = RouteContext::with_mentions(&mentions);
        let mut hints = AgentRoleHints::default();
        hints.min_files_for_claude = 3;
        hints.max_files_for_codex = 2;

        // Plain text + 2 mentions + hint shift → falls through past Claude;
        // no codex keyword, so Default.
        assert_eq!(
            classify_with_hints("hello", &ctx, &hints),
            AgentTarget::Default
        );
        // With a codex keyword and 2 mentions still <= max_files_for_codex.
        assert_eq!(
            classify_with_hints("fix these", &ctx, &hints),
            AgentTarget::Codex
        );
    }
}
