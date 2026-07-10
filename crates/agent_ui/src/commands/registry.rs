//! The runtime command registry (S0) — the single source both the `/`
//! typeahead (S1) and `/help` (S4) render from.
//!
//! Folds the curated static seed ([`super::static_seed`]) with the bridge's
//! google command lane ([`super::google_lane`], S5 — fetched from
//! `GET /commands/help`) and the user's LOCAL commands + skills discovered on
//! disk ([`super::discovery`]). Discovery + the lane fetch run on background
//! tasks at build + on manual refresh; static rows are available synchronously
//! from the first frame (the menu is never empty while the walk is in flight,
//! and the static google stubs stand in — honestly disabled — until the
//! bridge answers).
//!
//! Filtering + scoping ([`Self::visible`]) is PURE (no cx) so the typeahead
//! fold is unit-tested directly against fixtures.

use std::path::PathBuf;
use std::sync::Arc;

use fs::Fs;
use gpui::{AppContext as _, Context, Task, WeakEntity};
use workspace::Workspace;

use super::discovery;
use super::static_seed::static_entries;
use super::types::{Classification, CommandEntry, CommandKind, Mechanism, Vendor};

/// The registry entity. Built once (per workspace) and shared; the composer
/// reads it each keystroke to filter, and `/help` reads it whole.
pub struct CommandRegistry {
    /// The curated static seed — always present from construction.
    static_rows: Vec<CommandEntry>,
    /// The bridge's live google lane (S5, `GET /commands/help`): gemini + agy
    /// rows carrying the BRIDGE's classifications. Empty until the fetch
    /// lands (the static stubs stand in, honestly disabled); overrides the
    /// stubs on (name, vendor) once it does.
    google_rows: Vec<CommandEntry>,
    /// The user's local commands/skills — populated by the background walk.
    dynamic_rows: Vec<CommandEntry>,
    /// Whether the first (or a manual-refresh) discovery walk is in flight.
    discovering: bool,
    fs: Arc<dyn Fs>,
    home: PathBuf,
    /// The hosting workspace — the discovery task resolves the project root
    /// from it LAZILY (never in `new`, which runs inside the workspace's own
    /// update: a synchronous read there re-enters the mid-update entity and
    /// panics per RUST_PORT_NOTES §11).
    workspace: WeakEntity<Workspace>,
    _discovery_task: Option<Task<()>>,
    _google_task: Option<Task<()>>,
}

impl CommandRegistry {
    /// Build the registry and kick the first background discovery walk. The
    /// static seed is live immediately; dynamic rows stream in when the walk
    /// lands (`cx.notify()` repaints the menu if open).
    pub fn new(
        fs: Arc<dyn Fs>,
        home: PathBuf,
        workspace: WeakEntity<Workspace>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self {
            static_rows: static_entries(),
            google_rows: Vec::new(),
            dynamic_rows: Vec::new(),
            discovering: false,
            fs,
            home,
            workspace,
            _discovery_task: None,
            _google_task: None,
        };
        this.refresh(cx);
        this
    }

    /// Kick (or re-kick) the background discovery walk. Idempotent-ish: a
    /// second call replaces the in-flight task (the newer walk wins). The
    /// project root is resolved inside the task (safe — the workspace is
    /// settled by the time the deferred task runs).
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.discovering = true;
        let fs = self.fs.clone();
        let home = self.home.clone();
        let workspace = self.workspace.clone();
        self._discovery_task = Some(cx.spawn(async move |this, cx| {
            let ws_root = workspace
                .read_with(cx, |workspace, cx| {
                    workspace
                        .project()
                        .read(cx)
                        .visible_worktrees(cx)
                        .next()
                        .map(|wt| wt.read(cx).abs_path().to_path_buf())
                })
                .ok()
                .flatten();
            let rows = cx
                .background_spawn(async move { discovery::discover(fs, home, ws_root).await })
                .await;
            this.update(cx, |this, cx| {
                this.dynamic_rows = rows;
                this.discovering = false;
                cx.notify();
            })
            .ok();
        }));
        self.fetch_google_lane(cx);
        cx.notify();
    }

    /// Fetch the bridge's S5 google lane (`GET /commands/help` — the full
    /// documented row set, N/A reasons included) on a background task. A
    /// failed fetch or an empty body keeps the previous rows — offline, the
    /// static stubs stay visible and honestly disabled; a stale-but-live
    /// lane beats a blanked one.
    fn fetch_google_lane(&mut self, cx: &mut Context<Self>) {
        let http_client = cx.http_client();
        self._google_task = Some(cx.spawn(async move |this, cx| {
            let rows = cx
                .background_spawn(async move {
                    let url =
                        format!("{}/commands/help", crate::bridge::BRIDGE_BASE_URL);
                    let raw = crate::bridge::fetch_json(http_client.as_ref(), &url).await?;
                    anyhow::Ok(super::google_lane::parse_rows(&raw))
                })
                .await;
            if let Ok(rows) = rows
                && !rows.is_empty()
            {
                this.update(cx, |this, cx| {
                    if this.google_rows != rows {
                        this.google_rows = rows;
                        cx.notify();
                    }
                })
                .ok();
            }
        }));
    }

    /// True while the first/refresh walk is running (a header spinner hook).
    pub fn is_discovering(&self) -> bool {
        self.discovering
    }

    /// Every row (static + google lane + dynamic), for `/help` and count
    /// reporting. Later bands override same-(name, vendor) earlier rows — the
    /// bridge's live google rows replace the static offline stubs, and a user
    /// skill named like a built-in is the user's (new outranks old, the
    /// workspace's own rule).
    pub fn all_rows(&self) -> Vec<CommandEntry> {
        fold_bands(
            self.static_rows
                .iter()
                .chain(self.google_rows.iter())
                .chain(self.dynamic_rows.iter()),
        )
    }

    /// Test seam: inject a fetched google lane without a live bridge.
    #[cfg(test)]
    pub(crate) fn set_google_rows_for_test(&mut self, rows: Vec<CommandEntry>) {
        self.google_rows = rows;
    }

    /// The typeahead's filtered + scoped rows for the current composer text.
    /// `query` is the token after `/` (may be empty); `forced_vendor` is the
    /// `@vendor` earlier in the message, if any. Pure delegate to [`visible`].
    pub fn visible(&self, query: &str, forced_vendor: Option<Vendor>) -> Vec<CommandEntry> {
        visible(&self.all_rows(), query, forced_vendor)
    }
}

/// The band fold (pure — unit-tested directly): rows in band order, a later
/// row REPLACING any earlier row with the same (name, vendor). New outranks
/// old — the bridge's live google rows replace the static offline stubs the
/// same way a discovered user skill replaces a built-in.
fn fold_bands<'a>(bands: impl Iterator<Item = &'a CommandEntry>) -> Vec<CommandEntry> {
    let mut rows: Vec<CommandEntry> = Vec::new();
    for row in bands {
        if let Some(slot) = rows
            .iter_mut()
            .find(|existing| existing.name == row.name && existing.vendor == row.vendor)
        {
            *slot = row.clone();
        } else {
            rows.push(row.clone());
        }
    }
    rows
}

/// The scoping + filter fold (contract §2, resolved collision rule):
///
/// - **Scope.** Unforced → orchestrator-native + skills ONLY. A `@vendor`
///   forced earlier in the text → THAT vendor's rows join, badged. Other
///   vendors' rows never appear (the collision rule: `/compact` unforced shows
///   the orchestrator one; codex's variant only under `@codex`).
/// - **Never-in-menu.** `NaByDesign` rows are excluded here (they surface only
///   in `/help <vendor>`), honoring "documented, never silently absent".
/// - **Filter.** Case-insensitive substring on the name (prefix matches rank
///   first), then a stable name sort.
/// - **Disabled rows STAY.** An `Unbuilt` (disabled-with-tooltip) row is
///   visible — greyed, not hidden (the honesty bar).
pub fn visible(
    rows: &[CommandEntry],
    query: &str,
    forced_vendor: Option<Vendor>,
) -> Vec<CommandEntry> {
    let q = query.to_ascii_lowercase();
    let mut matched: Vec<CommandEntry> = rows
        .iter()
        .filter(|row| in_scope(row, forced_vendor))
        .filter(|row| !matches!(row.classification, Classification::NaByDesign { .. }))
        .filter(|row| q.is_empty() || row.name.to_ascii_lowercase().contains(&q))
        .cloned()
        .collect();
    matched.sort_by(|a, b| rank(a, &q).cmp(&rank(b, &q)).then_with(|| a.name.cmp(&b.name)));
    matched
}

/// Scope test: orchestrator-native + skills are ALWAYS in scope; a vendor's
/// own rows join only when that vendor is forced.
fn in_scope(row: &CommandEntry, forced_vendor: Option<Vendor>) -> bool {
    match row.kind {
        CommandKind::Orchestrator => true,
        // Skills are surfaced identically for every vendor (shared-root
        // strategy) — always in the unforced menu.
        CommandKind::Skill => true,
        // A vendor built-in / custom command is in scope only under its
        // forcing `@vendor` — where gemini + agy are ONE lane (2026-07-10
        // swap: gemini = the display name, agy = the CLI underneath), so
        // forcing either name joins both row sets.
        CommandKind::Builtin | CommandKind::Custom => match (row.vendor, forced_vendor) {
            (Some(rv), Some(fv)) => rv == fv || (is_google(rv) && is_google(fv)),
            _ => row.vendor == forced_vendor,
        },
    }
}

fn is_google(vendor: Vendor) -> bool {
    matches!(vendor, Vendor::Gemini | Vendor::Agy)
}

/// Sort rank: prefix matches (0) before mid-string matches (1); empty query is
/// all rank-0. Ties break on the name sort applied by the caller.
fn rank(row: &CommandEntry, q: &str) -> u8 {
    if q.is_empty() || row.name.to_ascii_lowercase().starts_with(q) {
        0
    } else {
        1
    }
}

/// Count of rows by classification bucket (for the final-report registry
/// tallies). Counts the FOLDED set (`all_rows`), so a discovered override is
/// counted once.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ClassCounts {
    pub passthrough: usize,
    pub polyfill: usize,
    pub na_by_design: usize,
    pub disabled: usize,
    pub total: usize,
}

pub fn class_counts(rows: &[CommandEntry]) -> ClassCounts {
    let mut c = ClassCounts::default();
    for row in rows {
        c.total += 1;
        match row.classification {
            Classification::Passthrough => c.passthrough += 1,
            Classification::Polyfill { .. } => c.polyfill += 1,
            Classification::NaByDesign { .. } => c.na_by_design += 1,
        }
        if matches!(row.mechanism, Mechanism::Unbuilt { .. }) {
            c.disabled += 1;
        }
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::types::{Classification, CommandKind, Mechanism, OrchTarget};

    fn seed() -> Vec<CommandEntry> {
        static_entries()
    }

    #[test]
    fn static_seed_rows_are_wellformed() {
        let rows = seed();
        assert!(!rows.is_empty());
        for row in &rows {
            assert!(!row.name.is_empty(), "every row has a name");
            assert!(!row.name.starts_with('/'), "name is bare, no slash: {}", row.name);
            assert!(!row.description.is_empty(), "{} has a description", row.name);
            // Orchestrator rows carry no vendor; vendor rows carry one.
            match row.kind {
                CommandKind::Orchestrator => assert!(row.vendor.is_none()),
                _ => assert!(row.vendor.is_some(), "{} needs a vendor", row.name),
            }
        }
    }

    #[test]
    fn orchestrator_native_set_is_present() {
        let rows = seed();
        for name in ["plan", "board", "usage", "adversary", "vault", "import", "help", "compact"] {
            let row = rows
                .iter()
                .find(|r| r.name == name && r.kind == CommandKind::Orchestrator)
                .unwrap_or_else(|| panic!("missing orchestrator /{name}"));
            if name == "plan" {
                assert!(matches!(
                    row.mechanism,
                    Mechanism::OrchEndpoint(OrchTarget::PlanEscalate)
                ));
            }
        }
    }

    #[test]
    fn compact_orchestrator_row_is_live() {
        // POST /conv/<id>/compact shipped (dogfood 2026-07-08): the
        // orchestrator /compact is now a LIVE OrchEndpoint row — the old
        // honest-disabled state is retired with the endpoint's arrival.
        let rows = seed();
        let compact = rows
            .iter()
            .find(|r| r.name == "compact" && r.kind == CommandKind::Orchestrator)
            .unwrap();
        assert!(!compact.is_disabled());
        assert!(matches!(
            compact.mechanism,
            Mechanism::OrchEndpoint(OrchTarget::Compact)
        ));
    }

    #[test]
    fn unforced_scope_is_orchestrator_plus_skills_only() {
        // A codex/claude BUILTIN must NOT appear unforced; orchestrator rows do.
        let rows = seed();
        let vis = visible(&rows, "", None);
        assert!(vis.iter().any(|r| r.name == "plan" && r.vendor.is_none()));
        assert!(
            vis.iter().all(|r| r.kind != CommandKind::Builtin),
            "no vendor built-in leaks into the unforced menu"
        );
    }

    #[test]
    fn forcing_claude_joins_claude_builtins() {
        let rows = seed();
        let unforced = visible(&rows, "review", None);
        let forced = visible(&rows, "review", Some(Vendor::Claude));
        // claude built-in /review joins only under @claude.
        assert!(!unforced.iter().any(|r| r.vendor == Some(Vendor::Claude)));
        assert!(forced.iter().any(|r| r.name == "review" && r.vendor == Some(Vendor::Claude)));
        // codex's /review still hidden (wrong vendor forced).
        assert!(!forced.iter().any(|r| r.vendor == Some(Vendor::Codex)));
    }

    #[test]
    fn collision_rule_unforced_prefers_orchestrator() {
        // /compact exists for orchestrator AND codex; unforced shows only the
        // orchestrator one.
        let rows = seed();
        let vis = visible(&rows, "compact", None);
        let compacts: Vec<_> = vis.iter().filter(|r| r.name == "compact").collect();
        assert_eq!(compacts.len(), 1);
        assert!(compacts[0].vendor.is_none());
        // Under @codex, codex's /compact joins.
        let forced = visible(&rows, "compact", Some(Vendor::Codex));
        assert!(forced.iter().any(|r| r.name == "compact" && r.vendor == Some(Vendor::Codex)));
    }

    #[test]
    fn na_by_design_never_in_the_menu() {
        let rows = seed();
        // /login (claude NaByDesign) is excluded even under @claude.
        assert!(!visible(&rows, "login", Some(Vendor::Claude))
            .iter()
            .any(|r| r.name == "login"));
        // …but it IS in the full row set for /help.
        assert!(rows.iter().any(|r| r.name == "login"));
    }

    #[test]
    fn disabled_rows_stay_visible_greyed() {
        // codex /compact is Unbuilt (S3 pending) — it must still SHOW under
        // @codex (greyed), not vanish.
        let rows = seed();
        let forced = visible(&rows, "compact", Some(Vendor::Codex));
        let codex_compact = forced
            .iter()
            .find(|r| r.name == "compact" && r.vendor == Some(Vendor::Codex))
            .unwrap();
        assert!(codex_compact.is_disabled());
    }

    #[test]
    fn filter_ranks_prefix_before_substring() {
        let rows = vec![
            CommandEntry {
                name: "review".into(),
                vendor: None,
                kind: CommandKind::Orchestrator,
                classification: Classification::Passthrough,
                mechanism: Mechanism::OrchEndpoint(OrchTarget::Help),
                description: "d".into(),
            },
            CommandEntry {
                name: "code-review".into(),
                vendor: None,
                kind: CommandKind::Orchestrator,
                classification: Classification::Passthrough,
                mechanism: Mechanism::OrchEndpoint(OrchTarget::Help),
                description: "d".into(),
            },
        ];
        let vis = visible(&rows, "review", None);
        assert_eq!(vis[0].name, "review", "prefix match ranks first");
        assert_eq!(vis[1].name, "code-review");
    }

    #[test]
    fn google_lane_rows_override_the_static_stubs() {
        // S5: the bridge's live agy `/skills` row replaces the static seed's
        // Unbuilt stub on (name, vendor) — the fold's new-outranks-old law.
        let live = CommandEntry {
            name: "skills".into(),
            vendor: Some(Vendor::Agy),
            kind: CommandKind::Builtin,
            classification: Classification::Passthrough,
            mechanism: Mechanism::GoogleExec { mech: "agy-subcommand".into() },
            description: "Browse loaded Agent Skills (agy plugin surface).".into(),
        };
        let statics = seed();
        let stub = statics
            .iter()
            .find(|r| r.name == "skills" && r.vendor == Some(Vendor::Agy))
            .expect("the static seed carries the offline /skills stub");
        assert!(stub.is_disabled(), "offline (pre-fetch) the stub is greyed");

        let folded = fold_bands(statics.iter().chain(std::iter::once(&live)));
        let skills: Vec<_> = folded
            .iter()
            .filter(|r| r.name == "skills" && r.vendor == Some(Vendor::Agy))
            .collect();
        assert_eq!(skills.len(), 1, "override, never a duplicate row");
        assert!(!skills[0].is_disabled(), "the live bridge row wins");
        assert!(matches!(skills[0].mechanism, Mechanism::GoogleExec { .. }));

        // And the live row JOINS the menu under either google force token
        // (one lane: @gemini and @agy scope to both vendors' rows).
        for force in [Vendor::Gemini, Vendor::Agy] {
            let vis = visible(&folded, "skills", Some(force));
            assert!(
                vis.iter().any(|r| r.name == "skills"
                    && r.vendor == Some(Vendor::Agy)
                    && !r.is_disabled()),
                "live /skills visible under {force:?}"
            );
        }
        // …and never unforced (vendor built-ins stay scoped).
        assert!(!visible(&folded, "skills", None)
            .iter()
            .any(|r| r.vendor == Some(Vendor::Agy)));
    }

    #[test]
    fn class_counts_tally_the_folded_set() {
        let counts = class_counts(&seed());
        assert!(counts.total > 0);
        assert!(counts.passthrough > 0);
        assert!(counts.polyfill > 0);
        assert!(counts.na_by_design > 0);
        assert!(counts.disabled > 0, "some rows are Unbuilt-disabled");
        assert_eq!(
            counts.passthrough + counts.polyfill + counts.na_by_design,
            counts.total
        );
    }
}
