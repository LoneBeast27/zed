//! Resource orchestration banner — surfaces GPU/CPU/disk/net lock state from
//! `.claude/locks/` at the top of the agent panel chrome.
//!
//! Spec: `.planning/zed-fork/RESOURCE_ORCHESTRATION.md` §4.1 (R4 slice).
//!
//! Reads the on-disk lock state authored by `.claude/locks/lib.sh`:
//!   - `<workspace>/.claude/locks/<resource>.lock.d/{pid,label,session_id,acquired_at}`
//!   - `<workspace>/.claude/locks/<resource>.queue.jsonl` (line count = queue depth)
//!   - `<workspace>/.claude/locks/<resource>.events.jsonl` (tail = recent events)
//!
//! Live updates are driven by `fs::Fs::watch()` on `.claude/locks/`, which
//! internally uses the `notify` crate (already in the workspace dep tree).
//!
//! Banner is gated on `agent.canonical_agent_ui && agent.show_resource_banner`
//! (the AgentPanel call site applies the canonical gate; this module reads
//! the `show_resource_banner` flag from settings).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use agent_settings::AgentSettings;
use fs::Fs;
use futures::StreamExt;
use gpui::{Context, Entity, Hsla, IntoElement, ParentElement, Rgba, Styled, Task, rgb};
use settings::Settings as _;
use ui::{Color, Label, LabelSize, Tooltip, h_flex, prelude::*};

/// Default polling latency for `fs.watch()` (200ms matches `notify`'s default debounce).
pub const LOCK_WATCH_LATENCY: Duration = Duration::from_millis(200);

/// Number of trailing events read from `<resource>.events.jsonl`.
const EVENTS_TAIL: usize = 5;

/// Owner of a held lock, deserialized from the four files inside
/// `<resource>.lock.d/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockOwner {
    pub pid: String,
    pub label: String,
    pub session_id: String,
    pub acquired_at: String,
}

/// One parsed line from `<resource>.events.jsonl`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventEntry {
    pub ts: String,
    pub action: String,
    pub resource: String,
    pub by_pid: String,
    pub label: String,
}

/// Aggregate on-disk lock state for a single resource.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LockState {
    /// Resource name (e.g. "gpu", "cpu", "disk", "net").
    pub resource: String,
    /// `Some(owner)` when the lock dir is present and well-formed.
    pub held_by: Option<LockOwner>,
    /// Line count of `<resource>.queue.jsonl` (0 if file is missing).
    pub queue_depth: usize,
    /// Last `EVENTS_TAIL` lines of `<resource>.events.jsonl`.
    pub events_tail: Vec<EventEntry>,
    /// True when `.claude/locks/` does not exist on disk. In this state the
    /// banner renders "monitoring disabled" rather than "free" — they mean
    /// different things and the user wants to distinguish.
    pub monitoring_disabled: bool,
}

impl LockState {
    /// Returns `true` when the lock dir was present but the resource is not held.
    pub fn is_free(&self) -> bool {
        !self.monitoring_disabled && self.held_by.is_none()
    }

    /// Held duration in whole minutes, derived from the `acquired_at` ISO
    /// timestamp of the holder. Returns `None` when no holder or when the
    /// timestamp is unparseable.
    pub fn held_minutes(&self) -> Option<i64> {
        let owner = self.held_by.as_ref()?;
        let acquired = chrono::DateTime::parse_from_rfc3339(owner.acquired_at.trim()).ok()?;
        let now = chrono::Utc::now();
        let elapsed = now.signed_duration_since(acquired.with_timezone(&chrono::Utc));
        Some(elapsed.num_minutes().max(0))
    }
}

/// Reads the on-disk state for one resource lock. All file reads are
/// fault-tolerant: missing files yield default values, malformed files are
/// treated as absent.
///
/// `locks_dir` is the workspace's `.claude/locks/` directory. `resource` is
/// the resource short-name (e.g. "gpu").
pub fn read_lock_state(locks_dir: &Path, resource: &str) -> LockState {
    let mut state = LockState {
        resource: resource.to_string(),
        ..Default::default()
    };

    if !locks_dir.exists() {
        state.monitoring_disabled = true;
        return state;
    }

    // Lock dir: `<resource>.lock.d/`
    let lock_dir = locks_dir.join(format!("{}.lock.d", resource));
    if lock_dir.is_dir() {
        let read_field = |name: &str| -> String {
            std::fs::read_to_string(lock_dir.join(name))
                .ok()
                .map(|s| s.trim().to_string())
                .unwrap_or_default()
        };
        let pid = read_field("pid");
        // A `lock.d` dir with no pid file is in the middle of being torn down
        // (or set up). Treat as free rather than fabricating a phantom owner.
        if !pid.is_empty() {
            state.held_by = Some(LockOwner {
                pid,
                label: read_field("label"),
                session_id: read_field("session_id"),
                acquired_at: read_field("acquired_at"),
            });
        }
    }

    // Queue: `<resource>.queue.jsonl` line count.
    let queue_path = locks_dir.join(format!("{}.queue.jsonl", resource));
    state.queue_depth = std::fs::read_to_string(&queue_path)
        .map(|s| s.lines().filter(|l| !l.trim().is_empty()).count())
        .unwrap_or(0);

    // Events tail: last `EVENTS_TAIL` lines of `<resource>.events.jsonl`.
    let events_path = locks_dir.join(format!("{}.events.jsonl", resource));
    if let Ok(contents) = std::fs::read_to_string(&events_path) {
        let lines: Vec<&str> = contents
            .lines()
            .filter(|l| !l.trim().is_empty())
            .collect();
        let tail_start = lines.len().saturating_sub(EVENTS_TAIL);
        for line in &lines[tail_start..] {
            if let Some(entry) = parse_event_line(line) {
                state.events_tail.push(entry);
            }
        }
    }

    state
}

/// Minimal JSON-line parser. We accept any object shape with the canonical
/// fields and gracefully ignore everything else.
fn parse_event_line(line: &str) -> Option<EventEntry> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let obj = v.as_object()?;
    let s = |key: &str| -> String {
        obj.get(key)
            .map(|v| match v {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            })
            .unwrap_or_default()
    };
    Some(EventEntry {
        ts: obj
            .get("ts_iso")
            .and_then(|v| v.as_str().map(|s| s.to_string()))
            .unwrap_or_else(|| s("ts")),
        action: s("action"),
        resource: s("resource"),
        by_pid: s("by_pid"),
        label: s("label"),
    })
}

/// Banner color thresholds — green / amber / red per §4.1.
fn banner_color(state: &LockState) -> Hsla {
    if state.monitoring_disabled {
        // Apple system gray — same fallback the canonical pill uses.
        Rgba::from(rgb(0x8E8E93)).into()
    } else if state.is_free() {
        // Green — Apple system green.
        Rgba::from(rgb(0x34C759)).into()
    } else {
        let held_long = state.held_minutes().map(|m| m >= 30).unwrap_or(false);
        if state.queue_depth >= 3 || held_long {
            // Red — Apple system red.
            Rgba::from(rgb(0xFF3B30)).into()
        } else {
            // Amber — Apple system orange.
            Rgba::from(rgb(0xFF9500)).into()
        }
    }
}

/// Formats the canonical one-line status string per §4.1:
///   "GPU lock: HELD by {session_id} ({label}) · ⏱ {N min} held · {Q} sessions queued"
///   "GPU lock: FREE"
///   "GPU lock: monitoring disabled"
pub fn format_banner_text(state: &LockState) -> String {
    let resource_upper = state.resource.to_uppercase();
    if state.monitoring_disabled {
        return format!("{} lock: monitoring disabled", resource_upper);
    }
    let Some(owner) = state.held_by.as_ref() else {
        return format!("{} lock: FREE", resource_upper);
    };
    let session = if owner.session_id.is_empty() {
        owner.pid.as_str()
    } else {
        owner.session_id.as_str()
    };
    let mins = state.held_minutes().unwrap_or(0);
    let label = if owner.label.is_empty() {
        "unlabeled".to_string()
    } else {
        owner.label.clone()
    };
    format!(
        "{} lock: HELD by {} ({}) · {} min held · {} sessions queued",
        resource_upper, session, label, mins, state.queue_depth
    )
}

/// Renders the one-line banner. Mirrors the visual grammar of
/// `render_agent_picker_pill` in `conversation_view/thread_view.rs` (commit
/// 7789ca5152): tinted background, accent border, colored dot, short label.
pub fn render_resource_banner(
    state: &LockState,
    cx: &mut Context<ResourceBanner>,
) -> impl IntoElement {
    let accent = banner_color(state);
    let mut accent_bg = accent;
    accent_bg.a = 0.12;
    let mut accent_border = accent;
    accent_border.a = 0.35;

    let lightning = if state.monitoring_disabled { "·" } else { "⚡" };
    let text = format_banner_text(state);
    let tooltip_text = build_tooltip(state);

    let _ = cx; // touch to silence the unused warning when the on-click branch is dropped

    h_flex()
        .id("resource-banner")
        .px_2()
        .py_0p5()
        .gap_1p5()
        .w_full()
        .rounded_sm()
        .bg(accent_bg)
        .border_1()
        .border_color(accent_border)
        .child(div().size_1p5().rounded_full().bg(accent))
        .child(
            Label::new(format!("{} {}", lightning, text))
                .size(LabelSize::XSmall)
                .color(Color::Custom(accent)),
        )
        .tooltip(move |_window, cx| Tooltip::simple(tooltip_text.clone(), cx))
}

/// Multi-line tooltip with the last few events — useful when the banner
/// itself is collapsed to a single line.
fn build_tooltip(state: &LockState) -> String {
    let mut lines = Vec::new();
    lines.push(format_banner_text(state));
    if !state.events_tail.is_empty() {
        lines.push(String::new());
        lines.push("Recent events:".to_string());
        for ev in state.events_tail.iter().rev() {
            lines.push(format!(
                "  {} {} {} (pid {}, {})",
                ev.ts, ev.action, ev.resource, ev.by_pid, ev.label
            ));
        }
    }
    lines.join("\n")
}

/// Entity wrapping the live lock state for one resource. Holds the `fs.watch`
/// task; dropping the entity drops the task and tears down the watcher.
pub struct ResourceBanner {
    locks_dir: PathBuf,
    resource: String,
    state: LockState,
    _watcher: Option<Task<()>>,
}

impl ResourceBanner {
    /// Construct a banner for `resource` (e.g. "gpu") backed by `locks_dir`
    /// (the workspace `.claude/locks/` path). Spawns a background task that
    /// listens to fs events on `locks_dir` and refreshes `state` on each
    /// change, then calls `cx.notify()` to drive a re-render.
    pub fn new(
        locks_dir: PathBuf,
        resource: impl Into<String>,
        fs: Arc<dyn Fs>,
        cx: &mut Context<Self>,
    ) -> Self {
        let resource = resource.into();
        let state = read_lock_state(&locks_dir, &resource);

        // Only spawn the watcher when the locks dir actually exists. If it
        // doesn't, we render "monitoring disabled" and stay silent.
        let watcher_task = if locks_dir.exists() {
            let watch_dir = locks_dir.clone();
            let resource_for_task = resource.clone();
            let fs_for_task = fs.clone();
            Some(cx.spawn(async move |this, cx| {
                let (mut events, _watcher) = fs_for_task
                    .watch(&watch_dir, LOCK_WATCH_LATENCY)
                    .await;
                while events.next().await.is_some() {
                    let next = read_lock_state(&watch_dir, &resource_for_task);
                    if this
                        .update(cx, |banner, cx| {
                            if banner.state != next {
                                banner.state = next;
                                cx.notify();
                            }
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            }))
        } else {
            None
        };

        Self {
            locks_dir,
            resource,
            state,
            _watcher: watcher_task,
        }
    }

    pub fn state(&self) -> &LockState {
        &self.state
    }

    pub fn locks_dir(&self) -> &Path {
        &self.locks_dir
    }

    pub fn resource(&self) -> &str {
        &self.resource
    }

    /// Returns true when the user has opted out of the banner via the
    /// `agent.show_resource_banner` setting.
    pub fn is_enabled_in_settings(cx: &gpui::App) -> bool {
        AgentSettings::get_global(cx).show_resource_banner
    }
}

impl gpui::Render for ResourceBanner {
    fn render(&mut self, _window: &mut gpui::Window, cx: &mut Context<Self>) -> impl IntoElement {
        render_resource_banner(&self.state, cx)
    }
}

/// Convenience: build a `ResourceBanner` Entity for the GPU resource.
pub fn build_gpu_banner(
    locks_dir: PathBuf,
    fs: Arc<dyn Fs>,
    cx: &mut gpui::App,
) -> Entity<ResourceBanner> {
    cx.new(|cx| ResourceBanner::new(locks_dir, "gpu", fs, cx))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write(path: &Path, content: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    #[test]
    fn test_read_lock_state_free_no_locks_dir() {
        // Locks dir does not exist → monitoring disabled.
        let temp = TempDir::new().unwrap();
        let locks_dir = temp.path().join(".claude").join("locks");
        let state = read_lock_state(&locks_dir, "gpu");
        assert!(state.monitoring_disabled);
        assert!(state.held_by.is_none());
        assert_eq!(state.queue_depth, 0);
        assert!(state.events_tail.is_empty());
        assert!(!state.is_free()); // monitoring_disabled != free
    }

    #[test]
    fn test_read_lock_state_free_empty_dir() {
        // Locks dir exists but has no lock files → free.
        let temp = TempDir::new().unwrap();
        let locks_dir = temp.path().to_path_buf();
        std::fs::create_dir_all(&locks_dir).unwrap();
        let state = read_lock_state(&locks_dir, "gpu");
        assert!(!state.monitoring_disabled);
        assert!(state.held_by.is_none());
        assert_eq!(state.queue_depth, 0);
        assert!(state.is_free());
        assert_eq!(format_banner_text(&state), "GPU lock: FREE");
    }

    #[test]
    fn test_read_lock_state_held() {
        // Lock dir + pid file → held.
        let temp = TempDir::new().unwrap();
        let locks_dir = temp.path().to_path_buf();
        let lock_d = locks_dir.join("gpu.lock.d");
        write(&lock_d.join("pid"), "171538\n");
        write(&lock_d.join("label"), "afk-smoke-test");
        write(&lock_d.join("session_id"), "claude-acp:thread-abc123");
        write(&lock_d.join("acquired_at"), "2026-05-26T01:21:05Z");

        let state = read_lock_state(&locks_dir, "gpu");
        assert!(!state.monitoring_disabled);
        let owner = state.held_by.as_ref().expect("should be held");
        assert_eq!(owner.pid, "171538");
        assert_eq!(owner.label, "afk-smoke-test");
        assert_eq!(owner.session_id, "claude-acp:thread-abc123");
        assert_eq!(owner.acquired_at, "2026-05-26T01:21:05Z");
        assert_eq!(state.queue_depth, 0);
        let text = format_banner_text(&state);
        assert!(text.contains("HELD by claude-acp:thread-abc123"));
        assert!(text.contains("(afk-smoke-test)"));
        assert!(text.contains("sessions queued"));
    }

    #[test]
    fn test_read_lock_state_held_with_queue() {
        // Lock dir + pid file + queue.jsonl with 5 lines → held with queue=5.
        let temp = TempDir::new().unwrap();
        let locks_dir = temp.path().to_path_buf();
        let lock_d = locks_dir.join("gpu.lock.d");
        write(&lock_d.join("pid"), "12345");
        write(&lock_d.join("label"), "training");
        write(&lock_d.join("session_id"), "pid-12345");
        write(&lock_d.join("acquired_at"), "2026-05-26T00:00:00Z");

        let queue_lines = (0..5)
            .map(|i| format!(r#"{{"ts":1779758465,"pid":{},"label":"waiter-{}"}}"#, i, i))
            .collect::<Vec<_>>()
            .join("\n");
        write(&locks_dir.join("gpu.queue.jsonl"), &format!("{}\n", queue_lines));

        // Also drop a few events so the events_tail path is exercised.
        let events = vec![
            r#"{"ts_iso":"2026-05-26T01:00:00Z","action":"acquired","resource":"gpu","by_pid":"12345","label":"training"}"#,
            r#"{"ts_iso":"2026-05-26T01:05:00Z","action":"queued","resource":"gpu","by_pid":"99999","label":"waiter-0"}"#,
        ]
        .join("\n");
        write(&locks_dir.join("gpu.events.jsonl"), &events);

        let state = read_lock_state(&locks_dir, "gpu");
        assert!(state.held_by.is_some());
        assert_eq!(state.queue_depth, 5);
        assert_eq!(state.events_tail.len(), 2);
        assert_eq!(state.events_tail[0].action, "acquired");
        assert_eq!(state.events_tail[1].action, "queued");

        // queue_depth >= 3 → red color path.
        let color = banner_color(&state);
        // Red is ~ HSL of 0xFF3B30.
        let expected_red: Hsla = Rgba::from(rgb(0xFF3B30)).into();
        assert_eq!(color.h, expected_red.h);
        assert_eq!(color.s, expected_red.s);
    }

    #[test]
    fn test_format_banner_text_monitoring_disabled() {
        let state = LockState {
            resource: "gpu".into(),
            monitoring_disabled: true,
            ..Default::default()
        };
        assert_eq!(format_banner_text(&state), "GPU lock: monitoring disabled");
    }

    #[test]
    fn test_banner_color_free_is_green() {
        let state = LockState {
            resource: "gpu".into(),
            ..Default::default()
        };
        let color = banner_color(&state);
        let expected_green: Hsla = Rgba::from(rgb(0x34C759)).into();
        assert_eq!(color.h, expected_green.h);
    }

    #[test]
    fn test_banner_color_held_amber_when_queue_under_three() {
        let temp = TempDir::new().unwrap();
        let locks_dir = temp.path().to_path_buf();
        let lock_d = locks_dir.join("gpu.lock.d");
        write(&lock_d.join("pid"), "100");
        write(&lock_d.join("label"), "x");
        write(&lock_d.join("session_id"), "s");
        // Recent acquired_at so held_minutes < 30.
        let now = chrono::Utc::now().to_rfc3339();
        write(&lock_d.join("acquired_at"), &now);

        let state = read_lock_state(&locks_dir, "gpu");
        let color = banner_color(&state);
        let expected_amber: Hsla = Rgba::from(rgb(0xFF9500)).into();
        assert_eq!(color.h, expected_amber.h);
    }
}
