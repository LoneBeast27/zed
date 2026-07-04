//! The composer `/` typeahead (S1) — parse layer.
//!
//! PURE parsing of the composer's text + cursor offset into a
//! [`SlashContext`]: whether a `/token` is being typed at the cursor (the
//! menu trigger), what the token is (the filter), and which vendor — if any —
//! is forced earlier in the message (the scoping key, per the resolved
//! collision rule, contract §2). No cx, no editor — unit-tested against string
//! fixtures. The render + selection wiring lives in
//! [`super::typeahead_menu`]; dispatch in [`super::dispatch`].

use crate::commands::Vendor;

/// The parsed `/` state at the cursor. `None` (from [`slash_context`]) means
/// no menu: the cursor isn't inside a slash token at a token boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlashContext {
    /// The token typed after `/`, up to the cursor (may be empty right after
    /// `/`). This is the filter query.
    pub query: String,
    /// Byte range of the `/token` in the source text (the `/` through the
    /// cursor), so selection can splice the completion in.
    pub start: usize,
    pub end: usize,
    /// A `@vendor` forced earlier in the message — the scoping key.
    pub forced_vendor: Option<Vendor>,
}

/// Detect a `/`-typeahead context at `cursor` (a byte offset into `text`).
///
/// The menu opens when the cursor sits inside a `/token` whose `/` is at a
/// TOKEN START — i.e. at the very start of the message, or immediately after
/// whitespace. This matches the web's `/` typeahead trigger and avoids firing
/// on `and/or`, file paths (`src/foo`), etc.
///
/// The token runs from `/` to the first whitespace at/after the cursor; the
/// query is `/`→cursor. A `@vendor` word anywhere before the token forces that
/// vendor into scope.
pub fn slash_context(text: &str, cursor: usize) -> Option<SlashContext> {
    let cursor = cursor.min(text.len());
    let before = &text[..cursor];

    // Find the last `/` that begins a token (start-of-text or post-whitespace)
    // and whose token still contains the cursor (no whitespace between it and
    // the cursor).
    let slash = token_start_slash(before)?;

    // The query is everything from just past `/` to the cursor; reject if it
    // already contains whitespace (the token ended before the cursor).
    let query = &before[slash + 1..];
    if query.contains(char::is_whitespace) {
        return None;
    }

    // The token's end extends to the next whitespace at/after the cursor (so a
    // completion replaces the whole token, not just the typed prefix).
    let end = cursor
        + text[cursor..]
            .find(char::is_whitespace)
            .unwrap_or(text.len() - cursor);

    Some(SlashContext {
        query: query.to_string(),
        start: slash,
        end,
        forced_vendor: forced_vendor(&text[..slash]),
    })
}

/// The byte index of the last `/` in `before` that starts a token AND has no
/// whitespace between it and the end of `before` (the cursor). `None` if the
/// cursor isn't in such a token.
fn token_start_slash(before: &str) -> Option<usize> {
    // Walk back from the cursor: the token is the trailing run of
    // non-whitespace. It must begin with `/`, and the char before it must be
    // whitespace or nothing.
    let trailing_start = before
        .char_indices()
        .rev()
        .take_while(|(_, c)| !c.is_whitespace())
        .last()
        .map(|(i, _)| i)
        .unwrap_or(before.len());
    // `trailing_start == before.len()` means the trailing char IS whitespace
    // (empty trailing run) — no token.
    if trailing_start >= before.len() {
        return None;
    }
    if !before[trailing_start..].starts_with('/') {
        return None;
    }
    Some(trailing_start)
}

/// The forced vendor from a `@vendor` prefix in `head` (the text before the
/// slash token). Mirrors `orchestrator/loop.py` `FORCE_RE = ^@(vendor)\b` — a
/// force is only honored as a LEADING prefix of the message. Also accepts a
/// composer hint prefix with leading whitespace trimmed.
pub fn forced_vendor(head: &str) -> Option<Vendor> {
    let head = head.trim_start();
    let rest = head.strip_prefix('@')?;
    let word_len = rest
        .find(|c: char| !(c.is_alphanumeric() || c == '_'))
        .unwrap_or(rest.len());
    if word_len == 0 {
        return None;
    }
    Vendor::from_word(&rest[..word_len])
}

/// Splice a chosen completion into `text`: replace the `[start, end)` slash
/// token with `/name ` (trailing space so the user types args next). Returns
/// the new text + the cursor offset (just past the inserted space). Pure —
/// the menu's selection handler applies this to the editor.
pub fn apply_completion(text: &str, ctx: &SlashContext, name: &str) -> (String, usize) {
    let insert = format!("/{name} ");
    let mut out = String::with_capacity(text.len() + insert.len());
    out.push_str(&text[..ctx.start]);
    out.push_str(&insert);
    let cursor = out.len();
    out.push_str(&text[ctx.end.min(text.len())..]);
    (out, cursor)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(text: &str) -> Option<SlashContext> {
        // Cursor at end of text (the common typing case).
        slash_context(text, text.len())
    }

    #[test]
    fn bare_slash_opens_empty_query() {
        let c = ctx("/").unwrap();
        assert_eq!(c.query, "");
        assert_eq!(c.start, 0);
        assert_eq!(c.forced_vendor, None);
    }

    #[test]
    fn typing_filters_by_query() {
        let c = ctx("/comp").unwrap();
        assert_eq!(c.query, "comp");
        assert_eq!(c.start, 0);
        assert_eq!(c.end, 5);
    }

    #[test]
    fn slash_must_start_a_token() {
        // Mid-word slash (file path / and-or) does NOT open the menu.
        assert_eq!(ctx("src/foo"), None);
        assert_eq!(ctx("and/or"), None);
    }

    #[test]
    fn slash_after_whitespace_opens() {
        let c = ctx("@claude /pri").unwrap();
        assert_eq!(c.query, "pri");
        assert_eq!(c.forced_vendor, Some(Vendor::Claude));
    }

    #[test]
    fn whitespace_after_token_closes_menu() {
        // Once the token is completed (space typed), no menu.
        assert_eq!(ctx("/plan "), None);
        assert_eq!(ctx("/plan do the thing"), None);
    }

    #[test]
    fn forced_vendor_only_as_leading_prefix() {
        // @vendor mid-message is not a force (matches FORCE_RE ^@).
        let c = ctx("ask @codex then /rev").unwrap();
        assert_eq!(c.forced_vendor, None);
        // Leading @codex forces.
        let c = ctx("@codex /rev").unwrap();
        assert_eq!(c.forced_vendor, Some(Vendor::Codex));
    }

    #[test]
    fn cursor_midtoken_extends_end_over_the_whole_token() {
        // Cursor after "/pl", token is "/plan" — end covers the whole token.
        let text = "/plan";
        let c = slash_context(text, 3).unwrap();
        assert_eq!(c.query, "pl");
        assert_eq!(c.start, 0);
        assert_eq!(c.end, 5, "end extends to the token's real end");
    }

    #[test]
    fn apply_completion_splices_name_and_space() {
        let text = "/comp";
        let c = ctx(text).unwrap();
        let (out, cursor) = apply_completion(text, &c, "compact");
        assert_eq!(out, "/compact ");
        assert_eq!(cursor, out.len());
    }

    #[test]
    fn apply_completion_preserves_forced_prefix_and_tail() {
        let text = "@claude /rev tail";
        // Cursor right after "/rev".
        let c = slash_context(text, 12).unwrap();
        let (out, cursor) = apply_completion(text, &c, "review");
        assert_eq!(out, "@claude /review  tail");
        // Cursor lands just past the inserted "/review ".
        assert_eq!(&out[..cursor], "@claude /review ");
    }

    #[test]
    fn empty_text_no_menu() {
        assert_eq!(ctx(""), None);
        assert_eq!(ctx("plain message"), None);
    }
}
