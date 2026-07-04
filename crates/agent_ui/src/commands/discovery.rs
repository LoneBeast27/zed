//! Dynamic discovery of the user's LOCAL commands + skills (S0).
//!
//! Walks the four discovery roots the contract names (§S0) and yields one
//! [`CommandEntry`] per file — NAME + first description line only, never the
//! body:
//!
//! - `~/.claude/commands/*.md`            → custom commands (name = stem)
//! - `~/.claude/skills/*/SKILL.md`        → user skills
//! - `<workspace>/.claude/commands/*.md`  → project custom commands
//! - `<workspace>/.claude/skills/*/SKILL.md`   → project skills
//! - `<workspace>/.agents/skills/*/SKILL.md`   → shared skills root
//!   (the tree gemini + agy + the harness all read — contract §1.3)
//!
//! Runs on a background task (streamed prefix reads, never the full body —
//! RUST_PORT_NOTES §8 Lightness). All rows are [`Mechanism::ClaudePrefix`]:
//! the shared-root strategy surfaces skills identically for every vendor, and
//! the claude worker's `-p` expands `/name args` for any of them (S2).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use fs::Fs;
use futures::StreamExt as _;

use super::types::{Classification, CommandEntry, CommandKind, Mechanism, Vendor};
use crate::vault_browser::parser::parse_frontmatter;

/// Cap the prefix read of any discovered file — the description lives in the
/// frontmatter / first lines, never deep in the body.
const DESC_PREFIX_CAP: u64 = 4096;

/// A discovered command root: where to walk + how to interpret its files.
struct DiscoveryRoot {
    dir: PathBuf,
    kind: CommandKind,
    /// `true` = `*/SKILL.md` subdirs (name = subdir); `false` = `*.md` files
    /// directly in `dir` (name = file stem).
    skill_layout: bool,
}

/// Build the discovery root list for a workspace root (or `None` when there is
/// no workspace — then only the `~/.claude` user roots are walked).
pub fn discovery_roots(home: &Path, workspace_root: Option<&Path>) -> Vec<PathBuf> {
    roots(home, workspace_root)
        .into_iter()
        .map(|root| root.dir)
        .collect()
}

fn roots(home: &Path, workspace_root: Option<&Path>) -> Vec<DiscoveryRoot> {
    let mut roots = vec![
        DiscoveryRoot {
            dir: home.join(".claude").join("commands"),
            kind: CommandKind::Custom,
            skill_layout: false,
        },
        DiscoveryRoot {
            dir: home.join(".claude").join("skills"),
            kind: CommandKind::Skill,
            skill_layout: true,
        },
    ];
    if let Some(ws) = workspace_root {
        roots.push(DiscoveryRoot {
            dir: ws.join(".claude").join("commands"),
            kind: CommandKind::Custom,
            skill_layout: false,
        });
        roots.push(DiscoveryRoot {
            dir: ws.join(".claude").join("skills"),
            kind: CommandKind::Skill,
            skill_layout: true,
        });
        roots.push(DiscoveryRoot {
            dir: ws.join(".agents").join("skills"),
            kind: CommandKind::Skill,
            skill_layout: true,
        });
    }
    roots
}

/// Discover every local command/skill. Runs on the caller's background task.
/// De-dupes by name (project + shared roots often carry the same skill — the
/// LATER root wins, matching the vendor override precedence, e.g. the user's
/// `.agents/skills/skill-creator` overriding a built-in). Read failures skip a
/// file silently (honest degrade — a missing dir is the common case).
pub async fn discover(
    fs: Arc<dyn Fs>,
    home: PathBuf,
    workspace_root: Option<PathBuf>,
) -> Vec<CommandEntry> {
    let mut by_name: Vec<CommandEntry> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for root in roots(&home, workspace_root.as_deref()) {
        let mut found = Vec::new();
        if root.skill_layout {
            walk_skill_dirs(fs.as_ref(), &root.dir, root.kind, &mut found).await;
        } else {
            walk_command_files(fs.as_ref(), &root.dir, root.kind, &mut found).await;
        }
        for entry in found {
            let key = entry.name.to_string();
            if seen.insert(key.clone()) {
                by_name.push(entry);
            } else if let Some(slot) = by_name.iter_mut().find(|e| e.name == entry.name) {
                *slot = entry; // later root overrides (shared root wins)
            }
        }
    }
    by_name.sort_by(|a, b| a.name.cmp(&b.name));
    by_name
}

/// Walk `dir/*/SKILL.md` — one skill per immediate subdirectory (name = subdir
/// name). Non-recursive past the one level (skills are `<name>/SKILL.md`).
async fn walk_skill_dirs(
    fs: &dyn Fs,
    dir: &Path,
    kind: CommandKind,
    out: &mut Vec<CommandEntry>,
) {
    let Ok(mut entries) = fs.read_dir(dir).await else {
        return;
    };
    while let Some(Ok(entry)) = entries.next().await {
        if !fs.is_dir(&entry).await {
            continue;
        }
        let Some(name) = entry.file_name().and_then(|n| n.to_str()).map(str::to_owned) else {
            continue;
        };
        let skill_md = entry.join("SKILL.md");
        let description = read_description(fs, &skill_md).await;
        if let Some(description) = description {
            out.push(make_entry(name, kind, description));
        }
    }
}

/// Walk `dir/*.md` — one custom command per file (name = file stem).
async fn walk_command_files(
    fs: &dyn Fs,
    dir: &Path,
    kind: CommandKind,
    out: &mut Vec<CommandEntry>,
) {
    let Ok(mut entries) = fs.read_dir(dir).await else {
        return;
    };
    while let Some(Ok(entry)) = entries.next().await {
        if fs.is_dir(&entry).await {
            continue;
        }
        let file_name = entry.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        let Some(stem) = file_name.strip_suffix(".md") else {
            continue;
        };
        if stem.is_empty() {
            continue;
        }
        let description = read_description(fs, &entry)
            .await
            .unwrap_or_else(|| format!("Custom command ({file_name})."));
        out.push(make_entry(stem.to_owned(), kind, description));
    }
}

/// One discovered row — always a claude-prefix passthrough skill/custom
/// command (the shared-root strategy: any vendor's `-p` expands it, but the
/// claude lane is the wired one).
fn make_entry(name: String, kind: CommandKind, description: String) -> CommandEntry {
    CommandEntry {
        name: name.into(),
        vendor: Some(Vendor::Claude),
        kind,
        classification: Classification::Passthrough,
        mechanism: Mechanism::ClaudePrefix,
        description: description.into(),
    }
}

/// Read the file's description: the frontmatter `description` field if present,
/// else the first non-empty, non-heading, non-frontmatter prose line. Only a
/// bounded prefix is read (never the full body). `None` when the file is
/// unreadable/absent.
async fn read_description(fs: &dyn Fs, path: &Path) -> Option<String> {
    let text = read_prefix(fs, path, DESC_PREFIX_CAP).await?;
    Some(extract_description(&text))
}

/// Pull a one-line description out of a SKILL.md / command markdown prefix.
/// Pure — unit-tested directly. Frontmatter `description:` wins; else the first
/// prose line after any frontmatter block; else a generic fallback.
pub fn extract_description(text: &str) -> String {
    let fm = parse_frontmatter(text);
    if let Some(desc) = fm.get("description") {
        let desc = desc.trim();
        if !desc.is_empty() {
            return one_line(desc);
        }
    }
    let body = &text[fm.body_offset.min(text.len())..];
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with("---") {
            continue;
        }
        return one_line(line);
    }
    "No description.".to_string()
}

/// Collapse to a single trimmed line, capped so the typeahead row stays tidy.
fn one_line(s: &str) -> String {
    let first = s.lines().next().unwrap_or(s).trim();
    let capped: String = first.chars().take(120).collect();
    capped
}

/// Bounded prefix read (mirrors the vault index idiom — never slurps a body).
async fn read_prefix(fs: &dyn Fs, path: &Path, cap: u64) -> Option<String> {
    use std::io::Read as _;
    let mut reader = fs.open_sync(path).await.ok()?;
    let mut buf = vec![0u8; cap as usize];
    let mut filled = 0usize;
    while filled < buf.len() {
        match reader.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(_) => break,
        }
    }
    buf.truncate(filled);
    Some(String::from_utf8_lossy(&buf).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frontmatter_description_wins() {
        let md = "---\nname: prime\ndescription: Prime the agent with codebase understanding.\n---\n\n# Prime\n\nbody text";
        assert_eq!(
            extract_description(md),
            "Prime the agent with codebase understanding."
        );
    }

    #[test]
    fn falls_back_to_first_prose_line() {
        let md = "# Commit\n\nCreate a new commit for all of our uncommitted changes.\n";
        assert_eq!(
            extract_description(md),
            "Create a new commit for all of our uncommitted changes."
        );
    }

    #[test]
    fn frontmatterless_body_first_line() {
        let md = "---\ntitle: x\n---\n\n## Heading\n\nThe real description line.\n";
        assert_eq!(extract_description(md), "The real description line.");
    }

    #[test]
    fn empty_body_uses_fallback() {
        assert_eq!(extract_description("---\nname: x\n---\n\n"), "No description.");
    }

    #[test]
    fn long_description_is_capped() {
        let long = "a".repeat(300);
        let md = format!("---\ndescription: {long}\n---\n");
        assert_eq!(extract_description(&md).chars().count(), 120);
    }

    #[test]
    fn discovery_roots_include_user_and_workspace() {
        let home = Path::new("/home/u");
        let ws = Path::new("/ws");
        let roots = discovery_roots(home, Some(ws));
        assert!(roots.contains(&home.join(".claude").join("commands")));
        assert!(roots.contains(&home.join(".claude").join("skills")));
        assert!(roots.contains(&ws.join(".claude").join("skills")));
        assert!(roots.contains(&ws.join(".agents").join("skills")));
        // No workspace → only the two user roots.
        assert_eq!(discovery_roots(home, None).len(), 2);
    }
}
