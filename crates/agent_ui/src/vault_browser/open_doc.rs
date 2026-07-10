//! Opening vault docs in the center (split from `panel.rs` — single concern:
//! the two open paths).
//!
//! READING VIEW FIRST (vault-UX pass 2026-07-10): a row click opens Zed's
//! real `markdown_preview::MarkdownPreviewView` — the same view the
//! `markdown::OpenPreview` action builds — as ONE center tab, backed by a
//! pane-less editor over the project-opened buffer. This reuses the action's
//! exact construction path (`MarkdownPreviewView::new`, Default mode) while
//! skipping its "raw source tab first" precondition, so a note reads like a
//! note-app page, not a grep target. The pencil affordance (and the created
//! tab's standard machinery) reaches raw source via [`request_open`].
//!
//! Dispatch law (RUST_PORT_NOTES §11): both paths are layout mutations
//! (workspace item adds) reached from click listeners — the raw open defers
//! one cycle (`cx.defer_in`), the preview open runs entirely in a spawned
//! continuation (`cx.spawn_in` schedules; nothing re-enters the panel's
//! update from inside the listener).

use std::path::PathBuf;

use editor::Editor;
use gpui::{AppContext as _, Context, TaskExt as _, WeakEntity, Window};
use markdown_preview::markdown_preview_view::{MarkdownPreviewMode, MarkdownPreviewView};
use workspace::{OpenOptions, OpenVisible};

use super::panel::VaultBrowserPanel;

impl VaultBrowserPanel {
    /// Open a vault md file as RAW source in a center editor tab. DEFERRED
    /// out of the click listener (§11: `open_abs_path` is a layout mutation
    /// reached synchronously from a row's `on_click`, which runs inside this
    /// panel's update — defer the workspace routing one cycle).
    pub fn request_open(&mut self, abs_path: PathBuf, window: &Window, cx: &mut Context<Self>) {
        let workspace = self.workspace.clone();
        cx.defer_in(window, move |_, window, cx| {
            workspace
                .update(cx, |workspace, cx| {
                    workspace
                        .open_abs_path(
                            abs_path,
                            OpenOptions {
                                visible: Some(OpenVisible::None),
                                ..Default::default()
                            },
                            window,
                            cx,
                        )
                        .detach_and_log_err(cx);
                })
                .ok();
        });
    }

    /// Open a vault md file as the RENDERED reading view (one preview tab).
    /// Re-clicking a doc whose preview tab is still open ACTIVATES it instead
    /// of stacking duplicates (the panel keeps a path→view map). If the
    /// buffer can't be opened the click degrades to the raw-source path —
    /// never a silent no-op.
    pub fn request_open_preview(
        &mut self,
        abs_path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let existing = self.previews.get(&abs_path).and_then(WeakEntity::upgrade);
        let workspace = self.workspace.clone();
        cx.spawn_in(window, async move |this, cx| {
            // Dedup: activate the already-open preview tab if it survives.
            if let Some(view) = existing {
                let activated = workspace
                    .update_in(cx, |workspace, window, cx| {
                        workspace.activate_item(&view, true, true, window, cx)
                    })
                    .unwrap_or(false);
                if activated {
                    return;
                }
            }
            // Open the buffer through the project (invisible worktree — the
            // vault lives outside the project root), then build the preview
            // view over a pane-less editor and add ONE item to the pane.
            let Ok(open_buffer) = workspace.update(cx, |workspace, cx| {
                workspace
                    .project()
                    .update(cx, |project, cx| project.open_local_buffer(&abs_path, cx))
            }) else {
                return;
            };
            let buffer = match open_buffer.await {
                Ok(buffer) => buffer,
                Err(error) => {
                    // Honest degrade: fall back to the raw-source open (which
                    // surfaces the workspace's own error handling).
                    log::warn!("vault preview: buffer open failed, falling back to source: {error}");
                    this.update_in(cx, |this, window, cx| {
                        this.request_open(abs_path.clone(), window, cx)
                    })
                    .ok();
                    return;
                }
            };
            this.update_in(cx, |this, window, cx| {
                let Some(workspace) = this.workspace.upgrade() else {
                    return;
                };
                let view = workspace.update(cx, |workspace, cx| {
                    let project = workspace.project().clone();
                    let languages = project.read(cx).languages().clone();
                    let editor =
                        cx.new(|cx| Editor::for_buffer(buffer, Some(project), window, cx));
                    let view = MarkdownPreviewView::new(
                        MarkdownPreviewMode::Default,
                        editor,
                        workspace.weak_handle(),
                        languages,
                        window,
                        cx,
                    );
                    workspace.active_pane().update(cx, |pane, cx| {
                        pane.add_item(Box::new(view.clone()), true, true, None, window, cx);
                    });
                    view
                });
                this.previews.insert(abs_path, view.downgrade());
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}
