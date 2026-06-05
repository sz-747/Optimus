//! glyphon glyph atlas + in-pass text renderer, plus monospace cell metrics (plan §8 U6).
//!
//! The terminal renderer builds one shaped [`Buffer`] per visible row (coalescing same-style
//! cell runs into colored spans), positions each at `row * cell_height`, and hands them to
//! glyphon for a single prepared draw. Cell metrics (advance width + line height in physical
//! pixels) are derived from the font at the current DPI and are the grid's source of truth.

use glyphon::{
    Attrs, Buffer, Cache, Color, Family, FontSystem, Metrics, Resolution, Shaping, Style,
    SwashCache, TextArea, TextAtlas, TextBounds, TextRenderer, Viewport, Weight,
};

use super::RenderError;

/// Base (logical, DPI-unscaled) font size in pixels. Multiplied by the DPI scale for rendering.
const BASE_FONT_SIZE: f32 = 14.0;
/// Line height as a multiple of font size.
const LINE_HEIGHT_RATIO: f32 = 1.2;

/// A run of identically-styled text within a row.
pub struct Span {
    pub text: String,
    pub color: [u8; 4],
    pub bold: bool,
    pub italic: bool,
}

pub struct TextLayer {
    font_system: FontSystem,
    swash_cache: SwashCache,
    viewport: Viewport,
    atlas: TextAtlas,
    renderer: TextRenderer,

    font_size: f32,
    line_height: f32,
    cell_width: f32,
}

impl TextLayer {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        dpi_scale: f32,
    ) -> Self {
        let font_system = FontSystem::new();
        let swash_cache = SwashCache::new();
        let cache = Cache::new(device);
        let viewport = Viewport::new(device, &cache);
        let mut atlas = TextAtlas::new(device, queue, &cache, format);
        let renderer =
            TextRenderer::new(&mut atlas, device, wgpu::MultisampleState::default(), None);

        let mut layer = Self {
            font_system,
            swash_cache,
            viewport,
            atlas,
            renderer,
            font_size: BASE_FONT_SIZE,
            line_height: BASE_FONT_SIZE * LINE_HEIGHT_RATIO,
            cell_width: BASE_FONT_SIZE * 0.6,
        };
        layer.set_scale(dpi_scale);
        layer
    }

    /// Recompute font size + cell metrics for a DPI scale (1.0 = 96 dpi).
    pub fn set_scale(&mut self, dpi_scale: f32) {
        let scale = dpi_scale.max(0.1);
        self.font_size = (BASE_FONT_SIZE * scale).round().max(1.0);
        self.line_height = (self.font_size * LINE_HEIGHT_RATIO).ceil().max(1.0);
        self.cell_width = self.measure_advance().ceil().max(1.0);
    }

    /// Physical-pixel cell size (advance width, line height).
    pub fn cell_size(&self) -> (f32, f32) {
        (self.cell_width, self.line_height)
    }

    fn metrics(&self) -> Metrics {
        Metrics::new(self.font_size, self.line_height)
    }

    /// Measure the monospace advance width by shaping a run of identical glyphs.
    fn measure_advance(&mut self) -> f32 {
        let metrics = self.metrics();
        let mut buffer = Buffer::new(&mut self.font_system, metrics);
        buffer.set_size(&mut self.font_system, Some(10_000.0), Some(metrics.line_height));
        buffer.set_text(
            &mut self.font_system,
            "MMMMMMMMMMMMMMMMMMMM",
            &Attrs::new().family(Family::Monospace),
            Shaping::Advanced,
            None,
        );
        buffer.shape_until_scroll(&mut self.font_system, false);
        for run in buffer.layout_runs() {
            if run.line_w > 0.0 {
                return run.line_w / 20.0;
            }
        }
        self.font_size * 0.6
    }

    /// Shape one row of spans into a positioned [`Buffer`] (width clamps to `px_width`).
    pub fn shape_row(&mut self, spans: &[Span], px_width: f32) -> Buffer {
        let metrics = self.metrics();
        let mut buffer = Buffer::new(&mut self.font_system, metrics);
        buffer.set_size(&mut self.font_system, Some(px_width.max(1.0)), Some(self.line_height));

        let default = Attrs::new().family(Family::Monospace);
        let rich: Vec<(&str, Attrs)> = spans
            .iter()
            .map(|s| {
                let mut attrs = Attrs::new()
                    .family(Family::Monospace)
                    .color(Color::rgba(s.color[0], s.color[1], s.color[2], s.color[3]));
                if s.bold {
                    attrs = attrs.weight(Weight::BOLD);
                }
                if s.italic {
                    attrs = attrs.style(Style::Italic);
                }
                (s.text.as_str(), attrs)
            })
            .collect();

        buffer.set_rich_text(
            &mut self.font_system,
            rich,
            &default,
            Shaping::Advanced,
            None,
        );
        buffer.shape_until_scroll(&mut self.font_system, false);
        buffer
    }

    /// Prepare the glyph draw for this frame. `text_areas` reference buffers from
    /// [`Self::shape_row`] that must stay alive for the duration of this call.
    pub fn prepare<'a>(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
        text_areas: impl IntoIterator<Item = TextArea<'a>>,
    ) -> Result<(), RenderError> {
        self.viewport.update(
            queue,
            Resolution {
                width: width.max(1),
                height: height.max(1),
            },
        );
        self.renderer
            .prepare(
                device,
                queue,
                &mut self.font_system,
                &mut self.atlas,
                &self.viewport,
                text_areas,
                &mut self.swash_cache,
            )
            .map_err(|e| RenderError::Text(format!("{e:?}")))
    }

    /// Draw the prepared glyphs into the pass.
    pub fn render<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) -> Result<(), RenderError> {
        self.renderer
            .render(&self.atlas, &self.viewport, pass)
            .map_err(|e| RenderError::Text(format!("{e:?}")))
    }

    /// Build a full-surface text bounds rectangle.
    pub fn full_bounds(width: u32, height: u32) -> TextBounds {
        TextBounds {
            left: 0,
            top: 0,
            right: width as i32,
            bottom: height as i32,
        }
    }
}
