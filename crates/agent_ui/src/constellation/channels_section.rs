//! The Channels observe surface (TEAMS §4/T2) — a section in the
//! constellation's below-graph detail area showing what `GET /channels`
//! serves: grant rows (state · budget k · participants), the Tier-0 receipt
//! feed, and T3 overlap venn scores. Pure read of the frame's already-
//! gathered snapshot — no entity access, no bridge I/O of its own (the
//! supplemental poll in feed.rs feeds it), so there is zero §11 re-entrancy
//! surface here.
//!
//! Grants are observe-only for now: the "Grant channel" affordance renders
//! DISABLED with an honest tooltip — the bridge has no POST grant route yet
//! (channel grants land with T2). Never-silently-absent: the button exists
//! and says why it is inert.
//!
//! All three lists are unbounded on the wire, so ONE `uniform_list`
//! virtualizes a flattened row vec (section headers + data rows), the vault
//! list idiom — rows stay fixed-height, render cost is bounded by the
//! viewport.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui::{AnyElement, Context, FontWeight, Rgba, SharedString, uniform_list};
use ui::prelude::*;
use ui::{Button, LabelSize, Tooltip};

use crate::agent_accents::{STATUS_DONE, STATUS_ERROR, rgba_hex};
use crate::bridge::{ChannelEventRow, ChannelRow, OverlapRow};

use super::edges::is_terminal;
use super::panel::ConstellationPanel;

/// The section's panel-side state plumbing (fields live on the panel; the
/// behavior lives with the surface that owns it).
impl ConstellationPanel {
    /// Latest overlap venn rows from the feed — arrange weights (sim) + the
    /// channels section's display copy. Change-gated notify: an unchanged
    /// poll frame costs nothing.
    pub(super) fn set_overlap(&mut self, rows: Vec<OverlapRow>, cx: &mut Context<Self>) {
        if self.overlap_rows != rows {
            self.overlap_rows = rows.clone();
            cx.notify();
        }
        self.sim.set_overlap(rows);
    }

    /// Latest Tier-0 channel receipts from the feed (display only) —
    /// change-gated like every other poll fold.
    pub(super) fn set_channel_events(
        &mut self,
        events: Vec<ChannelEventRow>,
        cx: &mut Context<Self>,
    ) {
        if self.channel_events != events {
            self.channel_events = events;
            cx.notify();
        }
    }

    /// Header toggle for the channels observe surface.
    pub(super) fn toggle_channels(&mut self, cx: &mut Context<Self>) {
        self.channels_shown = !self.channels_shown;
        cx.notify();
    }
}

/// The boost-channel violet (draw.rs `CHAN_DOT` twin — same grammar as the
/// edge the graph paints for the same grant).
const CHAN_VIOLET: Rgba = rgba_hex(0xa78bfaff);
/// Fixed row height for EVERY row — `uniform_list` measures one item and
/// gives all items that slot (vault-list lesson 2026-07-07: mixed heights
/// silently overlap).
const ROW_HEIGHT: f32 = 40.;
/// Receipt rows rendered per frame (the wire caps at ~400; the flattened vec
/// is rebuilt every panel frame, so the display slice stays small). The trim
/// is announced by a trailing row, never silent.
const EVENTS_SHOWN: usize = 40;

/// A flattened list row.
enum Row {
    Section(SharedString),
    Channel(ChannelRow),
    Event(ChannelEventRow),
    Overlap(OverlapRow),
    /// An honest muted line (per-section empty, or the trim notice).
    Note(SharedString),
}

/// Build the section: header (title · counts · disabled Grant affordance) +
/// the virtualized flattened list. `connected == false` degrades honestly —
/// a warning banner over the last snapshot (the store keeps it).
pub(super) fn channels_section(
    channels: &[ChannelRow],
    events: &[ChannelEventRow],
    overlap: &[OverlapRow],
    connected: bool,
    cx: &App,
) -> AnyElement {
    let colors = cx.theme().colors();

    let open = channels.iter().filter(|c| !is_terminal(&c.state)).count();
    let terminal = channels.len() - open;
    let counts: SharedString = if channels.is_empty() && events.is_empty() {
        "no grants yet".into()
    } else {
        let mut s = format!("{open} open");
        if terminal > 0 {
            s.push_str(&format!(" · {terminal} terminal"));
        }
        if !events.is_empty() {
            s.push_str(&format!(" · {} receipt{}", events.len(), plural(events.len())));
        }
        s.into()
    };

    let header = h_flex()
        .flex_none()
        .items_center()
        .gap(px(10.))
        .px(px(16.))
        .py(px(8.))
        .border_b_1()
        .border_color(colors.border)
        .child(
            div()
                .text_size(px(13.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(colors.text)
                .child("Channels"),
        )
        .child(
            div()
                .text_size(px(11.))
                .text_color(colors.text_placeholder)
                .child(counts),
        )
        .child(div().flex_1())
        .child(
            // Observe-only for now: DISABLED, with the honest why. There is
            // no POST grant route on the bridge yet — do not invent one.
            Button::new("channels-grant", "Grant channel")
                .label_size(LabelSize::Small)
                .disabled(true)
                .tooltip(Tooltip::text(
                    "channel grants land with T2 — no server route yet",
                )),
        );

    let body: AnyElement = if channels.is_empty() && events.is_empty() && overlap.is_empty() {
        // The all-empty state IS the normal state on a quiet bridge: say so
        // instead of rendering a blank pane.
        let copy: SharedString = if connected {
            "Boost channels appear here when the heartbeat grants a pairwise relay between sibling runs (T2).".into()
        } else {
            "The bridge is unreachable — nothing observed. Channels appear here once it is back.".into()
        };
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .text_center()
            .gap(px(4.))
            .p(px(16.))
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(colors.text_muted)
                    .child("No boost channels yet"),
            )
            .child(
                div()
                    .max_w(px(340.))
                    .text_size(px(11.))
                    .text_color(colors.text_placeholder)
                    .child(copy),
            )
            .into_any_element()
    } else {
        let rows = Arc::new(flatten(channels, events, overlap));
        uniform_list("constellation-channels", rows.len(), move |range, _window, cx| {
            range
                .map(|ix| match &rows[ix] {
                    Row::Section(label) => section_row(label.clone(), cx),
                    Row::Channel(ch) => channel_row(ch, cx),
                    Row::Event(ev) => event_row(ev, cx),
                    Row::Overlap(ov) => overlap_row(ov, cx),
                    Row::Note(text) => note_row(text.clone(), cx),
                })
                .collect()
        })
        .size_full()
        .px(px(16.))
        .pb(px(12.))
        .into_any_element()
    };

    v_flex()
        .flex_none()
        .h(relative(0.32))
        .min_h(px(140.))
        .border_t_1()
        .border_color(colors.border)
        .bg(colors.panel_background)
        .child(header)
        // Bridge-down banner: the panel already greys out; this names it so
        // the snapshot below is never mistaken for live data.
        .when(!connected, |section| {
            section.child(
                div()
                    .flex_none()
                    .px(px(16.))
                    .py(px(4.))
                    .text_size(px(11.))
                    .text_color(cx.theme().status().warning)
                    .child("bridge offline — showing the last snapshot"),
            )
        })
        .child(div().flex_1().min_h_0().child(body))
        .into_any_element()
}

/// Flatten the three feeds into one row vec: grants (open first), the
/// receipt feed newest-first (display-capped, trim announced), and overlap
/// only when non-empty (no data is not a verdict — omit, don't fake).
fn flatten(
    channels: &[ChannelRow],
    events: &[ChannelEventRow],
    overlap: &[OverlapRow],
) -> Vec<Row> {
    let shown = events.len().min(EVENTS_SHOWN);
    let mut rows = Vec::with_capacity(channels.len() + shown + overlap.len() + 5);

    rows.push(Row::Section(
        format!("grants ({})", channels.len()).into(),
    ));
    if channels.is_empty() {
        rows.push(Row::Note("no live or terminal grants".into()));
    } else {
        for ch in channels.iter().filter(|c| !is_terminal(&c.state)) {
            rows.push(Row::Channel(ch.clone()));
        }
        for ch in channels.iter().filter(|c| is_terminal(&c.state)) {
            rows.push(Row::Channel(ch.clone()));
        }
    }

    rows.push(Row::Section(
        format!("recent receipts ({})", events.len()).into(),
    ));
    if events.is_empty() {
        rows.push(Row::Note("no receipts yet".into()));
    } else {
        // Wire order is newest LAST; the feed reads newest first.
        for ev in events.iter().rev().take(EVENTS_SHOWN) {
            rows.push(Row::Event(ev.clone()));
        }
        if events.len() > EVENTS_SHOWN {
            rows.push(Row::Note(
                format!("… showing {EVENTS_SHOWN} of {}", events.len()).into(),
            ));
        }
    }

    if !overlap.is_empty() {
        rows.push(Row::Section(format!("overlap ({})", overlap.len()).into()));
        for ov in overlap {
            rows.push(Row::Overlap(ov.clone()));
        }
    }
    rows
}

/// A section header — bottom-anchored in the uniform slot (reads as section
/// spacing, the vault-list idiom).
fn section_row(label: SharedString, cx: &mut App) -> AnyElement {
    let colors = cx.theme().colors();
    h_flex()
        .h(px(ROW_HEIGHT))
        .w_full()
        .items_end()
        .pb(px(6.))
        .child(
            div()
                .text_size(px(11.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(colors.text_placeholder)
                .child(label),
        )
        .into_any_element()
}

/// One grant: dot (channel violet, dimmed when held/terminal) · a ↔ b ·
/// state, over channel_id · budget k/max · tokens · close reason.
fn channel_row(ch: &ChannelRow, cx: &mut App) -> AnyElement {
    let colors = cx.theme().colors();
    let terminal = is_terminal(&ch.state);
    let dot = if terminal {
        match ch.state.as_str() {
            "converged" => STATUS_DONE,
            _ => STATUS_ERROR,
        }
    } else if ch.state == "working" {
        CHAN_VIOLET
    } else {
        // awaiting_* — the held glow, dimmed.
        rgba_hex(0xa78bfa80)
    };

    let pair: SharedString = format!("{} ↔ {}", ch.a, ch.b).into();
    let mut meta = format!("{} · {}/{} batches", ch.channel_id, ch.k, ch.max_batches);
    if ch.token_budget > 0 {
        meta.push_str(&format!(" · {}/{} tok", ch.tokens_spent, ch.token_budget));
    }
    if terminal && !ch.close_reason.is_empty() {
        meta.push_str(&format!(" · {}", ch.close_reason));
    } else if let Some(reason) = ch.reason.as_deref().filter(|r| !r.is_empty()) {
        meta.push_str(&format!(" · {reason}"));
    }

    h_flex()
        .h(px(ROW_HEIGHT))
        .w_full()
        .items_center()
        .gap(px(8.))
        .border_b_1()
        .border_color(colors.border_variant)
        .child(div().flex_none().size(px(7.)).rounded_full().bg(dot))
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap(px(1.))
                .child(
                    h_flex()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(px(12.))
                                .text_color(colors.text)
                                .child(pair),
                        )
                        .child(
                            div()
                                .flex_none()
                                .text_size(px(11.))
                                .text_color(if terminal {
                                    colors.text_placeholder
                                } else {
                                    colors.text_muted
                                })
                                .child(SharedString::from(ch.state.clone())),
                        ),
                )
                .child(
                    div()
                        .truncate()
                        .text_size(px(10.))
                        .text_color(colors.text_placeholder)
                        .child(SharedString::from(meta)),
                ),
        )
        .into_any_element()
}

/// One Tier-0 receipt, single compact line: age · kind · channel · note.
fn event_row(ev: &ChannelEventRow, cx: &mut App) -> AnyElement {
    let colors = cx.theme().colors();
    let kind_color = match ev.kind.as_str() {
        "open" => CHAN_VIOLET,
        "close" if ev.state == "converged" => STATUS_DONE,
        "close" => STATUS_ERROR,
        _ => colors.text_muted.into(),
    };
    let what: SharedString = if ev.frm.is_empty() {
        ev.channel_id.clone().into()
    } else {
        format!("{} ← {}", ev.channel_id, ev.frm).into()
    };
    h_flex()
        .h(px(ROW_HEIGHT))
        .w_full()
        .items_center()
        .gap(px(8.))
        .child(
            div()
                .flex_none()
                .w(px(30.))
                .text_size(px(10.))
                .text_color(colors.text_placeholder)
                .child(SharedString::from(age(ev.ts))),
        )
        .child(
            div()
                .flex_none()
                .text_size(px(11.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(kind_color)
                .child(SharedString::from(ev.kind.clone())),
        )
        .child(
            div()
                .flex_none()
                .text_size(px(11.))
                .text_color(colors.text_muted)
                .child(what),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(px(11.))
                .text_color(colors.text_placeholder)
                .child(SharedString::from(ev.note.clone())),
        )
        .into_any_element()
}

/// One T3 venn row: a ∩ b · score.
fn overlap_row(ov: &OverlapRow, cx: &mut App) -> AnyElement {
    let colors = cx.theme().colors();
    h_flex()
        .h(px(ROW_HEIGHT))
        .w_full()
        .items_center()
        .gap(px(8.))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(px(11.))
                .text_color(colors.text_muted)
                .child(SharedString::from(format!("{} ∩ {}", ov.a, ov.b))),
        )
        .child(
            div()
                .flex_none()
                .text_size(px(11.))
                .text_color(colors.text)
                .child(SharedString::from(format!("{:.0}", ov.score))),
        )
        .into_any_element()
}

/// A muted single-line note (per-section empty state / trim notice).
fn note_row(text: SharedString, cx: &mut App) -> AnyElement {
    let colors = cx.theme().colors();
    h_flex()
        .h(px(ROW_HEIGHT))
        .w_full()
        .items_center()
        .child(
            div()
                .text_size(px(11.))
                .text_color(colors.text_placeholder)
                .child(text),
        )
        .into_any_element()
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// Receipt age from a unix-seconds stamp — coarse on purpose (the feed
/// refreshes every 2s poll; sub-second precision would be a lie).
fn age(ts: f64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(ts);
    let dt = (now - ts).max(0.);
    if dt < 60. {
        format!("{dt:.0}s")
    } else if dt < 3600. {
        format!("{:.0}m", dt / 60.)
    } else {
        format!("{:.0}h", dt / 3600.)
    }
}
