//! The Artifact surface — agent-emitted HTML/CSS rendered NATIVELY in the
//! center pane (html_surface/Blitz -> BGRA `RenderImage` -> `img()`), no
//! webview. The functional pipeline of the display-panel plan: models emit
//! HTML (their native UI tongue), Blitz rasterizes it (Stylo CSS, ~28ms per
//! artifact), gpui composites the cached texture (zero idle cost).
//!
//! v1 scope: render + cache + `set_html` API, and the LIVE FEED (dogfood
//! 2026-07-08): completed runs are scanned for an HTML payload (fenced
//! ```html block or a full document in the run's result text) — the newest
//! one renders here, whichever vendor produced it. Interactivity
//! (`data-action` -> native handlers) and async off-thread rendering are
//! later increments.

use std::hash::{Hash as _, Hasher as _};
use std::sync::Arc;

use gpui::{
    AnimationExt as _, App, AppContext as _, Context, Entity, FocusHandle, Focusable, FontWeight,
    RenderImage, SharedString, Subscription, Task, Window,
};
use html_surface::{BlitzSurface, HtmlSurface as _};
use image::Frame;
use smallvec::SmallVec;
use ui::prelude::*;

use crate::bridge::{self, BRIDGE_BASE_URL, BridgeStore, fetch_json};

/// Fixed logical artifact width (v1): the card column the chat surface uses.
const ARTIFACT_WIDTH: u32 = 680;
/// Content-fit height cap — taller artifacts clamp (scroll container hosts).
/// Screen-like on purpose (sweep 2026-07-08): agents love `min-height:
/// 100vh`, and Blitz resolves vh against this cap — at 2000 a centered card
/// landed a full viewport below the fold; ~1000 reads as one screen.
const ARTIFACT_MAX_HEIGHT: u32 = 1000;

/// Extract an HTML artifact from a run's result text: a fenced ```html
/// block wins; else a document from `<!DOCTYPE html` / `<html` through the
/// closing `</html>` (or end of text). `None` = the run made no artifact.
pub(crate) fn extract_html(text: &str) -> Option<String> {
    if let Some(start) = text.find("```html") {
        let inner = &text[start + "```html".len()..];
        let inner = inner.strip_prefix('\n').unwrap_or(inner);
        let end = inner.find("```").unwrap_or(inner.len());
        let block = inner[..end].trim();
        if !block.is_empty() {
            return Some(block.to_string());
        }
    }
    let doc_start = text
        .find("<!DOCTYPE html")
        .or_else(|| text.find("<!doctype html"))
        .or_else(|| text.find("<html"))?;
    let tail = &text[doc_start..];
    let doc_end = tail
        .find("</html>")
        .map(|ix| ix + "</html>".len())
        .unwrap_or(tail.len());
    Some(tail[..doc_end].to_string())
}

pub struct ArtifactSurface {
    focus_handle: FocusHandle,
    engine: BlitzSurface,
    html: SharedString,
    rendered: Option<Arc<RenderImage>>,
    /// Device-pixel dims of the cached render (texture size).
    rendered_dims: (u32, u32),
    /// (html-hash, scale-in-millis) the cache was built for.
    rendered_key: Option<(u64, u32)>,
    /// Live feed: the run the current artifact came from (`None` = demo).
    source: Option<SharedString>,
    /// Completed runs already scanned for HTML (scanned once, artifact or
    /// not — a text-only run is never re-fetched).
    scanned: std::collections::HashSet<String>,
    /// Visibility gate (ModeSurface) — the feed scans only while on screen.
    active: bool,
    /// One fetch at a time: a store notify mid-fetch must NOT replace (and
    /// thereby cancel) the in-flight task — the cancelled candidate would
    /// stay `scanned` and never retry. The update tail re-scans, so the
    /// queue drains serially.
    fetch_in_flight: bool,
    store: Entity<BridgeStore>,
    _fetch: Option<Task<()>>,
    _store_subscription: Subscription,
}

impl ArtifactSurface {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let store = bridge::global_store(cx);
        let _store_subscription = cx.observe(&store, |this: &mut Self, _, cx| {
            if this.active {
                this.scan_for_artifacts(cx);
            }
        });
        Self {
            focus_handle: cx.focus_handle(),
            engine: BlitzSurface::new(),
            html: demo_artifact().into(),
            rendered: None,
            rendered_dims: (0, 0),
            rendered_key: None,
            source: None,
            scanned: std::collections::HashSet::new(),
            active: false,
            fetch_in_flight: false,
            store,
            _fetch: None,
            _store_subscription,
        }
    }

    /// Feed a new HTML artifact (the upstream API: chat / orchestrator /
    /// bridge push). Re-render happens lazily on the next frame.
    pub fn set_html(&mut self, html: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.html = html.into();
        cx.notify();
    }

    /// The live feed (dogfood 2026-07-08): fetch the newest unscanned
    /// completed run and adopt its HTML payload if it has one. One fetch at
    /// a time; every completed run is scanned exactly once.
    fn scan_for_artifacts(&mut self, cx: &mut Context<Self>) {
        if self.fetch_in_flight {
            return; // the in-flight fetch's tail re-scans (serial drain)
        }
        let candidate = {
            let store = self.store.read(cx);
            store
                .board
                .iter()
                .rev()
                .find(|run| run.status == "completed" && !self.scanned.contains(&run.run_id))
                .map(|run| run.run_id.clone())
        };
        let Some(run_id) = candidate else { return };
        self.scanned.insert(run_id.clone());
        self.fetch_in_flight = true;
        let client = cx.http_client();
        let fetch_id = run_id.clone();
        self._fetch = Some(cx.spawn(async move |this, cx| {
            let fetched = cx
                .background_spawn(async move {
                    let url = format!("{BRIDGE_BASE_URL}/run/{fetch_id}");
                    let raw = fetch_json(client.as_ref(), &url).await?;
                    anyhow::Ok(serde_json::from_str::<serde_json::Value>(&raw)?)
                })
                .await;
            let value = match fetched {
                Ok(value) => value,
                Err(_) => {
                    // Fetch failed — release the gate so later scans run.
                    this.update(cx, |this, _| this.fetch_in_flight = false).ok();
                    return;
                }
            };
            let html = value
                .get("text")
                .and_then(|t| t.as_str())
                .and_then(extract_html);
            let agent = value
                .get("agent")
                .and_then(|a| a.as_str())
                .unwrap_or_default()
                .to_string();
            this.update(cx, |this, cx| {
                this.fetch_in_flight = false;
                if let Some(html) = html {
                    this.html = html.into();
                    this.source = Some(format!("{agent} · {run_id}").into());
                    cx.notify();
                } else {
                    // No artifact in this run — scan the next candidate.
                    this.scan_for_artifacts(cx);
                }
            })
            .ok();
        }));
    }

    /// Render-on-change: rasterize the artifact when (html, scale) differs
    /// from the cache; otherwise the cached texture is composited for free.
    /// The ~28ms rasterize blocks one frame per CONTENT CHANGE only —
    /// acceptable v1; async render is a later increment.
    fn ensure_rendered(&mut self, scale: f32) {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.html.hash(&mut hasher);
        let key = (hasher.finish(), (scale * 1000.0) as u32);
        if self.rendered_key == Some(key) {
            return;
        }
        let out =
            self.engine
                .render_fit_height(&self.html, ARTIFACT_WIDTH, ARTIFACT_MAX_HEIGHT, scale);
        // gpui textures are BGRA (`RenderImage` docs) — swizzle Blitz's RGBA.
        let mut bytes = out.rgba;
        for px in bytes.chunks_exact_mut(4) {
            px.swap(0, 2);
        }
        let buffer = image::RgbaImage::from_raw(out.width, out.height, bytes)
            .expect("blitz buffer len == w*h*4");
        self.rendered = Some(Arc::new(RenderImage::new(SmallVec::from_elem(
            Frame::new(buffer),
            1,
        ))));
        self.rendered_dims = (out.width, out.height);
        self.rendered_key = Some(key);
    }

    fn render_header(&self, cx: &App) -> Div {
        let colors = cx.theme().colors();
        h_flex()
            .flex_none()
            .items_baseline()
            .gap(px(12.))
            .px(px(28.))
            .pt(px(18.))
            .pb(px(14.))
            .border_b_1()
            .border_color(colors.border)
            .child(
                div()
                    .text_size(px(18.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(colors.text)
                    .child("Artifact"),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(colors.text_muted)
                    .child(match &self.source {
                        Some(source) => source.clone(),
                        None => "demo — completed runs that emit HTML render here".into(),
                    }),
            )
    }
}

impl Render for ArtifactSurface {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let scale = window.scale_factor();
        self.ensure_rendered(scale);
        let colors = cx.theme().colors();
        let (dw, dh) = self.rendered_dims;
        // Display at logical size: device-pixel texture / scale = 1:1 pixels.
        let (lw, lh) = (dw as f32 / scale, dh as f32 / scale);
        let body: AnyElement = match &self.rendered {
            Some(image) => div()
                .id("artifact-scroll")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .px(px(28.))
                .pt(px(20.))
                .pb(px(32.))
                .child(
                    h_flex().w_full().justify_center().child(
                        // Artifact swap animation (user theme: "webview
                        // animations"): each new render fades in 300ms,
                        // keyed on the cache key so a settled artifact
                        // never replays.
                        gpui::img(image.clone())
                            .w(px(lw))
                            .h(px(lh))
                            .with_animation(
                                gpui::ElementId::NamedInteger(
                                    "artifact-swap".into(),
                                    self.rendered_key.map(|(hash, _)| hash).unwrap_or(0),
                                ),
                                gpui::Animation::new(std::time::Duration::from_millis(300))
                                    .with_easing(crate::task_board::motion::EFFECTS.easing()),
                                |img, t| img.opacity(t),
                            ),
                    ),
                )
                .into_any_element(),
            None => div().flex_1().into_any_element(),
        };
        v_flex()
            .key_context("ArtifactSurface")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(colors.panel_background)
            .child(self.render_header(cx))
            .child(body)
    }
}

impl Focusable for ArtifactSurface {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl crate::mode_item::ModeSurface for ArtifactSurface {
    fn fallback_tab_title() -> SharedString {
        "Artifact".into()
    }

    fn fallback_tab_icon() -> IconName {
        IconName::Eye
    }

    fn set_surface_active(&mut self, active: bool, cx: &mut Context<Self>) {
        self.active = active;
        if active {
            self.scan_for_artifacts(cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::extract_html;

    #[test]
    fn fenced_html_block_wins() {
        let text = "here is your artifact:\n```html\n<div>hi</div>\n```\ndone";
        assert_eq!(extract_html(text).as_deref(), Some("<div>hi</div>"));
    }

    #[test]
    fn full_document_extracts_to_closing_tag() {
        let text = "prose <!DOCTYPE html><html><body>x</body></html> trailing";
        assert_eq!(
            extract_html(text).as_deref(),
            Some("<!DOCTYPE html><html><body>x</body></html>")
        );
    }

    #[test]
    fn bare_html_tag_without_close_takes_tail() {
        let text = "note: <html><body>partial";
        assert_eq!(extract_html(text).as_deref(), Some("<html><body>partial"));
    }

    #[test]
    fn plain_text_is_no_artifact() {
        assert_eq!(extract_html("just a normal reply"), None);
        assert_eq!(extract_html("```html\n\n```"), None);
    }
}

/// The pipeline-proof demo artifact (until upstream feeds real ones): dark
/// card in the design palette exercising gradients, radius, flex, inline SVG,
/// and type hierarchy — the same fidelity axes the standalone spike verified.
fn demo_artifact() -> &'static str {
    r##"<!DOCTYPE html>
<html><head><style>
  * { box-sizing: border-box; margin: 0; padding: 0;
      font-family: 'IBM Plex Sans', 'Segoe UI', sans-serif; }
  body { background: #0a0a0a; padding: 8px; }
  .card { background: #121212; border: 1px solid #282828; border-radius: 16px;
          padding: 20px 22px; color: #ededed;
          box-shadow: 0 12px 34px rgba(0,0,0,0.55); }
  .head { display: flex; align-items: center; gap: 12px; }
  .logo { width: 40px; height: 40px; }
  .title { font-size: 20px; font-weight: 600; color: #f5f5f5; }
  .sub { font-size: 12px; color: #8f8f8f; margin-top: 2px; }
  .pill { margin-left: auto; font-size: 11px; font-weight: 600; color: #0a0a0a;
          background: linear-gradient(90deg,#f5f5f5,#d4d4d4);
          padding: 5px 11px; border-radius: 999px; }
  .stats { display: flex; gap: 12px; margin-top: 18px; }
  .tile { flex: 1; background: #0d0d0d; border: 1px solid #1f1f1f;
          border-radius: 12px; padding: 12px 14px; }
  .k { font-size: 11px; color: #6f6f6f; text-transform: uppercase;
       letter-spacing: .5px; }
  .v { font-size: 22px; font-weight: 600; color: #e5e5e5; margin-top: 4px; }
  .foot { margin-top: 18px; font-size: 12px; color: #8f8f8f; }
  .vendors { display: flex; gap: 8px; margin-top: 8px; }
  .dot { width: 10px; height: 10px; border-radius: 999px; display: inline-block; }
</style></head>
<body>
  <div class="card">
    <div class="head">
      <svg class="logo" viewBox="0 0 48 48">
        <circle cx="24" cy="24" r="22" fill="#121212" stroke="#e5e5e5" stroke-width="2"/>
        <path d="M14 30 L24 12 L34 30 Z" fill="#a3a3a3"/>
        <circle cx="24" cy="27" r="4" fill="#f5f5f5"/>
      </svg>
      <div>
        <div class="title">Artifact Surface</div>
        <div class="sub">Blitz native render &middot; Stylo CSS &middot; zero webview</div>
      </div>
      <div class="pill">LIVE</div>
    </div>
    <div class="stats">
      <div class="tile"><div class="k">Raster</div><div class="v">~28ms</div></div>
      <div class="tile"><div class="k">Idle</div><div class="v">0ms</div></div>
      <div class="tile"><div class="k">Engine</div><div class="v">CPU</div></div>
    </div>
    <div class="foot">Vendors
      <div class="vendors">
        <span class="dot" style="background:#da7756"></span>
        <span class="dot" style="background:#10a37f"></span>
        <span class="dot" style="background:#8ab4f8"></span>
        <span class="dot" style="background:#a78bfa"></span>
      </div>
    </div>
  </div>
</body></html>"##
}
