//! The Artifact surface — agent-emitted HTML/CSS rendered NATIVELY in the
//! center pane (html_surface/Blitz -> BGRA `RenderImage` -> `img()`), no
//! webview. The functional pipeline of the display-panel plan: models emit
//! HTML (their native UI tongue), Blitz rasterizes it (Stylo CSS, ~28ms per
//! artifact), gpui composites the cached texture (zero idle cost).
//!
//! v1 scope: render + cache + `set_html` API for upstream (chat/orchestrator)
//! to feed artifacts. Interactivity (`data-action` -> native handlers) and
//! async off-thread rendering are later increments.

use std::hash::{Hash as _, Hasher as _};
use std::sync::Arc;

use gpui::{App, Context, FocusHandle, Focusable, FontWeight, RenderImage, SharedString, Window};
use html_surface::{BlitzSurface, HtmlSurface as _};
use image::Frame;
use smallvec::SmallVec;
use ui::prelude::*;

/// Fixed logical artifact width (v1): the card column the chat surface uses.
const ARTIFACT_WIDTH: u32 = 680;
/// Content-fit height cap — taller artifacts clamp (scroll container hosts).
const ARTIFACT_MAX_HEIGHT: u32 = 2000;

pub struct ArtifactSurface {
    focus_handle: FocusHandle,
    engine: BlitzSurface,
    html: SharedString,
    rendered: Option<Arc<RenderImage>>,
    /// Device-pixel dims of the cached render (texture size).
    rendered_dims: (u32, u32),
    /// (html-hash, scale-in-millis) the cache was built for.
    rendered_key: Option<(u64, u32)>,
}

impl ArtifactSurface {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            engine: BlitzSurface::new(),
            html: demo_artifact().into(),
            rendered: None,
            rendered_dims: (0, 0),
            rendered_key: None,
        }
    }

    /// Feed a new HTML artifact (the upstream API: chat / orchestrator /
    /// bridge push). Re-render happens lazily on the next frame.
    pub fn set_html(&mut self, html: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.html = html.into();
        cx.notify();
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
                    .child("native HTML render — no webview"),
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
                    h_flex()
                        .w_full()
                        .justify_center()
                        .child(gpui::img(image.clone()).w(px(lw)).h(px(lh))),
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
}

/// The pipeline-proof demo artifact (until upstream feeds real ones): dark
/// card in the design palette exercising gradients, radius, flex, inline SVG,
/// and type hierarchy — the same fidelity axes the standalone spike verified.
fn demo_artifact() -> &'static str {
    r##"<!DOCTYPE html>
<html><head><style>
  * { box-sizing: border-box; margin: 0; padding: 0;
      font-family: 'IBM Plex Sans', 'Segoe UI', sans-serif; }
  body { background: #09090b; padding: 8px; }
  .card { background: #121216; border: 1px solid #26262e; border-radius: 16px;
          padding: 20px 22px; color: #ececef;
          box-shadow: 0 12px 34px rgba(0,0,0,0.55); }
  .head { display: flex; align-items: center; gap: 12px; }
  .logo { width: 40px; height: 40px; }
  .title { font-size: 20px; font-weight: 600; color: #f4f4f6; }
  .sub { font-size: 12px; color: #8f8fa3; margin-top: 2px; }
  .pill { margin-left: auto; font-size: 11px; font-weight: 600; color: #052e16;
          background: linear-gradient(90deg,#34d399,#10b981);
          padding: 5px 11px; border-radius: 999px; }
  .stats { display: flex; gap: 12px; margin-top: 18px; }
  .tile { flex: 1; background: #0d0d11; border: 1px solid #1f1f27;
          border-radius: 12px; padding: 12px 14px; }
  .k { font-size: 11px; color: #6f6f82; text-transform: uppercase;
       letter-spacing: .5px; }
  .v { font-size: 22px; font-weight: 600; color: #8ab4f8; margin-top: 4px; }
  .foot { margin-top: 18px; font-size: 12px; color: #8f8fa3; }
  .vendors { display: flex; gap: 8px; margin-top: 8px; }
  .dot { width: 10px; height: 10px; border-radius: 999px; display: inline-block; }
</style></head>
<body>
  <div class="card">
    <div class="head">
      <svg class="logo" viewBox="0 0 48 48">
        <circle cx="24" cy="24" r="22" fill="#121216" stroke="#8ab4f8" stroke-width="2"/>
        <path d="M14 30 L24 12 L34 30 Z" fill="#a78bfa"/>
        <circle cx="24" cy="27" r="4" fill="#34d399"/>
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
