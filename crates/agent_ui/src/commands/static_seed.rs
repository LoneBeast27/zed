//! The curated STATIC seed of the command registry (S0).
//!
//! A hand-folded subset of the four vendor inventory docs — NOT all ~180 rows.
//! Quality over completeness (contract §0): every row shipped here is truthful
//! about its mechanism TODAY. Three bands:
//!
//! 1. **Orchestrator-native** — the product's own commands, wired live in S4.
//! 2. **Claude passthrough** — the built-in slash rows the docs mark reachable
//!    via `-p` prefix expansion (`claude/command-surface.md` §2), which fire
//!    through the verified [`Mechanism::ClaudePrefix`] path today (S2).
//! 3. **Notable N/A-by-design + not-yet-built rows** for `/help` honesty
//!    (never silently absent) — codex/gemini/agy passthrough rows carried with
//!    their real future mechanism but marked unbuilt until S3/S5.
//!
//! Each row cites its source doc + section in a trailing comment. Dynamic
//! discovery ([`super::discovery`]) folds the user's LOCAL skills/commands on
//! top of this at registry build.

use super::types::{
    Classification, CommandEntry, CommandKind, Mechanism, OrchTarget, Vendor,
};

/// Build one orchestrator-native row.
fn orch(
    name: &'static str,
    target: OrchTarget,
    classification: Classification,
    description: &'static str,
) -> CommandEntry {
    CommandEntry {
        name: name.into(),
        vendor: None,
        kind: CommandKind::Orchestrator,
        classification,
        mechanism: Mechanism::OrchEndpoint(target),
        description: description.into(),
    }
}

/// Build one claude built-in passthrough row (fires via `-p` prefix today).
fn claude_builtin(
    name: &'static str,
    description: &'static str,
) -> CommandEntry {
    CommandEntry {
        name: name.into(),
        vendor: Some(Vendor::Claude),
        kind: CommandKind::Builtin,
        classification: Classification::Passthrough,
        mechanism: Mechanism::ClaudePrefix,
        description: description.into(),
    }
}

/// Build one "documented but not-yet-built" row — real mechanism carried, but
/// disabled-with-tooltip until its lane lands (S3 codex / S5 google).
fn unbuilt(
    name: &'static str,
    vendor: Vendor,
    kind: CommandKind,
    classification: Classification,
    real_mechanism: Mechanism,
    description: &'static str,
    tooltip: &'static str,
) -> CommandEntry {
    // The row carries its REAL future mechanism in the classification/desc for
    // /help, but its live `mechanism` is Unbuilt so the menu greys it. We keep
    // the real mechanism discoverable by encoding it in the description tail.
    let _ = real_mechanism; // (documented in the description; live mech = Unbuilt)
    CommandEntry {
        name: name.into(),
        vendor: Some(vendor),
        kind,
        classification,
        mechanism: Mechanism::unbuilt(tooltip),
        description: description.into(),
    }
}

/// Build one N/A-by-design row (never in the menu; listed in `/help <vendor>`).
fn na(
    name: &'static str,
    vendor: Vendor,
    reason: &'static str,
    description: &'static str,
) -> CommandEntry {
    CommandEntry {
        name: name.into(),
        vendor: Some(vendor),
        kind: CommandKind::Builtin,
        classification: Classification::na(reason),
        mechanism: Mechanism::unbuilt(reason),
        description: description.into(),
    }
}

/// The full curated seed, in a stable order (orchestrator → claude → codex →
/// google → N/A). The registry sorts for display; this order keeps the table
/// readable + the tests deterministic.
pub fn static_entries() -> Vec<CommandEntry> {
    let mut entries = Vec::new();
    entries.extend(orchestrator_native());
    entries.extend(claude_passthrough());
    entries.extend(codex_rows());
    entries.extend(google_rows());
    entries.extend(na_by_design());
    entries
}

/// Band 1 — orchestrator-native (contract §1.1, wired live in S4).
fn orchestrator_native() -> Vec<CommandEntry> {
    vec![
        orch(
            "plan",
            OrchTarget::PlanEscalate,
            Classification::Passthrough, // real haiku judgment via /plan/escalate
            "Escalate a plan for this conversation (judgment-tier decomposition).",
        ), // routes_core.py POST /plan/escalate
        orch(
            "wave",
            OrchTarget::WaveSpawn,
            Classification::Passthrough, // deterministic linkage via /plan/wave/spawn
            "Run a plan wave: /wave <n> [@vendor] — spawns every ready subtask \
             with deterministic plan linkage.",
        ), // routes_core.py POST /plan/wave/spawn (2026-07-10)
        orch(
            "board",
            OrchTarget::OpenBoard,
            Classification::polyfill("task_board"),
            "Open the task board (runs, spawn-tree, drawers).",
        ), // COMMAND_SURFACE_PARITY.md §1.1
        orch(
            "usage",
            OrchTarget::OpenUsage,
            Classification::polyfill("usage_panel"),
            "Open the usage console (pool headroom + reset windows).",
        ), // §1.1
        orch(
            "adversary",
            OrchTarget::OpenAdversary,
            Classification::polyfill("adversary_panel"),
            "Cross-vendor adversarial broadcast (claude + codex + gemini).",
        ), // §1.1 + bridge/adversary.rs
        orch(
            "vault",
            OrchTarget::OpenVault,
            Classification::polyfill("vault_browser"),
            "Open the OKF vault browser (documents + import staging).",
        ), // §1.1
        orch(
            "import",
            OrchTarget::OpenImport,
            Classification::polyfill("vault_browser"),
            "Open the vault browser on its import-staging root.",
        ), // §1.1
        orch(
            "help",
            OrchTarget::Help,
            Classification::polyfill("commands::help"),
            "List every command by source, including documented-but-disabled ones.",
        ), // §2 (/help renders from the registry)
        orch(
            "compact",
            OrchTarget::Compact,
            Classification::polyfill("orchestrator/compaction.py"),
            "Compact the conversation context (forced heartbeat splice).",
        ), // routes_core.py POST /conv/<id>/compact (dogfood 2026-07-08)
    ]
}

/// Band 2 — claude built-ins that expand in `-p` TODAY (the load-bearing
/// headless fact, `claude/command-surface.md` §0 + §2). These fire through the
/// verified [`Mechanism::ClaudePrefix`] path (S2). Skills + custom commands are
/// discovered dynamically ([`super::discovery`]); these are the notable
/// BUILT-IN slash commands whose docs mark them `-p`-reachable and which have
/// no stronger orchestrator-native replacement worth hiding them behind.
fn claude_passthrough() -> Vec<CommandEntry> {
    vec![
        claude_builtin(
            "code-review",
            "Review the diff for bugs + cleanups (skill; --fix/--comment/ultra).",
        ), // claude §2b [Skill], -p
        claude_builtin(
            "review",
            "Read-only review of a GitHub PR (needs gh).",
        ), // claude §2b, -p
        claude_builtin(
            "security-review",
            "Security-focused review of the working-tree diff.",
        ), // claude §2b, -p
        claude_builtin(
            "simplify",
            "Cleanup-only review that applies its fixes (skill).",
        ), // claude §2b [Skill], -p
        claude_builtin(
            "deep-research",
            "Fan-out web search into a cited report (workflow).",
        ), // claude §2b [Workflow], -p
        claude_builtin(
            "verify",
            "Build + launch + drive the app to verify a change (skill).",
        ), // claude §2b [Skill], -p
        claude_builtin(
            "run",
            "Build, launch, and drive the project's app (skill).",
        ), // claude §2b [Skill], -p
        claude_builtin(
            "fewer-permission-prompts",
            "Add a read-only allowlist to settings from transcripts (skill).",
        ), // claude §2b [Skill], -p
        claude_builtin(
            "init",
            "Generate a starter CLAUDE.md for the project.",
        ), // claude §2c, --init-only / -p
        claude_builtin(
            "claude-api",
            "Load the Claude API reference; migrate model ids (skill).",
        ), // claude §2b [Skill], -p
    ]
}

/// Band 3a — notable CODEX rows. The passthrough slash rows the docs mark
/// reachable via the app-server (`codex/command-surface.md` §2/§5) carry their
/// REAL app-server method, but are Unbuilt-disabled until the codex lane (S3)
/// wires the RPC. `/compact` → `thread/compact/start` is the headline
/// upgrade named in the contract §3.
fn codex_rows() -> Vec<CommandEntry> {
    vec![
        unbuilt(
            "compact",
            Vendor::Codex,
            CommandKind::Builtin,
            Classification::Passthrough,
            Mechanism::CodexRpc("thread/compact/start".into()),
            "Native in-place compaction (app-server thread/compact/start).",
            "Codex lane (S3) not wired yet — maps to thread/compact/start.",
        ), // codex §2 + §5.5
        unbuilt(
            "review",
            Vendor::Codex,
            CommandKind::Builtin,
            Classification::Passthrough,
            Mechanism::CodexRpc("review/start".into()),
            "Review working-tree changes (app-server review/start).",
            "Codex lane (S3) not wired yet — maps to review/start.",
        ), // codex §2 + §5.5
        unbuilt(
            "fork",
            Vendor::Codex,
            CommandKind::Builtin,
            Classification::Passthrough,
            Mechanism::CodexRpc("thread/fork".into()),
            "Branch the conversation into a new thread (thread/fork).",
            "Codex lane (S3) not wired yet — maps to thread/fork.",
        ), // codex §2 + §5.5
        unbuilt(
            "undo",
            Vendor::Codex,
            CommandKind::Builtin,
            Classification::Passthrough,
            Mechanism::CodexRpc("thread/rollback".into()),
            "Undo N turns (app-server thread/rollback).",
            "Codex lane (S3) not wired yet — maps to thread/rollback.",
        ), // codex §5.5
    ]
}

/// Band 3b — notable GOOGLE (gemini + agy) rows. The real relay is a CLI
/// subcommand / ACP; carried Unbuilt-disabled until the google lane (S5).
fn google_rows() -> Vec<CommandEntry> {
    vec![
        unbuilt(
            "skills",
            Vendor::Agy,
            CommandKind::Builtin,
            Classification::Passthrough,
            Mechanism::AgySubcommand("agy plugin list".into()),
            "Browse loaded Agent Skills (agy plugin surface).",
            "Google lane (S5) not wired yet — relays via agy CLI.",
        ), // antigravity §1 + §2
        unbuilt(
            "goal",
            Vendor::Agy,
            CommandKind::Builtin,
            Classification::polyfill("afk goal-mode"),
            Mechanism::AgySubcommand("agy /goal".into()),
            "Long-running goal mode (runs until done/cancelled).",
            "Google lane (S5) not wired yet — orchestrator goal-mode polyfill.",
        ), // antigravity §2
    ]
}

/// Notable N/A-by-design rows — never in the menu, present in `/help <vendor>`
/// with the documented reason (contract §2, "documented, never silently
/// absent"). A representative slice, not the full ~20 per vendor.
fn na_by_design() -> Vec<CommandEntry> {
    vec![
        na(
            "login",
            Vendor::Claude,
            "Interactive OAuth flow — no headless trigger (claude auth status reads state).",
            "Authenticate (interactive).",
        ), // claude §2d
        na(
            "theme",
            Vendor::Claude,
            "The app owns its own chrome/theme.",
            "Set the CLI color theme.",
        ), // claude §2a
        na(
            "vim",
            Vendor::Codex,
            "TUI input mode — the app owns its editor keymap.",
            "Toggle vim keybindings in the CLI.",
        ), // codex §2 [ui]
        na(
            "quit",
            Vendor::Codex,
            "CLI process lifecycle — no relay meaning.",
            "Exit the CLI.",
        ), // codex §2 [ui]
        na(
            "credits",
            Vendor::Agy,
            "Vendor billing — the usage console shows the Google quota cluster instead.",
            "G1-credit panel + purchase link.",
        ), // antigravity §2
        na(
            "clear",
            Vendor::Gemini,
            "TUI-only screen clear — the app owns conversation lifecycle.",
            "Clear the terminal + visible history.",
        ), // gemini §2
    ]
}
