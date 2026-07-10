//! The WRITEBACK conflict modal — both sides of a drifted note (current
//! hand-edit vs incoming vendor copy, read from the local vault) over the
//! EXPLICIT resolutions. CONFLICT LAW: these buttons are the ONLY path to
//! `keep-mine` / `take-theirs` — a conflict is never accepted bare, never
//! silently overwritten. Split from `writeback_view.rs` for the 500-line
//! ceiling (single concern: the modal).

use gpui::{AnyElement, Context, Div, ElementId, FontWeight, SharedString};
use settings::Settings as _;
use theme_settings::ThemeSettings;
use ui::prelude::*;

use crate::agent_accents::{STATUS_BLOCKED, STATUS_ERROR};

use super::panel::VaultBrowserPanel;
use super::style::SURFACE_1;
use super::writeback::Resolution;
use super::writeback_state::{ConflictModal, SideLoad};
use super::writeback_view::{saving_note, wb_button};

impl VaultBrowserPanel {
    /// The conflict modal: both sides (current hand-edit vs incoming vendor
    /// copy, read from the local vault) over the EXPLICIT resolutions. The
    /// scrim occludes the list; Cancel leaves the candidate pending.
    pub(super) fn conflict_modal(
        &self,
        modal: &ConflictModal,
        connected: bool,
        cx: &Context<Self>,
    ) -> Div {
        let colors = cx.theme().colors();
        let mono = ThemeSettings::get_global(cx).buffer_font.family.clone();
        let in_flight = self.writeback.in_flight.contains(&modal.id);

        let mut card = v_flex()
            .id("wb-conflict-card")
            .w_full()
            .max_h_full()
            .overflow_y_scroll()
            .gap(px(10.))
            .p(px(14.))
            .rounded(px(12.))
            .bg(colors.elevated_surface_background)
            .border_1()
            .border_color(colors.border)
            .occlude()
            .child(
                v_flex()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(px(14.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(colors.text)
                            .child("Resolve conflict"),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(colors.text_muted)
                            .child(SharedString::from(format!(
                                "{} — {}",
                                modal.title, modal.reason
                            ))),
                    ),
            )
            .child(side_pane(
                "Current — your edit",
                &modal.path,
                &modal.current,
                &mono,
                cx,
            ))
            .child(side_pane(
                "Incoming — vendor update",
                &modal.incoming,
                &modal.candidate,
                &mono,
                cx,
            ))
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(colors.text_placeholder)
                    .child(
                        "Keep current marks the incoming version as reviewed; overwrite \
                         replaces the note and preserves your edit as a .mine copy. \
                         Neither destroys the other side.",
                    ),
            );

        let actions: AnyElement = if in_flight {
            saving_note(&mono, cx).into_any_element()
        } else {
            let keep_id = modal.id.clone();
            let overwrite_id = modal.id.clone();
            h_flex()
                .items_center()
                .flex_wrap()
                .gap(px(6.))
                .child(wb_button(
                    ElementId::Name("wb-conflict-keep".into()),
                    "Keep current".into(),
                    false,
                    connected && modal.keep_current,
                    cx,
                    move |this, cx| {
                        this.resolve_conflict(keep_id.clone(), Resolution::KeepCurrent, cx)
                    },
                ))
                .child(wb_button(
                    ElementId::Name("wb-conflict-overwrite".into()),
                    "Overwrite with incoming".into(),
                    true,
                    connected && modal.overwrite,
                    cx,
                    move |this, cx| {
                        this.resolve_conflict(overwrite_id.clone(), Resolution::Overwrite, cx)
                    },
                ))
                .child(div().flex_1())
                .child(wb_button(
                    ElementId::Name("wb-conflict-cancel".into()),
                    "Cancel".into(),
                    false,
                    true,
                    cx,
                    move |this, cx| this.close_conflict_modal(cx),
                ))
                .into_any_element()
        };
        card = card.child(actions);

        if !connected {
            card = card.child(
                div()
                    .text_size(px(11.))
                    .text_color(STATUS_BLOCKED)
                    .child("bridge offline — resolutions post to the bridge; reconnect to resolve"),
            );
        }
        if let Some(error) = &modal.error {
            card = card.child(
                div()
                    .text_size(px(11.))
                    .text_color(STATUS_ERROR)
                    .child(SharedString::from(error.clone())),
            );
        }

        // The scrim — absolute over the list, centring the card.
        div()
            .absolute()
            .inset_0()
            .occlude()
            .bg(gpui::black().opacity(0.55))
            .flex()
            .items_center()
            .justify_center()
            .p(px(10.))
            .child(card)
    }
}

/// One side of the modal: label · vault-relative path · body preview (or
/// the honest loading / could-not-read note).
fn side_pane(
    label: &'static str,
    rel_path: &str,
    load: &SideLoad,
    mono: &SharedString,
    cx: &Context<VaultBrowserPanel>,
) -> Div {
    let colors = cx.theme().colors();
    let body: AnyElement = match load {
        SideLoad::Loading => div()
            .text_size(px(11.))
            .italic()
            .text_color(colors.text_placeholder)
            .child("reading…")
            .into_any_element(),
        SideLoad::Loaded(preview) => div()
            .w_full()
            .text_size(px(11.))
            .font_family(mono.clone())
            .text_color(colors.text_muted)
            .child(SharedString::from(preview.clone()))
            .into_any_element(),
        SideLoad::Failed(message) => div()
            .text_size(px(11.))
            .italic()
            .text_color(STATUS_BLOCKED)
            .child(SharedString::from(message.clone()))
            .into_any_element(),
    };
    v_flex()
        .w_full()
        .gap(px(4.))
        .child(
            div()
                .text_size(px(11.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(colors.text_muted)
                .child(label),
        )
        .child(
            div()
                .w_full()
                .text_size(px(10.))
                .font_family(mono.clone())
                .text_color(colors.text_placeholder)
                .truncate()
                .child(SharedString::from(rel_path.to_string())),
        )
        .child(
            div()
                .w_full()
                .p(px(8.))
                .rounded(px(8.))
                .bg(SURFACE_1)
                .child(body),
        )
}
