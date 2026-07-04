//! Session-bundle promotion: move a staged import bundle
//! (`vault-import-staging/imported/<vendor>/<slug>/`) INTO the live vault at
//! `<vault>/imported/<vendor>/<slug>/` (amendment-4 item 4). Filesystem rename
//! (dir move); collision → numeric suffix; NEVER a silent overwrite. No
//! deletions in v1 (the source is moved, not copied+removed by us — `fs.rename`
//! is the atomic move).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context as _, Result};
use fs::{Fs, RenameOptions};

/// The live vault root the promote target is anchored under.
const VAULT_ROOT: &str = r"L:\Projects\atlas-vault";

/// Compute the promotion target dir for a staged bundle. The bundle keeps its
/// `<vendor>/<slug>` tail (taken from the staged path under `imported/`), so
/// `staging/imported/claude_code/2026-…-abc/` →
/// `vault/imported/claude_code/2026-…-abc/`.
///
/// If the tail can't be derived (bundle not under an `imported/` segment), fall
/// back to `imported/<bundle-dir-name>`.
pub fn target_dir(bundle_dir: &Path) -> PathBuf {
    let vault = Path::new(VAULT_ROOT);
    let tail = tail_under_imported(bundle_dir)
        .unwrap_or_else(|| bundle_dir.file_name().unwrap_or_default().into());
    vault.join("imported").join(tail)
}

/// Extract the `<vendor>/<slug>` portion after the `imported` path segment.
fn tail_under_imported(bundle_dir: &Path) -> Option<PathBuf> {
    let mut comps = bundle_dir.components();
    // Advance until we pass an "imported" component.
    let mut found = false;
    while let Some(comp) = comps.next() {
        if comp.as_os_str().eq_ignore_ascii_case("imported") {
            found = true;
            break;
        }
    }
    if !found {
        return None;
    }
    let tail: PathBuf = comps.as_path().to_path_buf();
    (!tail.as_os_str().is_empty()).then_some(tail)
}

/// Given a desired target dir and an existence check, produce a collision-free
/// dir by appending `-2`, `-3`, … to the final component. Pure — the caller
/// supplies `exists`, so this is unit-testable without a filesystem.
pub fn collision_free(target: &Path, mut exists: impl FnMut(&Path) -> bool) -> PathBuf {
    if !exists(target) {
        return target.to_path_buf();
    }
    let parent = target.parent().unwrap_or_else(|| Path::new(""));
    let stem = target
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("bundle");
    for n in 2..10_000 {
        let candidate = parent.join(format!("{stem}-{n}"));
        if !exists(&candidate) {
            return candidate;
        }
    }
    // Absurd fallback (10k collisions) — timestamp-suffix to guarantee unique.
    parent.join(format!("{stem}-{}", std::time::UNIX_EPOCH.elapsed().map_or(0, |d| d.as_nanos())))
}

/// Per-bundle promote UI state (the inline two-click arm/confirm affordance —
/// no modal framework, spec §4). Keyed by the doc id in [`PromoteState`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BundleState {
    /// Default — shows the "Promote" button.
    Rest,
    /// First click landed — shows "Confirm"/"Cancel" (the arm step).
    Armed,
    /// The move is running on the background executor.
    InFlight,
    /// Landed in the vault. `collided` = a numeric suffix was applied.
    Promoted { collided: bool },
    /// The move failed — the (short) error is shown inline.
    Failed(String),
}

/// The panel-owned map of per-bundle promote states. Entries default to
/// [`BundleState::Rest`] (absent = rest), so only touched bundles allocate.
#[derive(Debug, Clone, Default)]
pub struct PromoteState {
    states: HashMap<String, BundleState>,
}

impl PromoteState {
    /// State for a bundle id (absent → Rest).
    pub fn state_for(&self, key: &str) -> BundleState {
        self.states.get(key).cloned().unwrap_or(BundleState::Rest)
    }

    /// Set a bundle's state; `Rest` prunes the entry (map tracks only the
    /// non-default set).
    pub fn set(&mut self, key: String, state: BundleState) {
        if state == BundleState::Rest {
            self.states.remove(&key);
        } else {
            self.states.insert(key, state);
        }
    }
}

/// The outcome of a promote — surfaced to the panel to mark state / report.
#[derive(Debug, Clone)]
pub struct PromoteOutcome {
    /// Where the bundle actually landed (post collision-resolution).
    pub landed_at: PathBuf,
    /// True if a numeric suffix was applied (collision was hit).
    pub renamed_for_collision: bool,
}

/// Move a staged bundle dir into the vault. Resolves collisions by suffixing;
/// creates parent dirs; refuses to overwrite (RenameOptions.overwrite=false).
/// Runs on the async executor (caller spawns it).
pub async fn promote_bundle(fs: Arc<dyn Fs>, bundle_dir: PathBuf) -> Result<PromoteOutcome> {
    anyhow::ensure!(
        fs.is_dir(&bundle_dir).await,
        "staged bundle not found: {bundle_dir:?}"
    );
    let desired = target_dir(&bundle_dir);

    // Resolve collisions with a real fs existence probe. `collision_free` is
    // sync over a closure, so gather existence up front is not possible; probe
    // sequentially instead (rare path — a handful of iterations at most).
    let landed = collision_free_async(fs.as_ref(), &desired).await;
    let renamed_for_collision = landed != desired;

    fs.rename(
        &bundle_dir,
        &landed,
        RenameOptions {
            overwrite: false,
            ignore_if_exists: false,
            create_parents: true,
        },
    )
    .await
    .with_context(|| format!("promote {bundle_dir:?} → {landed:?}"))?;

    Ok(PromoteOutcome {
        landed_at: landed,
        renamed_for_collision,
    })
}

/// Async twin of [`collision_free`] using a live fs existence probe.
async fn collision_free_async(fs: &dyn Fs, target: &Path) -> PathBuf {
    if !path_exists(fs, target).await {
        return target.to_path_buf();
    }
    let parent = target.parent().unwrap_or_else(|| Path::new(""));
    let stem = target
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("bundle");
    for n in 2..10_000 {
        let candidate = parent.join(format!("{stem}-{n}"));
        if !path_exists(fs, &candidate).await {
            return candidate;
        }
    }
    parent.join(format!(
        "{stem}-{}",
        std::time::UNIX_EPOCH.elapsed().map_or(0, |d| d.as_nanos())
    ))
}

/// Whether a path exists (dir or file) — a bundle target is a dir, but a
/// stray file collision still counts.
async fn path_exists(fs: &dyn Fs, path: &Path) -> bool {
    fs.metadata(path).await.ok().flatten().is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn target_preserves_vendor_slug_tail() {
        let bundle = Path::new(
            r"L:\Projects\agentic-ide\vault-import-staging\imported\claude_code\2026-05-15-sudoku-abc",
        );
        let target = target_dir(bundle);
        // Lands under the vault's imported/<vendor>/<slug>.
        assert!(target.to_string_lossy().replace('\\', "/").ends_with(
            "atlas-vault/imported/claude_code/2026-05-15-sudoku-abc"
        ));
    }

    #[test]
    fn target_falls_back_to_bundle_name_without_imported_segment() {
        let bundle = Path::new(r"L:\some\other\place\my-bundle");
        let target = target_dir(bundle);
        assert!(target
            .to_string_lossy()
            .replace('\\', "/")
            .ends_with("atlas-vault/imported/my-bundle"));
    }

    #[test]
    fn collision_free_returns_target_when_clear() {
        let target = Path::new(r"L:\vault\imported\claude_code\sess");
        let picked = collision_free(target, |_| false);
        assert_eq!(picked, target);
    }

    #[test]
    fn collision_free_suffixes_on_conflict() {
        let target = Path::new(r"L:\vault\imported\claude_code\sess");
        // The target and -2 exist; -3 is free.
        let mut taken: HashSet<PathBuf> = HashSet::new();
        taken.insert(target.to_path_buf());
        taken.insert(target.parent().unwrap().join("sess-2"));
        let picked = collision_free(target, |p| taken.contains(p));
        assert_eq!(picked, target.parent().unwrap().join("sess-3"));
    }

    #[test]
    fn collision_free_never_overwrites_existing() {
        // Everything up to -5 taken → picks -6, never the original.
        let target = Path::new(r"L:\vault\imported\x\b");
        let mut taken: HashSet<PathBuf> = HashSet::new();
        taken.insert(target.to_path_buf());
        for n in 2..=5 {
            taken.insert(target.parent().unwrap().join(format!("b-{n}")));
        }
        let picked = collision_free(target, |p| taken.contains(p));
        assert_eq!(picked, target.parent().unwrap().join("b-6"));
        assert_ne!(picked, target);
    }
}
