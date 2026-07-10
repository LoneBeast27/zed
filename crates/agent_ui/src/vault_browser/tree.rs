//! Folder-tree derivation for the LIST view — the Obsidian file-explorer
//! idiom (vault-UX pass 2026-07-10): the vault's REAL hierarchy as collapsible
//! sections with counts, machine dirs invisible (the walker already skips
//! dot-dirs), plus a synthetic "Recent" section on top (last 5 by `updated`).
//!
//! Pure derivation: docs in → flattened row vec out (header rows + doc rows),
//! so one `uniform_list` virtualizes the whole tree at a single row height
//! (the list.rs overdraw law). Collapse state is a key set owned by the
//! panel; a live filter AUTO-EXPANDS (matches must never hide behind a
//! collapsed folder) and drops the Recent section (matches show in place).

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use gpui::SharedString;

use super::index::{VaultDoc, VaultRoot};

/// How many docs the synthetic Recent section shows.
pub const RECENT_COUNT: usize = 5;

/// Where a doc sits in the tree: a ranked top section + optional subsection.
/// Ranks order the top level (projects first — the human's active work — then
/// runs/routines, vault plumbing last).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SectionPath {
    pub rank: u8,
    pub top: String,
    pub sub: Option<String>,
    /// Pinned docs (a project's root note) list before the section's subs.
    pub pinned: bool,
}

/// One flattened list row: a collapsible section header or a document.
#[derive(Clone)]
pub enum Row {
    Header {
        /// Stable collapse key ("Vault/agentic-ide/Notes").
        key: SharedString,
        label: SharedString,
        count: usize,
        depth: usize,
        collapsed: bool,
    },
    Doc(Arc<VaultDoc>, usize),
}

/// Map a doc's `rel_path` to its tree section — the vault's real hierarchy
/// with human labels, machine `<id>`/`knowledge` intermediates folded away.
pub fn section_path(doc: &VaultDoc) -> SectionPath {
    let seg: Vec<&str> = doc.rel_path.split('/').collect();
    match doc.root {
        VaultRoot::Vault => vault_section(&seg),
        VaultRoot::Staging => staging_section(&seg),
    }
}

fn vault_section(seg: &[&str]) -> SectionPath {
    let section = |rank, top: &str, sub: Option<&str>, pinned| SectionPath {
        rank,
        top: top.to_string(),
        sub: sub.map(str::to_string),
        pinned,
    };
    match seg {
        // projects/<slug>/… — one section per project, subfoldered the way
        // the vault actually nests (notes/briefs/routines), with the
        // machine-y `knowledge/` intermediate folded away.
        ["projects", slug, "project.md"] => section(10, slug, None, true),
        ["projects", slug, "knowledge", "notes", ..] => section(10, slug, Some("Notes"), false),
        ["projects", slug, "briefs", ..] => section(10, slug, Some("Briefs"), false),
        ["projects", slug, "routines", ..] => section(10, slug, Some("Routines"), false),
        ["projects", slug, ..] => section(10, slug, None, false),
        // artifacts/runs/<id>/digest.md — the <id> dir level carries exactly
        // one digest, so the section lists digests directly.
        ["artifacts", "runs", ..] => section(20, "Run digests", None, false),
        ["artifacts", ..] => section(20, "Artifacts", None, false),
        ["global", "routines", ..] => section(30, "Routines", None, false),
        ["global", ..] => section(35, "Global", None, false),
        ["agents", "teams", ..] => section(40, "Agents", Some("Teams"), false),
        ["agents", ..] => section(40, "Agents", None, false),
        // Loose root files (README, the generated index, AXIOMS-pending).
        [_] => section(60, "Vault files", None, false),
        // Unmapped top dir (e.g. skills/, usage-history/, a future add):
        // humanise the first segment — new content self-organises instead
        // of vanishing.
        _ => section(50, &humanise(seg[0]), None, false),
    }
}

fn staging_section(seg: &[&str]) -> SectionPath {
    match seg {
        // imported/<vendor>/<bundle>/session.md — group by vendor.
        ["imported", vendor, ..] => SectionPath {
            rank: 10,
            top: humanise(vendor),
            sub: None,
            pinned: false,
        },
        [_] => SectionPath {
            rank: 50,
            top: "Staged files".to_string(),
            sub: None,
            pinned: false,
        },
        _ => SectionPath {
            rank: 40,
            top: humanise(seg[0]),
            sub: None,
            pinned: false,
        },
    }
}

/// "usage-history" → "Usage history", "claude_code" → "Claude code".
fn humanise(segment: &str) -> String {
    let spaced = segment.replace(['-', '_'], " ");
    let mut chars = spaced.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => spaced,
    }
}

/// Flatten docs into tree rows. `filtering` = the filter box is non-empty
/// (caller already filtered `docs`): auto-expand everything and skip Recent.
pub fn flatten_rows(
    docs: &[&VaultDoc],
    root: VaultRoot,
    filtering: bool,
    collapsed: &HashSet<SharedString>,
) -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::with_capacity(docs.len() + 8);
    let is_collapsed =
        |key: &SharedString| -> bool { !filtering && collapsed.contains(key) };

    // ── Recent (synthetic, top) ──
    if !filtering {
        let mut dated: Vec<&VaultDoc> = docs.iter().copied().filter(|d| !d.updated.is_empty()).collect();
        dated.sort_by(|a, b| b.updated.cmp(&a.updated).then(a.title.cmp(&b.title)));
        dated.truncate(RECENT_COUNT);
        if !dated.is_empty() {
            let key: SharedString = format!("{}/Recent", root.label()).into();
            let hidden = is_collapsed(&key);
            rows.push(Row::Header {
                key: key.clone(),
                label: "Recent".into(),
                count: dated.len(),
                depth: 0,
                collapsed: hidden,
            });
            if !hidden {
                for doc in dated {
                    rows.push(Row::Doc(Arc::new(doc.clone()), 1));
                }
            }
        }
    }

    // ── The folder tree ──
    // (rank, top) → (direct docs, sub → docs). BTreeMap = stable human order.
    #[allow(clippy::type_complexity)]
    let mut tops: BTreeMap<(u8, String), (Vec<(bool, &VaultDoc)>, BTreeMap<String, Vec<&VaultDoc>>)> =
        BTreeMap::new();
    for doc in docs.iter().copied() {
        let path = section_path(doc);
        let entry = tops.entry((path.rank, path.top)).or_default();
        match path.sub {
            Some(sub) => entry.1.entry(sub).or_default().push(doc),
            None => entry.0.push((path.pinned, doc)),
        }
    }

    for ((_, top), (mut direct, subs)) in tops {
        let count = direct.len() + subs.values().map(Vec::len).sum::<usize>();
        let top_key: SharedString = format!("{}/{}", root.label(), top).into();
        let top_collapsed = is_collapsed(&top_key);
        rows.push(Row::Header {
            key: top_key.clone(),
            label: top.clone().into(),
            count,
            depth: 0,
            collapsed: top_collapsed,
        });
        if top_collapsed {
            continue;
        }
        // Pinned first (a project's root note), then title order.
        direct.sort_by(|(ap, a), (bp, b)| {
            bp.cmp(ap)
                .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
        });
        for (_, doc) in direct {
            rows.push(Row::Doc(Arc::new(doc.clone()), 1));
        }
        for (sub, mut sub_docs) in subs {
            let sub_key: SharedString = format!("{}/{}/{}", root.label(), top, sub).into();
            let sub_collapsed = is_collapsed(&sub_key);
            rows.push(Row::Header {
                key: sub_key,
                label: sub.into(),
                count: sub_docs.len(),
                depth: 1,
                collapsed: sub_collapsed,
            });
            if sub_collapsed {
                continue;
            }
            sub_docs.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));
            for doc in sub_docs {
                rows.push(Row::Doc(Arc::new(doc.clone()), 2));
            }
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault_browser::docmeta::doc_from;

    fn vault_doc(rel: &str) -> VaultDoc {
        doc_from(
            VaultRoot::Vault,
            rel,
            "---\ntype: note\ntitle: T\nupdated: \"2026-07-01T00:00:00Z\"\n---\nb",
        )
    }

    #[test]
    fn sections_mirror_the_real_vault_hierarchy() {
        let cases = [
            ("projects/agentic-ide/project.md", "agentic-ide", None, true),
            (
                "projects/agentic-ide/knowledge/notes/how.md",
                "agentic-ide",
                Some("Notes"),
                false,
            ),
            ("projects/default/briefs/2026-07-08.md", "default", Some("Briefs"), false),
            ("projects/agentic-ide/routines/morning.md", "agentic-ide", Some("Routines"), false),
            ("artifacts/runs/claude-1a1c/digest.md", "Run digests", None, false),
            ("global/routines/console/h01.md", "Routines", None, false),
            ("agents/designer/IDENTITY.md", "Agents", None, false),
            ("agents/teams/build-verify-fix.md", "Agents", Some("Teams"), false),
            ("index.md", "Vault files", None, false),
            ("usage-history/2026-07.md", "Usage history", None, false),
        ];
        for (rel, top, sub, pinned) in cases {
            let path = section_path(&vault_doc(rel));
            assert_eq!(path.top, top, "top for {rel}");
            assert_eq!(path.sub.as_deref(), sub, "sub for {rel}");
            assert_eq!(path.pinned, pinned, "pinned for {rel}");
        }
    }

    #[test]
    fn staging_groups_by_vendor() {
        let doc = doc_from(
            VaultRoot::Staging,
            "imported/claude_code/x/session.md",
            "---\ntype: session\ntitle: S\n---\nb",
        );
        let path = section_path(&doc);
        assert_eq!(path.top, "Claude code");
        assert_eq!(path.sub, None);
    }

    #[test]
    fn recent_lists_last_five_newest_first() {
        let docs: Vec<VaultDoc> = (0..8)
            .map(|i| {
                doc_from(
                    VaultRoot::Vault,
                    &format!("projects/default/briefs/n{i}.md"),
                    &format!("---\ntitle: N{i}\nupdated: \"2026-07-0{}T00:00:00Z\"\n---\nb", i + 1),
                )
            })
            .collect();
        let refs: Vec<&VaultDoc> = docs.iter().collect();
        let rows = flatten_rows(&refs, VaultRoot::Vault, false, &HashSet::new());
        // First row is the Recent header, capped at RECENT_COUNT children.
        let Row::Header { label, count, .. } = &rows[0] else {
            panic!("expected Recent header first");
        };
        assert_eq!(label.as_ref(), "Recent");
        assert_eq!(*count, RECENT_COUNT);
        // Newest (N7, updated 07-08) leads.
        let Row::Doc(doc, _) = &rows[1] else { panic!("expected doc row") };
        assert_eq!(doc.title, "N7");
    }

    #[test]
    fn collapsed_section_hides_children_and_filter_expands() {
        let docs = [
            vault_doc("projects/agentic-ide/knowledge/notes/a.md"),
            vault_doc("projects/agentic-ide/knowledge/notes/b.md"),
        ];
        let refs: Vec<&VaultDoc> = docs.iter().collect();
        let mut collapsed = HashSet::new();
        collapsed.insert(SharedString::from("Vault/agentic-ide"));
        let rows = flatten_rows(&refs, VaultRoot::Vault, false, &collapsed);
        // Recent header + docs, then the collapsed top header with NO children.
        let doc_rows_after_top = rows
            .iter()
            .skip_while(|r| !matches!(r, Row::Header { label, .. } if label.as_ref() == "agentic-ide"))
            .skip(1)
            .count();
        assert_eq!(doc_rows_after_top, 0, "collapsed top must hide sub+docs");

        // Filtering ignores the collapse (matches never hide) and drops Recent.
        let rows = flatten_rows(&refs, VaultRoot::Vault, true, &collapsed);
        assert!(
            !rows
                .iter()
                .any(|r| matches!(r, Row::Header { label, .. } if label.as_ref() == "Recent")),
            "filtering drops the Recent section"
        );
        assert!(
            rows.iter().any(|r| matches!(r, Row::Doc(..))),
            "filtering auto-expands collapsed sections"
        );
    }

    #[test]
    fn top_counts_include_subsection_docs() {
        let docs = [
            vault_doc("projects/agentic-ide/project.md"),
            vault_doc("projects/agentic-ide/knowledge/notes/a.md"),
            vault_doc("projects/agentic-ide/briefs/b.md"),
        ];
        let refs: Vec<&VaultDoc> = docs.iter().collect();
        let rows = flatten_rows(&refs, VaultRoot::Vault, true, &HashSet::new());
        let Some(Row::Header { count, .. }) = rows
            .iter()
            .find(|r| matches!(r, Row::Header { label, .. } if label.as_ref() == "agentic-ide"))
        else {
            panic!("expected the project header");
        };
        assert_eq!(*count, 3);
    }
}
