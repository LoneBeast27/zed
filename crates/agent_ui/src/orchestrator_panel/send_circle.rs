//! The composer's send⇄stop circle (PARITY_SPEC §0 island morph, `chat.css
//! .send-circle`): ONE 40px element that flexes circle⇄rounded-square while
//! the fill crossfades accent⇄error and the two stacked glyphs (arrow /
//! stop square) crossfade — all on retargetable Instant-clocked motion
//! values, so a busy flip mid-reveal continues from wherever the circle is
//! (§8.7a). Split from `composer.rs` per the 500-line ceiling.

use std::time::Duration;

use gpui::{Animation, AnimationExt as _, AnyElement};
use ui::prelude::*;

use crate::agent_accents::{ACCENT_FILL, STATUS_ERROR};
use crate::task_board::motion::{AnimatedColor, AnimatedValue, EFFECTS, SPATIAL};

use super::panel::OrchestratorPanel;

/// Send-circle reveal (`.send-circle { transition: opacity .15s
/// var(--effects-curve) }`; the web's concurrent `scale(.7)` beat is dropped
/// — gpui has no element scale styling).
const REVEAL: Duration = Duration::from_millis(150);
/// Fill crossfade accent⇄error (`background .15s var(--effects-curve)`).
const FILL_FADE: Duration = Duration::from_millis(150);
/// The container radius morph circle⇄square (`border-radius .26s
/// var(--spatial-curve)`).
const MORPH: Duration = Duration::from_millis(260);
/// The stacked glyph crossfade (`.ic-send/.ic-stop { transition: opacity
/// .12s }`).
const GLYPH_FADE: Duration = Duration::from_millis(120);

/// The morph state — owned by the composer.
pub(super) struct SendCircle {
    /// Reveal opacity: 1 while there's text or the orchestrator is busy
    /// (the stop affordance), else 0.
    reveal: AnimatedValue,
    /// Container fill: `--accent-fill` (send) ⇄ `--error` (stop).
    fill: AnimatedColor,
    /// Container radius: 20 (a true circle on the 40px square) ⇄ 12.
    radius: AnimatedValue,
    /// Glyph crossfade scalar: 0 = arrow, 1 = stop square.
    glyph: AnimatedValue,
    /// The last busy flag the morph values were retargeted for.
    busy: bool,
}

impl SendCircle {
    pub fn new() -> Self {
        Self {
            reveal: AnimatedValue::settled(0., EFFECTS, REVEAL),
            fill: AnimatedColor::settled(ACCENT_FILL.into(), EFFECTS, FILL_FADE),
            radius: AnimatedValue::settled(20., SPATIAL, MORPH),
            glyph: AnimatedValue::settled(0., EFFECTS, GLYPH_FADE),
            busy: false,
        }
    }

    /// Retarget the morph values for this frame's state. Same-target
    /// retargets are no-ops, so render-loop calls never restart anything.
    pub fn update_motion(&mut self, busy: bool, has_text: bool) {
        self.reveal.retarget(if has_text || busy { 1. } else { 0. });
        if busy != self.busy {
            self.busy = busy;
            self.fill
                .retarget(if busy { STATUS_ERROR } else { ACCENT_FILL }.into());
            self.radius.retarget(if busy { 12. } else { 20. });
            self.glyph.retarget(if busy { 1. } else { 0. });
        }
    }

    fn animating(&self) -> bool {
        self.reveal.animating()
            || self.fill.animating()
            || self.radius.animating()
            || self.glyph.animating()
    }

    fn generation(&self) -> u64 {
        (self.reveal.generation() as u64) << 24
            ^ (self.fill.generation() as u64) << 16
            ^ (self.radius.generation() as u64) << 8
            ^ self.glyph.generation() as u64
    }

    /// The element. `interactive` mirrors the web's pointer-events rule
    /// (text present or busy); a click routes through
    /// [`OrchestratorPanel::send_circle_click`] — while busy the circle IS the
    /// Stop button (morphed to the stop square) and hard-stops (POST /abort,
    /// B-T4); idle it sends.
    pub fn render(
        &self,
        interactive: bool,
        cx: &mut gpui::Context<OrchestratorPanel>,
    ) -> AnyElement {
        let build = |radius: f32, fill: gpui::Hsla, glyph: f32, reveal: f32| {
            div()
                .size(px(40.))
                .rounded(px(radius))
                .bg(fill)
                .relative()
                .flex()
                .items_center()
                .justify_center()
                .opacity(reveal)
                .child(
                    // ic-send: the arrow glyph.
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .opacity(1. - glyph)
                        .child(
                            // `.ic-send .ms { font-size: 20px }` — the 40px
                            // accent circle carries a 20px arrow.
                            Icon::new(IconName::ArrowUp)
                                .size(IconSize::Custom(rems_from_px(20.)))
                                .color(Color::Custom(gpui::white())),
                        ),
                )
                .child(
                    // ic-stop: the 13px white rounded square.
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .opacity(glyph)
                        .child(div().size(px(13.)).rounded(px(3.)).bg(gpui::white())),
                )
        };

        let morph: AnyElement = if self.animating() {
            let (reveal, fill, radius, glyph) = (
                self.reveal.clone(),
                self.fill.clone(),
                self.radius.clone(),
                self.glyph.clone(),
            );
            // Frame pump over the Instant-clocked values (notif_card idiom;
            // SPATIAL evaluates inside, per the motion-module guard).
            div()
                .with_animation(
                    ElementId::NamedInteger("composer-send-morph".into(), self.generation()),
                    Animation::new(MORPH),
                    move |this, _| {
                        this.child(build(
                            radius.current(),
                            fill.current(),
                            glyph.current().clamp(0., 1.),
                            reveal.current().clamp(0., 1.),
                        ))
                    },
                )
                .into_any_element()
        } else {
            build(
                self.radius.target(),
                self.fill.target(),
                self.glyph.target(),
                self.reveal.target(),
            )
            .into_any_element()
        };

        let circle = div().id("composer-send").size(px(40.)).child(morph);
        if interactive {
            circle
                .cursor_pointer()
                .on_click(cx.listener(|this, _, window, cx| this.send_circle_click(window, cx)))
                .into_any_element()
        } else {
            // `pointer-events: none` until text is present.
            circle.into_any_element()
        }
    }
}
