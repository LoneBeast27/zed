//! The PERMISSIONS policy cards (design §5.1, split from `view.rs` at the
//! 500-line ceiling — single concern: how the CONFIGURED policy renders):
//! the effective-mode picker (POSTs `/permissions/mode`, session scope with
//! a followed conversation, user scope bare), the per-vendor ENFORCEMENT
//! honesty card (Design B graft — verbatim, amber when `degraded:`/
//! `pending:`), and the read-only rules card with source badges,
//! `config_errors` verbatim in amber, and the jump-to-json buttons.

use gpui::{AnyElement, Context, ElementId, FontWeight, SharedString};
use ui::prelude::*;

use crate::agent_accents::{ACCENT_FILL, STATUS_BLOCKED, accent_for_agent};
use crate::bridge;
use crate::task_board::style::SURFACE_1;

use super::actions::user_permissions_file;
use super::data::{MODES, describe_rule, enforcement_degraded, provenance_line, scope_hint};
use super::{MODE_KEY, PermissionsPanel};

impl PermissionsPanel {
    // ── card 1: effective mode ──

    pub(super) fn render_mode_card(&self, mono: &SharedString, cx: &mut Context<Self>) -> AnyElement {
        let Some(snapshot) = &self.snapshot else {
            return div().into_any_element();
        };
        let colors = cx.theme().colors();
        let demo = bridge::is_agentic_demo();
        let chain = snapshot.mode.clone();
        let conv = self.followed_conv(cx);

        let picker: AnyElement = if self.in_flight.contains(MODE_KEY) {
            self.working_note(mono, cx)
        } else {
            let mut row = h_flex().items_center().flex_wrap().gap(px(6.));
            for (id, label) in MODES {
                let active = chain.effective == id;
                let element_id = ElementId::Name(format!("permissions-mode-{id}").into());
                if active {
                    // The active mode: accent-filled, not clickable (a
                    // re-click of the winning value is never a write).
                    row = row.child(
                        div()
                            .id(element_id)
                            .px(px(9.))
                            .py(px(4.))
                            .rounded(px(8.))
                            .bg(ACCENT_FILL)
                            .text_size(px(12.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(gpui::black())
                            .child(label),
                    );
                } else {
                    row = row.child(self.button(
                        element_id,
                        label.into(),
                        false,
                        !demo,
                        cx,
                        move |this, _, cx| this.set_mode(id, cx),
                    ));
                }
            }
            row.into_any_element()
        };

        let rows: Vec<AnyElement> = vec![
            picker,
            div()
                .mt(px(8.))
                .text_size(px(11.))
                .font_family(mono.clone())
                .text_color(colors.text_muted)
                .child(SharedString::from(provenance_line(&chain)))
                .into_any_element(),
            div()
                .text_size(px(11.))
                .text_color(colors.text_placeholder)
                .child(SharedString::from(scope_hint(conv.as_deref())))
                .into_any_element(),
        ]
        .into_iter()
        .chain(self.note_line(MODE_KEY, mono, cx))
        .collect();
        self.card(
            "Effective mode",
            rows,
            Some(
                "Mode gates what a launched worker may do; rules gate the spawn \
                 itself — two orthogonal axes. auto on a headless claude lane: \
                 non-edit tools that would prompt become tool denials the model \
                 routes around (degrades, never hangs).",
            ),
            cx,
        )
    }

    // ── card 2: per-vendor enforcement honesty (the Design B graft) ──

    pub(super) fn render_enforcement_card(&self, mono: &SharedString, cx: &mut Context<Self>) -> AnyElement {
        let Some(snapshot) = &self.snapshot else {
            return div().into_any_element();
        };
        let colors = cx.theme().colors();
        let mut rows: Vec<AnyElement> = Vec::new();
        for (vendor, text) in snapshot.enforcement_rows() {
            let degraded = enforcement_degraded(&text);
            rows.push(
                h_flex()
                    .w_full()
                    .items_start()
                    .gap(px(10.))
                    .py(px(5.))
                    .child(
                        h_flex()
                            .flex_none()
                            .w(px(76.))
                            .items_center()
                            .gap(px(6.))
                            .child(
                                div()
                                    .size(px(7.))
                                    .rounded_full()
                                    .bg(accent_for_agent(&vendor)),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .font_family(mono.clone())
                                    .text_color(colors.text_muted)
                                    .child(SharedString::from(vendor.clone())),
                            ),
                    )
                    .child(
                        // The honesty string VERBATIM — the trust surface.
                        div()
                            .flex_1()
                            .text_size(px(11.))
                            .line_height(relative(1.5))
                            .font_family(mono.clone())
                            .text_color(if degraded {
                                STATUS_BLOCKED.into()
                            } else {
                                colors.text_muted
                            })
                            .child(SharedString::from(text)),
                    )
                    .into_any_element(),
            );
        }
        if rows.is_empty() {
            rows.push(
                div()
                    .text_size(px(11.))
                    .text_color(cx.theme().colors().text_placeholder)
                    .child("bridge served no enforcement field — older bridge build")
                    .into_any_element(),
            );
        }
        self.card(
            "Enforcement",
            rows,
            Some(
                "What the effective mode ACTUALLY binds on each vendor lane, \
                 self-described by the bridge and rendered verbatim — a lane \
                 that does not enforce says so in amber (the disengaged-probe \
                 law), never a silent lie.",
            ),
            cx,
        )
    }

    // ── card 3: rules + config errors + jump-to-json ──

    pub(super) fn render_rules_card(&self, mono: &SharedString, cx: &mut Context<Self>) -> AnyElement {
        let Some(snapshot) = &self.snapshot else {
            return div().into_any_element();
        };
        let colors = cx.theme().colors();
        let mut rows: Vec<AnyElement> = Vec::new();
        if snapshot.rules.is_empty() {
            rows.push(
                div()
                    .text_size(px(11.))
                    .text_color(colors.text_placeholder)
                    .child(
                        "No rules — every spawn evaluates allow (v1 default-allow; \
                         deny > ask > allow once rules exist).",
                    )
                    .into_any_element(),
            );
        }
        for rule in &snapshot.rules {
            let mut row = v_flex().w_full().py(px(4.)).gap(px(2.)).child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(11.))
                            .font_family(mono.clone())
                            .text_color(colors.text)
                            .child(SharedString::from(describe_rule(rule))),
                    )
                    .child(
                        // The source badge (project | user scope).
                        div()
                            .flex_none()
                            .px(px(7.))
                            .py(px(2.))
                            .rounded(px(6.))
                            .bg(SURFACE_1)
                            .text_size(px(10.))
                            .font_family(mono.clone())
                            .text_color(colors.text_muted)
                            .child(SharedString::from(rule.source.clone())),
                    ),
            );
            if let Some(note) = &rule.note {
                row = row.child(
                    div()
                        .text_size(px(11.))
                        .text_color(colors.text_muted)
                        .child(SharedString::from(note.clone())),
                );
            }
            rows.push(row.into_any_element());
        }
        // Config errors VERBATIM in amber, each with its own jump (the
        // error's `path` is the exact file the engine failed to read).
        for (ix, error) in snapshot.config_errors.iter().enumerate() {
            let path = std::path::PathBuf::from(&error.path);
            rows.push(
                v_flex()
                    .w_full()
                    .py(px(4.))
                    .gap(px(2.))
                    .child(
                        div()
                            .w_full()
                            .text_size(px(11.))
                            .font_family(mono.clone())
                            .text_color(STATUS_BLOCKED)
                            .child(SharedString::from(format!(
                                "{} · {} — {}",
                                error.scope, error.path, error.error
                            ))),
                    )
                    .child(h_flex().child(self.button(
                        ElementId::Name(format!("permissions-error-jump-{ix}").into()),
                        "open file".into(),
                        false,
                        !error.path.is_empty(),
                        cx,
                        move |this, window, cx| this.jump_to_path(path.clone(), window, cx),
                    )))
                    .into_any_element(),
            );
        }
        let user_file = user_permissions_file();
        let user_label = SharedString::from(format!("open {}", user_file.display()));
        rows.push(
            h_flex()
                .mt(px(8.))
                .child(self.button(
                    ElementId::Name("permissions-user-jump".into()),
                    user_label,
                    false,
                    true,
                    cx,
                    move |this, window, cx| this.jump_to_path(user_file.clone(), window, cx),
                ))
                .into_any_element(),
        );
        self.card(
            "Rules",
            rows,
            Some(
                "Read-only — rules live in the JSON files (user scope above; \
                 project scope in <project_root>/.agents/permissions.json). \
                 Edits take effect on the next spawn; no restart needed.",
            ),
            cx,
        )
    }
}
