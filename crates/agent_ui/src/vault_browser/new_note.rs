//! "+ New note" — the header quick-action (vault-UX pass 2026-07-10): an
//! inline compose row (project picker chips + title box) that POSTs
//! `{project, title}` to the bridge `POST /knowledge/note`, re-indexes, and
//! opens the created note in the reading view.
//!
//! Degrade law: the bridge writes notes (vault.py is the only sanctioned
//! author), so with the bridge down the + button DISABLES with an honest
//! tooltip — the tree itself keeps rendering from the filesystem regardless.
//!
//! §11: compose open/close, project picks, and the in-flight mark are pure
//! entity-state flips (synchronous, immediate feedback). The POST rides
//! `cx.spawn_in` (schedules, never re-enters this update); the open-created-
//! doc layout op happens inside the async continuation via
//! [`VaultBrowserPanel::request_open_preview`], which itself spawns.

use editor::Editor;
use gpui::{AppContext as _, Context, Entity, Focusable as _, FontWeight, SharedString, Window};
use serde::Deserialize;
use ui::prelude::*;

use crate::bridge::{BRIDGE_BASE_URL, post_json};

use super::index::{VaultRoot, vault_root_path};
use super::panel::VaultBrowserPanel;
use super::style::SURFACE_1;

/// `POST /knowledge/note` response (probed contract: engine.create_note
/// returns `{id, title, path}`, path vault-relative posix).
#[derive(Debug, Deserialize)]
struct CreatedNote {
    #[serde(default)]
    path: String,
}

/// Compose-row state riding the panel.
#[derive(Default)]
pub struct NewNoteState {
    /// The compose row is showing.
    pub composing: bool,
    /// Lazily created on first compose (needs a window).
    pub title_editor: Option<Entity<Editor>>,
    /// The picked project slug (None → the first known slug).
    pub project: Option<String>,
    /// A POST is in flight (button collapses so double-click can't double-POST).
    pub in_flight: bool,
    /// The last create failed — shown inline, honest.
    pub error: Option<String>,
}

impl VaultBrowserPanel {
    /// Toggle the compose row (header + button). Pure state flip; the title
    /// editor is created lazily and focused so typing starts immediately.
    pub(super) fn toggle_compose(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.new_note.composing {
            self.new_note.composing = false;
            cx.notify();
            return;
        }
        if self.new_note.title_editor.is_none() {
            let editor = cx.new(|cx| {
                let mut editor = Editor::single_line(window, cx);
                editor.set_placeholder_text("Note title…", window, cx);
                editor
            });
            // Re-render on typing so the Create button's enabled state tracks.
            let sub = cx.subscribe(&editor, |_, _, event, cx| {
                if let editor::EditorEvent::BufferEdited = event {
                    cx.notify();
                }
            });
            self.push_subscription(sub);
            self.new_note.title_editor = Some(editor);
        }
        self.new_note.composing = true;
        self.new_note.error = None;
        if let Some(editor) = &self.new_note.title_editor {
            editor.read(cx).focus_handle(cx).focus(window, cx);
        }
        cx.notify();
    }

    /// Project slugs known to the index (the `projects/<slug>/` dirs of the
    /// live vault), for the compose row's picker. Falls back to `default`.
    pub(super) fn project_slugs(&self) -> Vec<String> {
        let mut slugs = std::collections::BTreeSet::new();
        if let Some(index) = self.index() {
            for doc in &index.docs {
                if doc.root == VaultRoot::Vault
                    && let Some(rest) = doc.rel_path.strip_prefix("projects/")
                    && let Some((slug, _)) = rest.split_once('/')
                {
                    slugs.insert(slug.to_string());
                }
            }
        }
        if slugs.is_empty() {
            vec!["default".to_string()]
        } else {
            slugs.into_iter().collect()
        }
    }

    /// The effective picked project (picked, else the first known slug).
    fn picked_project(&self) -> String {
        self.new_note
            .project
            .clone()
            .unwrap_or_else(|| self.project_slugs().remove(0))
    }

    /// POST the note to the bridge, then re-index and open the created doc in
    /// the reading view.
    pub(super) fn create_note(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.new_note.in_flight {
            return;
        }
        let Some(editor) = self.new_note.title_editor.clone() else {
            return;
        };
        let title = editor.read(cx).text(cx).trim().to_string();
        if title.is_empty() {
            self.new_note.error = Some("A title is required.".to_string());
            cx.notify();
            return;
        }
        let project = self.picked_project();
        self.new_note.in_flight = true;
        self.new_note.error = None;
        cx.notify();
        let client = cx.http_client();
        cx.spawn_in(window, async move |this, cx| {
            let outcome = cx
                .background_spawn(async move {
                    let url = format!("{BRIDGE_BASE_URL}/knowledge/note");
                    let body =
                        serde_json::json!({ "project": project, "title": title }).to_string();
                    let raw = post_json(client.as_ref(), &url, body).await?;
                    anyhow::Ok(serde_json::from_str::<CreatedNote>(&raw)?)
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                this.new_note.in_flight = false;
                match outcome {
                    Ok(created) if !created.path.is_empty() => {
                        this.new_note.composing = false;
                        if let Some(editor) = &this.new_note.title_editor {
                            editor.update(cx, |editor, cx| editor.set_text("", window, cx));
                        }
                        // The new note appears in the tree…
                        this.refresh(cx);
                        // …and opens as a reading-view tab (normalise the
                        // bridge's posix rel-path for the dedup map).
                        let rel = created.path.replace('/', std::path::MAIN_SEPARATOR_STR);
                        let abs = vault_root_path().join(rel);
                        this.request_open_preview(abs, window, cx);
                    }
                    Ok(_) => {
                        this.new_note.error =
                            Some("Bridge created the note but returned no path.".to_string());
                    }
                    Err(error) => {
                        this.new_note.error = Some(short_error(&error.to_string()));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Pick a project chip. Pure state flip.
    pub(super) fn set_note_project(&mut self, slug: String, cx: &mut Context<Self>) {
        if self.new_note.project.as_deref() != Some(&slug) {
            self.new_note.project = Some(slug);
            cx.notify();
        }
    }

    /// The compose row (rendered under the header filter while composing):
    /// project chips · title box · Create/Cancel · inline error.
    pub(super) fn render_new_note(&self, connected: bool, cx: &mut Context<Self>) -> Option<Div> {
        if !self.new_note.composing {
            return None;
        }
        let editor = self.new_note.title_editor.clone()?;
        let colors = cx.theme().colors();
        let picked = self.picked_project();
        let title_empty = editor.read(cx).text(cx).trim().is_empty();

        // Project picker chips (segmented idiom, one per known slug).
        let mut chips = h_flex().items_center().flex_wrap().gap(px(4.));
        for slug in self.project_slugs() {
            let on = slug == picked;
            let pick = slug.clone();
            chips = chips.child(
                div()
                    .id(gpui::ElementId::Name(format!("note-project-{slug}").into()))
                    .px(px(8.))
                    .py(px(2.))
                    .rounded(px(6.))
                    .text_size(px(11.))
                    .cursor_pointer()
                    .when(on, |s| s.bg(SURFACE_1).text_color(colors.text))
                    .when(!on, |s| s.text_color(colors.text_placeholder))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.set_note_project(pick.clone(), cx)
                    }))
                    .child(SharedString::from(slug)),
            );
        }

        let create_enabled = connected && !title_empty && !self.new_note.in_flight;
        let create_label = if self.new_note.in_flight { "Creating…" } else { "Create" };
        let mut row = v_flex()
            .w_full()
            .gap(px(6.))
            .child(chips)
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        div()
                            .flex_1()
                            .px(px(8.))
                            .py(px(4.))
                            .rounded(px(8.))
                            .bg(SURFACE_1)
                            .child(editor),
                    )
                    .child(
                        div()
                            .id("note-create")
                            .px(px(9.))
                            .py(px(4.))
                            .rounded(px(8.))
                            .text_size(px(12.))
                            .font_weight(FontWeight::MEDIUM)
                            .when(create_enabled, |s| {
                                s.bg(crate::agent_accents::ACCENT_FILL)
                                    .text_color(gpui::white())
                                    .cursor_pointer()
                            })
                            .when(!create_enabled, |s| {
                                s.border_1()
                                    .border_color(colors.border)
                                    .text_color(colors.text_placeholder)
                            })
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.create_note(window, cx)
                            }))
                            .child(create_label),
                    )
                    .child(
                        div()
                            .id("note-cancel")
                            .px(px(6.))
                            .py(px(4.))
                            .text_size(px(12.))
                            .text_color(colors.text_placeholder)
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.toggle_compose(window, cx)
                            }))
                            .child("Cancel"),
                    ),
            );
        if !connected {
            row = row.child(
                div()
                    .text_size(px(11.))
                    .text_color(crate::agent_accents::STATUS_BLOCKED)
                    .child("bridge offline — notes are written by the bridge; reconnect to create"),
            );
        }
        if let Some(error) = &self.new_note.error {
            row = row.child(
                div()
                    .text_size(px(11.))
                    .text_color(crate::agent_accents::STATUS_ERROR)
                    .child(SharedString::from(error.clone())),
            );
        }
        Some(row)
    }
}

/// Trim a bridge error to a short inline string.
fn short_error(msg: &str) -> String {
    msg.lines().next().unwrap_or(msg).chars().take(64).collect()
}
