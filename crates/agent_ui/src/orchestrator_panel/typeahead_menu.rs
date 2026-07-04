//! The `/` typeahead menu (S1) — state + render.
//!
//! An anchored popover rendered ABOVE the composer deck (in flow, like the
//! tasks-island slot), reusing the mention-menu idiom (RUST_PORT_NOTES §3):
//! `crates/ui` list rows in a bordered surface. It reads the live composer
//! text + cursor each frame via [`super::typeahead::slash_context`], filters
//! the [`CommandRegistry`] with the resolved scoping rule, and renders one row
//! per match with name · description · source badge · disabled-with-tooltip.
//!
//! Dispatch law (RUST_PORT_NOTES §11): selecting a row splices text into the
//! editor (a pure editor edit, safe) — it never mutates layout. Orchestrator
//! rows that DO open a surface run their layout op through the deferred
//! dispatcher ([`super::dispatch`]), not from the menu's click listener.

use editor::{Editor, MultiBufferOffset, ToOffset as _};
use gpui::{AnyElement, Entity, Focusable as _, Hsla};
use settings::Settings as _;
use ui::prelude::*;

use crate::commands::CommandEntry;
use crate::task_board::style::{HAIRLINE_HI, SURFACE_2, SURFACE_2B};

use super::panel::OrchestratorPanel;
use super::typeahead::{SlashContext, slash_context};

/// Max rows shown at once (the menu scrolls its selection, not a huge list).
const MAX_ROWS: usize = 8;

/// The menu's mutable view state. Whether it is OPEN is derived each frame from
/// the composer text (a `/token` at the cursor) — this struct only carries the
/// selection index, which persists across keystrokes while the menu stays open.
pub(super) struct TypeaheadMenu {
    /// Selected row index into the CURRENT filtered set.
    selected: usize,
    /// Whether the menu was open last frame (to reset selection on (re)open).
    was_open: bool,
}

impl TypeaheadMenu {
    pub fn new() -> Self {
        Self {
            selected: 0,
            was_open: false,
        }
    }

    /// The current selected row index — read by the dispatch tests to assert
    /// keyboard nav moved the SELECTION (not the editor caret).
    #[cfg(test)]
    pub(super) fn selected_index(&self) -> usize {
        self.selected
    }
}

/// The live menu snapshot for a frame: the parsed slash context + the filtered
/// rows. `None` when the menu is closed (no `/token` at the cursor).
pub(super) struct MenuFrame {
    pub ctx: SlashContext,
    pub rows: Vec<CommandEntry>,
}

impl OrchestratorPanel {
    /// Compute the menu state for this frame from the composer editor + the
    /// registry. Returns `None` when there is no active `/token`. Also resets
    /// the selection index on the closed→open edge and clamps it to the row
    /// count (a shrinking filter can strand the cursor past the end).
    pub(super) fn typeahead_frame(&mut self, cx: &mut gpui::Context<Self>) -> Option<MenuFrame> {
        let (text, cursor) = composer_text_cursor(&self.composer.editor, cx);
        let ctx = slash_context(&text, cursor);
        let open = ctx.is_some();
        if open != self.typeahead.was_open {
            self.typeahead.was_open = open;
            self.typeahead.selected = 0;
        }
        let ctx = ctx?;
        let rows = self
            .registry
            .read(cx)
            .visible(&ctx.query, ctx.forced_vendor);
        if rows.is_empty() {
            return None;
        }
        if self.typeahead.selected >= rows.len() {
            self.typeahead.selected = rows.len() - 1;
        }
        Some(MenuFrame { ctx, rows })
    }

    /// Move the selection (`+1`/`-1`), wrapping. No-op when the menu is closed.
    pub(super) fn typeahead_move(&mut self, delta: isize, cx: &mut gpui::Context<Self>) {
        let Some(frame) = self.typeahead_frame(cx) else {
            return;
        };
        let len = frame.rows.len() as isize;
        let next = (self.typeahead.selected as isize + delta).rem_euclid(len);
        self.typeahead.selected = next as usize;
        cx.notify();
    }

    /// Accept the current selection: splice `/name ` into the editor. A pure
    /// editor edit (no layout mutation) — safe to run synchronously from the
    /// Enter/Tab/click handler. Returns `true` when it consumed the key (the
    /// menu was open), so `send_message` can early-return.
    pub(super) fn typeahead_accept(
        &mut self,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) -> bool {
        let Some(frame) = self.typeahead_frame(cx) else {
            return false;
        };
        let row = frame.rows[self.typeahead.selected].clone();
        self.accept_row(&row, window, cx);
        true
    }

    /// Splice a chosen row's `/name ` into the editor at the live slash-token
    /// range (re-parsed here so a click never uses a stale offset). No-op when
    /// the menu isn't actually open on the live text.
    pub(super) fn accept_row(
        &mut self,
        row: &CommandEntry,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let (text, cursor) = composer_text_cursor(&self.composer.editor, cx);
        let Some(ctx) = slash_context(&text, cursor) else {
            return;
        };
        let (new_text, new_cursor) = super::typeahead::apply_completion(&text, &ctx, &row.name);
        self.composer.editor.update(cx, |editor, cx| {
            editor.set_text(new_text, window, cx);
            let at = MultiBufferOffset(new_cursor);
            editor.change_selections(Default::default(), window, cx, |selections| {
                selections.select_ranges([at..at]);
            });
        });
        self.typeahead.was_open = false;
        self.typeahead.selected = 0;
        window.focus(&self.composer.editor.read(cx).focus_handle(cx), cx);
        cx.notify();
    }

    /// Close the menu (Escape): clear the trailing `/token` so the menu shuts
    /// AND the stray slash doesn't linger. No-op when closed. Returns whether
    /// it consumed the key.
    pub(super) fn typeahead_dismiss(
        &mut self,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) -> bool {
        let Some(frame) = self.typeahead_frame(cx) else {
            return false;
        };
        let (text, _) = composer_text_cursor(&self.composer.editor, cx);
        // Drop the `/token` the menu was tracking, leaving the head + tail.
        let mut trimmed = String::with_capacity(text.len());
        trimmed.push_str(&text[..frame.ctx.start]);
        trimmed.push_str(&text[frame.ctx.end.min(text.len())..]);
        let cursor = frame.ctx.start;
        self.composer.editor.update(cx, |editor, cx| {
            editor.set_text(trimmed, window, cx);
            let at = MultiBufferOffset(cursor);
            editor.change_selections(Default::default(), window, cx, |selections| {
                selections.select_ranges([at..at]);
            });
        });
        self.typeahead.was_open = false;
        self.typeahead.selected = 0;
        window.focus(&self.composer.editor.read(cx).focus_handle(cx), cx);
        cx.notify();
        true
    }

    /// Render the menu popover for this frame, or `None` when closed. Anchored
    /// ABOVE the deck (the tasks-island slot idiom): it displaces the composer
    /// downward while open, which keeps it inside the centered chat column.
    pub(super) fn render_typeahead_menu(
        &mut self,
        cx: &mut gpui::Context<Self>,
    ) -> Option<AnyElement> {
        let frame = self.typeahead_frame(cx)?;
        let selected = self.typeahead.selected;
        let colors = cx.theme().colors();
        let shown = frame.rows.len().min(MAX_ROWS);

        let mut list = v_flex()
            .w_full()
            .max_w(px(780.))
            .rounded(px(12.))
            .bg(SURFACE_2)
            .border_1()
            .border_color(HAIRLINE_HI)
            .overflow_hidden();

        // Keep the selected row within the shown window (scroll the window, not
        // a real virtualized list — the filtered set is small).
        let start = selected.saturating_sub(MAX_ROWS - 1).min(frame.rows.len() - shown);
        for (offset, row) in frame.rows[start..start + shown].iter().enumerate() {
            let ix = start + offset;
            list = list.child(self.render_menu_row(row, ix, ix == selected, colors, cx));
        }

        Some(
            v_flex()
                .w_full()
                .items_center()
                .pb(px(6.))
                .child(list)
                .into_any_element(),
        )
    }

    /// One menu row: `/name` · description · source badge, with the
    /// disabled-with-tooltip treatment for Unbuilt rows. Click accepts (live
    /// rows) or is inert with a tooltip (disabled rows).
    fn render_menu_row(
        &self,
        row: &CommandEntry,
        ix: usize,
        selected: bool,
        colors: &theme::ThemeColors,
        cx: &gpui::Context<Self>,
    ) -> AnyElement {
        let disabled = row.is_disabled();
        let name = row.slash_name();
        let name_color = if disabled {
            colors.text_placeholder
        } else {
            colors.text
        };
        let desc_color = colors.text_placeholder;

        let mut root = h_flex()
            .id(("typeahead-row", ix))
            .w_full()
            .items_center()
            .gap(px(10.))
            .px(px(12.))
            .py(px(7.))
            .bg(if selected {
                colors.element_hover
            } else {
                Hsla::transparent_black()
            })
            .child(
                div()
                    .flex_none()
                    .font_family(
                        theme_settings::ThemeSettings::get_global(cx)
                            .buffer_font
                            .family
                            .clone(),
                    )
                    .text_size(px(13.))
                    .text_color(name_color)
                    .child(SharedString::from(name)),
            )
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .text_size(px(12.))
                    .text_color(desc_color)
                    .truncate()
                    .child(row.description.clone()),
            );

        if let Some(badge) = row.source_badge() {
            root = root.child(render_badge(badge, colors));
        }

        if disabled {
            // Disabled-with-tooltip: greyed, inert, hover explains why.
            let tooltip = row.disabled_tooltip().unwrap_or("Not available yet").to_string();
            root.child(render_badge("disabled", colors))
                .tooltip(move |_, cx| ui::Tooltip::simple(tooltip.clone(), cx))
                .into_any_element()
        } else {
            let row = row.clone();
            root.cursor_pointer()
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.accept_row(&row, window, cx);
                }))
                .into_any_element()
        }
    }
}

/// Read the composer editor's full text + cursor byte-offset (the anchor→
/// offset idiom from `message_editor`, since `usize` isn't a display-snapshot
/// dimension).
fn composer_text_cursor(editor: &Entity<Editor>, cx: &gpui::App) -> (String, usize) {
    editor.read_with(cx, |editor, cx| {
        let text = editor.text(cx);
        let snapshot = editor.buffer().read(cx).snapshot(cx);
        let cursor = editor.selections.newest_anchor().head().to_offset(&snapshot).0;
        (text, cursor)
    })
}

/// A small pill badge (`orch` / `skill` / vendor / `disabled`).
fn render_badge(label: &str, colors: &theme::ThemeColors) -> impl IntoElement {
    div()
        .flex_none()
        .px(px(6.))
        .py(px(1.))
        .rounded(px(5.))
        .bg(SURFACE_2B)
        .border_1()
        .border_color(colors.border)
        .text_size(px(10.))
        .text_color(colors.text_placeholder)
        .child(SharedString::from(label.to_string()))
}
