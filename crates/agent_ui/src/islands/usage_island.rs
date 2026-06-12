//! The Usage Island entity (PARITY_SPEC §4.8) — the corner-cluster HEAD.
//! ONE persistent element, four representations reached only by morphing in
//! place (rest ↔ notify ↔ held ↔ expanded). Right-edge-pinned by the
//! cluster, so notify extensions grow LEFT and the expanded card grows DOWN
//! out of the corner anchor (§4.9 anchored emergence).
//!
//! The §8.7 fluidity gate, natively: geometry (width/height/radius) morphs
//! on the spatial curve (500ms, overshoot mapped inside the animator) while
//! the container tint and the dual-layer content crossfade ride the effects
//! curve — ALL keyed by the same state flip, starting the same frame; no
//! sequential phases, no property snaps (morph targets are measured through
//! the same text shaper that paints, so explicit pixel geometry is exact).
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
use crate::task_board::motion::{AnimatedValue, EFFECTS, SPATIAL, StateFade, mix};
use crate::task_board::style::HAIRLINE_HI;
use crate::usage_panel::{UsagePanel, fmt_pct};

use super::island_faces::{Face, build_face, measure, short_pool};
use super::state::{IslandMachine, IslandState, TimerCmd};

/// Island morph duration — the §4.9 spatial class (500ms).
const MORPH: std::time::Duration = std::time::Duration::from_millis(500);
/// Notify auto-revert hold (web `NOTIFY_HOLD_MS`).
const NOTIFY_HOLD: std::time::Duration = std::time::Duration::from_secs(6);
/// Incoming face fade-in (effects class).
const FACE_IN: std::time::Duration = std::time::Duration::from_millis(200);
/// Outgoing face fade-out (web `.isl-face.out` 120ms).
const FACE_OUT: std::time::Duration = std::time::Duration::from_millis(120);
/// Container tint crossfade share of the morph (200ms of the 500ms,
/// concurrent — §4.9 split).
const TINT_RESCALE: f32 = 2.5;
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
    /// Container geometry — retargetable so an interrupt mid-morph re-bases
    /// from the interpolated value instead of snapping (§8.7a).
    geom_w: AnimatedValue,
    geom_h: AnimatedValue,
    geom_r: AnimatedValue,
    geom_init: bool,
    /// Critical tint state (held = error-tinted container).
    was_held: bool,
    tint_fade: StateFade,
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
        let mut this = Self {
            store,
            workspace,
            machine: IslandMachine::default(),
            pools: Vec::new(),
            reset_phrase: None,
            current_face: None,
            outgoing_face: None,
            face_fade: StateFade::default(),
            geom_w: AnimatedValue::settled(0., SPATIAL, MORPH),
            geom_h: AnimatedValue::settled(0., SPATIAL, MORPH),
            geom_r: AnimatedValue::settled(0., SPATIAL, MORPH),
            geom_init: false,
            was_held: false,
            tint_fade: StateFade::default(),
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
    /// native `location.hash = "#/usage"`; falls back to focusing the
    /// panel when no `usage` mode is installed).
    fn row_clicked(&mut self, _pool: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        let cmd = self.machine.row_clicked();
        self.apply_cmd(cmd, cx);
        self.workspace
            .update(cx, |workspace, cx| {
                if !crate::workspace_mode_switcher::switch_to_mode_id(
                    "usage", workspace, window, cx,
                ) {
                    workspace.focus_panel::<UsagePanel>(window, cx);
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
        let store = self.store.read(cx);
        let reset_phrase = store
            .usage_meta
            .scraped
            .as_ref()
            .and_then(|scrape| scrape.reset_phrase.clone());
        let pools: Vec<PoolView> = store
            .usage
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
        let face = self.desired_face();
        if self.current_face.as_ref() != Some(&face) {
            // Same-content guard is the PartialEq above — poll ticks that
            // change nothing never run morph theater (web innerHTML check).
            if self.current_face.is_some() {
                self.outgoing_face = self.current_face.take();
                self.face_fade.bump();
            }
            self.current_face = Some(face);
        }
        if !self.face_fade.fresh() {
            self.outgoing_face = None;
        }
        let current = self.current_face.as_ref().expect("set above").clone();
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
        if held != self.was_held {
            self.was_held = held;
            self.tint_fade.bump();
        }
        let (to_bg, to_border) = Self::tint(held);
        let (from_bg, from_border) = Self::tint(!held);

        // ── faces (dual-layer: outgoing overlays + fades out 120ms,
        //    incoming fades in 200ms — both effects-curve, concurrent with
        //    the geometry from frame one) ──
        let weak = cx.weak_entity();
        let on_row = move |pool: SharedString, window: &mut Window, cx: &mut App| {
            weak.update(cx, |this, cx| this.row_clicked(pool, window, cx))
                .ok();
        };
        let face_generation = self.face_fade.generation() as u64;
        let incoming = build_face(&current, on_row, cx);
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
                .child(build_face(face, |_, _, _| {}, cx))
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
            .children(outgoing)
            .child(incoming);

        let geometry_live =
            self.geom_w.animating() || self.geom_h.animating() || self.geom_r.animating();
        if geometry_live || self.tint_fade.fresh() {
            let (w, h, r) = (
                self.geom_w.clone(),
                self.geom_h.clone(),
                self.geom_r.clone(),
            );
            let generation = (self.geom_w.generation() as u64) << 24
                ^ (self.geom_h.generation() as u64) << 16
                ^ (self.geom_r.generation() as u64) << 8
                ^ (self.tint_fade.generation() as u64);
            base.with_animation(
                ElementId::NamedInteger("usage-island-morph".into(), generation),
                // Linear delta in; SPATIAL is mapped inside value_at and the
                // tint inside EFFECTS — concurrent classes, one wrapper
                // (motion.rs GUARD: overshoot never reaches gpui's easing
                // assert).
                Animation::new(MORPH),
                move |container, t| {
                    let tint_t = EFFECTS.eval((t * TINT_RESCALE).min(1.));
                    container
                        .w(px(w.value_at(t)))
                        .h(px(h.value_at(t)))
                        .rounded(px(r.value_at(t).max(0.)))
                        .bg(mix(from_bg, to_bg, tint_t))
                        .border_color(mix(from_border, to_border, tint_t))
                },
            )
            .into_any_element()
        } else {
            base.w(px(self.geom_w.target()))
                .h(px(self.geom_h.target()))
                .rounded(px(self.geom_r.target()))
                .bg(to_bg)
                .border_color(to_border)
                .into_any_element()
        }
    }
}
