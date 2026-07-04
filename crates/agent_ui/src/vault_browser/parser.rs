//! Pure OKF-note parsing — the hand-rolled flat-YAML frontmatter reader plus
//! the link extractor. No `serde_yaml`/`regex` dependency (neither is in
//! `agent_ui`'s Cargo, and the amendment-4 fields are flat scalars/lists);
//! this is a small deterministic scanner instead.
//!
//! OKF conformance (MEMORY_SYSTEM §2.1): every note opens with a `---` fenced
//! frontmatter block of flat `key: value` / `key: [a, b]` pairs (the
//! `session.md` shape in FORMATS.md), except the reserved `index.md` which is
//! frontmatter-less. Everything here tolerates malformed/absent input and
//! defaults every field — a note that fails to parse still lists (honest
//! degrade), it never panics the indexer.

use std::collections::BTreeMap;

/// Parsed frontmatter: the flat key→value map plus the byte offset where the
/// body begins (so link extraction can skip the header). `has_frontmatter`
/// distinguishes a genuine empty header from a frontmatter-less file (the
/// reserved `index.md`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Frontmatter {
    pub fields: BTreeMap<String, String>,
    /// List-valued fields (e.g. `tags: [imported, claude_code]`), stored
    /// pre-split. Scalar and list live in separate maps so `tags` reads as a
    /// vec while `title` reads as a string.
    pub lists: BTreeMap<String, Vec<String>>,
    pub has_frontmatter: bool,
    /// Byte offset in the source where the body (post-`---`) starts.
    pub body_offset: usize,
}

impl Frontmatter {
    /// A scalar field, unquoted/untrimmed-value already handled by the parser.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.fields.get(key).map(String::as_str)
    }

    /// A list field (`tags`, or a comma/bracket list). Empty if absent.
    pub fn list(&self, key: &str) -> &[String] {
        self.lists.get(key).map_or(&[], Vec::as_slice)
    }
}

/// Parse the leading frontmatter block. Recognises a `---`-fenced YAML-subset
/// header at byte 0; anything else is treated as a frontmatter-less document
/// (`has_frontmatter = false`, `body_offset = 0`).
///
/// Grammar handled (the whole OKF authoring surface, per MEMORY_SYSTEM §2 +
/// the real `session.md`):
/// - `key: scalar` — value trimmed, surrounding quotes stripped.
/// - `key: [a, b, c]` — inline flow list → `lists`.
/// - `key:` then `  - item` block-sequence lines → `lists` (the `guards:`
///   shape in the live routine notes).
/// - blank lines and `# comment` lines skipped.
/// Malformed lines (no colon, not a sequence item) are skipped silently.
pub fn parse_frontmatter(source: &str) -> Frontmatter {
    let mut fm = Frontmatter::default();
    // A frontmatter block must start at the very first line with `---`.
    let Some(rest) = strip_open_fence(source) else {
        return fm;
    };
    let open_len = source.len() - rest.len();

    // Find the closing `---` (or `...`) line.
    let mut consumed = 0usize; // bytes consumed within `rest`
    let mut lines = rest.split_inclusive('\n');
    let mut body_offset = source.len(); // default: no closing fence → whole file was header
    let mut pending_list_key: Option<String> = None;
    let mut found_close = false;

    for line in lines.by_ref() {
        let line_start = consumed;
        consumed += line.len();
        let trimmed = line.trim_end_matches(['\n', '\r']);
        let content = trimmed.trim();

        if content == "---" || content == "..." {
            body_offset = open_len + consumed;
            found_close = true;
            break;
        }
        if content.is_empty() || content.starts_with('#') {
            continue;
        }

        // Block-sequence continuation: `  - item` under a `key:` with no
        // inline value.
        if let Some(item) = as_sequence_item(line) {
            if let Some(key) = &pending_list_key {
                fm.lists.entry(key.clone()).or_default().push(item);
                continue;
            }
            // Orphan sequence item (no owning key) — skip.
            continue;
        }
        // Any non-indented (or key-shaped) line closes an open sequence.
        pending_list_key = None;

        let Some((key, value)) = split_key_value(content) else {
            let _ = line_start; // (kept for clarity; offsets tracked via consumed)
            continue;
        };
        let key = key.to_string();
        if value.is_empty() {
            // `key:` alone → opens a block sequence (items on following lines).
            pending_list_key = Some(key.clone());
            // Register an (empty) list so `list()` returns a slice even if no
            // items follow.
            fm.lists.entry(key).or_default();
        } else if let Some(items) = parse_inline_list(value) {
            fm.lists.insert(key, items);
        } else {
            fm.fields.insert(key, unquote(value).to_string());
        }
    }

    fm.has_frontmatter = true;
    fm.body_offset = if found_close { body_offset } else { source.len() };
    fm
}

/// If `source` opens with a `---` fence line, return the remainder after it.
fn strip_open_fence(source: &str) -> Option<&str> {
    // Tolerate a UTF-8 BOM (FORMATS.md: "tolerate one").
    let src = source.strip_prefix('\u{feff}').unwrap_or(source);
    let first_line_end = src.find('\n').map_or(src.len(), |i| i + 1);
    let first = src[..first_line_end].trim_end_matches(['\n', '\r']).trim();
    if first == "---" {
        Some(&src[first_line_end..])
    } else {
        None
    }
}

/// `  - value` → `Some("value")`; anything else → None. Requires leading
/// whitespace then `- ` (block-sequence indent).
fn as_sequence_item(line: &str) -> Option<String> {
    let indent_len = line.len() - line.trim_start().len();
    if indent_len == 0 {
        return None;
    }
    let body = line.trim();
    let item = body.strip_prefix("- ").or_else(|| body.strip_prefix("-"))?;
    // A map-shaped sequence item (`- key: val`) reduces to its whole text for
    // v1 — we only need scalar list members (tags, kinds).
    Some(unquote(item.trim()).to_string())
}

/// Split `key: value` at the FIRST colon-space (or trailing colon). Returns
/// None if there's no colon (malformed line → skipped by the caller).
fn split_key_value(content: &str) -> Option<(&str, &str)> {
    // A bare `key:` (block-sequence opener) has a trailing colon, no value.
    if let Some(key) = content.strip_suffix(':') {
        if !key.contains(':') || is_plain_key(key) {
            return Some((key.trim(), ""));
        }
    }
    let idx = content.find(':')?;
    let (key, rest) = content.split_at(idx);
    let value = rest[1..].trim();
    Some((key.trim(), value))
}

/// Whether `s` looks like a plain unquoted key (no spaces/special chars) —
/// used to disambiguate a trailing-colon block opener from a value with a
/// colon in it.
fn is_plain_key(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// `[a, b, c]` → `Some(vec!["a","b","c"])`; non-bracketed → None. Empty
/// `[]` → `Some(vec![])`.
fn parse_inline_list(value: &str) -> Option<Vec<String>> {
    let inner = value.strip_prefix('[')?.strip_suffix(']')?;
    let inner = inner.trim();
    if inner.is_empty() {
        return Some(Vec::new());
    }
    Some(
        inner
            .split(',')
            .map(|item| unquote(item.trim()).to_string())
            .filter(|item| !item.is_empty())
            .collect(),
    )
}

/// Strip a single pair of matching surrounding quotes.
fn unquote(value: &str) -> &str {
    let bytes = value.as_bytes();
    if bytes.len() >= 2 {
        let (first, last) = (bytes[0], bytes[bytes.len() - 1]);
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return &value[1..value.len() - 1];
        }
    }
    value
}

/// An extracted outbound link target from a note body.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum LinkTarget {
    /// `[[wikilink]]` — the raw inner text (may carry `#anchor`/`|alias`,
    /// stripped to the note ref).
    Wiki(String),
    /// `[text](relative.md)` — the relative path (as written; `.md` only).
    Md(String),
}

/// Extract outbound links from a note body, SKIPPING fenced code blocks
/// (```/~~~) and inline-code spans (`` `…` ``). This is the anti-false-edge
/// guard the spec calls out: real staging session bodies contain shell like
/// `[[ $type == agent.message ]]` inside code fences that must NOT become
/// graph edges.
///
/// Both `[[wiki]]` and `[text](path.md)` forms are recognised (the live vault
/// `index.md` uses the markdown-link form; typed notes may use wikilinks).
pub fn extract_links(body: &str) -> Vec<LinkTarget> {
    let mut out = Vec::new();
    let mut in_fence = false;
    let mut fence_marker = "";
    for line in body.lines() {
        let trimmed = line.trim_start();
        // Toggle fenced code blocks on ``` or ~~~ (matching marker length ≥3).
        if let Some(marker) = fence_open(trimmed) {
            if in_fence {
                // Closing fence must match the opener's kind.
                if trimmed.starts_with(fence_marker) {
                    in_fence = false;
                    fence_marker = "";
                }
            } else {
                in_fence = true;
                fence_marker = marker;
            }
            continue;
        }
        if in_fence {
            continue;
        }
        extract_line_links(line, &mut out);
    }
    out
}

/// If `trimmed` opens/closes a code fence, return the fence marker slice
/// (```` ``` ```` or `~~~`, possibly longer). Only the leading run counts.
fn fence_open(trimmed: &str) -> Option<&'static str> {
    if trimmed.starts_with("```") {
        Some("```")
    } else if trimmed.starts_with("~~~") {
        Some("~~~")
    } else {
        None
    }
}

/// Scan one line (already known to be outside a fence) for links, skipping
/// inline `` `code` `` spans.
fn extract_line_links(line: &str, out: &mut Vec<LinkTarget>) {
    let bytes = line.as_bytes();
    let mut i = 0;
    let mut in_code = false;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'`' {
            in_code = !in_code;
            i += 1;
            continue;
        }
        if in_code {
            i += 1;
            continue;
        }
        // `[[wiki]]`
        if b == b'[' && i + 1 < bytes.len() && bytes[i + 1] == b'[' {
            if let Some(end) = line[i + 2..].find("]]") {
                let inner = &line[i + 2..i + 2 + end];
                if let Some(target) = clean_wiki(inner) {
                    out.push(LinkTarget::Wiki(target));
                }
                i = i + 2 + end + 2;
                continue;
            }
        }
        // `[text](target)`
        if b == b'[' {
            if let Some(link) = parse_md_link(&line[i..]) {
                let (consumed, target) = link;
                if let Some(t) = clean_md(&target) {
                    out.push(LinkTarget::Md(t));
                }
                i += consumed;
                continue;
            }
        }
        i += 1;
    }
}

/// Parse a `[text](target)` starting at `s[0] == '['`. Returns
/// `(bytes_consumed, target)`. The text span must not contain a nested
/// unescaped `]` before `](`.
fn parse_md_link(s: &str) -> Option<(usize, String)> {
    // Skip the `[[wiki]]` case (handled by the caller).
    if s.as_bytes().get(1) == Some(&b'[') {
        return None;
    }
    let close_text = s.find("](")?;
    // No `[` should re-open inside the text span (keeps it a flat link).
    if s[1..close_text].contains('[') {
        return None;
    }
    let after = &s[close_text + 2..];
    let close_paren = after.find(')')?;
    let target = &after[..close_paren];
    let consumed = close_text + 2 + close_paren + 1;
    Some((consumed, target.to_string()))
}

/// Normalise a wikilink inner: drop `|alias` and `#anchor`, trim. Empty →
/// None.
fn clean_wiki(inner: &str) -> Option<String> {
    let base = inner.split('|').next().unwrap_or(inner);
    let base = base.split('#').next().unwrap_or(base).trim();
    (!base.is_empty()).then(|| base.to_string())
}

/// Keep only local `.md` markdown-link targets (skip http(s), anchors,
/// images-by-scheme). Strips a trailing `#anchor`.
fn clean_md(target: &str) -> Option<String> {
    let target = target.trim();
    if target.is_empty()
        || target.starts_with("http://")
        || target.starts_with("https://")
        || target.starts_with('#')
        || target.starts_with("mailto:")
    {
        return None;
    }
    let path = target.split('#').next().unwrap_or(target).trim();
    // Edges are between OKF notes → only `.md` targets carry a relation.
    path.ends_with(".md").then(|| path.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── frontmatter ──

    #[test]
    fn parses_the_real_session_frontmatter() {
        // The exact shape from a real staging bundle (FORMATS.md / index.rs).
        let src = "---\nid: 01KWNYB1YKSR1S15AJ5933VC1X\ntype: session\n\
                   title: Build challenging sudoku puzzle generator\n\
                   created: \"2026-05-15T20:40:26.474Z\"\n\
                   tags: [imported, claude_code]\nproject: docker-speed-drive\n\
                   okf_version: \"0.1\"\nvendor: claude_code\nmessage_count: 1959\n\
                   redactions: 0\nredaction_kinds: []\n---\n## User\nbody here";
        let fm = parse_frontmatter(src);
        assert!(fm.has_frontmatter);
        assert_eq!(fm.get("type"), Some("session"));
        assert_eq!(fm.get("vendor"), Some("claude_code"));
        // Quotes stripped from the ISO created stamp.
        assert_eq!(fm.get("created"), Some("2026-05-15T20:40:26.474Z"));
        assert_eq!(fm.get("message_count"), Some("1959"));
        assert_eq!(fm.list("tags"), &["imported", "claude_code"]);
        // Empty inline list → present but empty.
        assert!(fm.list("redaction_kinds").is_empty());
        // Body offset lands past the closing fence.
        assert_eq!(&src[fm.body_offset..], "## User\nbody here");
    }

    #[test]
    fn frontmatterless_document_reports_no_header() {
        // The reserved index.md has no frontmatter.
        let src = "# atlas-vault — index\n\n- a note (path.md)\n";
        let fm = parse_frontmatter(src);
        assert!(!fm.has_frontmatter);
        assert_eq!(fm.body_offset, 0);
        assert_eq!(fm.get("type"), None);
    }

    #[test]
    fn malformed_frontmatter_degrades_without_panicking() {
        // Unterminated header (no closing ---): parse what's there, whole file
        // becomes the header (body empty) — never a panic.
        let src = "---\ntype: note\ntitle broken line no colon\n: orphan\n\nmore";
        let fm = parse_frontmatter(src);
        assert!(fm.has_frontmatter);
        assert_eq!(fm.get("type"), Some("note"));
        // The malformed lines are skipped, not captured.
        assert_eq!(fm.get("title broken line no colon"), None);
    }

    #[test]
    fn block_sequence_list_parses() {
        // The `guards:` shape from the live routine notes.
        let src = "---\ntype: hack\nguards:\n  - no_active_window\n  - headroom_gt\n---\nbody";
        let fm = parse_frontmatter(src);
        assert_eq!(fm.list("guards"), &["no_active_window", "headroom_gt"]);
    }

    #[test]
    fn tolerates_bom() {
        let src = "\u{feff}---\ntype: note\n---\nbody";
        let fm = parse_frontmatter(src);
        assert!(fm.has_frontmatter);
        assert_eq!(fm.get("type"), Some("note"));
    }

    #[test]
    fn value_with_internal_colon_is_kept() {
        // A ToS line has a colon in the value — split on the FIRST colon only.
        let src = "---\ntos: \"Anthropic Terms: permitted\"\n---\nb";
        let fm = parse_frontmatter(src);
        assert_eq!(fm.get("tos"), Some("Anthropic Terms: permitted"));
    }

    // ── link extraction (fenced-code skip is the load-bearing guard) ──

    #[test]
    fn extracts_wikilinks_and_md_links() {
        let body = "See [[other-note]] and the [index](../index.md) for more.";
        let links = extract_links(body);
        assert!(links.contains(&LinkTarget::Wiki("other-note".to_string())));
        assert!(links.contains(&LinkTarget::Md("../index.md".to_string())));
    }

    #[test]
    fn skips_wikilinks_inside_fenced_code() {
        // The real false-positive from staging session bodies: shell inside a
        // code fence must NOT become a graph edge.
        let body = "Prose [[real-link]].\n```bash\nif [[ $type == agent.message ]]; then\n\
                    echo [[not-a-link]]\nfi\n```\nMore prose [[second-link]].";
        let links = extract_links(body);
        assert!(links.contains(&LinkTarget::Wiki("real-link".to_string())));
        assert!(links.contains(&LinkTarget::Wiki("second-link".to_string())));
        // The fenced shell `[[ ]]` and echo must be skipped.
        assert!(!links.iter().any(|l| matches!(l, LinkTarget::Wiki(t) if t.contains("$type"))));
        assert!(!links.contains(&LinkTarget::Wiki("not-a-link".to_string())));
        assert_eq!(links.len(), 2);
    }

    #[test]
    fn skips_links_inside_inline_code() {
        let body = "Real [[link]] but `[[inline-code-link]]` is skipped.";
        let links = extract_links(body);
        assert_eq!(links, vec![LinkTarget::Wiki("link".to_string())]);
    }

    #[test]
    fn skips_tilde_fences_too() {
        let body = "[[a]]\n~~~\n[[b]]\n~~~\n[[c]]";
        let links = extract_links(body);
        assert_eq!(
            links,
            vec![
                LinkTarget::Wiki("a".to_string()),
                LinkTarget::Wiki("c".to_string())
            ]
        );
    }

    #[test]
    fn wiki_alias_and_anchor_are_stripped() {
        let body = "[[note#section|Display Text]]";
        let links = extract_links(body);
        assert_eq!(links, vec![LinkTarget::Wiki("note".to_string())]);
    }

    #[test]
    fn md_links_skip_http_and_non_md() {
        let body = "[web](https://example.com) [img](pic.png) [note](real.md)";
        let links = extract_links(body);
        // Only the .md local link survives.
        assert_eq!(links, vec![LinkTarget::Md("real.md".to_string())]);
    }
}
