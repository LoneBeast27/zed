//! Tests for the dock-pill activity bar: mode enumeration/sort (M0 loader
//! contract), active-mode fallback + switching, and the pure squircle
//! geometry (top/bottom capsule item positions). Sibling file per the
//! `#[path]` idiom to keep `activity_bar.rs` under the modularity ceiling.

use super::*;
use tempfile::TempDir;

fn write_mode(dir: &std::path::Path, id: &str, icon: &str, pin: Option<&str>) {
    std::fs::create_dir_all(dir).unwrap();
    let pin_json = match pin {
        Some(p) => format!(", \"pinned_position\": \"{}\"", p),
        None => String::new(),
    };
    let json = format!(
        r#"{{
            "schema_version": 1,
            "id": "{id}",
            "display_name": "{id}",
            "description": "test mode",
            "icon": "{icon}",
            "accent_color_hex": "DA7756",
            "layout": {{}},
            "default_keybinding": "Ctrl+Alt+1"
            {pin_json}
        }}"#,
        id = id,
        icon = icon,
        pin_json = pin_json,
    );
    std::fs::write(dir.join(format!("{id}.json")), json).unwrap();
}

#[test]
fn top_capsule_item_positions_stack_from_the_inset() {
    // First item sits at inset + hairline border + capsule padding; each
    // further item adds one hit area + gap.
    assert_eq!(top_item_y(0), 15.0); // 10 inset + 1 border + 4 pad
    assert_eq!(top_item_y(1), 59.0); // + 40 + 4
    assert_eq!(top_item_y(4), 191.0); // 15 + 4*44
}

#[test]
fn bottom_capsule_items_anchor_to_the_bar_foot() {
    // One bottom item (the shipped Settings): capsule outer height =
    // 2*(4 pad + 1 border) + 40 = 50, so the item's y =
    // H - 10 (breathing) - 50 + 5 = H - 55.
    assert_eq!(bottom_item_y(600.0, 1, 0), 545.0);
    // Two bottom items: capsule outer height = 10 + 80 + 4 = 94.
    assert_eq!(bottom_item_y(600.0, 2, 0), 501.0);
    assert_eq!(bottom_item_y(600.0, 2, 1), 545.0);
}

#[test]
fn bar_width_covers_inset_capsule_and_gap() {
    // 10 left inset + 48 capsule + 8 right gap — the floating island's
    // column reservation (NOT an edge-flush 48px VSCode rail).
    assert_eq!(ACTIVITY_BAR_WIDTH_PX, 66.0);
}

#[test]
fn mode_enumeration_from_temp_dir_sorts_correctly() {
    // Verify the M0 loader's sort contract is preserved end-to-end:
    // pinned-top first, then unpinned alphabetical, then pinned-bottom.
    let temp = TempDir::new().unwrap();
    let modes_dir = temp.path().join(".agents").join("modes");
    write_mode(&modes_dir, "zeta", "Hub", Some("bottom"));
    write_mode(&modes_dir, "alpha", "Hub", Some("top"));
    write_mode(&modes_dir, "beta", "Hub", None);

    let modes = load_modes_from_dir(&modes_dir);
    let ids: Vec<&str> = modes.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, vec!["alpha", "beta", "zeta"]);
}

#[test]
fn empty_modes_dir_yields_no_modes() {
    let temp = TempDir::new().unwrap();
    let modes_dir = temp.path().join(".agents").join("modes");
    std::fs::create_dir_all(&modes_dir).unwrap();
    assert!(load_modes_from_dir(&modes_dir).is_empty());
}

#[test]
fn missing_modes_dir_yields_no_modes() {
    let temp = TempDir::new().unwrap();
    let modes_dir = temp.path().join(".agents").join("does-not-exist");
    assert!(load_modes_from_dir(&modes_dir).is_empty());
}

#[gpui::test]
fn effective_active_falls_back_when_default_mode_missing(cx: &mut gpui::TestAppContext) {
    let temp = TempDir::new().unwrap();
    let modes_dir = temp.path().join(".agents").join("modes");
    write_mode(&modes_dir, "orchestrator", "Chat", None);
    write_mode(&modes_dir, "settings", "Settings", Some("bottom"));

    cx.update(|cx| {
        let entity = cx.new(|cx| {
            ActivityBar::new(modes_dir.clone(), "nonexistent-mode".to_string(), None, cx)
        });
        entity.update(cx, |bar, _cx| {
            // No mode matches "nonexistent-mode", so the bar falls back
            // to the first non-bottom-pinned mode (orchestrator).
            let active = bar.effective_active();
            assert!(active.is_some());
            assert_eq!(active.unwrap().id, "orchestrator");
        });
    });
}

#[gpui::test]
fn set_active_updates_and_notifies(cx: &mut gpui::TestAppContext) {
    let temp = TempDir::new().unwrap();
    let modes_dir = temp.path().join(".agents").join("modes");
    write_mode(&modes_dir, "orchestrator", "Chat", None);
    write_mode(&modes_dir, "symphony", "AudioOn", None);

    cx.update(|cx| {
        let entity = cx.new(|cx| {
            ActivityBar::new(modes_dir.clone(), "orchestrator".to_string(), None, cx)
        });
        entity.update(cx, |bar, cx| {
            assert_eq!(bar.active_mode_id(), "orchestrator");
            bar.set_active("symphony", cx);
            assert_eq!(bar.active_mode_id(), "symphony");
            assert_eq!(bar.effective_active().unwrap().id, "symphony");
        });
    });
}

#[gpui::test]
fn squircle_targets_follow_the_active_mode_across_clusters(cx: &mut gpui::TestAppContext) {
    // The sliding-squircle aim: top-cluster targets resolve immediately;
    // bottom-cluster targets need the measured bar height (None before the
    // first measure — the render pass skips placement that frame).
    let temp = TempDir::new().unwrap();
    let modes_dir = temp.path().join(".agents").join("modes");
    write_mode(&modes_dir, "orchestrator", "Chat", Some("top"));
    write_mode(&modes_dir, "taskboard", "ListTodo", None);
    write_mode(&modes_dir, "settings", "Settings", Some("bottom"));

    cx.update(|cx| {
        let entity = cx.new(|cx| {
            ActivityBar::new(modes_dir.clone(), "orchestrator".to_string(), None, cx)
        });
        entity.update(cx, |bar, _cx| {
            let (bottom, top): (Vec<&WorkspaceMode>, Vec<&WorkspaceMode>) = bar
                .modes
                .iter()
                .partition(|m| m.pinned_position == Some(PinnedPosition::Bottom));
            // Top cluster: first and second items.
            assert_eq!(bar.squircle_target(&top, &bottom, "orchestrator"), Some(15.0));
            assert_eq!(bar.squircle_target(&top, &bottom, "taskboard"), Some(59.0));
            // Bottom cluster before any measure: unresolvable.
            assert_eq!(bar.squircle_target(&top, &bottom, "settings"), None);
            // After a measure the bottom target anchors to the foot.
            bar.bar_height = Some(gpui::px(600.0));
            assert_eq!(bar.squircle_target(&top, &bottom, "settings"), Some(545.0));
        });
    });
}
