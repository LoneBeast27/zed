//! Doc construction — parse one note's text (frontmatter + body prefix) into
//! a [`VaultDoc`]: kind classification, HUMAN title resolution, and the
//! secondary meta (run id / status / schedule). Split from `index.rs` (the
//! walker) to hold the 500-line ceiling — this file owns "what a doc says",
//! the walker owns "which files exist".
//!
//! Title law (vault-UX pass 2026-07-10, user ruling "human titles first"):
//! a row NEVER leads with a machine id. Resolution order:
//! 1. frontmatter `title:` (or `name:`),
//! 2. run digests: the frontmatter `outcome:` line,
//! 3. the body's first markdown heading,
//! 4. humanised filename (run digests: the run-id directory — last resort).
//! The run id / status / schedule land in dedicated fields so the row can
//! show them SMALL, as secondary meta, instead of as the title.

use std::path::Path;

use super::index::{DocKind, VaultDoc, VaultRoot};
use super::parser;

/// Parse `text` (frontmatter + body prefix) into a `VaultDoc`.
pub(super) fn build_doc(
    root: VaultRoot,
    root_path: &Path,
    abs_path: &Path,
    body_len: u64,
    truncated: bool,
    text: &str,
) -> VaultDoc {
    let fm = parser::parse_frontmatter(text);
    let body = &text[fm.body_offset.min(text.len())..];
    let links = parser::extract_links(body);

    let rel_path = abs_path
        .strip_prefix(root_path)
        .unwrap_or(abs_path)
        .to_string_lossy()
        .replace('\\', "/");

    let file_name = abs_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();

    let kind = classify(&fm, file_name, &rel_path);
    let run_id = run_id_for(&fm, kind, &rel_path);
    let title = resolve_title(&fm, kind, body, file_name, &rel_path);

    let id = fm
        .get("id")
        .map(str::to_string)
        .filter(|i| !i.is_empty())
        .unwrap_or_else(|| format!("{}:{}", root.label(), rel_path));

    let doc_type = fm.get("type").unwrap_or_default().to_string();
    let vendor = fm.get("vendor").unwrap_or_default().to_string();
    let project = fm.get("project").unwrap_or_default().to_string();
    let status = fm.get("status").unwrap_or_default().to_string();
    let schedule = fm.get("schedule").unwrap_or_default().to_string();
    let updated = fm
        .get("updated")
        .or_else(|| fm.get("timestamp"))
        .unwrap_or_default()
        .to_string();
    let message_count = fm.get("message_count").and_then(parse_u64).unwrap_or(0);
    let redactions = fm.get("redactions").and_then(parse_u64).unwrap_or(0);
    let supersedes = parse_refs(&fm);

    // The filter box matches title + tags + path segments + run id, so the
    // one search field finds a doc by ANY name the user knows it by
    // (Obsidian's quick-switcher matches paths too).
    let mut search_terms = title.to_lowercase();
    for tag in fm.list("tags") {
        search_terms.push(' ');
        search_terms.push_str(&tag.to_lowercase());
    }
    search_terms.push(' ');
    search_terms.push_str(&rel_path.replace('/', " ").to_lowercase());
    if !run_id.is_empty() {
        search_terms.push(' ');
        search_terms.push_str(&run_id.to_lowercase());
    }

    // A staging session's promote unit is its bundle directory (the parent of
    // `session.md`).
    let bundle_dir = (root == VaultRoot::Staging && kind == DocKind::Session)
        .then(|| abs_path.parent().map(Path::to_path_buf))
        .flatten();

    VaultDoc {
        abs_path: abs_path.to_path_buf(),
        rel_path,
        root,
        kind,
        id,
        title,
        doc_type,
        vendor,
        project,
        run_id,
        status,
        schedule,
        updated,
        message_count,
        redactions,
        body_len,
        link_scan_truncated: truncated,
        search_key: search_terms,
        links,
        supersedes,
        bundle_dir,
    }
}

/// Classify a doc by OKF type first, path second.
fn classify(fm: &parser::Frontmatter, file_name: &str, rel_path: &str) -> DocKind {
    if file_name == "index.md" && !fm.has_frontmatter {
        return DocKind::Index;
    }
    match fm.get("type").unwrap_or_default() {
        "session" => DocKind::Session,
        "project" => DocKind::Project,
        "hack" | "routine" => DocKind::Routine,
        _ => {
            // Path-based fallbacks for notes without a type field.
            if rel_path.contains("artifacts/runs/") && file_name == "digest.md" {
                DocKind::Run
            } else if rel_path.contains("routines/") {
                DocKind::Routine
            } else if file_name == "index.md" {
                DocKind::Index
            } else {
                DocKind::Note
            }
        }
    }
}

/// Human title resolution (see the module doc's title law).
fn resolve_title(
    fm: &parser::Frontmatter,
    kind: DocKind,
    body: &str,
    file_name: &str,
    rel_path: &str,
) -> String {
    if let Some(title) = fm
        .get("title")
        .or_else(|| fm.get("name"))
        .filter(|t| !t.is_empty())
    {
        return title.to_string();
    }
    // Run digests carry their one-line summary in `outcome:` — that IS the
    // human title ("Echo task — replied BOARD-LIVE DONE"), the hash is meta.
    if kind == DocKind::Run
        && let Some(outcome) = fm.get("outcome").filter(|o| !o.is_empty())
    {
        return outcome.to_string();
    }
    if let Some(heading) = first_heading(body) {
        return heading;
    }
    default_title(file_name, rel_path)
}

/// The run-id (small secondary meta on run rows). Frontmatter `run_id:` first,
/// else the `artifacts/runs/<id>/` directory component.
fn run_id_for(fm: &parser::Frontmatter, kind: DocKind, rel_path: &str) -> String {
    if kind != DocKind::Run {
        return String::new();
    }
    if let Some(id) = fm.get("run_id").filter(|i| !i.is_empty()) {
        return id.to_string();
    }
    rel_path
        .rsplit('/')
        .nth(1)
        .unwrap_or_default()
        .to_string()
}

/// The first markdown heading in the body (fence-skipping, like the link
/// extractor) — the title fallback for frontmatter-less docs (README, the
/// generated index).
fn first_heading(body: &str) -> Option<String> {
    let mut in_fence = false;
    for line in body.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        let hashes = trimmed.len() - trimmed.trim_start_matches('#').len();
        if (1..=6).contains(&hashes) {
            let rest = &trimmed[hashes..];
            if let Some(text) = rest.strip_prefix(' ') {
                let text = text.trim().trim_end_matches('#').trim();
                if !text.is_empty() {
                    return Some(text.to_string());
                }
            }
        }
    }
    None
}

/// A frontmatter-less / title-less doc falls back to a humanised filename.
fn default_title(file_name: &str, rel_path: &str) -> String {
    // Run digests live in `artifacts/runs/<id>/digest.md` — surface the id
    // (last resort only; outcome/heading resolve first).
    if file_name == "digest.md" {
        if let Some(id) = rel_path.rsplit('/').nth(1).filter(|s| !s.is_empty()) {
            return id.to_string();
        }
    }
    file_name.trim_end_matches(".md").replace(['-', '_'], " ")
}

/// Parse a scalar u64 (frontmatter values are unquoted by the parser).
fn parse_u64(s: &str) -> Option<u64> {
    s.trim().parse().ok()
}

/// Collect `supersedes:` refs (scalar `[[id]]` or a list). Wiki-bracket
/// wrapping is stripped to the bare ref.
fn parse_refs(fm: &parser::Frontmatter) -> Vec<String> {
    let mut refs: Vec<String> = fm
        .list("supersedes")
        .iter()
        .map(|r| strip_wiki(r))
        .filter(|r| !r.is_empty())
        .collect();
    if refs.is_empty()
        && let Some(single) = fm.get("supersedes").filter(|s| !s.is_empty())
    {
        let cleaned = strip_wiki(single);
        if !cleaned.is_empty() {
            refs.push(cleaned);
        }
    }
    refs
}

/// `[[ref]]` → `ref`; `ref` → `ref`.
fn strip_wiki(raw: &str) -> String {
    raw.trim()
        .trim_start_matches("[[")
        .trim_end_matches("]]")
        .trim()
        .to_string()
}

#[cfg(test)]
pub(super) fn doc_from(root: VaultRoot, rel: &str, text: &str) -> VaultDoc {
    let root_path = Path::new(r"L:\root");
    let abs = root_path.join(rel.replace('/', std::path::MAIN_SEPARATOR_STR));
    build_doc(root, root_path, &abs, text.len() as u64, false, text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_session_bundle() {
        let text = "---\ntype: session\ntitle: Build sudoku\nvendor: claude_code\n\
                    message_count: 1959\nredactions: 0\nproject: docker-speed-drive\n---\nbody";
        let doc = doc_from(VaultRoot::Staging, "imported/claude_code/x/session.md", text);
        assert_eq!(doc.kind, DocKind::Session);
        assert_eq!(doc.vendor, "claude_code");
        assert_eq!(doc.message_count, 1959);
        assert_eq!(doc.project, "docker-speed-drive");
        // A staging session carries its bundle dir (promote unit).
        assert!(doc.bundle_dir.is_some());
    }

    #[test]
    fn run_digest_titles_from_outcome_not_hash() {
        // The real digest shape: no `title:`, but an `outcome:` one-liner.
        let text = "---\nrun_id: claude-1a1cfefd83\nstatus: succeeded\n\
                    outcome: \"Echo task — replied BOARD-LIVE DONE as instructed\"\n---\n\
                    ## Outcome\nbody";
        let doc = doc_from(VaultRoot::Vault, "artifacts/runs/claude-1a1cfefd83/digest.md", text);
        assert_eq!(doc.kind, DocKind::Run);
        // HUMAN title first — the outcome line, not the hash.
        assert_eq!(doc.title, "Echo task — replied BOARD-LIVE DONE as instructed");
        // The hash is secondary meta.
        assert_eq!(doc.run_id, "claude-1a1cfefd83");
        assert_eq!(doc.status, "succeeded");
        // The filter still finds it by run id (search key carries it).
        assert!(doc.search_key.contains("claude-1a1cfefd83"));
    }

    #[test]
    fn run_digest_without_outcome_falls_to_heading_then_id() {
        let heading = "---\nrun_id: claude-9\nstatus: failed\n---\n## What happened\nb";
        let doc = doc_from(VaultRoot::Vault, "artifacts/runs/claude-9/digest.md", heading);
        assert_eq!(doc.title, "What happened");

        let bare = "---\nstatus: failed\n---\nno heading here";
        let doc2 = doc_from(VaultRoot::Vault, "artifacts/runs/claude-9/digest.md", bare);
        // Last resort: the run-id dir. run_id falls back to the path too.
        assert_eq!(doc2.title, "claude-9");
        assert_eq!(doc2.run_id, "claude-9");
    }

    #[test]
    fn routine_carries_schedule_and_status_meta() {
        let text = "---\nid: X\ntype: hack\ntitle: \"Night-shift window anchor (~23:00)\"\n\
                    schedule: \"daily 23:00\"\nstatus: active\n---\nbody";
        let doc = doc_from(VaultRoot::Vault, "global/routines/console/h01.md", text);
        assert_eq!(doc.kind, DocKind::Routine);
        assert_eq!(doc.title, "Night-shift window anchor (~23:00)");
        assert_eq!(doc.schedule, "daily 23:00");
        assert_eq!(doc.status, "active");
    }

    #[test]
    fn frontmatterless_doc_titles_from_first_heading() {
        let doc = doc_from(VaultRoot::Vault, "index.md", "# atlas-vault — index\n\n- a\n");
        assert_eq!(doc.kind, DocKind::Index);
        // First heading beats the raw filename (note-app title law).
        assert_eq!(doc.title, "atlas-vault — index");
    }

    #[test]
    fn first_heading_skips_code_fences() {
        let body = "```\n# not a title\n```\ntext\n## Real heading ##\n";
        assert_eq!(first_heading(body), Some("Real heading".to_string()));
        assert_eq!(first_heading("no headings"), None);
    }

    #[test]
    fn search_key_includes_tags_and_path() {
        let text = "---\ntitle: Night Anchor\ntags: [console, hack, claude]\n---\nbody";
        let doc = doc_from(VaultRoot::Vault, "global/routines/console/x.md", text);
        assert!(doc.search_key.contains("night anchor"));
        assert!(doc.search_key.contains("console"));
        assert!(doc.search_key.contains("claude"));
        // Path segments are searchable (folder-name filtering).
        assert!(doc.search_key.contains("routines"));
    }

    #[test]
    fn supersedes_relation_parses_scalar_and_list() {
        let scalar = "---\ntype: note\ntitle: New\nsupersedes: \"[[old-id]]\"\n---\nb";
        let doc = doc_from(VaultRoot::Vault, "n.md", scalar);
        assert_eq!(doc.supersedes, vec!["old-id".to_string()]);

        let list = "---\ntype: note\ntitle: New\nsupersedes: [[[a]], [[b]]]\n---\nb";
        let doc2 = doc_from(VaultRoot::Vault, "n2.md", list);
        assert!(doc2.supersedes.contains(&"a".to_string()));
        assert!(doc2.supersedes.contains(&"b".to_string()));
    }
}
