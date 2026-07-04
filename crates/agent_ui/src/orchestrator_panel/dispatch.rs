//! Orchestrator-native command dispatch (S4) + claude passthrough submission
//! (S2).
//!
//! On send, the composer text is parsed ([`parse_submission`], pure): an
//! orchestrator-native leading `/command` routes to its [`OrchTarget`]; anything
//! else (a `@vendor`-forced message, or a message whose leading `/name` is a
//! skill/custom command) submits to the bridge with the slash prefix INTACT so
//! the claude worker's `-p` expands it (verified mechanism, contract §S2).
//!
//! Dispatch law (RUST_PORT_NOTES §11): every [`OrchTarget`] that opens/activates
//! a surface is a LAYOUT mutation reached from a click/Enter listener — it is
//! wrapped in `window.defer` before touching the workspace. The bridge POSTs
//! (`/plan/escalate`, `/adversary`) are `cx.spawn` background tasks (already
//! deferred by the framework), not layout ops.

use gpui::{AppContext as _, Window};
use ui::prelude::*;
use workspace::Workspace;

use crate::bridge::{BRIDGE_BASE_URL, post_json};
use crate::commands::OrchTarget;

use super::panel::OrchestratorPanel;

/// What a submitted composer message resolves to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Submission {
    /// An orchestrator-native command — route in-app, never POST to the bridge
    /// as a chat message. Carries the argument text after the command name.
    Orchestrator { target: OrchTarget, args: String },
    /// A normal message (possibly `@vendor`-forced, possibly a skill/custom
    /// `/name` for claude) — POST to `/message` with the text unchanged.
    Bridge,
}

/// Parse a trimmed submission into its route. PURE (registry rows in, no cx) so
/// it is unit-tested directly. Only a LEADING `/command` (optionally after a
/// `@vendor` force) that resolves to an ORCHESTRATOR-native row is intercepted;
/// vendor/skill `/name`s fall through to [`Submission::Bridge`] (the claude
/// worker expands them).
pub(super) fn parse_submission(
    text: &str,
    orchestrator_names: &[&str],
) -> Submission {
    let trimmed = text.trim();
    // A `@vendor`-forced message is always a bridge submission (the force means
    // the user wants that vendor to handle the whole thing, prefix and all).
    if trimmed.starts_with('@') {
        return Submission::Bridge;
    }
    let Some(rest) = trimmed.strip_prefix('/') else {
        return Submission::Bridge;
    };
    let (name, args) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
    let name = name.to_ascii_lowercase();
    match orchestrator_target(&name) {
        Some(target) if orchestrator_names.contains(&name.as_str()) => Submission::Orchestrator {
            target,
            args: args.trim().to_string(),
        },
        _ => Submission::Bridge,
    }
}

/// Map an orchestrator-native command NAME to its target. Kept in lockstep with
/// the static seed's orchestrator rows ([`crate::commands::static_seed`]).
fn orchestrator_target(name: &str) -> Option<OrchTarget> {
    Some(match name {
        "plan" => OrchTarget::PlanEscalate,
        "board" => OrchTarget::OpenBoard,
        "usage" => OrchTarget::OpenUsage,
        "adversary" => OrchTarget::OpenAdversary,
        "vault" => OrchTarget::OpenVault,
        "import" => OrchTarget::OpenImport,
        "help" => OrchTarget::Help,
        // /compact is orchestrator-native but Unbuilt (no bridge trigger) — it
        // never routes here; it falls through and the composer no-ops it.
        _ => return None,
    })
}

impl OrchestratorPanel {
    /// The orchestrator-native command names, read from the registry (so the
    /// dispatcher and the menu never drift). Only names that route to a LIVE
    /// target — the Unbuilt `/compact` is excluded (it must not silently POST).
    pub(super) fn orchestrator_command_names(&self, cx: &App) -> Vec<String> {
        self.registry
            .read(cx)
            .all_rows()
            .into_iter()
            .filter(|row| {
                row.vendor.is_none()
                    && matches!(row.kind, crate::commands::CommandKind::Orchestrator)
                    && !row.is_disabled()
            })
            .map(|row| row.name.to_string())
            .collect()
    }

    /// Route an orchestrator-native command. Layout-opening targets defer per
    /// §11; the bridge POSTs are background spawns. Returns `true` when it
    /// consumed the submission (so `send_message` skips the bridge POST).
    pub(super) fn dispatch_orchestrator(
        &mut self,
        target: OrchTarget,
        args: String,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        match target {
            OrchTarget::PlanEscalate => self.dispatch_plan(args, cx),
            OrchTarget::OpenAdversary => self.dispatch_adversary(args, window, cx),
            OrchTarget::OpenBoard => self.defer_open_board(window, cx),
            OrchTarget::OpenUsage => self.defer_open_usage(window, cx),
            OrchTarget::OpenVault | OrchTarget::OpenImport => {
                self.defer_open_vault(window, cx)
            }
            OrchTarget::Help => self.dispatch_help(args, window, cx),
        }
    }

    /// `/plan <text>` → `POST /plan/escalate {"request", "conv"}` (real
    /// haiku-judgment decomposition). Background spawn; the transcript refetch
    /// picks up the resulting plan. Empty args → no-op (need a request).
    fn dispatch_plan(&mut self, args: String, cx: &mut gpui::Context<Self>) {
        if args.is_empty() {
            return;
        }
        let conv = self
            .store
            .read(cx)
            .transcript_conv
            .clone()
            .unwrap_or_default();
        let http_client = cx.http_client();
        let store = self.store.clone();
        cx.spawn(async move |_this, cx| {
            let _ = cx
                .background_spawn(async move {
                    let body =
                        serde_json::json!({ "request": args, "conv": conv }).to_string();
                    post_json(
                        http_client.as_ref(),
                        &format!("{BRIDGE_BASE_URL}/plan/escalate"),
                        body,
                    )
                    .await
                })
                .await;
            // Refetch so the new plan lands in the symphony/transcript view
            // (the composer's post-send pattern — an AsyncApp update that is a
            // no-op if the store was dropped; the `let _` mirrors `enqueue_send`
            // which does not `.ok()` the result).
            let _ = store.update(cx, |store, cx| store.refetch_transcript_soon(cx));
        })
        .detach();
    }

    /// `/adversary <text>` → open the adversary center surface, then broadcast
    /// if there's text (the surface's own job entity drives the poll). The open
    /// is a layout mutation → deferred; the broadcast rides the surface entity.
    fn dispatch_adversary(
        &mut self,
        args: String,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let workspace = self.workspace.clone();
        window.defer(cx, move |window, cx| {
            workspace
                .update(cx, |workspace, cx| {
                    let Some(surfaces) = crate::mode_item::workspace_surfaces(workspace, cx) else {
                        return;
                    };
                    let adversary = surfaces.adversary.clone();
                    crate::mode_item::open_center_item(&adversary, None, workspace, window, cx);
                    if !args.is_empty() {
                        adversary
                            .update(cx, |panel, cx| panel.broadcast_text(args, window, cx));
                    }
                })
                .ok();
        });
    }

    /// `/board` / `/usage` → open-or-activate the matching center surface
    /// (idempotent; deferred per §11).
    fn defer_open_board(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        self.defer_open_center(crate::mode_item::CenterSurface::TaskBoard, window, cx);
    }

    fn defer_open_usage(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        self.defer_open_center(crate::mode_item::CenterSurface::Usage, window, cx);
    }

    fn defer_open_center(
        &mut self,
        surface: crate::mode_item::CenterSurface,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let workspace = self.workspace.clone();
        window.defer(cx, move |window, cx| {
            workspace
                .update(cx, |workspace, cx| {
                    crate::mode_item::open_center_surface(surface, None, workspace, window, cx);
                })
                .ok();
        });
    }

    /// `/vault` and `/import` → focus the OKF vault browser (left dock), which
    /// hosts BOTH the live vault and the import-staging root (its header toggle
    /// switches between them). Focusing a dock panel is a layout mutation →
    /// deferred (§11). The staging-root pre-select is owned by the vault-browser
    /// panel (a separate agent's surface) and is left to its header toggle — we
    /// only open the panel, never reach into its internals.
    fn defer_open_vault(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        let workspace = self.workspace.clone();
        window.defer(cx, move |window, cx| {
            workspace
                .update(cx, |workspace, cx| {
                    workspace.focus_panel::<crate::vault_browser::VaultBrowserPanel>(window, cx);
                })
                .ok();
        });
    }

    /// `/help [vendor]` → open a transient markdown buffer rendered from the
    /// registry (including N/A-by-design reasons). Deferred (opens a center
    /// item).
    fn dispatch_help(&mut self, args: String, window: &mut Window, cx: &mut gpui::Context<Self>) {
        let vendor = crate::commands::Vendor::from_word(args.trim().trim_start_matches('@'));
        let markdown = crate::commands::help::render_help(&self.registry.read(cx).all_rows(), vendor);
        let workspace = self.workspace.clone();
        window.defer(cx, move |window, cx| {
            workspace
                .update(cx, |workspace, cx| {
                    open_help_buffer(markdown, workspace, window, cx);
                })
                .ok();
        });
    }
}

/// Open the `/help` markdown as a transient scratch buffer in the center pane
/// (the agent-panel `open_active_thread_as_markdown` idiom): a local buffer +
/// singleton MultiBuffer + editor item. The content is REAL + visible — never a
/// silent no-op. A dedicated rendered-markdown preview Item is the
/// coordinator's visual pass (banked); the raw markdown reads fine meanwhile.
fn open_help_buffer(
    markdown: String,
    workspace: &mut Workspace,
    window: &mut Window,
    cx: &mut gpui::Context<Workspace>,
) {
    use multi_buffer::MultiBuffer;
    let project = workspace.project().clone();
    let buffer = project.update(cx, |project, cx| {
        project.create_local_buffer(&markdown, None, false, cx)
    });
    let title: SharedString = "Command surface (/help)".into();
    let multibuffer =
        cx.new(|cx| MultiBuffer::singleton(buffer, cx).with_title(title.to_string()));
    let editor = cx.new(|cx| {
        let mut editor =
            editor::Editor::for_multibuffer(multibuffer, Some(project.clone()), window, cx);
        editor.set_breadcrumb_header(title.to_string());
        editor.set_read_only(true);
        editor
    });
    workspace.add_item_to_active_pane(Box::new(editor), None, true, window, cx);
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORCH: [&str; 6] = ["plan", "board", "usage", "adversary", "vault", "import"];

    #[test]
    fn orchestrator_command_is_intercepted() {
        assert_eq!(
            parse_submission("/board", &ORCH),
            Submission::Orchestrator {
                target: OrchTarget::OpenBoard,
                args: String::new()
            }
        );
    }

    #[test]
    fn plan_carries_its_request_text() {
        assert_eq!(
            parse_submission("/plan add a login page", &ORCH),
            Submission::Orchestrator {
                target: OrchTarget::PlanEscalate,
                args: "add a login page".to_string()
            }
        );
    }

    #[test]
    fn help_with_vendor_arg() {
        assert_eq!(
            parse_submission("/help codex", &["help"]),
            Submission::Orchestrator {
                target: OrchTarget::Help,
                args: "codex".to_string()
            }
        );
    }

    #[test]
    fn forced_vendor_message_goes_to_bridge_intact() {
        // @claude /prime must POST unchanged — the worker's -p expands /prime.
        assert_eq!(parse_submission("@claude /prime", &ORCH), Submission::Bridge);
    }

    #[test]
    fn plain_message_goes_to_bridge() {
        assert_eq!(parse_submission("fix the bug", &ORCH), Submission::Bridge);
    }

    #[test]
    fn skill_slash_command_falls_through_to_bridge() {
        // A skill/custom `/name` (not orchestrator-native) POSTs intact — the
        // claude lane expands it (S2). Only orchestrator names intercept.
        assert_eq!(parse_submission("/prime", &ORCH), Submission::Bridge);
    }

    #[test]
    fn disabled_compact_is_not_intercepted() {
        // /compact is not in the LIVE orchestrator name set → falls to Bridge,
        // never routed as a silent no-op. (The composer guards it separately.)
        assert_eq!(parse_submission("/compact", &ORCH), Submission::Bridge);
    }

    #[test]
    fn case_insensitive_command_name() {
        assert!(matches!(
            parse_submission("/BOARD", &ORCH),
            Submission::Orchestrator { .. }
        ));
    }
}
