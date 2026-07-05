//! Workspace-modes rail — the floating dock-pill island (PARITY_SPEC §3).
//!
//! **User override 2026-06-11: the VSCode activity bar is an ANTI-PATTERN**
//! ("read incredibly Windows-like"). This is NOT an edge-flush full-height
//! bar: it renders TWO floating fully-rounded capsules (macOS-dock grouping)
//! inset from the left edge with top/bottom breathing room — the main mode
//! cluster top-aligned and a small bottom cluster (Settings et al., per each
//! mode file's `pinned_position`).
//!
//! Anatomy per §3:
//! - 40px circular hit areas, 20–22px icons, muted inactive color.
//! - Active indicator = ONE sliding tonal squircle (rgba-white fill) that
//!   morphs between items on mode change — a single persistent element
//!   (stable `ElementId`) whose offset is driven by a velocity-carrying
//!   spring (the §4.9 retained-element idiom; it subtly stretches in
//!   transit). NEVER a boxed outline, NEVER a side border-bar.
//! - Hover: gentle scale ~1.06 + brightness fade 150ms ([`StateFades`]).
//!
//! Modes load from `<workspace_root>/.agents/modes/*.json` (M0 loader);
//! clicking an item applies the mode's dock layout via
//! `workspace_mode_switcher::switch_to_mode` (M2). Gated on
//! `agent.workspace_modes` — when false the bar is never instantiated and
//! the workspace render is byte-identical to stock Zed.

use std::path::PathBuf;

use gpui::{AnyElement, Context, Entity, MouseButton, Pixels, SharedString, Window, canvas};
use ui::{Tooltip, prelude::*};
use workspace::Workspace;

use crate::agent_accents::rgba_hex;
use crate::mode_icons::icon_name_for;
use crate::mode_item::ModeSurfaces;
use crate::task_board::motion::{AnimatedValue, CHROME_OMEGA, STATE_FADE, StateFades, mix};
use crate::task_board::style::SURFACE_1;
use crate::workspace_mode_switcher;
use crate::workspace_modes::{PinnedPosition, WorkspaceMode, load_modes_from_dir};

/// Total rail column width: left inset + capsule + right breathing gap.
pub const ACTIVITY_BAR_WIDTH_PX: f32 = PILL_INSET + CAPSULE_WIDTH + PILL_RIGHT_GAP;

/// Island inset from the left edge AND the top/bottom breathing room (§3
/// "inset ~10px from the left edge with top/bottom breathing room").
const PILL_INSET: f32 = 10.0;
/// Gap between the capsule's right edge and the sidebar/center content.
const PILL_RIGHT_GAP: f32 = 8.0;
/// Capsule inner padding around the item column.
const CAPSULE_PAD: f32 = 4.0;
/// The capsule's hairline border width — consumes layout space (border-box),
/// so item positions inside the capsule are offset by it.
const CAPSULE_BORDER: f32 = 1.0;
/// Circular hit area per item (§3 "40px circular hit areas").
const ITEM_SIZE: f32 = 40.0;
/// Vertical gap between items inside a capsule.
const ITEM_GAP: f32 = 4.0;
/// Capsule width = one item + padding.
const CAPSULE_WIDTH: f32 = ITEM_SIZE + 2.0 * CAPSULE_PAD;
/// Resting icon size (§3 "20–22px icons"); hover scales it by ~1.06.
const ICON_PX: f32 = 21.0;
/// Hover magnification delta ("a whisper of dock magnification").
const HOVER_SCALE: f32 = 0.06;
/// Squircle corner radius — rounded enough to read squircle at 40px,
/// visibly NOT a full circle (and NEVER a boxed outline).
const SQUIRCLE_RADIUS: f32 = 13.0;
/// The sliding indicator's tonal rgba-white fill (§3).
const SQUIRCLE_FILL: gpui::Rgba = rgba_hex(0xffffff1a);
/// Transit stretch: px of extra height per (px/s) of spring velocity, capped.
const STRETCH_PER_VELOCITY: f32 = 0.012;
const STRETCH_MAX: f32 = 8.0;

/// Y of item `index` inside the TOP capsule, in bar coordinates.
fn top_item_y(index: usize) -> f32 {
    PILL_INSET + CAPSULE_BORDER + CAPSULE_PAD + index as f32 * (ITEM_SIZE + ITEM_GAP)
}

/// The vertical PITCH between consecutive item hit-rects inside a capsule —
/// one hit area plus the inter-item gap (Finding 3(c)). The hit-rect HEIGHT is
/// [`ITEM_SIZE`]; the `ITEM_GAP` between them is dead space where a synthetic
/// click can fall through (the taskboard-miss hypothesis). Pinned by a test so
/// a future padding/gap tweak can't silently shrink the targets or widen the
/// dead band.
const fn item_hit_pitch() -> f32 {
    ITEM_SIZE + ITEM_GAP
}

/// The clickable hit-rect for top-cluster item `index`: `(top_y, height)` in
/// bar coordinates. The WHOLE 40px circle is the target (the item div carries
/// `on_mouse_down`, not the glyph), so a click anywhere in this band switches
/// the mode — confirmed by [`ActivityBar::render_mode_item`] sizing the
/// listener div to `ITEM_SIZE`, not the icon.
fn top_item_hit_rect(index: usize) -> (f32, f32) {
    (top_item_y(index), ITEM_SIZE)
}

/// Y of item `index` inside the BOTTOM capsule (anchored to the bar's foot),
/// in bar coordinates. Needs the measured bar height.
fn bottom_item_y(bar_height: f32, bottom_count: usize, index: usize) -> f32 {
    let capsule_h = 2.0 * (CAPSULE_PAD + CAPSULE_BORDER)
        + bottom_count as f32 * ITEM_SIZE
        + bottom_count.saturating_sub(1) as f32 * ITEM_GAP;
    bar_height - PILL_INSET - capsule_h
        + CAPSULE_BORDER
        + CAPSULE_PAD
        + index as f32 * (ITEM_SIZE + ITEM_GAP)
}

/// Entity backing the dock-pill rail.
pub struct ActivityBar {
    /// All modes loaded from disk at construction. Order is the loader's
    /// canonical sort (Top-pinned, then alphabetical, then Bottom-pinned).
    modes: Vec<WorkspaceMode>,
    /// `id` of the currently active mode. May be empty when no mode matches
    /// the configured default — in that case the first non-bottom-pinned
    /// mode is highlighted as a soft fallback.
    active_mode_id: String,
    /// Weak handle to the hosting workspace, used by the M2 switcher to apply
    /// a mode's dock layout on click. `None` in unit tests that exercise the
    /// bar without a workspace — clicks then only update the highlight.
    workspace: Option<gpui::WeakEntity<Workspace>>,
    /// Per-item hover crossfades (brightness + scale), 150ms effects.
    hover_fades: StateFades,
    /// The sliding squircle's Y — a velocity-carrying spring so mode-change
    /// morphs are continuous even when interrupted mid-flight.
    squircle_y: AnimatedValue,
    /// False until the squircle has been jumped to its first position (the
    /// first placement must not slide in from 0).
    squircle_placed: bool,
    /// Bar height measured each frame by a layout canvas (prepaint, no
    /// notify) — bottom-capsule item positions resolve against last frame's
    /// height, the drawer's width-measure idiom.
    bar_height: Option<Pixels>,
    /// The six center-surface entities (Amendment 2026-07-04 (2)) — the bar
    /// is the mode system's per-workspace anchor, so the switcher and the
    /// islands reach the surfaces through it
    /// ([`crate::mode_item::workspace_surfaces`]). `None` until the
    /// workspace-modes init installs them (and in bar-only unit tests).
    surfaces: Option<ModeSurfaces>,
}

impl ActivityBar {
    /// Construct an `ActivityBar` Entity anchored at `modes_dir`. `default_mode`
    /// selects which icon renders as active on first paint. The constructor
    /// reads the filesystem once — hot-reload on file change is M-future.
    pub fn new(
        modes_dir: PathBuf,
        default_mode: impl Into<String>,
        workspace: Option<gpui::WeakEntity<Workspace>>,
        _cx: &mut Context<Self>,
    ) -> Self {
        let modes = load_modes_from_dir(&modes_dir);
        Self {
            modes,
            active_mode_id: default_mode.into(),
            workspace,
            hover_fades: StateFades::new(),
            squircle_y: AnimatedValue::spring(0.0, CHROME_OMEGA),
            squircle_placed: false,
            bar_height: None,
            surfaces: None,
        }
    }

    pub fn modes(&self) -> &[WorkspaceMode] {
        &self.modes
    }

    /// Install the center-surface registry (workspace-modes init).
    pub fn set_surfaces(&mut self, surfaces: ModeSurfaces) {
        self.surfaces = Some(surfaces);
    }

    pub fn surfaces(&self) -> Option<&ModeSurfaces> {
        self.surfaces.as_ref()
    }

    pub fn active_mode_id(&self) -> &str {
        &self.active_mode_id
    }

    /// Set the active mode id and request a redraw — the render pass
    /// retargets the squircle spring toward the new item.
    pub fn set_active(&mut self, mode_id: impl Into<String>, cx: &mut Context<Self>) {
        let new_id = mode_id.into();
        if new_id != self.active_mode_id {
            self.active_mode_id = new_id;
            cx.notify();
        }
    }

    /// Returns the effective active mode id — the explicitly-set id if it
    /// matches a loaded mode, otherwise the first non-`Bottom`-pinned mode.
    fn effective_active(&self) -> Option<&WorkspaceMode> {
        if let Some(m) = self.modes.iter().find(|m| m.id == self.active_mode_id) {
            return Some(m);
        }
        self.modes
            .iter()
            .find(|m| m.pinned_position != Some(PinnedPosition::Bottom))
    }

    /// The squircle's target Y for the active item, if resolvable this frame
    /// (bottom-cluster items need the measured bar height).
    fn squircle_target(
        &self,
        top: &[&WorkspaceMode],
        bottom: &[&WorkspaceMode],
        active_id: &str,
    ) -> Option<f32> {
        if let Some(ix) = top.iter().position(|m| m.id == active_id) {
            return Some(top_item_y(ix));
        }
        let ix = bottom.iter().position(|m| m.id == active_id)?;
        let height = self.bar_height?.as_f32();
        Some(bottom_item_y(height, bottom.len(), ix))
    }

    /// One 40px circular item: icon at hover-scaled size + brightness fade.
    /// The squircle (not the item) carries the active treatment — the item
    /// itself never gets a box, tile, or border.
    fn render_mode_item(&self, mode: &WorkspaceMode, is_active: bool, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors();
        let element_id = ElementId::Name(SharedString::from(format!("dock-pill-{}", mode.id)));
        let hover_t = self.hover_fades.t(&element_id);

        let icon = icon_name_for(&mode.icon);
        let icon_px = ICON_PX * (1.0 + HOVER_SCALE * hover_t);
        // Active icon at full brightness; inactive muted, fading brighter on
        // hover (never all the way to the active white).
        let icon_color = if is_active {
            colors.text
        } else {
            mix(colors.text_muted, colors.text, 0.6 * hover_t)
        };

        let tooltip_text = match &mode.default_keybinding {
            Some(kb) => format!("{} ({})", mode.display_name, kb),
            None => mode.display_name.clone(),
        };
        let mode_id_for_click = mode.id.clone();
        let hover_key = element_id.clone();

        div()
            .id(element_id)
            .size(px(ITEM_SIZE))
            .rounded_full()
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if this.hover_fades.set(hover_key.clone(), *hovered, STATE_FADE) {
                    cx.notify();
                }
            }))
            .child(
                Icon::new(icon)
                    .size(IconSize::Custom(rems_from_px(icon_px)))
                    .color(Color::Custom(icon_color)),
            )
            // Finding 3(b): a NON-hoverable tooltip (`Tooltip::simple` via
            // `.tooltip`, `hoverable: false`). gpui non-hoverable tooltips are a
            // passive deferred overlay that dismisses on mouse-move and never
            // occludes the source element's hitbox, so a tooltip (this item's or
            // a neighbor's) cannot eat the taskboard click. If this were ever
            // switched to `.hoverable_tooltip`, the hoverable overlay CAN sit
            // over a sibling and intercept — keep it simple() here, or move the
            // tooltip anchor off the rail.
            .tooltip(move |_window, cx| Tooltip::simple(tooltip_text.clone(), cx))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _ev, window, cx| {
                    // Finding 3(a): log the mode id at the TOP of the listener,
                    // BEFORE any logic, so an eval can tell a MISSED click (this
                    // line never prints — the event never reached the target) from
                    // an EATEN click (this prints but the switch didn't happen)
                    // instantly. info!, not debug!, so it survives the default
                    // log filter during a live eval.
                    log::info!("activity_bar: rail click → mode '{mode_id_for_click}'");
                    this.set_active(mode_id_for_click.clone(), cx);
                    let Some(mode) = this
                        .modes
                        .iter()
                        .find(|m| m.id == mode_id_for_click)
                        .cloned()
                    else {
                        return;
                    };
                    let Some(workspace) = this.workspace.as_ref().and_then(|w| w.upgrade())
                    else {
                        log::warn!(
                            "activity_bar: no workspace handle — cannot apply mode '{}'",
                            mode.id
                        );
                        return;
                    };
                    // DEFERRED: this listener runs inside the ActivityBar's
                    // own update, and switch_to_mode reads the bar (surfaces
                    // registry lives on it) — synchronous dispatch panics
                    // with a re-entrant read in a real window (symphony
                    // click crash 2026-07-04; same class as the t9 launch
                    // crash). Aim the squircle immediately (set_active
                    // above), apply the layout one cycle later.
                    window.defer(cx, move |window, cx| {
                        workspace.update(cx, |workspace, cx| {
                            workspace_mode_switcher::switch_to_mode(&mode, workspace, window, cx);
                        });
                    });
                }),
            )
            .into_any_element()
    }

    /// One floating capsule: fully-rounded, surface-1-class fill, hairline
    /// border (macOS-dock pill grouping).
    fn capsule(children: Vec<AnyElement>, border: gpui::Hsla) -> Div {
        v_flex()
            .flex_none()
            .p(px(CAPSULE_PAD))
            .gap(px(ITEM_GAP))
            .rounded_full()
            .bg(SURFACE_1)
            .border_1()
            .border_color(border)
            .children(children)
    }
}

impl gpui::Render for ActivityBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors().clone();

        // Split modes into the main cluster and the bottom-pinned cluster
        // (per each mode file's `pinned_position` — Settings pins bottom).
        let (bottom, top): (Vec<&WorkspaceMode>, Vec<&WorkspaceMode>) = self
            .modes
            .iter()
            .partition(|m| m.pinned_position == Some(PinnedPosition::Bottom));

        let active_id = self
            .effective_active()
            .map(|m| m.id.clone())
            .unwrap_or_default();

        // Aim the squircle spring at the active item. First placement jumps
        // (no slide-in from nowhere); afterwards retargets are continuous
        // mid-flight (same-target retargets no-op).
        if let Some(target) = self.squircle_target(&top, &bottom, &active_id) {
            if self.squircle_placed {
                self.squircle_y.retarget(target);
            } else {
                self.squircle_y.jump(target);
                self.squircle_placed = true;
            }
        }

        let top_children: Vec<AnyElement> = top
            .clone()
            .into_iter()
            .map(|mode| self.render_mode_item(mode, mode.id == active_id, cx))
            .collect();
        let bottom_children: Vec<AnyElement> = bottom
            .clone()
            .into_iter()
            .map(|mode| self.render_mode_item(mode, mode.id == active_id, cx))
            .collect();

        // Measure the bar each frame (prepaint, no notify) — bottom-capsule
        // squircle positions read last frame's height.
        let measure = {
            let bar = cx.weak_entity();
            canvas(
                move |bounds, _, cx| {
                    bar.update(cx, |this, _| this.bar_height = Some(bounds.size.height))
                        .ok();
                },
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0()
        };

        // The single sliding tonal squircle — one persistent element (stable
        // id) travelling behind the icons, subtly stretching with velocity.
        let squircle = self.squircle_placed.then(|| {
            let y = self.squircle_y.current();
            let stretch = (self.squircle_y.velocity().abs() * STRETCH_PER_VELOCITY)
                .min(STRETCH_MAX);
            div()
                .id("dock-pill-squircle")
                .absolute()
                .left(px(PILL_INSET + CAPSULE_BORDER + CAPSULE_PAD))
                .top(px(y - stretch / 2.0))
                .w(px(ITEM_SIZE))
                .h(px(ITEM_SIZE + stretch))
                .rounded(px(SQUIRCLE_RADIUS))
                .bg(SQUIRCLE_FILL)
        });

        // Frame pump: while the squircle flies or a hover fades — plus one
        // bootstrap frame when a bottom-pinned active item waits on the
        // first height measurement.
        if self.squircle_y.animating()
            || self.hover_fades.any_animating()
            || (!self.squircle_placed && self.bar_height.is_none() && !self.modes.is_empty())
        {
            window.request_animation_frame();
        }

        div()
            .id("activity-bar")
            .relative()
            .w(px(ACTIVITY_BAR_WIDTH_PX))
            .h_full()
            .flex_none()
            .bg(colors.background)
            // Right border so the rail reads as its own extreme-left column
            // (leftmost in the workspace flex — workspace.rs) rather than
            // blending into the left dock beside it.
            .border_r_1()
            .border_color(colors.border)
            .child(measure)
            .children(squircle)
            .child(
                v_flex()
                    .size_full()
                    .pl(px(PILL_INSET))
                    .pr(px(PILL_RIGHT_GAP))
                    .py(px(PILL_INSET))
                    .items_start()
                    .when(!top_children.is_empty(), |bar| {
                        bar.child(Self::capsule(top_children, colors.border))
                    })
                    .child(div().flex_1())
                    .when(!bottom_children.is_empty(), |bar| {
                        bar.child(Self::capsule(bottom_children, colors.border))
                    }),
            )
    }
}

/// Convenience constructor mirroring `resource_banner::build_gpu_banner` —
/// builds an `ActivityBar` Entity anchored at `<workspace_root>/.agents/modes/`
/// unless `modes_dir_override` is supplied (settings escape hatch).
pub fn build_activity_bar(
    workspace_root: PathBuf,
    modes_dir_override: Option<PathBuf>,
    default_mode: impl Into<String>,
    workspace: gpui::WeakEntity<Workspace>,
    cx: &mut gpui::App,
) -> Entity<ActivityBar> {
    let modes_dir = modes_dir_override.unwrap_or_else(|| workspace_root.join(".agents/modes"));
    let default_mode = default_mode.into();
    cx.new(|cx| ActivityBar::new(modes_dir, default_mode, Some(workspace), cx))
}

#[cfg(test)]
#[path = "activity_bar_tests.rs"]
mod tests;
