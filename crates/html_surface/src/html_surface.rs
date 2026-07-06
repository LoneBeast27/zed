//! The HtmlSurface seam — agent-emitted HTML/CSS to native pixels, no webview.
//!
//! Models speak HTML fluently (it rides the training distribution), so the
//! artifact language is HTML/CSS rendered NATIVELY: Blitz (Stylo + Taffy +
//! Parley + vello_cpu) parses, styles, lays out, and rasterizes into an RGBA
//! buffer the panel wraps into a gpui `RenderImage`. Interactivity comes later
//! via `data-action` attributes mapped to native handlers — never JS.
//!
//! The trait is the swap point: if pre-alpha Blitz bites, a Servo-backed
//! implementation slots in behind the same contract without touching consumers.

use anyrender::render_to_buffer;
use anyrender_vello_cpu::VelloCpuImageRenderer;
use blitz_dom::DocumentConfig;
use blitz_html::HtmlDocument;
use blitz_paint::paint_scene;
use blitz_traits::shell::{ColorScheme, Viewport};

/// A rasterized HTML artifact: tightly-packed RGBA8, row-major.
pub struct RenderedHtml {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Render agent-emitted HTML/CSS into pixels at a device scale.
///
/// `width`/`height` are LOGICAL pixels; the buffer comes back at
/// `width*scale x height*scale` device pixels (the returned dims are the
/// device ones — feed them straight to the texture).
pub trait HtmlSurface {
    fn render(&mut self, html: &str, width: u32, height: u32, scale: f32) -> RenderedHtml;

    /// Render at `width`, letting content determine height (clamped to
    /// `max_height` logical px) — the common chat-artifact shape where the
    /// card is as tall as its content.
    fn render_fit_height(
        &mut self,
        html: &str,
        width: u32,
        max_height: u32,
        scale: f32,
    ) -> RenderedHtml;
}

/// Blitz-backed implementation. Stateless per render (a fresh parse + layout +
/// paint measures ~28ms for a real mockup card — cheap enough that document
/// caching is a later optimization, not a launch requirement).
#[derive(Default)]
pub struct BlitzSurface;

impl BlitzSurface {
    pub fn new() -> Self {
        Self
    }

    fn document(html: &str, dw: u32, dh: u32, scale: f32) -> HtmlDocument {
        let mut doc = HtmlDocument::from_html(
            html,
            DocumentConfig {
                viewport: Some(Viewport::new(dw, dh, scale, ColorScheme::Dark)),
                ..Default::default()
            },
        );
        doc.as_mut().resolve(0.0);
        doc
    }

    fn paint(doc: &mut HtmlDocument, dw: u32, dh: u32, scale: f32) -> RenderedHtml {
        let rgba = render_to_buffer::<VelloCpuImageRenderer, _>(
            |scene| paint_scene(scene, doc.as_mut(), scale as f64, dw, dh, 0, 0),
            dw,
            dh,
        );
        RenderedHtml {
            rgba,
            width: dw,
            height: dh,
        }
    }
}

impl HtmlSurface for BlitzSurface {
    fn render(&mut self, html: &str, width: u32, height: u32, scale: f32) -> RenderedHtml {
        let (dw, dh) = (scale_dim(width, scale), scale_dim(height, scale));
        let mut doc = Self::document(html, dw, dh, scale);
        Self::paint(&mut doc, dw, dh, scale)
    }

    fn render_fit_height(
        &mut self,
        html: &str,
        width: u32,
        max_height: u32,
        scale: f32,
    ) -> RenderedHtml {
        let dw = scale_dim(width, scale);
        // Lay out against the max height first, then shrink to content.
        let mut doc = Self::document(html, dw, scale_dim(max_height, scale), scale);
        let content = doc.as_ref().root_element().final_layout.size.height;
        let logical = (content.ceil() as u32).clamp(1, max_height);
        let dh = scale_dim(logical, scale);
        Self::paint(&mut doc, dw, dh, scale)
    }
}

fn scale_dim(logical: u32, scale: f32) -> u32 {
    ((logical as f32) * scale).round().max(1.0) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Solid-color probe: sample the buffer center and assert the CSS
    /// background actually landed (engine engaged, not a blank buffer).
    fn center_pixel(r: &RenderedHtml) -> [u8; 4] {
        let (x, y) = (r.width / 2, r.height / 2);
        let i = ((y * r.width + x) * 4) as usize;
        [r.rgba[i], r.rgba[i + 1], r.rgba[i + 2], r.rgba[i + 3]]
    }

    #[test]
    fn renders_css_background_to_pixels() {
        let mut s = BlitzSurface::new();
        let out = s.render(
            "<html><body style=\"background:#ff0080;margin:0\"></body></html>",
            64,
            64,
            1.0,
        );
        assert_eq!(out.width, 64);
        assert_eq!(out.height, 64);
        assert_eq!(out.rgba.len(), 64 * 64 * 4);
        let px = center_pixel(&out);
        assert_eq!((px[0], px[1], px[2]), (0xff, 0x00, 0x80));
    }

    #[test]
    fn scale_produces_device_pixels() {
        let mut s = BlitzSurface::new();
        let out = s.render("<div>hi</div>", 100, 50, 2.0);
        assert_eq!((out.width, out.height), (200, 100));
        assert_eq!(out.rgba.len(), 200 * 100 * 4);
    }

    #[test]
    fn fit_height_shrinks_to_content() {
        let mut s = BlitzSurface::new();
        // One 40px-tall block in a 400px max viewport -> height ~40, not 400.
        let out = s.render_fit_height(
            "<html><body style=\"margin:0\"><div style=\"height:40px;background:#123456\"></div></body></html>",
            120,
            400,
            1.0,
        );
        assert_eq!(out.width, 120);
        assert!(
            out.height >= 40 && out.height < 100,
            "content-fit height was {}",
            out.height
        );
    }

    #[test]
    fn fit_height_clamps_to_max() {
        let mut s = BlitzSurface::new();
        let out = s.render_fit_height(
            "<html><body style=\"margin:0\"><div style=\"height:9000px\"></div></body></html>",
            80,
            300,
            1.0,
        );
        assert_eq!(out.height, 300);
    }
}
