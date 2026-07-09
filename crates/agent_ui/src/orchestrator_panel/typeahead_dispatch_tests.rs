//! Live-dispatch tests for the `/` typeahead keyboard nav (the fix for dead
//! Up/Down/Tab/Enter while the menu is open).
//!
//! These render the REAL `OrchestratorPanel` as a window root (so its composer
//! editor is the focused node), load the SAME two keymaps the launch path loads
//! (`default-<os>.json` for the editor's native caret keys + `workspace_modes.json`
//! for the typeahead bindings), then dispatch real keystrokes through the real
//! dispatch tree. They assert the nav key reaches `typeahead_move`/
//! `typeahead_accept` — i.e. the intended wiring is intact end-to-end.
//!
//! HONEST HARNESS CAVEAT (RUST_PORT_NOTES §11, verified 2026-07-05): these tests
//! PASS on the PRE-FIX code too. The `TestDispatcher`/`VisualTestContext` keymap
//! resolution routes the keys to the typeahead actions regardless of whether the
//! `menu_open` flag sits on the deck (broken live) or the editor node (the fix),
//! because at the pure-keymap layer BOTH predicates resolve the typeahead
//! binding first. The real-window failure lives in the platform dispatch the
//! harness abstracts away. So this suite is a FORWARD guard (it catches a future
//! break of the action plumbing / accept semantics), NOT a regression gate for
//! THIS bug — live-window keyboard confirmation is owed to the coordinator.

use gpui::{AppContext as _, Focusable as _, TestAppContext, VisualTestContext, px, size};
use project::Project;
use settings::KeymapFile;
use workspace::Workspace;

use super::panel::OrchestratorPanel;

/// Build a real workspace, then render the orchestrator panel as its OWN window
/// root (focused), with both launch keymaps loaded. Returns the panel entity +
/// its window's `VisualTestContext` (borrowed from `cx`, the `add_window_view`
/// idiom).
async fn panel_window(
    cx: &mut TestAppContext,
) -> (gpui::Entity<OrchestratorPanel>, &mut VisualTestContext) {
    crate::test_support::init_test(cx);
    cx.update(|cx| {
        // The editor's native auto_height caret keys (up/down/enter) — the
        // bindings the typeahead nav must outrank.
        let default = KeymapFile::load_asset_allow_partial_failure(
            settings::DEFAULT_KEYMAP_PATH,
            cx,
        )
        .unwrap();
        cx.bind_keys(default);
        // The fork typeahead bindings (workspace-modes keymap), loaded exactly
        // as `zed.rs::load_default_keymap` does behind the flag.
        let modes =
            KeymapFile::load_asset_allow_partial_failure("keymaps/workspace_modes.json", cx)
                .unwrap();
        cx.bind_keys(modes);
    });

    let fs = fs::FakeFs::new(cx.executor());
    let project = Project::test(fs, [], cx).await;
    // Window A hosts a real workspace purely so the panel constructor has a live
    // `WeakEntity<Workspace>` (entities are app-global, so the weak handle is
    // usable from the panel's own window). The window's cx is discarded — we
    // only need the entity handle.
    let weak = {
        let (workspace, _cx_a) =
            cx.add_window_view(|window, cx| Workspace::test_new(project.clone(), window, cx));
        workspace.downgrade()
    };

    // Window B: the panel IS the root view → focused, rendered, dispatch-ready.
    let (panel, cx) = cx.add_window_view(|window, cx| {
        let stack = cx.new(|cx| crate::islands::NotifStack::new(weak.clone(), cx));
        OrchestratorPanel::new(weak, stack, window, cx)
    });
    cx.run_until_parked();
    // A concrete size forces a real rendered frame (and thus a dispatch tree).
    cx.simulate_resize(size(px(900.), px(700.)));
    cx.run_until_parked();
    (panel, cx)
}

/// Seed the composer text and place the cursor at the end, then let the frame
/// settle so `render_composer` derives menu-open and stamps `menu_open` on the
/// editor's key context.
fn type_into_composer(
    panel: &gpui::Entity<OrchestratorPanel>,
    text: &str,
    cx: &mut VisualTestContext,
) {
    panel.update_in(cx, |panel, window, cx| {
        let editor = panel.composer.editor.clone();
        editor.update(cx, |editor, cx| {
            editor.set_text(text, window, cx);
        });
        window.focus(&panel.composer.editor.read(cx).focus_handle(cx), cx);
    });
    cx.run_until_parked();
    // Force a render so the menu-open flag lands on the editor's key context for
    // the dispatch tree the next keystroke will resolve against.
    cx.simulate_resize(size(px(900.), px(700.)));
    cx.run_until_parked();
}

fn selected(panel: &gpui::Entity<OrchestratorPanel>, cx: &mut VisualTestContext) -> usize {
    panel.read_with(cx, |panel, _| panel.typeahead.selected_index())
}

fn composer_text(panel: &gpui::Entity<OrchestratorPanel>, cx: &mut VisualTestContext) -> String {
    panel.read_with(cx, |panel, cx| panel.composer.editor.read(cx).text(cx))
}

fn menu_row_count(panel: &gpui::Entity<OrchestratorPanel>, cx: &mut VisualTestContext) -> usize {
    panel.update(cx, |panel, cx| {
        panel.typeahead_frame(cx).map_or(0, |frame| frame.rows.len())
    })
}

#[gpui::test]
async fn down_arrow_moves_typeahead_selection_not_the_caret(cx: &mut TestAppContext) {
    let (panel, cx) = panel_window(cx).await;
    // `/` alone opens the menu with the full orchestrator+skills set.
    type_into_composer(&panel, "/", cx);
    let rows = menu_row_count(&panel, cx);
    assert!(rows >= 2, "menu should have multiple rows to move between (got {rows})");
    assert_eq!(selected(&panel, cx), 0, "selection starts at row 0");

    cx.simulate_keystrokes("down");
    assert_eq!(
        selected(&panel, cx),
        1,
        "Down moved the typeahead selection (NOT the editor caret)"
    );

    cx.simulate_keystrokes("up");
    assert_eq!(
        selected(&panel, cx),
        0,
        "Up moved the selection back"
    );

    // The text is untouched — the key drove the menu, never the buffer.
    assert_eq!(composer_text(&panel, cx), "/");
}

#[gpui::test]
async fn up_arrow_wraps_selection(cx: &mut TestAppContext) {
    let (panel, cx) = panel_window(cx).await;
    type_into_composer(&panel, "/", cx);
    let rows = menu_row_count(&panel, cx);
    assert!(rows >= 2, "need multiple rows to prove wrap");
    // Up from row 0 wraps to the last row.
    cx.simulate_keystrokes("up");
    assert_eq!(
        selected(&panel, cx),
        rows - 1,
        "Up from the first row wraps to the last"
    );
}

#[gpui::test]
async fn enter_accepts_the_highlighted_command_when_menu_open(cx: &mut TestAppContext) {
    let (panel, cx) = panel_window(cx).await;
    // Narrow to the `/board` orchestrator command (a live, non-disabled row).
    type_into_composer(&panel, "/board", cx);
    assert_eq!(menu_row_count(&panel, cx), 1, "'/board' filters to one row");

    cx.simulate_keystrokes("enter");
    assert_eq!(
        composer_text(&panel, cx),
        "/board ",
        "Enter accepted the highlighted row (spliced '/board ') instead of sending/newlining"
    );
}

#[gpui::test]
async fn tab_also_accepts_the_highlighted_command(cx: &mut TestAppContext) {
    let (panel, cx) = panel_window(cx).await;
    type_into_composer(&panel, "/board", cx);
    cx.simulate_keystrokes("tab");
    assert_eq!(
        composer_text(&panel, cx),
        "/board ",
        "Tab accepted the highlighted row"
    );
}

#[gpui::test]
async fn escape_dismisses_and_drops_the_slash_token(cx: &mut TestAppContext) {
    let (panel, cx) = panel_window(cx).await;
    type_into_composer(&panel, "hi /bo", cx);
    assert!(menu_row_count(&panel, cx) >= 1, "the '/bo' token opens the menu");
    cx.simulate_keystrokes("escape");
    assert_eq!(
        composer_text(&panel, cx),
        "hi ",
        "Escape closed the menu AND dropped the stray '/bo' token"
    );
}

#[gpui::test]
async fn typing_more_of_the_slash_token_narrows_the_menu(cx: &mut TestAppContext) {
    // The char-filter path: a longer `/token` re-parses via the editor's
    // text-change → panel re-render → `typeahead_frame` recompute, narrowing the
    // filtered row set. This is INDEPENDENT of the keymap nav fix (typed letters
    // are not bound keys — they flow to the editor's input handler, and the
    // BufferEdited subscription drives the recompute).
    let (panel, cx) = panel_window(cx).await;
    type_into_composer(&panel, "/", cx);
    let all = menu_row_count(&panel, cx);
    assert!(all >= 2, "bare '/' shows the whole in-scope set (got {all})");

    // Narrow to a specific command — the row set must shrink to the matches.
    type_into_composer(&panel, "/board", cx);
    let narrowed = menu_row_count(&panel, cx);
    assert!(
        narrowed < all,
        "a longer slash token narrows the menu ({narrowed} < {all})"
    );
    assert_eq!(narrowed, 1, "'/board' matches exactly the one orchestrator row");
}

#[gpui::test]
async fn enter_with_menu_closed_does_not_accept(cx: &mut TestAppContext) {
    let (panel, cx) = panel_window(cx).await;
    // A plain message (no `/token`) — the menu is closed. Enter routes to Send
    // (a no-op POST path in-test, but crucially it does NOT splice a command and
    // does NOT insert a newline — proving the menu_open flag gates correctly).
    type_into_composer(&panel, "just text", cx);
    assert_eq!(menu_row_count(&panel, cx), 0, "no menu for a plain message");
    cx.simulate_keystrokes("enter");
    // Send clears the composer on a real POST attempt; either way the text was
    // NOT turned into a spliced command and NOT given a newline.
    let text = composer_text(&panel, cx);
    assert!(
        !text.contains('\n'),
        "menu-closed Enter must not newline the auto_height editor (got {text:?})"
    );
    assert!(
        !text.starts_with('/'),
        "menu-closed Enter must not splice a command (got {text:?})"
    );
}
