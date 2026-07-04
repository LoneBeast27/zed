//! Command-registry data model (S0 of the command-surface build ladder,
//! `COMMAND_SURFACE_PARITY.md`).
//!
//! A [`CommandEntry`] is one row of the `/` surface: its name, the vendor it
//! belongs to (if any), its kind (built-in / skill / custom / orchestrator),
//! how the parity law classifies it (passthrough / polyfill / N-A-by-design),
//! and the concrete mechanism that fires it. The registry ([`super::registry`])
//! folds a curated static seed ([`super::static_seed`]) with the user's local
//! commands + skills discovered on disk ([`super::discovery`]); the `/`
//! typeahead ([`super::super::orchestrator_panel`]) and `/help` both render
//! from it.

use gpui::SharedString;

/// Which vendor a command belongs to. `None` = orchestrator-native (the
/// product's own commands, vendor-agnostic).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Vendor {
    Claude,
    Codex,
    Gemini,
    /// Antigravity (`agy`) — the live Google worker for this user's Pro auth.
    Agy,
}

impl Vendor {
    /// The `@vendor` token that forces this vendor (composer force prefix,
    /// `orchestrator/loop.py` `FORCE_RE`). Also the collision-scoping key.
    pub fn force_token(self) -> &'static str {
        match self {
            Vendor::Claude => "@claude",
            Vendor::Codex => "@codex",
            Vendor::Gemini => "@gemini",
            Vendor::Agy => "@agy",
        }
    }

    /// Short badge label rendered in the typeahead row (the "vendor glyph"
    /// slot of the row anatomy — a text badge until real glyphs land).
    pub fn badge(self) -> &'static str {
        match self {
            Vendor::Claude => "claude",
            Vendor::Codex => "codex",
            Vendor::Gemini => "gemini",
            Vendor::Agy => "agy",
        }
    }

    /// Parse a bare `@vendor` word (lowercased, no `@`) — the composer's
    /// forced-target detection reuses this.
    pub fn from_word(word: &str) -> Option<Vendor> {
        match word.to_ascii_lowercase().as_str() {
            "claude" => Some(Vendor::Claude),
            "codex" => Some(Vendor::Codex),
            "gemini" => Some(Vendor::Gemini),
            "agy" => Some(Vendor::Agy),
            _ => None,
        }
    }
}

/// What sort of command a row is — drives the source badge and the scoping
/// rule (skills are always in-scope unforced; vendor built-ins join only
/// under `@vendor`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandKind {
    /// A vendor's own built-in slash command (e.g. claude `/compact`).
    Builtin,
    /// A skill — surfaced identically for every vendor (the shared-root
    /// strategy, contract §1.3). Always in the unforced `/` menu.
    Skill,
    /// A user-authored custom command (`.claude/commands/*.md`, etc.).
    Custom,
    /// Orchestrator-native — the product's own commands (`/plan`, `/board`…).
    Orchestrator,
}

impl CommandKind {
    /// The source-badge text (`orch` / `skill` / vendor glyph handled by the
    /// row builder). Custom + Builtin fall back to the vendor badge.
    pub fn badge(self) -> Option<&'static str> {
        match self {
            CommandKind::Orchestrator => Some("orch"),
            CommandKind::Skill => Some("skill"),
            CommandKind::Builtin | CommandKind::Custom => None,
        }
    }
}

/// The parity-law classification of a command (contract §2). A `Polyfill`
/// whose module isn't built yet renders disabled-with-tooltip; a `NaByDesign`
/// row never appears in the menu but is listed in `/help <vendor>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Classification {
    /// The vendor exposes it headlessly; the orchestrator relays it.
    Passthrough,
    /// The orchestrator reimplements the behavior; `module` names the polyfill
    /// (or the reason it is not yet built, for the disabled-tooltip text).
    Polyfill { module: SharedString },
    /// Interactive/host/account affordance meaningless in a composer —
    /// documented in `/help`, never silently absent.
    NaByDesign { reason: SharedString },
}

impl Classification {
    pub fn polyfill(module: impl Into<SharedString>) -> Self {
        Classification::Polyfill {
            module: module.into(),
        }
    }

    pub fn na(reason: impl Into<SharedString>) -> Self {
        Classification::NaByDesign {
            reason: reason.into(),
        }
    }
}

/// The concrete wire mechanism that fires a command. `Unbuilt` marks a row
/// that is real-in-principle but has no working trigger today — it renders
/// disabled-with-tooltip in the menu (honesty: never silently absent).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mechanism {
    /// Claude passthrough: the `/name args` prefix rides INTACT into the
    /// worker task text; the vendor CLI expands it (`claude -p <task>`,
    /// verified via `orchestrator/loop.py` forced-spawn → `worker.start`).
    ClaudePrefix,
    /// Codex app-server method the slash command maps to (e.g.
    /// `thread/compact/start`). Wired in S3 (not this slice) — carried so the
    /// row is truthful about HOW it will fire.
    CodexRpc(SharedString),
    /// An `agy`/gemini CLI subcommand relay (S5). Carried for `/help` honesty.
    AgySubcommand(SharedString),
    /// Orchestrator-native: hit a bridge endpoint or open an in-app surface.
    OrchEndpoint(OrchTarget),
    /// Real in principle, but no working trigger today → disabled-with-tooltip.
    /// `reason` is the tooltip text.
    Unbuilt { reason: SharedString },
}

impl Mechanism {
    pub fn unbuilt(reason: impl Into<SharedString>) -> Self {
        Mechanism::Unbuilt {
            reason: reason.into(),
        }
    }
}

/// The orchestrator-native targets an [`Mechanism::OrchEndpoint`] resolves to
/// (S4). The dispatcher ([`super::super::orchestrator_panel`]) matches on this
/// and defers the layout mutation per RUST_PORT_NOTES §11.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrchTarget {
    /// `/plan <text>` → `POST /plan/escalate {"request": text, "conv": conv}`.
    PlanEscalate,
    /// `/board` → open/activate the task-board center surface.
    OpenBoard,
    /// `/usage` → open/activate the usage center surface.
    OpenUsage,
    /// `/adversary <text>` → open the adversary surface (+ broadcast if text).
    OpenAdversary,
    /// `/vault` → focus the OKF vault browser (left dock).
    OpenVault,
    /// `/import` → focus the vault browser on its import-staging root.
    OpenImport,
    /// `/help [vendor]` → render the transient help buffer from the registry.
    Help,
}

/// One row of the command surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandEntry {
    /// The bare command name WITHOUT the leading slash (`compact`, `plan`).
    pub name: SharedString,
    /// The vendor this belongs to, or `None` for orchestrator-native.
    pub vendor: Option<Vendor>,
    pub kind: CommandKind,
    pub classification: Classification,
    pub mechanism: Mechanism,
    /// One-line description (the row's second column; `/help` body).
    pub description: SharedString,
}

impl CommandEntry {
    /// The slash-prefixed display name (`/compact`).
    pub fn slash_name(&self) -> String {
        format!("/{}", self.name)
    }

    /// A row is DISABLED (rendered greyed with a tooltip) when its mechanism
    /// is [`Mechanism::Unbuilt`] — real in principle, no working trigger yet.
    /// Everything else fires.
    pub fn is_disabled(&self) -> bool {
        matches!(self.mechanism, Mechanism::Unbuilt { .. })
    }

    /// The disabled-tooltip text (the `Unbuilt` reason), or `None` when the
    /// row is live.
    pub fn disabled_tooltip(&self) -> Option<&str> {
        match &self.mechanism {
            Mechanism::Unbuilt { reason } => Some(reason.as_ref()),
            _ => None,
        }
    }

    /// The source badge shown in the typeahead row: `orch` / `skill` for those
    /// kinds, else the vendor glyph (`claude`/`codex`/…), else nothing.
    pub fn source_badge(&self) -> Option<&'static str> {
        self.kind
            .badge()
            .or_else(|| self.vendor.map(|vendor| vendor.badge()))
    }
}
