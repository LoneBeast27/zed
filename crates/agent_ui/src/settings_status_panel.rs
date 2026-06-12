//! The Settings status panel (PARITY_SPEC §4.6, RUST_PORT_NOTES §4.6) —
//! **Zed philosophy: settings live in settings.json.** This panel renders
//! READ-ONLY live status in the web's section-card language (`.setting-card`
//! anatomy from panels.css) plus "open settings.json" jumps — no form
//! widgets, no web-style form state.
//!
//! Sections: bridge connection (connected / transport / last-frame age from
//! the [`BridgeStore`]), usage scrape freshness (`_scraped`), the active
//! brain (from the transcript snapshot — a TranscriptWatch is held only
//! while the dock shows the panel), the workspace-modes flag state, and the
//! theme/fonts in effect. The only timer is a 1s age-label roll, alive only
//! while the panel is visible (the §5 worked-for ticker class).

use std::time::Duration;

use gpui::{
    Action, AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable, FontWeight,
    SharedString, Subscription, Task, Window, actions,
};
use settings::Settings as _;
use theme_settings::ThemeSettings;
use ui::prelude::*;
use workspace::dock::{DockPosition, Panel, PanelEvent};

use crate::agent_accents::{STATUS_BLOCKED, STATUS_ERROR, STATUS_RUNNING};
use crate::bridge::{self, BRIDGE_BASE_URL, BridgeStore, Transport, TranscriptWatch, UsageMeta};
use crate::task_board::motion::{STATE_FADE, StateFades, mix};
use crate::task_board::style::{SURFACE_1, rel, tabular_nums};

actions!(
    settings_status_panel,
    [
        /// Toggles focus on the settings status panel.
        ToggleFocus
    ]
);

/// The 1s roll for the live age labels (last frame / scrape freshness).
const AGE_TICK: Duration = Duration::from_secs(1);

/// Bridge-connection transport label.
pub fn transport_label(transport: Transport) -> &'static str {
    match transport {
        Transport::Sse => "SSE push",
        Transport::Polling => "polling fallback",
        Transport::None => "—",
    }
}

/// The scrape-freshness line (the usage panel's phrasing, §4.4 — degrades
/// to "self-metered", never vanishes).
pub fn freshness_line(meta: &UsageMeta) -> String {
    match meta.scraped.as_ref() {
        Some(scrape) if scrape.stale => match scrape.age_h {
            Some(age_h) => format!("stale — scraped {age_h}h ago"),
            None => "stale".to_string(),
        },
        Some(scrape) => match scrape.age_min {
            Some(age_min) => format!("scraped {age_min}m ago"),
            None => "fresh".to_string(),
        },
        None => "self-metered".to_string(),
    }
}

pub struct SettingsStatusPanel {
    focus_handle: FocusHandle,
    position: DockPosition,
    store: Entity<BridgeStore>,
    /// Held only while the dock shows this panel — keeps the `/transcript`
    /// poll (the brain's source) alive, the orchestrator-panel pattern.
    transcript_watch: Option<TranscriptWatch>,
    /// 1s repaint for the age labels — `Some` only while visible.
    ticker: Option<Task<()>>,
    fades: StateFades,
    _store_subscription: Subscription,
}

impl SettingsStatusPanel {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let store = bridge::global_store(cx);
        // The store change-gates its own notifies; this panel renders cheap
        // text rows, so a plain re-render per store notify is in budget.
        let _store_subscription = cx.observe(&store, |_, _, cx| cx.notify());
        Self {
            focus_handle: cx.focus_handle(),
            position: DockPosition::Left,
            store,
            transcript_watch: None,
            ticker: None,
            fades: StateFades::new(),
            _store_subscription,
        }
    }

    fn set_live(&mut self, active: bool, cx: &mut Context<Self>) {
        if active {
            if self.transcript_watch.is_none() {
                self.transcript_watch =
                    Some(self.store.update(cx, |store, cx| store.watch_transcript(cx)));
            }
            if self.ticker.is_none() {
                self.ticker = Some(cx.spawn(async move |this, cx| {
                    loop {
                        cx.background_executor().timer(AGE_TICK).await;
                        if this.update(cx, |_, cx| cx.notify()).is_err() {
                            return;
                        }
                    }
                }));
            }
        } else {
            self.transcript_watch = None;
            self.ticker = None;
        }
    }

    /// `.panel-head`: "Settings" 18px/500 + the philosophy sub-line.
    fn render_header(&self, cx: &App) -> Div {
        let colors = cx.theme().colors();
        h_flex()
            .flex_none()
            .items_baseline()
            .gap(px(12.))
            .px(px(28.))
            .pt(px(18.))
            .pb(px(14.))
            .border_b_1()
            .border_color(colors.border)
            .child(
                div()
                    .text_size(px(18.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(colors.text)
                    .child("Settings"),
            )
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(colors.text_placeholder)
                    .child("live status only — settings live in settings.json"),
            )
    }

    /// One `.setting-card` (panels.css:144-149): h2 over rows (+ optional
    /// hint and the "open settings.json" jump).
    fn card(
        &self,
        title: &'static str,
        rows: Vec<AnyElement>,
        hint: Option<&'static str>,
        jump: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = cx.theme().colors();
        v_flex()
            .w_full()
            .max_w(px(720.))
            .rounded(px(12.))
            .bg(SURFACE_1)
            .border_1()
            .border_color(colors.border)
            .px(px(18.))
            .py(px(16.))
            .mb(px(16.))
            .child(
                div()
                    .mb(px(12.))
                    .text_size(px(15.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(colors.text)
                    .child(title),
            )
            .children(rows)
            .children(hint.map(|hint| {
                div()
                    .mt(px(10.))
                    .text_size(px(13.))
                    .line_height(relative(1.6))
                    .text_color(colors.text_placeholder)
                    .child(hint)
            }))
            .children(jump.then(|| self.render_jump(title, cx)))
            .into_any_element()
    }

    /// The "open settings.json" jump — dispatches `zed::OpenSettings`
    /// (read-only panel; edits happen where Zed edits live).
    fn render_jump(&self, key: &'static str, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors();
        let jump_id = ElementId::Name(format!("settings-jump-{key}").into());
        let hover_t = self.fades.t(&jump_id);
        let mono = ThemeSettings::get_global(cx).buffer_font.family.clone();
        h_flex()
            .mt(px(10.))
            .child(
                h_flex()
                    .id(jump_id.clone())
                    .items_center()
                    .gap(px(6.))
                    .px(px(8.))
                    .py(px(3.))
                    .rounded(px(6.))
                    .bg(colors.element_hover.opacity(hover_t))
                    .font_family(mono)
                    .text_size(px(12.))
                    .text_color(mix(colors.text_muted, colors.text, hover_t))
                    .cursor_pointer()
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        if this.fades.set(jump_id.clone(), *hovered, STATE_FADE) {
                            cx.notify();
                        }
                    }))
                    .on_click(|_, window, cx| {
                        window.dispatch_action(Box::new(zed_actions::OpenSettings), cx);
                    })
                    .child(
                        Icon::new(IconName::Settings)
                            .size(IconSize::Small)
                            .color(Color::Muted),
                    )
                    .child("open settings.json"),
            )
            .into_any_element()
    }

    /// One `.setting-row`: 14px label left, value right (mono variant for
    /// code-ish values, optional status tint).
    fn row(
        &self,
        label: &'static str,
        value: String,
        mono: bool,
        tint: Option<gpui::Hsla>,
        cx: &App,
    ) -> AnyElement {
        let colors = cx.theme().colors();
        let mono_family = ThemeSettings::get_global(cx).buffer_font.family.clone();
        h_flex()
            .w_full()
            .items_baseline()
            .justify_between()
            .gap(px(16.))
            .py(px(7.))
            .child(
                div()
                    .text_size(px(14.))
                    .text_color(colors.text_muted)
                    .child(label),
            )
            .child(
                div()
                    .min_w_0()
                    .text_size(px(if mono { 12. } else { 14. }))
                    .when(mono, |this| {
                        this.font_family(mono_family).font_features(tabular_nums())
                    })
                    .text_color(tint.unwrap_or(colors.text))
                    .truncate()
                    .child(SharedString::from(value)),
            )
            .into_any_element()
    }

    fn render_bridge_card(&self, cx: &mut Context<Self>) -> AnyElement {
        let (connected, transport, frame_age) = {
            let store = self.store.read(cx);
            (store.connected, store.transport, store.last_event_age())
        };
        let connection = if connected {
            ("connected", Some(gpui::Hsla::from(STATUS_RUNNING)))
        } else {
            ("offline", Some(gpui::Hsla::from(STATUS_ERROR)))
        };
        let frame = frame_age
            .map(|age| format!("{} ago", rel(age.as_secs_f64())))
            .unwrap_or_else(|| "—".to_string());
        let rows = vec![
            self.row("Connection", connection.0.to_string(), false, connection.1, cx),
            self.row("Transport", transport_label(transport).to_string(), true, None, cx),
            self.row("Last frame", frame, true, None, cx),
            self.row("Endpoint", BRIDGE_BASE_URL.to_string(), true, None, cx),
        ];
        self.card("Bridge", rows, None, false, cx)
    }

    fn render_scrape_card(&self, cx: &mut Context<Self>) -> AnyElement {
        let meta = self.store.read(cx).usage_meta.clone();
        let stale = meta.stale();
        let mut rows = vec![
            self.row(
                "Source",
                meta.source.clone().unwrap_or_else(|| "—".to_string()),
                true,
                None,
                cx,
            ),
            self.row(
                "Freshness",
                freshness_line(&meta),
                false,
                stale.then(|| gpui::Hsla::from(STATUS_BLOCKED)),
                cx,
            ),
        ];
        if let Some(phrase) = meta.scraped.as_ref().and_then(|s| s.reset_phrase.clone()) {
            rows.push(self.row("Next reset", format!("resets {phrase}"), true, None, cx));
        }
        self.card(
            "Usage scrape",
            rows,
            Some(
                "Real remaining-quota numbers come from the vendor-page scrape; \
                 without one the Usage panel degrades to self-metered estimates.",
            ),
            false,
            cx,
        )
    }

    fn render_brain_card(&self, cx: &mut Context<Self>) -> AnyElement {
        let brain = self
            .store
            .read(cx)
            .transcript
            .as_ref()
            .and_then(|snapshot| snapshot.brain.clone())
            .unwrap_or_else(|| "idle".to_string());
        let rows = vec![self.row("Active", brain, true, None, cx)];
        self.card(
            "Heartbeat brain",
            rows,
            Some(
                "Read-only. Override with the ORCH_BRAIN environment variable \
                 before launching the bridge.",
            ),
            false,
            cx,
        )
    }

    fn render_modes_card(&self, cx: &mut Context<Self>) -> AnyElement {
        let settings = agent_settings::AgentSettings::get_global(cx);
        let flag = if settings.workspace_modes {
            "enabled"
        } else {
            "disabled"
        };
        let default_mode = settings.default_mode.clone();
        let modes_dir = settings
            .modes_dir
            .clone()
            .map(|dir| dir.display().to_string())
            .unwrap_or_else(|| ".agents/modes (workspace default)".to_string());
        let rows = vec![
            self.row("agent.workspace_modes", flag.to_string(), true, None, cx),
            self.row("Default mode", default_mode, true, None, cx),
            self.row("Modes dir", modes_dir, true, None, cx),
        ];
        self.card("Workspace modes", rows, None, true, cx)
    }

    fn render_appearance_card(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme_name = cx.theme().name.to_string();
        let (ui_font, buffer_font) = {
            let settings = ThemeSettings::get_global(cx);
            (
                settings.ui_font.family.to_string(),
                settings.buffer_font.family.to_string(),
            )
        };
        let rows = vec![
            self.row("Theme", theme_name, false, None, cx),
            self.row("UI font", ui_font, true, None, cx),
            self.row("Buffer font", buffer_font, true, None, cx),
        ];
        self.card("Appearance", rows, None, true, cx)
    }
}

impl Render for SettingsStatusPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let cards = vec![
            self.render_bridge_card(cx),
            self.render_scrape_card(cx),
            self.render_brain_card(cx),
            self.render_modes_card(cx),
            self.render_appearance_card(cx),
        ];
        let colors = cx.theme().colors();
        let panel = v_flex()
            .key_context("SettingsStatusPanel")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(colors.panel_background)
            .child(self.render_header(cx))
            .child(
                // `.settings-content`: 22/28/32 padding (the web's left nav
                // is web-form-era chrome — the native panel is the status
                // stack alone, per the §4.6 ruling).
                v_flex()
                    .id("settings-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(px(28.))
                    .pt(px(22.))
                    .pb(px(32.))
                    .items_start()
                    .children(cards),
            );
        // The §8.7a frame pump for the jump-link hover crossfades.
        if self.fades.any_animating() {
            window.request_animation_frame();
        }
        panel
    }
}

impl Focusable for SettingsStatusPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<PanelEvent> for SettingsStatusPanel {}

impl Panel for SettingsStatusPanel {
    fn persistent_name() -> &'static str {
        "SettingsStatusPanel"
    }

    fn panel_key() -> &'static str {
        "SettingsStatusPanel"
    }

    fn position(&self, _window: &Window, _cx: &App) -> DockPosition {
        self.position
    }

    fn position_is_valid(&self, position: DockPosition) -> bool {
        matches!(position, DockPosition::Left | DockPosition::Right)
    }

    fn set_position(&mut self, position: DockPosition, _: &mut Window, cx: &mut Context<Self>) {
        // Runtime-only, mirroring the task board (Z1).
        self.position = position;
        cx.notify();
    }

    fn set_active(&mut self, active: bool, _window: &mut Window, cx: &mut Context<Self>) {
        // Visibility gates BOTH live feeds: the transcript watch (brain) and
        // the 1s age-label ticker.
        self.set_live(active, cx);
    }

    fn default_size(&self, _window: &Window, _cx: &App) -> Pixels {
        px(560.)
    }

    fn icon(&self, _window: &Window, _cx: &App) -> Option<IconName> {
        Some(IconName::Settings)
    }

    fn icon_tooltip(&self, _window: &Window, _cx: &App) -> Option<&'static str> {
        Some("Settings status")
    }

    fn toggle_action(&self) -> Box<dyn Action> {
        Box::new(ToggleFocus)
    }

    fn activation_priority(&self) -> u32 {
        9
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::ScrapeMeta;

    #[test]
    fn transport_labels_cover_every_variant() {
        assert_eq!(transport_label(Transport::Sse), "SSE push");
        assert_eq!(transport_label(Transport::Polling), "polling fallback");
        assert_eq!(transport_label(Transport::None), "—");
    }

    #[test]
    fn freshness_degrades_visibly_never_vanishes() {
        // §4.4 lesson applied to §4.6: unknown/stale stays readable.
        assert_eq!(freshness_line(&UsageMeta::default()), "self-metered");
        let fresh = UsageMeta {
            source: None,
            scraped: Some(ScrapeMeta {
                stale: false,
                age_h: None,
                age_min: Some(12.0),
                reset_phrase: Some("14:32".to_string()),
            }),
        };
        assert_eq!(freshness_line(&fresh), "scraped 12m ago");
        let stale = UsageMeta {
            source: None,
            scraped: Some(ScrapeMeta {
                stale: true,
                age_h: Some(36.2),
                age_min: None,
                reset_phrase: None,
            }),
        };
        assert_eq!(freshness_line(&stale), "stale — scraped 36.2h ago");
    }
}
