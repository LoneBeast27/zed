//! Adversary broadcast jobs (PARITY_SPEC §4.5) — the data path behind the
//! three-column panel: `POST /adversary {"text"}` → `{job}`, then a
//! poll of `GET /adversary/<job>` until the job leaves `running`.
//!
//! The poll is watch-gated exactly like [`super::store::TranscriptWatch`]
//! (RUST_PORT_NOTES §4.5: the broadcast goes through the bridge connection
//! task, never blocking the UI thread): the panel holds an
//! [`AdversaryWatch`] only while the dock shows it, the loop checks the
//! refcount on every wake and dies at zero — a hidden panel never keeps a
//! job poll alive. Re-showing the panel mid-job resumes the poll (a strict
//! improvement on the web, whose `unmountAdversary` orphans the job).

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use gpui::{AppContext as _, AsyncApp, Task, WeakEntity};
use http_client::HttpClient;
use serde::Deserialize;

use super::client::{BRIDGE_BASE_URL, fetch_json, post_json};

/// `GET /adversary/<job>` cadence (adversary.js `setInterval(…, 2500)`).
const JOB_POLL_INTERVAL: Duration = Duration::from_millis(2500);

/// The three broadcast columns, in render order (adversary.js `COLS`).
pub const VENDOR_COLUMNS: [&str; 3] = ["claude", "codex", "gemini"];

/// `broadcast()`'s output riding the job dict (`orchestrator/adversary.py`):
/// per-vendor answers + the synthesis judge's text. Liberal like the rest of
/// the protocol — fields default so bridge drift never breaks the panel.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct AdversaryResult {
    #[serde(default)]
    pub answers: HashMap<String, String>,
    #[serde(default)]
    pub synthesis: Option<String>,
}

/// One `GET /adversary/<job>` snapshot (`bridge/adversary_jobs.py`):
/// `status` is `"running"` / `"done"` / `"error"`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct AdversaryJobSnapshot {
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub result: Option<AdversaryResult>,
}

/// `POST /adversary` reply.
#[derive(Default, Deserialize)]
struct StartResponse {
    #[serde(default)]
    job: String,
}

/// The panel-facing job phase.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum AdversaryPhase {
    /// No broadcast yet — the panel shows only the composer.
    #[default]
    Idle,
    /// POST in flight or the bridge job still `running` — shimmer columns.
    Pending,
    /// Terminal: the three columns + synthesis.
    Done(AdversaryResult),
    /// Terminal: the broadcast was ABORTED (bridge job `status == "cancelled"`,
    /// `orchestrator/adversary.py`). Carries the partial result ("(aborted)"
    /// legs) so the panel can render the columns WITH a distinct aborted
    /// banner, rather than passing it off as a normal completed Done (P4).
    Cancelled(AdversaryResult),
    /// Terminal: POST failed or the bridge job errored.
    Failed(String),
}

/// The job state entity the adversary panel renders from.
#[derive(Default)]
pub struct AdversaryJobs {
    pub phase: AdversaryPhase,
    /// Bumped on every [`Self::start`] — one-shot entrance animations key
    /// off it so a re-broadcast replays them.
    pub generation: u64,
    /// The pending bridge job id (`None` while the POST is in flight).
    job: Option<String>,
    /// Live [`AdversaryWatch`] count — the poll loop reads it each wake and
    /// exits at zero (panel visibility gates the poll, never a free timer).
    watchers: Arc<AtomicUsize>,
    /// The poll task — `Some` only while a job is pending AND watched.
    /// Replacing the slot drops (cancels) any old loop, so a re-broadcast
    /// can never apply a stale job's terminal state.
    poll_task: Option<Task<()>>,
    /// The in-flight `POST /adversary`.
    start_task: Option<Task<()>>,
    /// The in-flight `POST /adversary/<job>/abort` (the abort affordance).
    abort_task: Option<Task<()>>,
}

/// RAII registration of a visible adversary panel. Dropping it decrements
/// the watcher count; the poll loop notices on its next wake and exits —
/// cx-free teardown, the [`super::store::TranscriptWatch`] pattern.
pub struct AdversaryWatch {
    watchers: Arc<AtomicUsize>,
}

impl Drop for AdversaryWatch {
    fn drop(&mut self) {
        self.watchers.fetch_sub(1, Ordering::SeqCst);
    }
}

impl AdversaryJobs {
    /// Registers a visible panel and resumes the poll on the 0→1 transition
    /// when a job is still pending (re-activation picks the job back up).
    pub fn watch(&mut self, cx: &mut gpui::Context<Self>) -> AdversaryWatch {
        let watchers = self.watchers.clone();
        if watchers.fetch_add(1, Ordering::SeqCst) == 0 {
            self.ensure_poll(cx);
        }
        AdversaryWatch { watchers }
    }

    /// Fire a broadcast (the web's `broadcast()`): flip to `Pending`, POST
    /// on the background executor, then arm the job poll. A re-broadcast
    /// replaces any previous job — the old poll task is cancelled by the
    /// slot overwrite, so its terminal state can never land over the new
    /// job's shimmer.
    pub fn start(&mut self, text: String, cx: &mut gpui::Context<Self>) {
        self.phase = AdversaryPhase::Pending;
        self.job = None;
        self.poll_task = None;
        self.generation += 1;
        let http_client = cx.http_client();
        self.start_task = Some(cx.spawn(async move |this, cx| {
            let posted = cx
                .background_spawn(async move {
                    let body = serde_json::json!({ "text": text }).to_string();
                    let raw = post_json(
                        http_client.as_ref(),
                        &format!("{BRIDGE_BASE_URL}/adversary"),
                        body,
                    )
                    .await?;
                    anyhow::Ok(serde_json::from_str::<StartResponse>(&raw)?)
                })
                .await;
            this.update(cx, |this, cx| {
                match posted {
                    Ok(response) if !response.job.is_empty() => {
                        this.job = Some(response.job);
                        this.ensure_poll(cx);
                    }
                    // The web's `await post()` rejection is silent; failing
                    // visibly degrades better (the §4.4 lesson).
                    Ok(_) => this.fail("bridge returned no job id".to_string(), cx),
                    Err(error) => this.fail(error.to_string(), cx),
                }
            })
            .ok();
        }));
        cx.notify();
    }

    fn fail(&mut self, error: String, cx: &mut gpui::Context<Self>) {
        self.phase = AdversaryPhase::Failed(error);
        self.poll_task = None;
        cx.notify();
    }

    /// Abort the in-flight broadcast (the abort affordance): POST
    /// `/adversary/<job>/abort` on the background executor. The bridge kills
    /// the three legs' tracked children and flags the job cancelled; the live
    /// job poll then lands the terminal (aborted) snapshot, so no local phase
    /// flip is needed here. A no-op unless a job is Pending with an id.
    pub fn abort(&mut self, cx: &mut gpui::Context<Self>) {
        if !matches!(self.phase, AdversaryPhase::Pending) {
            return;
        }
        let Some(job) = self.job.clone() else {
            return; // POST still in flight — no job id to abort yet
        };
        let http_client = cx.http_client();
        self.abort_task = Some(cx.spawn(async move |_this, cx| {
            cx.background_spawn(async move {
                let url = format!("{BRIDGE_BASE_URL}/adversary/{job}/abort");
                post_json(http_client.as_ref(), &url, "{}".to_string()).await
            })
            .await
            .ok();
        }));
    }

    /// (Re)arm the poll when — and only when — a job id is pending and at
    /// least one panel is watching. Both [`Self::start`]'s POST completion
    /// and [`Self::watch`]'s 0→1 transition funnel through here.
    fn ensure_poll(&mut self, cx: &mut gpui::Context<Self>) {
        if !matches!(self.phase, AdversaryPhase::Pending) {
            return;
        }
        let Some(job) = self.job.clone() else {
            return; // POST still in flight — its completion re-enters here
        };
        if self.watchers.load(Ordering::SeqCst) == 0 {
            return; // hidden panel — the next watch re-enters here
        }
        let http_client = cx.http_client();
        let watchers = self.watchers.clone();
        self.poll_task = Some(cx.spawn(async move |this, cx| {
            job_poll_loop(http_client, job, watchers, this, cx).await
        }));
    }

    /// A terminal `GET /adversary/<job>` snapshot lands: `error` fails,
    /// `cancelled` lands the partial result as a distinct ABORTED phase (P4 —
    /// not a normal Done), and any other non-`running` status renders the
    /// result (the web's `renderResult(j.result)` else-branch — liberal toward
    /// future statuses).
    fn apply_terminal(&mut self, snapshot: AdversaryJobSnapshot, cx: &mut gpui::Context<Self>) {
        self.poll_task = None;
        self.phase = match snapshot.status.as_str() {
            "error" => AdversaryPhase::Failed(
                snapshot
                    .error
                    .unwrap_or_else(|| "unknown error".to_string()),
            ),
            "cancelled" => AdversaryPhase::Cancelled(snapshot.result.unwrap_or_default()),
            _ => AdversaryPhase::Done(snapshot.result.unwrap_or_default()),
        };
        cx.notify();
    }
}

/// The job poll: sleep the web cadence, check the watcher refcount (dies
/// when the panel hides), fetch, and keep going while `running`. Fetch
/// failures keep polling — the web's silent `catch (e) { return; }` tick.
async fn job_poll_loop(
    http_client: Arc<dyn HttpClient>,
    job: String,
    watchers: Arc<AtomicUsize>,
    this: WeakEntity<AdversaryJobs>,
    cx: &mut AsyncApp,
) {
    loop {
        cx.background_executor().timer(JOB_POLL_INTERVAL).await;
        if watchers.load(Ordering::SeqCst) == 0 {
            return; // panel hidden — the next watch respawns the loop
        }
        let client = http_client.clone();
        let url = format!("{BRIDGE_BASE_URL}/adversary/{job}");
        let fetched = cx
            .background_spawn(async move {
                let raw = fetch_json(client.as_ref(), &url).await?;
                anyhow::Ok(serde_json::from_str::<AdversaryJobSnapshot>(&raw)?)
            })
            .await;
        let Ok(snapshot) = fetched else {
            continue; // bridge hiccup — keep polling (web parity)
        };
        if snapshot.status == "running" {
            continue;
        }
        this.update(cx, |this, cx| this.apply_terminal(snapshot, cx))
            .ok();
        return;
    }
}

/// The synthesis judge's `AGREEMENTS: / DISAGREEMENTS: / SYNTHESIS:`
/// sections, split out for the tinted block titles (adversary.js
/// `parseSections`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SynthesisSections {
    pub agreements: String,
    pub disagreements: String,
    pub synthesis: String,
}

/// Port of `parseSections` (`/(AGREEMENTS|DISAGREEMENTS|SYNTHESIS)\s*:/gi`):
/// each section runs from its marker to the next marker; text before the
/// first marker is dropped; no markers at all puts the whole text in
/// `synthesis`; a repeated marker keeps the LAST occurrence's slice.
pub fn parse_synthesis_sections(text: &str) -> SynthesisSections {
    let marks = find_section_marks(text);
    if marks.is_empty() {
        return SynthesisSections {
            synthesis: text.to_string(),
            ..Default::default()
        };
    }
    let mut sections = SynthesisSections::default();
    for (ix, (_, content_start, key)) in marks.iter().enumerate() {
        let end = marks
            .get(ix + 1)
            .map(|(mark_start, ..)| *mark_start)
            .unwrap_or(text.len());
        let body = text[*content_start..end].trim().to_string();
        match *key {
            "AGREEMENTS" => sections.agreements = body,
            "DISAGREEMENTS" => sections.disagreements = body,
            _ => sections.synthesis = body,
        }
    }
    sections
}

/// `(marker start, content start past the colon, KEY)` for every section
/// marker, non-overlapping like the JS global regex (the scan resumes past
/// each match, so the `AGREEMENTS` inside `DISAGREEMENTS` never re-matches).
fn find_section_marks(text: &str) -> Vec<(usize, usize, &'static str)> {
    const KEYS: [&str; 3] = ["AGREEMENTS", "DISAGREEMENTS", "SYNTHESIS"];
    let mut marks = Vec::new();
    let mut char_starts = text.char_indices().map(|(ix, _)| ix).peekable();
    while let Some(ix) = char_starts.next() {
        // Alternation order mirrors the regex; DISAGREEMENTS wins at 'D'
        // because AGREEMENTS fails there immediately.
        let Some(key) = KEYS.iter().find(|key| {
            text.as_bytes()[ix..]
                .get(..key.len())
                .is_some_and(|bytes| bytes.eq_ignore_ascii_case(key.as_bytes()))
        }) else {
            continue;
        };
        // `\s*:` after the keyword.
        let after_key = ix + key.len();
        let rest = &text[after_key..];
        let trimmed = rest.trim_start();
        if !trimmed.starts_with(':') {
            continue;
        }
        let content_start = after_key + (rest.len() - trimmed.len()) + 1;
        marks.push((ix, content_start, *key));
        // Resume past the match (regex lastIndex semantics).
        while char_starts.peek().is_some_and(|&next| next < content_start) {
            char_starts.next();
        }
    }
    marks
}

#[cfg(test)]
#[path = "adversary_tests.rs"]
mod tests;
