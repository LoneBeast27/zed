//! Interrupt + steer + queue wiring (FEATURE_SENTIMENT §B) — the busy-turn
//! keybinding scheme and the honest live-vs-queued render.
//!
//! The load-bearing law is B-T3 (affordance honesty): "interrupt" must ACTUALLY
//! interrupt, never silently queue-and-finish. So the steer POST reads the
//! bridge's `landed` field VERBATIM (`"live"` = delivered to the running turn at
//! its next boundary; `"queued"` = it rides as the next turn) and surfaces that
//! truth on the composer note surface — never the documented-but-false "stops
//! what it's doing" (the #1 documented steer frustration, [S-B1/S-B3]).
//!
//! Q14b ruling + sentiment consensus — while a turn is BUSY:
//! - **Enter (composer has text)** = STEER (the in-context redirect, B-T1);
//! - **Tab** = QUEUE (the message rides as the next turn, temporal order kept,
//!   B-T6);
//! - **Esc / Stop button** = HARD-STOP (abort — keeps completed edits on disk,
//!   B-T4).
//! When IDLE, Enter = send (unchanged). NEVER steal focus: the POST is a
//! fire-and-forget background spawn (the existing `abort_turn` idiom).

use gpui::{AppContext as _, Window};
use serde::Deserialize;

use crate::bridge::{BRIDGE_BASE_URL, error_message, post_json_status};

use super::panel::OrchestratorPanel;

/// What the composer's Enter/Tab press resolves to, derived PURELY from the
/// live (busy, has_text) state (menu-open is handled upstream by the typeahead
/// binding — this maps the menu-CLOSED submission keys). Unit-tested directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ComposerAction {
    /// Idle Enter — the normal `/message` send (unchanged path).
    Send,
    /// Busy Enter with text — steer the running turn (POST /steer).
    Steer,
    /// Busy Tab with text — queue as the next turn (POST /steer, the bridge
    /// decides live-vs-queued; Tab merely signals the QUEUE intent so temporal
    /// order is kept — B-T6). Rendered honestly by its `landed` response.
    Queue,
    /// Nothing to do (busy with an empty composer, or an idle Tab): never a
    /// misleading affordance — an empty steer is a no-op, not a phantom send.
    Ignore,
}

/// Map the Enter key (menu closed) to its action. Idle = send; busy+text =
/// steer; busy+empty = ignore (no phantom send of nothing).
pub(super) fn enter_action(busy: bool, has_text: bool) -> ComposerAction {
    match (busy, has_text) {
        (false, _) => ComposerAction::Send,
        (true, true) => ComposerAction::Steer,
        (true, false) => ComposerAction::Ignore,
    }
}

/// Map the Tab key (menu closed) to its action. Busy+text = queue; otherwise
/// ignore (idle Tab does nothing in the composer — the editor keeps focus, no
/// indentation nonsense in a chat box).
pub(super) fn tab_action(busy: bool, has_text: bool) -> ComposerAction {
    match (busy, has_text) {
        (true, true) => ComposerAction::Queue,
        _ => ComposerAction::Ignore,
    }
}

/// The bridge's `POST /conv/<id>/steer` reply. `landed` is the honesty field:
/// `"live"` (delivered to the running turn at its next tool-call boundary) vs
/// `"queued"` (rode as the next turn). `turn` is the turn index it attached to.
#[derive(Debug, Clone, Default, Deserialize)]
struct SteerResponse {
    #[serde(default)]
    landed: String,
    #[serde(default)]
    turn: Option<i64>,
}

/// The honest note the composer surfaces after a steer, computed PURELY from
/// the bridge's `landed` string so the message is never a documented-but-false
/// "stopped what it's doing" (B-T3). Unknown/absent `landed` degrades to a
/// neutral-but-honest "delivered" rather than claiming an interrupt. `turn`,
/// when the bridge supplies it, is appended so the message's temporal
/// association is VISIBLE (B-T6 — the user sees WHICH turn it rode).
pub(super) fn steer_note(landed: &str, queue_intent: bool, turn: Option<i64>) -> String {
    let base = match landed {
        // The interrupt actually interrupted — delivered to the RUNNING turn.
        "live" => "Steered — delivered to the running turn at its next boundary.".to_string(),
        // Honestly surfaced as queued (never silent): the badge the sentiment
        // spec demands when the product chooses queue-on-Enter (B-T3).
        "queued" => "Queued — will apply as the next turn (temporal order kept).".to_string(),
        // Empty/unknown: don't fabricate a landing state. If the USER asked to
        // queue (Tab), say queued-request; else say delivered.
        _ if queue_intent => "Queued for the next turn.".to_string(),
        _ => "Message delivered to the turn.".to_string(),
    };
    // B-T6 temporal fidelity: name the turn it attached to when the bridge
    // reports one (queued messages must keep their temporal context).
    match turn {
        Some(turn) => format!("{base} (turn {turn})"),
        None => base,
    }
}

impl OrchestratorPanel {
    /// POST `/conv/<id>/steer {text}` for the on-screen conversation, then
    /// render the bridge's HONEST `landed` (live vs queued) on the composer
    /// note surface — B-T3. Fire-and-forget on the background executor (never
    /// steals focus; the existing `abort_turn` idiom); a 409 (no live turn) or
    /// any bridge refusal lands VERBATIM on the same note surface, never a
    /// silent no-op.
    ///
    /// `queue_intent` = the user pressed Tab (QUEUE) rather than Enter (STEER):
    /// it only tempers the fallback note wording; the bridge's `landed` field
    /// is authoritative when present (the bridge, not the client, decides
    /// whether a mid-turn message could land live).
    pub(super) fn send_steer(
        &mut self,
        queue_intent: bool,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let text = self.composer.editor.read(cx).text(cx).trim().to_string();
        if text.is_empty() {
            return;
        }
        let conv = {
            let store = self.store.read(cx);
            store
                .transcript
                .as_ref()
                .map(|snapshot| snapshot.id.clone())
                .or_else(|| store.transcript_conv.clone())
        };
        let Some(conv) = conv else { return };
        // Clear the composer immediately (the message left) and drop any prior
        // note — the send-then-clear idiom, so a rapid follow-up steer isn't
        // blocked on the note.
        self.composer
            .editor
            .update(cx, |editor, cx| editor.clear(window, cx));
        self.wave_note = None;
        cx.notify();

        let client = cx.http_client();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let url = format!("{BRIDGE_BASE_URL}/conv/{conv}/steer");
                    let body = serde_json::json!({ "text": text }).to_string();
                    post_json_status(client.as_ref(), &url, body).await
                })
                .await;
            this.update(cx, |this, cx| {
                let note = match result {
                    Ok((status, raw)) if (200..300).contains(&status) => {
                        let parsed: SteerResponse =
                            serde_json::from_str(&raw).unwrap_or_default();
                        steer_note(&parsed.landed, queue_intent, parsed.turn)
                    }
                    // 409 = no live turn to steer (it settled between the
                    // keystroke and the POST) / any bridge refusal — the
                    // bridge's {"error"} VERBATIM, never swallowed (B-T3).
                    Ok((status, raw)) => error_message(status, &raw),
                    Err(error) => format!("steer failed: {error}"),
                };
                this.wave_note = Some(note.into());
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enter_maps_idle_send_busy_steer_empty_ignore() {
        // Idle Enter = send, regardless of text (empty-idle send is filtered
        // downstream by the send path's own is_empty guard).
        assert_eq!(enter_action(false, true), ComposerAction::Send);
        assert_eq!(enter_action(false, false), ComposerAction::Send);
        // Busy Enter WITH text = steer (the load-bearing in-context redirect).
        assert_eq!(enter_action(true, true), ComposerAction::Steer);
        // Busy Enter with an EMPTY composer = ignore — never a phantom steer.
        assert_eq!(enter_action(true, false), ComposerAction::Ignore);
    }

    #[test]
    fn tab_maps_busy_text_queue_else_ignore() {
        // Busy Tab WITH text = queue (rides as the next turn, B-T6).
        assert_eq!(tab_action(true, true), ComposerAction::Queue);
        // Busy Tab with no text = ignore.
        assert_eq!(tab_action(true, false), ComposerAction::Ignore);
        // Idle Tab = ignore (no indentation in a chat composer).
        assert_eq!(tab_action(false, true), ComposerAction::Ignore);
        assert_eq!(tab_action(false, false), ComposerAction::Ignore);
    }

    #[test]
    fn steer_note_is_honest_live_vs_queued() {
        // "live" = the interrupt ACTUALLY reached the running turn (B-T3): the
        // note must say so, never a false "stopped".
        let live = steer_note("live", false, None);
        assert!(live.contains("running turn"), "live must name the running turn: {live}");
        assert!(!live.to_lowercase().contains("queue"), "live is not a queue: {live}");
        // "queued" = surfaced explicitly as queued, never silent.
        let queued = steer_note("queued", false, None);
        assert!(queued.to_lowercase().contains("queued"), "queued must say queued: {queued}");
        assert!(
            queued.to_lowercase().contains("next turn"),
            "queued must state it applies at the next turn: {queued}"
        );
        // A "live" landing and a "queued" landing must be DISTINGUISHABLE — the
        // whole point of B-T3 is that the two are never conflated.
        assert_ne!(live, queued);
    }

    #[test]
    fn steer_note_unknown_landing_never_fabricates_interrupt() {
        // Empty/unknown `landed`: never claim "live"/interrupt. Tab-intent
        // degrades to "queued", Enter-intent to a neutral "delivered".
        let unknown_enter = steer_note("", false, None);
        assert!(
            !unknown_enter.to_lowercase().contains("running turn"),
            "unknown landing must not fabricate a live interrupt: {unknown_enter}"
        );
        let unknown_tab = steer_note("", true, None);
        assert!(
            unknown_tab.to_lowercase().contains("queue"),
            "a Tab (queue) intent degrades to queued wording: {unknown_tab}"
        );
    }

    #[test]
    fn steer_note_surfaces_the_turn_for_temporal_fidelity() {
        // B-T6: when the bridge reports the turn the message rode, the note
        // names it — the queued message keeps its temporal context visibly.
        let with_turn = steer_note("queued", true, Some(7));
        assert!(with_turn.contains("(turn 7)"), "the turn must be visible: {with_turn}");
        // Absent turn = no fabricated "(turn …)" marker appended.
        let without = steer_note("queued", true, None);
        assert!(!without.contains("(turn "), "no fabricated turn marker: {without}");
    }
}
