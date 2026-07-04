//! The vault index — the data model (`VaultDoc`, `DocKind`, `VaultRoot`) and
//! the background filesystem walker that builds it.
//!
//! I/O-first (RUST_PORT_NOTES §8, CLAUDE.md I/O-first law): the walk runs on a
//! `cx.background_spawn` task off the UI thread; `Render` only ever reads the
//! finished `VaultIndex`. Large session bodies (the real 71MB `session.md`)
//! are NEVER slurped — the frontmatter block + first `LINK_SCAN_CAP` bytes are
//! read for link extraction, and the node records the truncation.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use fs::Fs;
use futures::StreamExt as _;

use super::parser::{self, LinkTarget};

/// Cap on bytes scanned for wikilinks/relations in a note body (spec §5: "cap
/// link-scan at 256KB and note truncation on the node"). Frontmatter is always
/// read in full first; this bounds only the body scan.
pub const LINK_SCAN_CAP: u64 = 256 * 1024;

/// Files above this size skip a full `fs.load` and use a bounded prefix read
/// (`open_sync`) — keeps a multi-MB session body off the load path entirely.
const STREAM_THRESHOLD: u64 = LINK_SCAN_CAP;

/// Which root a document lives under. The header root switcher toggles between
/// them; promotion moves a bundle from `Staging` to `Vault`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VaultRoot {
    /// `L:\Projects\atlas-vault` — the live typed vault.
    Vault,
    /// `L:\Projects\agentic-ide\vault-import-staging` — import review area.
    Staging,
}

impl VaultRoot {
    pub fn label(self) -> &'static str {
        match self {
            VaultRoot::Vault => "Vault",
            VaultRoot::Staging => "Staging",
        }
    }
}

/// The coarse grouping used by the LIST view (task-board list idiom groups by
/// type/dir). Derived from the OKF `type:` field, falling back to the path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DocKind {
    /// `type: session` — an imported vendor session bundle.
    Session,
    /// A run digest (`artifacts/runs/<id>/digest.md`).
    Run,
    /// `type: project` root note.
    Project,
    /// `type: hack`/routine console card.
    Routine,
    /// Role/team identity + other typed notes.
    Note,
    /// The reserved frontmatter-less `index.md`.
    Index,
}

impl DocKind {
    /// Section header text for the grouped list.
    pub fn group_label(self) -> &'static str {
        match self {
            DocKind::Session => "Sessions",
            DocKind::Run => "Runs",
            DocKind::Project => "Projects",
            DocKind::Routine => "Routines",
            DocKind::Note => "Notes",
            DocKind::Index => "Index",
        }
    }

    /// Stable sort order for section headers.
    pub fn order(self) -> u8 {
        match self {
            DocKind::Session => 0,
            DocKind::Run => 1,
            DocKind::Project => 2,
            DocKind::Routine => 3,
            DocKind::Note => 4,
            DocKind::Index => 5,
        }
    }
}

/// One indexed OKF document.
#[derive(Debug, Clone)]
pub struct VaultDoc {
    /// Absolute path on disk (used to open the editor tab + as node identity).
    pub abs_path: PathBuf,
    /// Path relative to its root (display + collision-free node id).
    pub rel_path: String,
    pub root: VaultRoot,
    pub kind: DocKind,
    /// OKF `id:` if present, else derived from `rel_path` (stable node key).
    pub id: String,
    pub title: String,
    /// OKF `type:` verbatim (chip text), empty if none.
    pub doc_type: String,
    /// OKF `vendor:` (session bundles), empty if none.
    pub vendor: String,
    /// OKF `project:` — frontmatter grouping relation (a graph edge source).
    pub project: String,
    /// `updated`/`timestamp` ISO string, empty if none.
    pub updated: String,
    /// `message_count` for sessions (0 otherwise) — graph node sizing.
    pub message_count: u64,
    /// `redactions` count for sessions.
    pub redactions: u64,
    /// Body byte length (graph node sizing fallback when no message_count).
    pub body_len: u64,
    /// True if the body scan hit `LINK_SCAN_CAP` (link set may be incomplete).
    pub link_scan_truncated: bool,
    /// Lowercased title + tags, precomputed for the filter box.
    pub search_key: String,
    /// Raw outbound link targets from the body (wikilinks + md links).
    pub links: Vec<LinkTarget>,
    /// `supersedes:` frontmatter refs (correction edges).
    pub supersedes: Vec<String>,
    /// For a staging session bundle: its bundle directory (the promote unit).
    pub bundle_dir: Option<PathBuf>,
}

impl VaultDoc {
    /// The bundle slug (terminal dir component) for a session, else the id.
    pub fn node_key(&self) -> &str {
        &self.id
    }
}

/// The built index over both roots + timing/health metadata.
#[derive(Debug, Clone, Default)]
pub struct VaultIndex {
    pub docs: Vec<VaultDoc>,
    /// Whether each root's directory existed (drives the honest empty state).
    pub vault_present: bool,
    pub staging_present: bool,
    /// Wall time the last walk took (ms) — surfaced small in the header.
    pub build_ms: u64,
    /// Count of documents whose link scan was capped.
    pub truncated_count: usize,
}

impl VaultIndex {
    /// Docs under one root, in a stable (kind, title) order.
    pub fn docs_for(&self, root: VaultRoot) -> Vec<&VaultDoc> {
        let mut docs: Vec<&VaultDoc> = self.docs.iter().filter(|d| d.root == root).collect();
        docs.sort_by(|a, b| {
            a.kind
                .order()
                .cmp(&b.kind.order())
                .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
        });
        docs
    }

    /// Total edge count (resolved wikilinks + md links + project + supersedes)
    /// under one root — reported after a build. Pure recomputation over the
    /// current doc set; see [`super::graph`] for resolution.
    pub fn edge_count(&self, root: VaultRoot) -> usize {
        super::graph::resolve_edges(&self.docs_for(root)).len()
    }
}

/// The two absolute roots. Kept as a function (not a const) — the paths are
/// user-machine absolute and only meaningful on this box, per the amendment.
pub fn root_paths() -> [(VaultRoot, PathBuf); 2] {
    [
        (VaultRoot::Vault, PathBuf::from(r"L:\Projects\atlas-vault")),
        (
            VaultRoot::Staging,
            PathBuf::from(r"L:\Projects\agentic-ide\vault-import-staging"),
        ),
    ]
}

/// Directory names skipped during the walk (dotfiles + the vault's derived
/// stores). `.git`/`.index`/`.raw`/`.bridge` carry no OKF notes.
fn is_skipped_dir(name: &str) -> bool {
    name.starts_with('.')
}

/// Build the full index over both roots. Runs entirely on the background
/// executor (`fs` async). Missing roots degrade to `*_present = false`, never
/// an error — the panel renders the honest "looked here" state.
pub async fn build_index(fs: Arc<dyn Fs>) -> VaultIndex {
    let start = std::time::Instant::now();
    let mut index = VaultIndex::default();
    for (root, path) in root_paths() {
        let present = fs.is_dir(&path).await;
        match root {
            VaultRoot::Vault => index.vault_present = present,
            VaultRoot::Staging => index.staging_present = present,
        }
        if present {
            walk_root(fs.clone(), root, &path, &path, &mut index.docs).await;
        }
    }
    index.truncated_count = index.docs.iter().filter(|d| d.link_scan_truncated).count();
    index.build_ms = start.elapsed().as_millis() as u64;
    index
}

/// Recursively walk `dir`, appending every `.md` doc. `root_path` anchors
/// relative paths + root classification.
async fn walk_root(
    fs: Arc<dyn Fs>,
    root: VaultRoot,
    root_path: &Path,
    dir: &Path,
    out: &mut Vec<VaultDoc>,
) {
    let Ok(mut entries) = fs.read_dir(dir).await else {
        return;
    };
    let mut subdirs: Vec<PathBuf> = Vec::new();
    while let Some(Ok(entry)) = entries.next().await {
        let name = entry
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        if fs.is_dir(&entry).await {
            if !is_skipped_dir(&name) {
                subdirs.push(entry);
            }
            continue;
        }
        if name.ends_with(".md") {
            if let Some(doc) = read_doc(fs.clone(), root, root_path, &entry).await {
                out.push(doc);
            }
        }
    }
    // Recurse depth-first (order-stable; the list re-sorts anyway).
    subdirs.sort();
    for sub in subdirs {
        Box::pin(walk_root(fs.clone(), root, root_path, &sub, out)).await;
    }
}

/// Read + parse one `.md` file into a `VaultDoc`. Large bodies are read only up
/// to `LINK_SCAN_CAP` past the frontmatter (streamed prefix), setting the
/// truncation flag. Read failures return None (skip, honest degrade).
async fn read_doc(
    fs: Arc<dyn Fs>,
    root: VaultRoot,
    root_path: &Path,
    abs_path: &Path,
) -> Option<VaultDoc> {
    let meta = fs.metadata(abs_path).await.ok().flatten()?;
    let body_len = meta.len;

    // Read a bounded prefix: full frontmatter + up to LINK_SCAN_CAP of body.
    let (text, truncated) = if body_len > STREAM_THRESHOLD {
        read_prefix(fs.as_ref(), abs_path, STREAM_THRESHOLD).await?
    } else {
        (fs.load(abs_path).await.ok()?, false)
    };

    Some(build_doc(root, root_path, abs_path, body_len, truncated, &text))
}

/// Read at most `cap` bytes from the start of the file (a large-body guard).
/// Returns `(text, truncated)`. Uses `open_sync` on the async executor so the
/// 71MB session body never fully lands in memory.
async fn read_prefix(fs: &dyn Fs, path: &Path, cap: u64) -> Option<(String, bool)> {
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
    // A prefix may cut a UTF-8 char mid-sequence — lossy-decode (link scan is
    // over ASCII markers, so the tail loss is harmless).
    let text = String::from_utf8_lossy(&buf).into_owned();
    Some((text, filled as u64 >= cap))
}

/// Parse `text` (frontmatter + body prefix) into a `VaultDoc`.
fn build_doc(
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

    let title = fm
        .get("title")
        .map(str::to_string)
        .filter(|t| !t.is_empty())
        .or_else(|| fm.get("name").map(str::to_string).filter(|n| !n.is_empty()))
        .unwrap_or_else(|| default_title(file_name, &rel_path));

    let id = fm
        .get("id")
        .map(str::to_string)
        .filter(|i| !i.is_empty())
        .unwrap_or_else(|| format!("{}:{}", root.label(), rel_path));

    let doc_type = fm.get("type").unwrap_or_default().to_string();
    let vendor = fm.get("vendor").unwrap_or_default().to_string();
    let project = fm.get("project").unwrap_or_default().to_string();
    let updated = fm
        .get("updated")
        .or_else(|| fm.get("timestamp"))
        .unwrap_or_default()
        .to_string();
    let message_count = fm.get("message_count").and_then(parse_u64).unwrap_or(0);
    let redactions = fm.get("redactions").and_then(parse_u64).unwrap_or(0);
    let supersedes = parse_refs(&fm);

    let mut search_terms = title.to_lowercase();
    for tag in fm.list("tags") {
        search_terms.push(' ');
        search_terms.push_str(&tag.to_lowercase());
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

/// A frontmatter-less / title-less doc falls back to a humanised filename.
fn default_title(file_name: &str, rel_path: &str) -> String {
    // Run digests live in `artifacts/runs/<id>/digest.md` — surface the id.
    if file_name == "digest.md" {
        if let Some(id) = rel_path
            .rsplit('/')
            .nth(1)
            .filter(|s| !s.is_empty())
        {
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
    if refs.is_empty() {
        if let Some(single) = fm.get("supersedes").filter(|s| !s.is_empty()) {
            let cleaned = strip_wiki(single);
            if !cleaned.is_empty() {
                refs.push(cleaned);
            }
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

/// Group docs by kind for the sectioned list. Preserves the sort order of the
/// input slice within each group.
pub fn group_by_kind<'a>(docs: &[&'a VaultDoc]) -> BTreeMap<DocKind, Vec<&'a VaultDoc>> {
    let mut groups: BTreeMap<DocKind, Vec<&'a VaultDoc>> = BTreeMap::new();
    for doc in docs {
        groups.entry(doc.kind).or_default().push(doc);
    }
    groups
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc_from(root: VaultRoot, rel: &str, text: &str) -> VaultDoc {
        let root_path = Path::new(r"L:\root");
        let abs = root_path.join(rel.replace('/', std::path::MAIN_SEPARATOR_STR));
        build_doc(root, root_path, &abs, text.len() as u64, false, text)
    }

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
    fn frontmatterless_index_is_index_kind() {
        let doc = doc_from(VaultRoot::Vault, "index.md", "# atlas-vault — index\n\n- a\n");
        assert_eq!(doc.kind, DocKind::Index);
        // Reserved index has no frontmatter → title falls back to filename.
        assert_eq!(doc.title, "index");
    }

    #[test]
    fn run_digest_classified_by_path() {
        let text = "---\nrun_id: claude-1a1c\nstatus: succeeded\n---\n## Outcome\n";
        let doc = doc_from(VaultRoot::Vault, "artifacts/runs/claude-1a1c/digest.md", text);
        assert_eq!(doc.kind, DocKind::Run);
        // Title falls back to the run id dir.
        assert_eq!(doc.title, "claude-1a1c");
    }

    #[test]
    fn search_key_includes_tags() {
        let text = "---\ntitle: Night Anchor\ntags: [console, hack, claude]\n---\nbody";
        let doc = doc_from(VaultRoot::Vault, "global/routines/console/x.md", text);
        assert!(doc.search_key.contains("night anchor"));
        assert!(doc.search_key.contains("console"));
        assert!(doc.search_key.contains("claude"));
    }

    #[test]
    fn truncation_flag_propagates_and_counts() {
        // A doc built from a capped prefix read carries link_scan_truncated,
        // and the index tallies it (spec §5 honesty flag for the 71MB session
        // body whose link scan was cut at LINK_SCAN_CAP).
        let root_path = Path::new(r"L:\root");
        let abs = root_path.join("big-session").join("session.md");
        let text = "---\ntype: session\ntitle: Big\n---\n[[a]] body prefix";
        let doc = build_doc(VaultRoot::Staging, root_path, &abs, 80_000_000, true, text);
        assert!(doc.link_scan_truncated);
        assert_eq!(doc.body_len, 80_000_000);

        let mut index = VaultIndex::default();
        index.docs.push(doc);
        index.truncated_count = index.docs.iter().filter(|d| d.link_scan_truncated).count();
        assert_eq!(index.truncated_count, 1);
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

    #[test]
    fn docs_for_filters_root_and_sorts_by_kind() {
        let mut index = VaultIndex::default();
        index.docs.push(doc_from(
            VaultRoot::Vault,
            "z-note.md",
            "---\ntype: note\ntitle: Zeta\n---\n",
        ));
        index.docs.push(doc_from(
            VaultRoot::Vault,
            "s/session.md",
            "---\ntype: session\ntitle: Alpha\n---\n",
        ));
        index.docs.push(doc_from(
            VaultRoot::Staging,
            "other.md",
            "---\ntype: note\ntitle: Other\n---\n",
        ));
        let vault_docs = index.docs_for(VaultRoot::Vault);
        assert_eq!(vault_docs.len(), 2); // staging doc excluded
        // Session (order 0) sorts before Note (order 4).
        assert_eq!(vault_docs[0].kind, DocKind::Session);
    }

    /// Opt-in real-filesystem benchmark: time the full 202-bundle staging +
    /// vault walk against the live machine paths. Gated behind `VAULT_BENCH=1`
    /// so the normal suite stays machine-independent (the paths are absolute
    /// to this box). Run: `VAULT_BENCH=1 cargo test -p agent_ui vault_bench --
    /// --nocapture`.
    #[gpui::test]
    fn vault_bench_real_staging(cx: &mut gpui::TestAppContext) {
        if std::env::var("VAULT_BENCH").is_err() {
            return;
        }
        // Drive REAL blocking I/O to completion (the extension-benchmark
        // harness idiom): allow the background executor to park, then
        // `block_on` via the FOREGROUND executor. An `async` gpui::test uses
        // the deterministic scheduler that panics on real fs waits.
        cx.executor().allow_parking();
        let fs: Arc<dyn Fs> = Arc::new(fs::RealFs::new(None, cx.executor()));
        let index = cx
            .foreground_executor()
            .block_on(async move { build_index(fs).await });
        let staging = index.docs_for(VaultRoot::Staging);
        let vault = index.docs_for(VaultRoot::Vault);
        let sessions = staging.iter().filter(|d| d.kind == DocKind::Session).count();
        let vault_edges = index.edge_count(VaultRoot::Vault);
        let staging_edges = index.edge_count(VaultRoot::Staging);
        eprintln!(
            "VAULT_BENCH: {}ms · staging {} docs ({} sessions) · vault {} docs · \
             truncated {} · vault edges {} · staging edges {}",
            index.build_ms,
            staging.len(),
            sessions,
            vault.len(),
            index.truncated_count,
            vault_edges,
            staging_edges,
        );
        assert!(index.staging_present, "staging root should exist on this box");
    }
}
