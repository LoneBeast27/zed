//! The Usage Island entity (PARITY_SPEC §4.8) — the corner-cluster HEAD.
//! ONE persistent element, four representations reached only by morphing in
//! place (rest ↔ notify ↔ held ↔ expanded). Right-edge-pinned by the
//! cluster, so notify extensions grow LEFT and the expanded card grows DOWN
//! out of the corner anchor (§4.9 anchored emergence).
//!
//! The §8.7 fluidity gate, natively: geometry morphs on the spatial curve
//! (width/height 280ms, radius 260ms — the approved build's island-scaled
//! durations; overshoot mapped inside the animator) while the container
//! tint and the dual-layer content crossfade ride the effects curve — ALL
//! keyed by the same state flip, starting the same frame; no sequential
//! phases, no property snaps (morph targets are measured through the same
//! text shaper that paints, so explicit pixel geometry is exact).
//! This ports the web's FIXED concurrent dual-layer FLIP (usage-island.js,
//! 2026-06-12) — the outgoing face overlays and fades while the incoming
//! face fades in and the container FLIPs width AND height.
//!
//! Threshold semantics live in [`super::state`] (75/90 crossing queue,
//! critical pin, ack); this entity owns the bridge ingest, the 6s notify
//! hold + post-morph pump timers, and the morph rendering.

use gpui::{
    Animation, AnimationExt as _, AnyElement, App, Context, Entity, SharedString, Subscription,
    Task, WeakEntity, Window,
};
use ui::prelude::*;
use workspace::Workspace;

use crate::agent_accents::{STATUS_ERROR, Tone, tone_for_used, used_pct};
use crate::bridge::{self, BridgeStore};
use crate::task_board::motion::{
    AnimatedColor, AnimatedValue, EFFECTS, RollValue, SPATIAL, StateFade,
};
use crate::task_board::style::HAIRLINE_HI;
use crate::usage_panel::fmt_pct;

use super::island_faces::{Face, build_face, measure, short_pool};
use super::state::{IslandMachine, IslandState, TimerCmd};

/// Island morph duration — the approved web build deliberately scales the
/// island's spatial class down to 280ms (`MORPH_MS = 280` /
/// `width/height .28s` in usage-island.{js,css}; per-island duration
/// scaling explicitly blessed by app.css "islands may scale durations").
/// Also the post-contract pump delay.
const MORPH: std::time::Duration = std::time::Duration::from_millis(280);
/// Radius rides a slightly shorter clock — web `border-radius .26s`.
const MORPH_R: std::time::Duration = std::time::Duration::from_millis(260);
/// Notify auto-revert hold (web `NOTIFY_HOLD_MS`).
const NOTIFY_HOLD: std::time::Duration = std::time::Duration::from_secs(6);
/// Incoming face fade-in (effects class).
const FACE_IN: std::time::Duration = std::time::Duration::from_millis(200);
/// Outgoing face fade-out (web `.isl-face.out` 120ms).
const FACE_OUT: std::time::Duration = std::time::Duration::from_millis(120);
/// Container tint crossfade — web `background-color .2s / border-color .2s
/// var(--effects-curve)`, concurrent with the geometry morph (§4.9 split).
/// Instant-clocked on its own [`AnimatedColor`]s, so it is independent of
/// the morph duration and of wrapper restarts.
const TINT_FADE: std::time::Duration = std::time::Duration::from_millis(200);
/// `--surface-float: #1e1e1e` — floating-layer fill (no backdrop material
/// natively; the solid fallback is the spec'd non-blur path). Shared with
/// the toast cards (one corner system, one surface).
pub(crate) const SURFACE_FLOAT: gpui::Rgba = crate::agent_accents::rgba_hex(0x1e1e1eff);

/// One pool as the island renders it.
#[derive(PartialEq)]
struct PoolView {
    name: String,
    short: SharedString,
    used: Option<f64>,
    window: String,
    tone: Tone,
}

pub struct UsageIsland {
    store: Entity<BridgeStore>,
    workspace: WeakEntity<Workspace>,
    machine: IslandMachine,
    pools: Vec<PoolView>,
    reset_phrase: Option<String>,
    /// The representation on screen; [`outgoing_face`](Self::outgoing_face)
    /// overlays during the dual-layer crossfade window.
    current_face: Option<Face>,
    outgoing_face: Option<Face>,
    face_fade: StateFade,
    /// The rest pill's tightest-pool % as a numeric roll (web `rollNumber`
    /// on the island %). A %-only change rolls the digits in place — no face
    /// crossfade — because [`Face::same_representation`] treats it as the
    /// same Rest representation.
    rest_pct_roll: RollValue,
    /// Container geometry — retargetable so an interrupt mid-morph re-bases
    /// from the interpolated value instead of snapping (§8.7a).
    geom_w: AnimatedValue,
    geom_h: AnimatedValue,
    geom_r: AnimatedValue,
    geom_init: bool,
    /// Critical tint (held = error-tinted container) — retargetable colors,
    /// so any interrupt re-bases from the CURRENTLY RENDERED tint, never
    /// the opposite endpoint (web: bg/border transition only when
    /// `.critical` actually flips).
    tint_bg: AnimatedColor,
    tint_border: AnimatedColor,
    /// `Some` only while armed — dropping cancels (no idle timers).
    hold_task: Option<Task<()>>,
    pump_task: Option<Task<()>>,
    _store_subscription: Subscription,
}

impl UsageIsland {
    pub fn new(workspace: WeakEntity<Workspace>, cx: &mut Context<Self>) -> Self {
        let store = bridge::global_store(cx);
        let _store_subscription =
            cx.observe(&store, |this: &mut Self, _, cx| this.sync_from_store(cx));
        let (rest_bg, rest_border) = Self::tint(false);
        let mut this = Self {
            store,
            workspace,
            machine: IslandMachine::default(),
            pools: Vec::new(),
            reset_phrase: None,
            current_face: None,
            outgoing_face: None,
            face_fade: StateFade::default(),
            rest_pct_roll: RollValue::new(String::new()),
            geom_w: AnimatedValue::settled(0., SPATIAL, MORPH),
            geom_h: AnimatedValue::settled(0., SPATIAL, MORPH),
            geom_r: AnimatedValue::settled(0., SPATIAL, MORPH_R),
            geom_init: false,
            tint_bg: AnimatedColor::settled(rest_bg, EFFECTS, TINT_FADE),
            tint_border: AnimatedColor::settled(rest_border, EFFECTS, TINT_FADE),
            hold_task: None,
            pump_task: None,
            _store_subscription,
        };
        this.sync_from_store(cx);
        this
    }

    pub fn is_expanded(&self) -> bool {
        self.machine.state() == IslandState::Expanded
    }

    /// Click-away while expanded (the cluster's backdrop routes here).
    pub fn click_away(&mut self, cx: &mut Context<Self>) {
        let cmd = self.machine.click_away();
        self.apply_cmd(cmd, cx);
        cx.notify();
    }

    fn clicked(&mut self, cx: &mut Context<Self>) {
        let cmd = self.machine.click();
        self.apply_cmd(cmd, cx);
        cx.notify();
    }

    /// Expanded-card row click → contract + route to the usage mode (the
    /// native `location.hash = "#/usage"`; falls back to opening the usage
    /// center tab directly when no `usage` mode is installed).
    fn row_clicked(&mut self, _pool: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        let cmd = self.machine.row_clicked();
        self.apply_cmd(cmd, cx);
        self.workspace
            .update(cx, |workspace, cx| {
                if !crate::workspace_mode_switcher::switch_to_mode_id(
                    "usage", workspace, window, cx,
                ) && let Some(surfaces) = crate::mode_item::workspace_surfaces(workspace, cx)
                {
                    crate::mode_item::open_center_item(
                        &surfaces.usage,
                        None,
                        workspace,
                        window,
                        cx,
                    );
                }
            })
            .ok();
        cx.notify();
    }

    /// A bridge snapshot arrived: rebuild the pool views and feed the
    /// threshold machine. The store notifies every second while a run
    /// ticks, so repaint only when the usage data or the machine state
    /// actually changed (§8 idle cost).
    fn sync_from_store(&mut self, cx: &mut Context<Self>) {
        // Agentic-demo gate: the corner island is the §4.8 proving ground and
        // is visible on EVERY route, so it stages the same pools + reset phrase
        // as the usage panel (crate::usage_panel_demo) — one shared demo
        // source, no island-local duplication. Falls through to the live store
        // when the gate is off.
        let (usage, usage_meta) = if crate::bridge::is_agentic_demo() {
            (
                crate::usage_panel_demo::demo_pools(),
                crate::usage_panel_demo::demo_meta(),
            )
        } else {
            let store = self.store.read(cx);
            (store.usage.clone(), store.usage_meta.clone())
        };
        let reset_phrase = usage_meta
            .scraped
            .as_ref()
            .and_then(|scrape| scrape.reset_phrase.clone());
        let pools: Vec<PoolView> = usage
            .iter()
            .map(|pool| {
                let used = used_pct(pool.headroom_pct);
                PoolView {
                    short: SharedString::from(short_pool(&pool.name).to_string()),
                    name: pool.name.clone(),
                    used,
                    window: pool.window.clone().unwrap_or_default(),
                    tone: tone_for_used(used),
                }
            })
            .collect();
        let changed = pools != self.pools || reset_phrase != self.reset_phrase;
        self.reset_phrase = reset_phrase;
        self.pools = pools;
        let snapshot: Vec<(String, Option<f64>)> = self
            .pools
            .iter()
            .map(|pool| (pool.name.clone(), pool.used))
            .collect();
        let state_before = self.machine.state();
        let cmd = self.machine.ingest(&snapshot);
        self.apply_cmd(cmd, cx);
        if changed || self.machine.state() != state_before {
            cx.notify();
        }
    }

    /// Arm/clear the machine's timers. Stale pump fires are harmless (the
    /// machine pumps only from rest), mirroring the web's guarded
    /// `setTimeout(pump, MORPH_MS)`.
    fn apply_cmd(&mut self, cmd: TimerCmd, cx: &mut Context<Self>) {
        match cmd {
            TimerCmd::ArmHold => {
                self.hold_task = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(NOTIFY_HOLD).await;
                    this.update(cx, |this, cx| {
                        let cmd = this.machine.hold_expired();
                        this.apply_cmd(cmd, cx);
                        cx.notify();
                    })
                    .ok();
                }));
            }
            TimerCmd::ArmPump => {
                self.hold_task = None;
                self.pump_task = Some(cx.spawn(async move |this, cx| {
                    // Let the contract-to-rest morph settle before the next
                    // queued crossing extends the pill again.
                    cx.background_executor().timer(MORPH).await;
                    this.update(cx, |this, cx| {
                        let cmd = this.machine.pump();
                        this.apply_cmd(cmd, cx);
                        cx.notify();
                    })
                    .ok();
                }));
            }
            TimerCmd::Clear => {
                self.hold_task = None;
            }
            // No transition — the armed 6s hold (or pump) survives. Store
            // ingests at 1Hz/per-SSE-frame route here, so ticking data can
            // never cancel the notify auto-revert (web: ingest never touches
            // the hold timer).
            TimerCmd::None => {}
        }
    }

    /// The representation the current state + data calls for.
    fn desired_face(&self) -> Face {
        match self.machine.state() {
            IslandState::Rest => {
                let dots = self.pools.iter().take(5).map(|pool| pool.tone).collect();
                // The tightest pool's % — first strictly-greatest wins
                // (web `reduce` with a -1 sentinel; all-unknown → no text).
                let mut best_used = -1.0_f64;
                let mut pct = None;
                for pool in &self.pools {
                    let used = pool.used.unwrap_or(-1.);
                    if used > best_used {
                        best_used = used;
                        pct = pool
                            .used
                            .map(|used| SharedString::from(format!("{}%", fmt_pct(used))));
                    }
                }
                Face::Rest { dots, pct }
            }
            IslandState::Notify | IslandState::Held => {
                let key = self.machine.active_pool().unwrap_or_default();
                let view = self.pools.iter().find(|pool| pool.name == key);
                let (short, window, used, tone) = match view {
                    Some(view) => (
                        view.short.clone(),
                        view.window.clone(),
                        view.used,
                        view.tone,
                    ),
                    // Vanished pool: web defaults `{used: 0, tone: "ok"}`.
                    None => (
                        SharedString::from(short_pool(key).to_string()),
                        String::new(),
                        Some(0.),
                        Tone::Ok,
                    ),
                };
                let resets = self
                    .reset_phrase
                    .as_ref()
                    .map(|phrase| format!(" · resets {phrase}"))
                    .unwrap_or_default();
                let text = format!(
                    "{short} {window} · {}%{resets}",
                    used.map(fmt_pct).unwrap_or_else(|| "—".into())
                );
                Face::Notify {
                    tone,
                    text: SharedString::from(text),
                }
            }
            IslandState::Expanded => Face::Card {
                rows: self
                    .pools
                    .iter()
                    .map(|pool| super::island_faces::CardRow {
                        pool: SharedString::from(pool.name.clone()),
                        short: pool.short.clone(),
                        tone: pool.tone,
                        used: pool.used.map(|used| used as f32),
                        pct: SharedString::from(
                            pool.used
                                .map(|used| format!("{}%", fmt_pct(used)))
                                .unwrap_or_else(|| "—".into()),
                        ),
                    })
                    .collect(),
            },
        }
    }

    /// Container fill/border: error-tinted while a critical is held
    /// (web `.critical`), the floating-surface pair otherwise.
    fn tint(held: bool) -> (gpui::Hsla, gpui::Hsla) {
        if held {
            (
                gpui::Hsla::from(STATUS_ERROR).opacity(0.10),
                gpui::Hsla::from(STATUS_ERROR).opacity(0.50),
            )
        } else {
            (SURFACE_FLOAT.into(), HAIRLINE_HI.into())
        }
    }
}

impl Render for UsageIsland {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // ── morph bookkeeping: face change → dual-layer crossfade +
        //    geometry retarget, all opened the same frame ──
        // The dual-layer crossfade triggers only on a REPRESENTATION change
        // (`same_representation`): a Rest %-only tick is NOT a face swap — the
        // % rolls in place instead (the numeric-roll primitive). Derived
        // `PartialEq` still gates the store-tick repaint skip below.
        let face = self.desired_face();
        if self
            .current_face
            .as_ref()
            .is_some_and(|current| !current.same_representation(&face))
        {
            self.outgoing_face = self.current_face.take();
            self.face_fade.bump();
        }
        self.current_face = Some(face);
        if !self.face_fade.fresh() {
            self.outgoing_face = None;
        }
        let current = self.current_face.as_ref().expect("set above").clone();
        // Roll the rest % toward its current value (no-op off-Rest / when
        // unchanged). Only the changed digits slide; "%" is a sep.
        self.rest_pct_roll
            .set(current.rest_pct().map(|pct| pct.to_string()).unwrap_or_default());
        let metrics = measure(&current, window, cx);
        if self.geom_init {
            self.geom_w.retarget(metrics.w);
            self.geom_h.retarget(metrics.h);
            self.geom_r.retarget(metrics.r);
        } else {
            // First paint: appear at rest geometry, no entrance morph.
            self.geom_init = true;
            self.geom_w.jump(metrics.w);
            self.geom_h.jump(metrics.h);
            self.geom_r.jump(metrics.r);
        }
        let held = self.machine.state() == IslandState::Held;
        // Same-target retargets are no-ops; a real flip crossfades from the
        // CURRENTLY RENDERED tint — never the opposite endpoint (§8.7a).
        let (to_bg, to_border) = Self::tint(held);
        self.tint_bg.retarget(to_bg);
        self.tint_border.retarget(to_border);

        // ── faces (dual-layer: outgoing overlays + fades out 120ms,
        //    incoming fades in 200ms — both effects-curve, concurrent with
        //    the geometry from frame one) ──
        let weak = cx.weak_entity();
        let on_row = move |pool: SharedString, window: &mut Window, cx: &mut App| {
            weak.update(cx, |this, cx| this.row_clicked(pool, window, cx))
                .ok();
        };
        let face_generation = self.face_fade.generation() as u64;
        // The live face's % rolls in place (keyed `isl-pct`, stable across
        // ticks — identity survives so a roll mid-flight is never reset).
        let rest_pct_el = current.rest_pct().map(|_| {
            self.rest_pct_roll
                .element("isl-pct", px(12.), gpui::FontWeight::MEDIUM, cx.theme().colors().text)
                .into_any_element()
        });
        let incoming = build_face(&current, on_row, rest_pct_el, cx);
        let incoming: AnyElement = if self.face_fade.fresh() {
            div()
                .child(incoming)
                .with_animation(
                    ElementId::NamedInteger("isl-face-in".into(), face_generation),
                    Animation::new(FACE_IN).with_easing(EFFECTS.easing()),
                    |face, t| face.opacity(t),
                )
                .into_any_element()
        } else {
            incoming
        };
        let outgoing = self.outgoing_face.as_ref().map(|face| {
            div()
                .absolute()
                .top_0()
                .left_0()
                .child(build_face(face, |_, _, _| {}, None, cx))
                .with_animation(
                    ElementId::NamedInteger("isl-face-out".into(), face_generation),
                    Animation::new(FACE_OUT).with_easing(EFFECTS.easing()),
                    |face, t| face.opacity(1. - t),
                )
        });

        // ── container ──
        let expanded = self.machine.state() == IslandState::Expanded;
        let base = div()
            .id("usage-island")
            .relative()
            .overflow_hidden()
            .border_1()
            .when(!expanded, |this| this.cursor_pointer())
            .on_click(cx.listener(|this, _, _, cx| this.clicked(cx)))
            // PASSIVE click-away (the web's document-level capture listener,
            // usage-island.js onDocClick): a mouse-down anywhere outside the
            // island contracts it WITHOUT consuming the event — the click
            // still lands, and hover/scroll beneath stay live (no occluding
            // backdrop).
            .when(expanded, |this| {
                this.on_mouse_down_out(cx.listener(|this, _, _, cx| this.click_away(cx)))
            })
            .children(outgoing)
            .child(incoming);

        let geometry_live =
            self.geom_w.animating() || self.geom_h.animating() || self.geom_r.animating();
        let tint_live = self.tint_bg.animating() || self.tint_border.animating();
        // The rest-% roll shares the island's single frame pump: while it is
        // in flight the morph wrapper stays attached (at settled geometry if
        // nothing else moves), re-running `render` each frame so `incoming`
        // rebuilds with the fresh roll progress. No second pump, no identity
        // churn on the roll element (keyed `isl-pct`, §8.7a).
        let roll_live = self.rest_pct_roll.animating();
        if geometry_live || tint_live || roll_live {
            let (w, h, r) = (
                self.geom_w.clone(),
                self.geom_h.clone(),
                self.geom_r.clone(),
            );
            let (bg, border) = (self.tint_bg.clone(), self.tint_border.clone());
            let generation = (self.geom_w.generation() as u64) << 32
                ^ (self.geom_h.generation() as u64) << 24
                ^ (self.geom_r.generation() as u64) << 16
                ^ (self.tint_bg.generation() as u64) << 12
                ^ (self.tint_border.generation() as u64) << 8
                ^ (self.rest_pct_roll.generation() as u64);
            base.with_animation(
                ElementId::NamedInteger("usage-island-morph".into(), generation),
                // The wrapper is only a FRAME PUMP: every axis is Instant-
                // clocked inside AnimatedValue/AnimatedColor (SPATIAL mapped
                // inside `current()` — motion GUARD; tint on its own 200ms
                // EFFECTS clock). A wrapper restart (any generation bump)
                // re-bases NOTHING — non-retargeted axes keep their own
                // flight and the tint continues from its current mix, so
                // interrupts are continuous on ALL axes (§8.7a).
                Animation::new(MORPH),
                move |container, _| {
                    container
                        .w(px(w.current()))
                        .h(px(h.current()))
                        .rounded(px(r.current().max(0.)))
                        .bg(bg.current())
                        .border_color(border.current())
                },
            )
            .into_any_element()
        } else {
            base.w(px(self.geom_w.target()))
                .h(px(self.geom_h.target()))
                .rounded(px(self.geom_r.target()))
                .bg(self.tint_bg.target())
                .border_color(self.tint_border.target())
                .into_any_element()
        }
    }
}
