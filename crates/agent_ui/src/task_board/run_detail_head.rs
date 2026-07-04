//! The run drawer's chrome (web `drawer.js` head anatomy + board.css
//! `.drawer-head`/`.drawer-worked`/`.drawer-tabs`/`#drawer-close`): vendor
//! swatch · title · raw-status pill · close button, the worked-for strip,
//! and the flat text tabs. Split from `run_detail` (entity + slide-over) per
//! the modularity ceiling.

use gpui::{Animation, AnimationExt as _, AnyElement, Context, FontWeight, SharedString};
use settings::Settings as _;
use theme_settings::ThemeSettings;
use ui::prelude::*;

use crate::agent_accents::accent_for_agent;

use super::motion::{EFFECTS, STATE_FADE, mix};
use super::run_detail::{DrawerTab, RunDetail, RunDrawer};
use super::style::{raw_status_pill, rel, route_reason, tabular_nums};

/// The drawer's header title (spec §4.2: header = TASK TITLE). Chain: the
/// run's task title (first line) → archetype → run id. NEVER the literal
/// "agent run" placeholder — when nothing better exists the run id IS the
/// honest label.
pub(super) fn drawer_title(detail: &RunDetail) -> String {
    if let Some(task) = detail.task.as_deref() {
        if let Some(line) = task.lines().find(|line| !line.trim().is_empty()) {
            return line.trim().to_string();
        }
    }
    if let Some(archetype) = detail.archetype.as_deref()
        && !archetype.trim().is_empty()
    {
        let mut chars = archetype.trim().chars();
        return match chars.next() {
            Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
            None => unreachable!(),
        };
    }
    detail.run_id.clone()
}

impl RunDrawer {
    pub(super) fn render_head(&self, cx: &mut Context<Self>) -> Div {
        let head = h_flex()
            .flex_none()
            .items_center()
            .gap(px(11.))
            .px(px(22.))
            .pt(px(18.));
        match self.detail.clone() {
            Some(detail) => self.render_loaded_head(head, detail, cx),
            // Nothing loaded yet: an HONEST pending/failed header — run id
            // as the label, no fake swatch dot, no empty status pill (the
            // "two grey blobs" anti-state).
            None => {
                let colors = cx.theme().colors();
                let mono = ThemeSettings::get_global(cx).buffer_font.family.clone();
                let meta: SharedString = match &self.load_failed {
                    Some(message) => message.clone(),
                    None => "connecting to bridge…".into(),
                };
                head.child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .text_size(px(15.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(colors.text)
                                .truncate()
                                .child(self.run_id.clone()),
                        )
                        .child(
                            div()
                                .mt(px(3.))
                                .font_family(mono)
                                .text_size(px(12.5))
                                .text_color(colors.text_placeholder)
                                .truncate()
                                .child(meta),
                        ),
                )
                .child(self.render_close(cx))
            }
        }
    }

    fn render_loaded_head(&self, head: Div, detail: RunDetail, cx: &mut Context<Self>) -> Div {
        let colors = cx.theme().colors();
        let mono = ThemeSettings::get_global(cx).buffer_font.family.clone();
        let agent = if detail.agent.is_empty() {
            "agent".to_string()
        } else {
            detail.agent.clone()
        };
        // drawer.js:51-54 `routeReason` — plain second chip segment, NO
        // forced-@agent rewrite (that vocabulary belongs to the inbox chip).
        let reason = route_reason(detail.chip.as_deref());
        head.child(
            div()
                .flex_none()
                .size(px(9.))
                .rounded_full()
                .bg(accent_for_agent(&agent)),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .text_size(px(15.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text)
                        .truncate()
                        .child(SharedString::from(drawer_title(&detail))),
                )
                .child(
                    div()
                        .mt(px(3.))
                        .font_family(mono.clone())
                        .text_size(px(12.5))
                        .text_color(colors.text_placeholder)
                        .truncate()
                        // Elapsed ticks while live: the bridge path re-lands
                        // detail at 1s; the demo path is pushed off the
                        // constellation's frame pump at the same cadence.
                        .child(SharedString::from(format!(
                            "{agent} · {reason} · {}",
                            rel(detail.elapsed_s)
                        ))),
                )
                // The worker's agent-session id (2026-07-04) — a second mono
                // line, present only when the bridge carries one. Traces a run
                // back to its worker session. Prefixed "session" so the raw
                // uuid isn't mistaken for the run id above.
                .children(
                    detail
                        .session_id
                        .as_deref()
                        .filter(|id| !id.trim().is_empty())
                        .map(|id| {
                            div()
                                .mt(px(2.))
                                .font_family(mono)
                                .text_size(px(11.))
                                .text_color(colors.text_placeholder.opacity(0.75))
                                .truncate()
                                .child(SharedString::from(format!("session {id}")))
                        }),
                ),
        )
        // Raw lowercase status — the web drawer's vocabulary
        // (drawer.js:37-38), distinct from the inbox's `statusLabel`.
        .child(raw_status_pill("drawer-pill", &detail.status, cx))
        // Abort affordance — only while the run is live (POSTs
        // /run/<id>/abort; the bridge kills the tracked child process).
        .children(self.render_abort(&detail.status, cx))
        .child(self.render_close(cx))
    }

    /// The abort button: a 30×30 Stop-glyph control shown only while the run
    /// is `running`. Click POSTs the abort endpoint; once posted it reads
    /// disabled (placeholder tint) until the tail-poll lands the killed status.
    fn render_abort(&self, status: &str, cx: &mut Context<Self>) -> Option<AnyElement> {
        // Local (demo-fed) drawers have no bridge run to kill.
        if self.local || status != "running" {
            return None;
        }
        let colors = cx.theme().colors();
        let aborting = self.aborting;
        let color = if aborting {
            colors.text_placeholder
        } else {
            crate::agent_accents::color_for_status("failed")
        };
        Some(
            div()
                .id("drawer-abort")
                .flex_none()
                .size(px(30.))
                .rounded(px(8.))
                .flex()
                .items_center()
                .justify_center()
                .when(!aborting, |this| {
                    this.cursor_pointer()
                        .on_click(cx.listener(|this, _, _, cx| this.abort(cx)))
                })
                .child(
                    Icon::new(IconName::Stop)
                        .size(IconSize::Custom(rems_from_px(18.)))
                        .color(Color::Custom(color)),
                )
                .into_any_element(),
        )
    }

    /// `#drawer-close`: 30×30 grid-centered button, 20px glyph, 8px radius;
    /// hover swaps text-3 → text over a hover bg, both eased 150ms effects
    /// (board.css:181-184).
    fn render_close(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors();
        let (off_text, on_text) = (colors.text_placeholder, colors.text);
        let (off_bg, on_bg) = (gpui::transparent_black(), colors.element_hover);
        let hovered = self.close_hovered;
        let icon = move |color: gpui::Hsla| {
            Icon::new(IconName::Close)
                .size(IconSize::Custom(rems_from_px(20.)))
                .color(Color::Custom(color))
        };
        let base = div()
            .id("drawer-close")
            .flex_none()
            .size(px(30.))
            .rounded(px(8.))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if this.close_hovered != *hovered {
                    this.close_hovered = *hovered;
                    this.close_fade.bump();
                    cx.notify();
                }
            }))
            .on_click(cx.listener(|this, _, _, cx| this.dismiss(cx)));
        if self.close_fade.fresh() {
            let (from_text, to_text) = if hovered {
                (off_text, on_text)
            } else {
                (on_text, off_text)
            };
            let (from_bg, to_bg) = if hovered { (off_bg, on_bg) } else { (on_bg, off_bg) };
            base.with_animation(
                ElementId::NamedInteger(
                    "drawer-close-fade".into(),
                    self.close_fade.generation() as u64,
                ),
                Animation::new(STATE_FADE).with_easing(EFFECTS.easing()),
                move |button, t| {
                    button
                        .bg(mix(from_bg, to_bg, t))
                        .child(icon(mix(from_text, to_text, t)))
                },
            )
            .into_any_element()
        } else if hovered {
            base.bg(on_bg).child(icon(on_text)).into_any_element()
        } else {
            base.child(icon(off_text)).into_any_element()
        }
    }

    pub(super) fn render_worked(&self, cx: &Context<Self>) -> Option<Div> {
        let colors = cx.theme().colors();
        let detail = self.detail.as_ref()?;
        let still = if detail.status == "running" {
            " · still running"
        } else {
            ""
        };
        Some(
            h_flex()
                .flex_none()
                .items_center()
                .gap(px(7.))
                .px(px(22.))
                .pt(px(12.))
                .text_size(px(13.))
                .text_color(colors.text_muted)
                .child(
                    // 16px text-3 (board.css:189 `.drawer-worked .ms`).
                    Icon::new(IconName::Clock)
                        .size(IconSize::Medium)
                        .color(Color::Placeholder),
                )
                .child("Worked for")
                .child(
                    div()
                        .text_color(colors.text)
                        .font_weight(FontWeight::MEDIUM)
                        // Ticks at 1s while running (board.css:190 `b`
                        // carries tabular-nums) — no digit-reflow jitter.
                        .font_features(tabular_nums())
                        .child(SharedString::from(rel(detail.elapsed_s))),
                )
                .child(SharedString::from(still.to_string())),
        )
    }

    /// Flat text tabs (13px/500, active = full text + 2px accent underline,
    /// no boxes — board.css `.drawer-tabs`). Each tab pulls down 1px so the
    /// active underline OVERLAPS the container hairline (one merged line),
    /// and tab switches crossfade color/underline over 150ms effects.
    pub(super) fn render_tabs(&self, cx: &mut Context<Self>) -> Div {
        let colors = cx.theme().colors();
        let border = colors.border;
        let on_text = colors.text;
        let off_text = colors.text_placeholder;
        let on_border = colors.text_accent;
        let off_border = gpui::transparent_black();
        let fresh = self.tab_fade.fresh();
        let generation = self.tab_fade.generation() as u64;
        let (current, previous) = (self.tab, self.prev_tab);

        let tab = |label: &'static str, value: DrawerTab, cx: &Context<Self>| -> AnyElement {
            let on = current == value;
            let was_on = previous == value;
            let base = div()
                .id(ElementId::Name(format!("drawer-tab-{label}").into()))
                .px(px(13.))
                .pt(px(8.))
                .pb(px(10.))
                // board.css `.drawer-tabs button { margin-bottom: -1px }` —
                // the 2px accent underline merges with the strip hairline.
                .mb(px(-1.))
                .text_size(px(13.))
                .font_weight(FontWeight::MEDIUM)
                .cursor_pointer()
                .border_b_2()
                .on_click(cx.listener(move |this, _, _, cx| this.set_tab(value, cx)))
                .child(label);
            if fresh && on != was_on {
                let (from_text, to_text) = if on {
                    (off_text, on_text)
                } else {
                    (on_text, off_text)
                };
                let (from_border, to_border) = if on {
                    (off_border, on_border)
                } else {
                    (on_border, off_border)
                };
                base.with_animation(
                    ElementId::NamedInteger(format!("drawer-tab-fade-{label}").into(), generation),
                    Animation::new(STATE_FADE).with_easing(EFFECTS.easing()),
                    move |tab, t| {
                        tab.text_color(mix(from_text, to_text, t))
                            .border_color(mix(from_border, to_border, t))
                    },
                )
                .into_any_element()
            } else if on {
                base.text_color(on_text)
                    .border_color(on_border)
                    .into_any_element()
            } else {
                base.text_color(off_text)
                    .border_color(off_border)
                    .into_any_element()
            }
        };
        h_flex()
            .flex_none()
            .gap(px(2.))
            .px(px(22.))
            .pt(px(14.))
            .border_b_1()
            .border_color(border)
            .child(tab("Summary", DrawerTab::Summary, cx))
            .child(tab("Result", DrawerTab::Result, cx))
            .child(tab("Logs", DrawerTab::Logs, cx))
    }
}
