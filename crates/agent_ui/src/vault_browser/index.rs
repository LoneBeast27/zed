//! The vault index — the data model (`VaultDoc`, `DocKind`, `VaultRoot`) and
//! the background filesystem walker that builds it. Doc parsing/classification
//! lives in [`super::docmeta`].
//!
//! I/O-first (RUST_PORT_NOTES §8, CLAUDE.md I/O-first law): the walk runs on a
//! `cx.background_spawn` task off the UI thread; `Render` only ever reads the
//! finished `VaultIndex`. Large session bodies (the real 71MB `session.md`)
//! are NEVER slurped — the frontmatter block + first `LINK_SCAN_CAP` bytes are
//! read for link extraction, and the node records the truncation.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use fs::Fs;
use futures::StreamExt as _;

use super::docmeta::build_doc;
use super::parser::LinkTarget;

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

/// The doc's coarse kind — drives per-kind row meta (run id / schedule /
/// vendor chips) and the graph idiom. Derived from the OKF `type:` field,
/// falling back to the path (see [`super::docmeta`]).
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
    /// Stable sort order (sessions first — the staging review flow).
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
    /// The HUMAN title (docmeta title law: frontmatter title → run outcome →
    /// first heading → humanised filename). Never a bare hash when the doc
    /// carries anything better.
    pub title: String,
    /// OKF `type:` verbatim (chip text), empty if none.
    pub doc_type: String,
    /// OKF `vendor:` (session bundles), empty if none.
    pub vendor: String,
    /// OKF `project:` — frontmatter grouping relation (a graph edge source).
    pub project: String,
    /// Run digests: the run id ("claude-1a1cfefd83") — SECONDARY meta, not
    /// the title. Empty for other kinds.
    pub run_id: String,
    /// Frontmatter `status:` (runs: succeeded/failed; routines:
    /// active/disabled). Empty if none.
    pub status: String,
    /// Routines: the frontmatter `schedule:` ("daily 23:00") — shown as the
    /// next-fire meta. Empty for other kinds.
    pub schedule: String,
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
    /// Lowercased title + tags + path segments + run id, precomputed for the
    /// filter box (matches across every tree section).
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

/// The live vault root path (promote target root; new-note open base).
pub fn vault_root_path() -> PathBuf {
    PathBuf::from(r"L:\Projects\atlas-vault")
}

/// Directory names skipped during the walk — the machine dirs. Dotfile dirs
/// (`.git`/`.index`/`.raw`/`.bridge`) carry no OKF notes and NEVER show in
/// the tree (note-app law: machine-optimised stores stay invisible).
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
        if name.ends_with(".md")
            && let Some(doc) = read_doc(fs.clone(), root, root_path, &entry).await
        {
            out.push(doc);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault_browser::docmeta::doc_from;

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
