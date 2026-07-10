//! The two-layer composer deck (PARITY_SPEC §4.1, the Antigravity
//! signature — `chat.css .composer`): auto-height editor over the controls
//! row ([+ ghost] [brain pill] [spacer] [send circle]) over the darker
//! `--surface-2b` hint footer. The send⇄stop island morph lives in
//! [`super::send_circle`].
//!
//! Composer idiom (RUST_PORT_NOTES §4.1): auto-height `Editor` entity
//! (message_editor pattern), Enter sends / Shift+Enter newlines via the
//! `OrchestratorComposer > Editor` keymap binding to [`Send`]
//! (`assets/keymaps/workspace_modes.json` — loaded only with the
//! workspace-modes flag, like the panel itself).

use std::cell::Cell;
use std::rc::Rc;

use editor::{Addon, Editor, EditorElement, EditorEvent, EditorStyle};
use gpui::{
    AnyElement, App, AsyncApp, Entity, Focusable as _, KeyContext, MouseButton, SharedString,
    Subscription, Task, TextStyle, Window,
};
use settings::Settings as _;
use theme_settings::ThemeSettings;
use ui::prelude::*;

use crate::agent_accents::{ACCENT_AGY, ACCENT_CLAUDE, ACCENT_CODEX, ACCENT_GEMINI};
use crate::bridge::{BRIDGE_BASE_URL, post_json};
use crate::commands::Vendor;
use crate::task_board::motion::{STATE_FADE, mix};
use crate::task_board::style::{HAIRLINE_HI, SURFACE_2, SURFACE_2B};

use super::panel::{OrchestratorPanel, Send};
use super::send_circle::SendCircle;
use super::typeahead::forced_vendor;

/// The vendor's identity accent (3-provider ruling 2026-07-08) — the color
/// the composer adopts when a task targets that vendor.
fn vendor_accent(vendor: Vendor) -> gpui::Hsla {
    match vendor {
        Vendor::Claude => ACCENT_CLAUDE.into(),
        Vendor::Codex => ACCENT_CODEX.into(),
        Vendor::Agy => ACCENT_AGY.into(),
        Vendor::Gemini => ACCENT_GEMINI.into(),
    }
}

/// The usage strip's horizontal-extend duration — a spatial reveal (~500px
/// of detail), so longer than the 150ms effects fades.
const USAGE_STRIP_MORPH: std::time::Duration = std::time::Duration::from_millis(260);

/// The hint strip's force tokens: (label, inserted prefix). ONE google chip
/// (2026-07-10 swap): "@gemini" is the lane's display name — the agy CLI
/// underneath; typing "@agy" still parses as an input alias, it just has no
/// chip of its own.
const HINTS: [(&str, &str); 4] = [
    ("@claude", "@claude "),
    ("@codex", "@codex "),
    ("@gemini", "@gemini "),
    ("/adversary", "/adversary "),
];

/// A shared "the `/` menu is open" flag whose truth is derived every frame in
/// [`OrchestratorPanel::render_composer`] and read back by the editor's
/// [`Addon::extend_key_context`]. The `Rc<Cell<bool>>` deliberately avoids the
/// addon reading the panel entity (which would re-enter a mid-update entity per
/// RUST_PORT_NOTES §11): the panel writes the flag synchronously during render,
/// the addon only reads a plain bool.
#[derive(Clone, Default)]
pub(super) struct MenuOpenFlag(Rc<Cell<bool>>);

impl MenuOpenFlag {
    fn set(&self, open: bool) {
        self.0.set(open);
    }
}

/// Editor addon that stamps `menu_open` onto the composer editor's OWN key
/// context while the `/` typeahead is open — the stock `Editor &&
/// showing_completions` idiom (editor.rs `key_context_internal`). Putting the
/// flag on the FOCUSED node (not an ancestor deck) is what lets
/// `Editor && menu_open` outrank the editor's native auto_height caret bindings
/// (up/down/enter) so the nav keys drive the menu instead of the caret.
struct TypeaheadAddon {
    menu_open: MenuOpenFlag,
}

impl Addon for TypeaheadAddon {
    fn extend_key_context(&self, key_context: &mut KeyContext, _cx: &App) {
        if self.menu_open.0.get() {
            key_context.add("menu_open");
        }
    }

    fn to_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// The composer deck's view state — owned by the panel (TranscriptView
/// pattern), not a separate entity.
pub(super) struct Composer {
    pub editor: Entity<Editor>,
    /// The send⇄stop morph state.
    send_circle: SendCircle,
    /// The send chain's tail — every send queues behind the previous one
    /// via [`enqueue_send`], so a rapid second Enter can never cancel an
    /// in-flight `POST /message` (the web fires every `send()` to
    /// completion; serialization additionally pins arrival order).
    send_task: Option<Task<()>>,
    /// The `/`-menu-open flag shared with the editor's [`TypeaheadAddon`].
    /// Written each render from `render_typeahead_menu().is_some()`, read by
    /// the addon when the editor rebuilds its key context.
    pub(super) menu_open: MenuOpenFlag,
    /// Whether the deck's usage cluster is horizontally extended (user
    /// ruling 2026-07-08: the per-pool detail extends sideways on click).
    pub(super) usage_open: bool,
    /// Whether the target-selector menu (the working brain pill, user
    /// ruling 2026-07-08) is open.
    pub(super) target_menu_open: bool,
    _editor_subscription: Subscription,
}

impl Composer {
    pub fn new(window: &mut Window, cx: &mut gpui::Context<OrchestratorPanel>) -> Self {
        let menu_open = MenuOpenFlag::default();
        let editor = cx.new(|cx| {
            let mut editor = Editor::auto_height(1, 8, window, cx);
            editor.set_placeholder_text(
                "Describe a task…  @ to force an agent, / for adversary",
                window,
                cx,
            );
            editor.set_soft_wrap();
            editor.set_show_indent_guides(false, cx);
            // Mirror the stock `Editor && showing_completions` context flag: the
            // addon stamps `menu_open` on the editor's OWN key context while the
            // `/` typeahead is open, so `Editor && menu_open` bindings win over
            // the native auto_height caret keys (the fix for dead keyboard nav).
            editor.register_addon(TypeaheadAddon {
                menu_open: menu_open.clone(),
            });
            editor
        });
        // Repaint on edits so the reveal retargets off the live has-text
        // state (the web's `input` listener toggling `.has-text`).
        let _editor_subscription =
            cx.subscribe(&editor, |_, _, event: &EditorEvent, cx| {
                if matches!(event, EditorEvent::BufferEdited) {
                    cx.notify();
                }
            });
        Self {
            editor,
            send_circle: SendCircle::new(),
            send_task: None,
            menu_open,
            usage_open: false,
            target_menu_open: false,
            _editor_subscription,
        }
    }
}

/// `box.value.replace(/^(@\w+\s|\/adversary\s)/, "")` — strip one leading
/// force prefix so hint clicks swap the target instead of stacking them.
fn strip_force_prefix(text: &str) -> &str {
    if let Some(rest) = text.strip_prefix("/adversary")
        && let Some(rest) = rest.strip_prefix(|c: char| c.is_whitespace())
    {
        return rest;
    }
    if let Some(stripped) = text.strip_prefix('@') {
        let word_len = stripped
            .find(|c: char| !(c.is_alphanumeric() || c == '_'))
            .unwrap_or(stripped.len());
        if word_len > 0
            && let Some(rest) = stripped[word_len..].strip_prefix(|c: char| c.is_whitespace())
        {
            return rest;
        }
    }
    text
}

/// The bridge's `POST /message` reply (`{"conv": id, …}`).
#[derive(Default, serde::Deserialize)]
struct MessageResponse {
    #[serde(default)]
    conv: String,
}

/// FIFO send serialization: queue `send` behind whatever send is already in
/// flight in `slot`. gpui tasks CANCEL when dropped, so overwriting the slot
/// would abort an in-flight `POST /message` after the editor was already
/// cleared — silent message loss, exactly while the bridge is slow (the one
/// window the web explicitly tolerates, chat.js "bridge restarting").
/// Instead the previous task is moved INTO the new one and awaited first:
/// every send completes, in submission order.
fn enqueue_send(
    slot: &mut Option<Task<()>>,
    cx: &mut App,
    send: impl AsyncFnOnce(&mut AsyncApp) + 'static,
) {
    let previous = slot.take();
    *slot = Some(cx.spawn(async move |cx| {
        if let Some(previous) = previous {
            previous.await;
        }
        send(cx).await;
    }));
}

impl OrchestratorPanel {
    /// Enter / send-circle click: POST the trimmed text on the background
    /// executor, then canonicalize the conversation + refetch the
    /// transcript (the web's `send()` + immediate `loop()`).
    /// The attach button (dogfood 2026-07-08): native multi-file picker →
    /// the chosen paths splice into the editor at the caret, quoted when
    /// they carry spaces. Vendor-unified: paths ride the task TEXT, which
    /// every worker already reads — no per-vendor upload plumbing.
    fn pick_attachments(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        let picked = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Attach".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
            if paths.is_empty() {
                return;
            }
            let joined = paths
                .iter()
                .map(|path| {
                    let s = path.display().to_string();
                    if s.contains(' ') { format!("\"{s}\"") } else { s }
                })
                .collect::<Vec<_>>()
                .join(" ");
            this.update_in(cx, |this, window, cx| {
                this.composer.editor.update(cx, |editor, cx| {
                    editor.insert(&format!(" {joined} "), window, cx);
                });
                window.focus(&this.composer.editor.read(cx).focus_handle(cx), cx);
            })
            .ok();
        })
        .detach();
    }

    /// POST the turn abort for the conversation on screen. Fire-and-forget
    /// on the background executor — the transcript poll (900ms busy cadence)
    /// carries the aborted reply back; no client-side state to unwind.
    fn abort_turn(&mut self, cx: &mut gpui::Context<Self>) {
        let conv = {
            let store = self.store.read(cx);
            store
                .transcript
                .as_ref()
                .map(|snapshot| snapshot.id.clone())
                .or_else(|| store.transcript_conv.clone())
        };
        let Some(conv) = conv else { return };
        let client = cx.http_client();
        cx.background_spawn(async move {
            let url = format!("{BRIDGE_BASE_URL}/conv/{conv}/abort");
            post_json(client.as_ref(), &url, "{}".to_string()).await.ok();
        })
        .detach();
    }

    pub(super) fn send_message(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        // Enter while the `/` typeahead is open ACCEPTS the highlighted command
        // (splices `/name ` into the editor) instead of sending — the menu owns
        // Enter, exactly like the mention menu. `typeahead_accept` returns
        // false when the menu is closed, so a normal Enter still sends.
        if self.typeahead_accept(window, cx) {
            return;
        }
        if self.busy {
            // The stop circle (dogfood 2026-07-08): a real turn abort now —
            // POST /conv/<id>/abort kills the in-flight turn's child runs
            // (vendor-unified) and stops the loop at its next seam; the busy
            // transcript poll then lands the "(turn aborted by user)" reply.
            self.abort_turn(cx);
            return;
        }
        let text = self.composer.editor.read(cx).text(cx);
        let text = text.trim().to_string();
        if text.is_empty() {
            return;
        }

        // S4: a leading orchestrator-native `/command` routes IN-APP (open a
        // surface, escalate a plan, render /help) instead of POSTing as a chat
        // message. A `@vendor`-forced message, or a `/name` that is a
        // skill/custom command, falls through to the bridge with the prefix
        // INTACT — the claude worker's `-p` expands it (S2, verified).
        let orch_names = self.orchestrator_command_names(cx);
        let name_refs: Vec<&str> = orch_names.iter().map(String::as_str).collect();
        if let super::dispatch::Submission::Orchestrator { target, args } =
            super::dispatch::parse_submission(&text, &name_refs)
        {
            self.composer
                .editor
                .update(cx, |editor, cx| editor.clear(window, cx));
            self.dispatch_orchestrator(target, args, window, cx);
            cx.notify();
            return;
        }

        self.composer
            .editor
            .update(cx, |editor, cx| editor.clear(window, cx));

        let conv = self.store.read(cx).transcript_conv.clone().unwrap_or_default();
        let http_client = cx.http_client();
        let store = self.store.clone();
        enqueue_send(&mut self.composer.send_task, cx, async move |cx| {
            let posted = cx
                .background_spawn(async move {
                    let body = serde_json::json!({ "text": text, "conv": conv }).to_string();
                    let raw =
                        post_json(http_client.as_ref(), &format!("{BRIDGE_BASE_URL}/message"), body)
                            .await?;
                    anyhow::Ok(serde_json::from_str::<MessageResponse>(&raw).unwrap_or_default())
                })
                .await;
            store.update(cx, |store, cx| {
                if let Ok(response) = posted
                    && !response.conv.is_empty()
                {
                    store.set_conversation(Some(response.conv), cx);
                }
                // Refetch even on a failed POST — the bridge may have
                // taken the message before the response broke.
                store.refetch_transcript_soon(cx);
            });
        });
        cx.notify();
    }

    /// Hint click: swap any existing force prefix for the clicked one and
    /// refocus the editor.
    fn insert_hint(&mut self, ins: &'static str, window: &mut Window, cx: &mut gpui::Context<Self>) {
        self.composer.editor.update(cx, |editor, cx| {
            let text = editor.text(cx);
            let rest = strip_force_prefix(&text).to_string();
            editor.set_text(format!("{ins}{rest}"), window, cx);
        });
        window.focus(&self.composer.editor.read(cx).focus_handle(cx), cx);
    }

    /// The whole composer block: deck + footer inside the centered
    /// `.chat-col` (`.composer-wrap { padding: 6px 9px 20px 0 }`; the
    /// scroll-fade gradient is dropped — the panel surface is already
    /// opaque).
    pub(super) fn render_composer(
        &mut self,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        let has_text = !self.composer.editor.read(cx).is_empty(cx);
        let busy = self.busy;
        self.composer.send_circle.update_motion(busy, has_text);

        // Compute the `/` typeahead FIRST (needs `&mut cx`): the menu element
        // AND the open flag. `render_typeahead_menu` is Some exactly when open.
        // Built before the `colors` borrow below so the mutable `cx` reborrow is
        // unambiguous.
        let menu = self.render_typeahead_menu(cx);
        // The `menu_open` flag rides on the EDITOR's own key context (via
        // `TypeaheadAddon`), NOT this ancestor deck: a focused-node flag is what
        // makes `Editor && menu_open` outrank the editor's native auto_height
        // up/down/enter caret bindings (an ancestor `... > Editor` binding ties
        // at the leaf depth and loses to the editor's own keys — the stock
        // `showing_completions` pattern puts the flag on the editor for exactly
        // this reason). The deck keeps only `OrchestratorComposer` for the
        // menu-closed Enter→Send binding; the typeahead `on_action` handlers
        // stay here and receive the actions as they bubble up from the editor.
        self.composer.menu_open.set(menu.is_some());
        let mut key_context = gpui::KeyContext::new_with_defaults();
        key_context.add("OrchestratorComposer");

        // The forced @vendor prefix drives the composer's ACCENT (user
        // ruling 2026-07-08: "for whichever vendor task we do, we utilise
        // the vendor colour as the accent colour") — border first; parsed
        // per keystroke from the live editor text.
        let editor_text = self.composer.editor.read(cx).text(cx);
        let forced = forced_vendor(&editor_text);
        let vendor_tint = forced.map(vendor_accent);
        // The usage cluster is built before the deck (it needs `&mut self`
        // for the extend morph's StateFades).
        let usage_cluster = self.render_usage_strip(cx);
        let target_menu = self.render_target_menu(cx);

        let colors = cx.theme().colors();
        let focused = self
            .composer
            .editor
            .read(cx)
            .focus_handle(cx)
            .is_focused(window);
        // `:focus-within { border-color: var(--hairline-hi) }` on the .15s
        // effects transition — render-driven retarget (same-target calls
        // are no-ops, the SendCircle::update_motion idiom).
        let focus_id = ElementId::Name("composer-focus".into());
        self.fades.set(focus_id.clone(), focused, STATE_FADE);
        let focus_t = self.fades.t(&focus_id);
        // Vendor accent outranks the neutral focus tint: a forced target
        // keeps the deck lit in that vendor's color even unfocused (dimmer),
        // brightening with focus.
        let deck_border = match vendor_tint {
            Some(accent) => mix(colors.border, accent, 0.45 + 0.35 * focus_t),
            None => mix(colors.border, HAIRLINE_HI.into(), focus_t),
        };

        let deck = v_flex()
            .key_context(key_context)
            .on_action(cx.listener(|this, _: &Send, window, cx| this.send_message(window, cx)))
            .on_action(cx.listener(|this, _: &super::panel::TypeaheadUp, _, cx| {
                this.typeahead_move(-1, cx);
            }))
            .on_action(cx.listener(|this, _: &super::panel::TypeaheadDown, _, cx| {
                this.typeahead_move(1, cx);
            }))
            .on_action(cx.listener(|this, _: &super::panel::TypeaheadAccept, window, cx| {
                this.typeahead_accept(window, cx);
            }))
            .on_action(cx.listener(|this, _: &super::panel::TypeaheadDismiss, window, cx| {
                this.typeahead_dismiss(window, cx);
            }))
            .w_full()
            .rounded(px(16.))
            .bg(SURFACE_2)
            .border_1()
            .border_color(deck_border)
            .overflow_hidden()
            // Whole-deck focus law (playtest 2026-07-10: clicks landing a few
            // px below the single text line hit deck padding and did NOTHING
            // — an hour of ghost inputs). Any press inside the deck focuses
            // the editor; interactive children still receive their own
            // events (focus is idempotent and never swallows a click).
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    window.focus(&this.composer.editor.read(cx).focus_handle(cx), cx);
                }),
            )
            .child(
                // textarea: padding 15px 17px 4px, 15px/1.55 UI font.
                div()
                    .pt(px(15.))
                    .px(px(17.))
                    .pb(px(4.))
                    .child(self.render_editor(cx)),
            )
            .child(self.render_controls_row(has_text, busy, forced, usage_cluster, cx))
            .child(self.render_foot(cx));

        h_flex()
            .w_full()
            .justify_center()
            .pt(px(6.))
            .pb(px(20.))
            .child(
                v_flex()
                    .w_full()
                    .max_w(px(780.))
                    .px(px(32.))
                    // The dynamic-island home (user ruling 2026-07-07): the
                    // toast stack floats above the deck in a ZERO-HEIGHT
                    // strip — bottom-anchored, growing UPWARD over the
                    // transcript, never displacing the deck while the user
                    // types. Replaces the retired top-right corner cluster.
                    .child(self.render_island_home(cx))
                    // §4.9 composer anchor slot: the running-tasks island
                    // (Task 4) emerges ABOVE the typing pill, in flow.
                    .children(self.render_tasks_island_slot(cx))
                    // S1: the `/` typeahead popover — anchored above the deck,
                    // in flow (displaces the deck downward while open).
                    .children(menu)
                    // The target-selector menu (the working brain pill) —
                    // same in-flow displacement idiom as the typeahead.
                    .children(target_menu)
                    .child(deck),
            )
            .into_any_element()
    }

    /// The zero-height overlay strip above the deck: the toast stack,
    /// bottom-anchored so it grows UPWARD over the transcript and never
    /// displaces the deck. (The usage pill that used to share this anchor
    /// is now the deck's own usage strip — user ruling 2026-07-08.)
    fn render_island_home(&self, _cx: &mut gpui::Context<Self>) -> AnyElement {
        div()
            .relative()
            .w_full()
            .h(px(0.))
            .child(
                div()
                    .absolute()
                    .bottom(px(8.))
                    .right_0()
                    .child(self.notif_stack.clone()),
            )
            .into_any_element()
    }

    /// The deck's usage strip (see [`super::usage_strip`]): folds the live
    /// pool rows per frame (≤6 rows, cheap) and drives the horizontal
    /// extend off the panel's StateFades so a mid-morph toggle re-bases.
    fn render_usage_strip(&mut self, cx: &mut gpui::Context<Self>) -> AnyElement {
        let rows = if crate::bridge::is_agentic_demo() {
            crate::usage_panel_demo::demo_pools()
        } else {
            self.store.read(cx).usage.clone()
        };
        let pools = super::usage_strip::fold_pools(&rows);
        let session = super::usage_strip::session_pct(&rows);
        let strip_id = gpui::ElementId::Name("usage-strip-extend".into());
        self.fades
            .set(strip_id.clone(), self.composer.usage_open, USAGE_STRIP_MORPH);
        let open_t = self.fades.t(&strip_id);
        super::usage_strip::render_usage_strip(
            &pools,
            session,
            open_t,
            cx.listener(|this, _, _, cx| {
                this.composer.usage_open = !this.composer.usage_open;
                cx.notify();
            }),
            cx,
        )
    }

    fn render_editor(&self, cx: &mut gpui::Context<Self>) -> AnyElement {
        let settings = ThemeSettings::get_global(cx);
        let text_style = TextStyle {
            color: cx.theme().colors().text,
            font_family: settings.ui_font.family.clone(),
            font_features: settings.ui_font.features.clone(),
            font_size: px(15.).into(),
            line_height: relative(1.55).into(),
            ..Default::default()
        };
        EditorElement::new(
            &self.composer.editor,
            EditorStyle {
                background: gpui::transparent_black(),
                local_player: cx.theme().players().local(),
                syntax: cx.theme().syntax().clone(),
                text: text_style,
                ..Default::default()
            },
        )
        .into_any_element()
    }

    /// `.composer-row`: [+ ghost 32] [brain pill] [spacer] [send 40].
    fn render_controls_row(
        &self,
        has_text: bool,
        busy: bool,
        forced: Option<Vendor>,
        usage_cluster: AnyElement,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        let colors = cx.theme().colors();
        let brain = self
            .store
            .read(cx)
            .transcript
            .as_ref()
            .and_then(|snapshot| snapshot.brain.clone())
            .unwrap_or_else(|| "idle".to_string());
        // The pill shows the message's TARGET: the forced vendor when a
        // `@vendor` prefix is live (with its identity dot), else the
        // orchestrator brain ("auto" routing).
        let (target_label, target_dot) = match forced {
            Some(vendor) => (vendor.badge().to_string(), Some(vendor_accent(vendor))),
            None => (brain, None),
        };

        h_flex()
            .items_center()
            .gap(px(8.))
            .pt(px(6.))
            .pr(px(10.))
            .pb(px(8.))
            .pl(px(12.))
            .child({
                // `.ghost-btn` — the attach button (dogfood 2026-07-08, was
                // anatomy-only): opens the native file picker and inserts
                // the chosen paths into the task text. Vendor-unified by
                // construction — every worker (claude/codex/agy/gemini)
                // reads file paths straight out of its task prompt.
                let ghost_id = ElementId::Name("composer-attach".into());
                let hover_t = self.fades.t(&ghost_id);
                div()
                    .id(ghost_id.clone())
                    .size(px(32.))
                    .rounded_full()
                    .border_1()
                    .border_color(mix(colors.border, HAIRLINE_HI.into(), hover_t))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        this.set_fade(ghost_id.clone(), *hovered, STATE_FADE, cx);
                    }))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.pick_attachments(window, cx);
                    }))
                    .child(
                        Icon::new(IconName::Plus)
                            .size(IconSize::Custom(rems_from_px(18.)))
                            .color(Color::Custom(mix(
                                colors.text_placeholder,
                                colors.text_muted,
                                hover_t,
                            ))),
                    )
            })
            .child({
                // The target pill (formerly the anatomy-only brain pill —
                // user ruling 2026-07-08 "make the gemini button work"):
                // shows the live target (forced vendor with its identity
                // dot, else the heartbeat brain); click opens the target-
                // selector menu, which sets/strips the `@vendor` prefix.
                let brain_id = ElementId::Name("composer-brain".into());
                let hover_t = self.fades.t(&brain_id);
                h_flex()
                    .id(brain_id.clone())
                    .items_center()
                    .gap(px(6.))
                    .pl(px(12.))
                    .pr(px(8.))
                    .py(px(6.))
                    .rounded_full()
                    .border_1()
                    .border_color(match target_dot {
                        Some(accent) => mix(colors.border, accent, 0.6),
                        None => mix(colors.border, HAIRLINE_HI.into(), hover_t),
                    })
                    .text_size(px(13.))
                    .text_color(mix(colors.text_muted, colors.text, hover_t))
                    .cursor_pointer()
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        this.set_fade(brain_id.clone(), *hovered, STATE_FADE, cx);
                    }))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.composer.target_menu_open = !this.composer.target_menu_open;
                        cx.notify();
                    }))
                    .children(target_dot.map(|accent| {
                        div().flex_none().size(px(7.)).rounded_full().bg(accent)
                    }))
                    .child(SharedString::from(target_label))
                    .child(
                        Icon::new(IconName::ChevronDown)
                            .size(IconSize::Medium)
                            .color(Color::Placeholder),
                    )
            })
            .child(div().flex_1())
            // The inline usage cluster (user ruling 2026-07-08: usage sits
            // to the SIDE, in the controls row; detail extends leftward
            // into the spacer).
            .child(usage_cluster)
            .child(self.composer.send_circle.render(has_text || busy, cx))
            .into_any_element()
    }

    /// The target-selector menu (the working brain pill): auto + the four
    /// vendors, each with its identity dot. Choosing one sets the message's
    /// `@vendor` force prefix (auto strips it) — the same mechanics as the
    /// foot hints, surfaced as a proper control.
    fn render_target_menu(&mut self, cx: &mut gpui::Context<Self>) -> Option<AnyElement> {
        if !self.composer.target_menu_open {
            return None;
        }
        let colors = cx.theme().colors();
        // ONE google row (2026-07-10 swap): gemini = the lane's display name,
        // the agy CLI underneath ("@agy" still parses as an input alias).
        let rows: [(&'static str, &'static str, Option<gpui::Hsla>); 4] = [
            ("auto", "", None),
            ("claude", "@claude ", Some(vendor_accent(Vendor::Claude))),
            ("codex", "@codex ", Some(vendor_accent(Vendor::Codex))),
            ("gemini", "@gemini ", Some(vendor_accent(Vendor::Gemini))),
        ];
        let items: Vec<AnyElement> = rows
            .into_iter()
            .map(|(label, prefix, dot)| {
                h_flex()
                    .id(ElementId::Name(format!("target-{label}").into()))
                    .items_center()
                    .gap(px(8.))
                    .px(px(10.))
                    .py(px(6.))
                    .rounded(px(6.))
                    .text_size(px(13.))
                    .text_color(colors.text_muted)
                    .cursor_pointer()
                    .hover(|s| s.bg(colors.element_hover))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.composer.target_menu_open = false;
                        this.insert_hint(prefix, window, cx);
                    }))
                    .child(
                        div()
                            .flex_none()
                            .size(px(7.))
                            .rounded_full()
                            .when_some(dot, |el, accent| el.bg(accent))
                            .when(dot.is_none(), |el| {
                                el.border_1().border_color(colors.text_placeholder)
                            }),
                    )
                    .child(label)
                    .into_any_element()
            })
            .collect();
        Some(
            div()
                .id("target-menu")
                .mb(px(6.))
                .w(px(180.))
                .p(px(4.))
                .rounded(px(10.))
                .bg(SURFACE_2)
                .border_1()
                .border_color(cx.theme().colors().border)
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    this.composer.target_menu_open = false;
                    cx.notify();
                }))
                .children(items)
                .into_any_element(),
        )
    }

    /// `.composer-foot` — the darker sub-deck env strip with the clickable
    /// force hints.
    fn render_foot(&self, cx: &mut gpui::Context<Self>) -> AnyElement {
        // Copy the Hsla tokens out so the hint closure can take `cx`
        // mutably (Hsla is Copy; the theme borrow must not outlive this).
        let (placeholder, muted, hover_bg) = {
            let colors = cx.theme().colors();
            (
                colors.text_placeholder,
                colors.text_muted,
                colors.element_hover,
            )
        };
        let mono = ThemeSettings::get_global(cx).buffer_font.family.clone();
        let fades = &self.fades;
        let hint = |label: &'static str, ins: &'static str, cx: &mut gpui::Context<Self>| {
            // `.hint { transition: color/background .15s var(--effects-curve) }`.
            let hint_id = ElementId::Name(format!("hint-{label}").into());
            let hover_t = fades.t(&hint_id);
            div()
                .id(hint_id.clone())
                .px(px(4.))
                .py(px(1.))
                .rounded(px(6.))
                .font_family(mono.clone())
                .text_size(px(12.))
                .text_color(mix(placeholder, muted, hover_t))
                .bg(hover_bg.opacity(hover_t))
                .cursor_pointer()
                .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                    this.set_fade(hint_id.clone(), *hovered, STATE_FADE, cx);
                }))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.insert_hint(ins, window, cx);
                }))
                .child(label)
        };

        let mut foot = h_flex()
            .items_center()
            .flex_wrap()
            .gap(px(8.))
            .px(px(14.))
            .py(px(8.))
            .bg(SURFACE_2B)
            .text_size(px(13.))
            .text_color(placeholder);
        // Vendor chips lead, "/adversary" (the last HINTS entry) trails the
        // "to force" label — index math derived from the array, not hardcoded.
        let (adversary, vendors) = HINTS.split_last().expect("HINTS non-empty");
        for (label, ins) in vendors {
            foot = foot.child(hint(label, ins, cx));
        }
        foot.child(div().opacity(0.7).child("to force"))
            .child(hint(adversary.0, adversary.1, cx))
            .into_any_element()
    }

    /// The §4.9 composer anchor: the running-tasks island occupies normal
    /// flow here while visible (its height displaces the deck downward —
    /// the choreography), and is absent from the tree otherwise.
    fn render_tasks_island_slot(&mut self, cx: &mut gpui::Context<Self>) -> Option<AnyElement> {
        if !self.tasks_island.visible() {
            return None;
        }
        Some(crate::islands::tasks_island::render_tasks_island(
            &self.tasks_island,
            &self.fades,
            cx,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    #[gpui::test]
    async fn queued_send_never_cancels_the_in_flight_post(cx: &mut gpui::TestAppContext) {
        // The double-send loss class: gpui tasks cancel on drop, so a second
        // Enter while POST #1 is still on the wire must queue behind it —
        // never overwrite (and thereby abort) it. Both sends must complete,
        // in submission order, even when the bridge is slow.
        let log: Rc<RefCell<Vec<&'static str>>> = Rc::default();
        let (gate_tx, gate_rx) = futures::channel::oneshot::channel::<()>();
        let mut slot: Option<Task<()>> = None;
        cx.update(|cx| {
            let log1 = log.clone();
            enqueue_send(&mut slot, cx, async move |_| {
                log1.borrow_mut().push("send-1 started");
                gate_rx.await.ok(); // a slow bridge holds POST #1 open
                log1.borrow_mut().push("send-1 completed");
            });
            // Enter again while POST #1 is parked on the wire.
            let log2 = log.clone();
            enqueue_send(&mut slot, cx, async move |_| {
                log2.borrow_mut().push("send-2 completed");
            });
        });
        cx.run_until_parked();
        assert_eq!(
            *log.borrow(),
            ["send-1 started"],
            "POST #1 must still be in flight (parked on the bridge), NOT cancelled"
        );

        gate_tx.send(()).unwrap();
        cx.run_until_parked();
        assert_eq!(
            *log.borrow(),
            ["send-1 started", "send-1 completed", "send-2 completed"],
            "both sends complete, in submission order — no silent drops"
        );
    }

    #[test]
    fn force_prefix_swap_matches_the_web_regex() {
        // `^(@\w+\s|\/adversary\s)` swapped for the clicked hint.
        assert_eq!(strip_force_prefix("@codex do the thing"), "do the thing");
        assert_eq!(strip_force_prefix("@claude x"), "x");
        assert_eq!(strip_force_prefix("/adversary debate this"), "debate this");
        // No prefix → untouched.
        assert_eq!(strip_force_prefix("plain task"), "plain task");
        // Mid-text mentions are not prefixes.
        assert_eq!(strip_force_prefix("ask @codex later"), "ask @codex later");
        // A bare @ or missing trailing whitespace is not a prefix (regex
        // requires \w+ then \s).
        assert_eq!(strip_force_prefix("@"), "@");
        assert_eq!(strip_force_prefix("@codex"), "@codex");
        assert_eq!(strip_force_prefix("/adversary"), "/adversary");
        // Newline counts as \s, like the web.
        assert_eq!(strip_force_prefix("@agy\nplan"), "plan");
    }
}
