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

/// Preferred terminal font families, best first (plan Phase 6 — renderer polish).
///
/// `Family::Monospace` resolves through fontdb's *default* monospace, which on Windows is
/// Courier New — no ligatures, dated metrics. We instead probe for the Cascadia family
/// (ships with Windows 11 / Windows Terminal; Cascadia Code carries the `calt` programming
/// ligatures that `Shaping::Advanced` already shapes), falling back to Consolas, and only
/// then to the generic monospace. Glyphs *outside* the chosen family (emoji, CJK, box
/// drawing) still resolve through cosmic-text's per-script fallback list, which on Windows
/// includes Segoe UI Emoji (color) and the Microsoft CJK fonts.
const PREFERRED_MONOSPACE: &[&str] = &["Cascadia Code", "Cascadia Mono", "Consolas"];

/// Pick the first family from [`PREFERRED_MONOSPACE`] present in the font database.
/// `None` means "use the generic `Family::Monospace`".
pub(crate) fn select_monospace_family(
    db: &glyphon::cosmic_text::fontdb::Database,
) -> Option<String> {
    use glyphon::cosmic_text::fontdb::{Family as DbFamily, Query};
    PREFERRED_MONOSPACE.iter().find_map(|name| {
        db.query(&Query {
            families: &[DbFamily::Name(name)],
            ..Query::default()
        })
        .map(|_| (*name).to_string())
    })
}

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

    /// Resolved preferred family name (see [`PREFERRED_MONOSPACE`]); `None` falls back to
    /// the generic `Family::Monospace`.
    family: Option<String>,
    font_size: f32,
    line_height: f32,
    cell_width: f32,
}

/// The [`Family`] attr for an optional resolved family name.
fn family_attr(family: &Option<String>) -> Family<'_> {
    match family {
        Some(name) => Family::Name(name),
        None => Family::Monospace,
    }
}

impl TextLayer {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        dpi_scale: f32,
    ) -> Self {
        let font_system = FontSystem::new();
        let family = select_monospace_family(font_system.db());
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
            family,
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
        // The cell advance MUST equal the advance glyphon lays glyphs out with — NOT a ceil'd
        // value. The grid (cursor block, cell backgrounds, selection) and the mouse→column
        // hit-test all place column `c` at `c * cell_width`, while glyphon places glyph `c` of a
        // row's shaped buffer at `c * advance`. Rounding `cell_width` up makes the two diverge by
        // a fraction of a pixel per column, which compounds across a row until the cursor block
        // and selection sit a full cell to the right of the text. Keeping the exact advance makes
        // `c * cell_width` coincide with glyphon's glyph origin for every monospace column.
        self.cell_width = self.measure_advance().max(1.0);
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
        let fallback = self.font_size * 0.6;
        // Disjoint borrows: the family name is read while the font system is borrowed mutably.
        let Self {
            font_system,
            family,
            ..
        } = self;
        let mut buffer = Buffer::new(font_system, metrics);
        buffer.set_size(font_system, Some(10_000.0), Some(metrics.line_height));
        buffer.set_text(
            font_system,
            "MMMMMMMMMMMMMMMMMMMM",
            &Attrs::new().family(family_attr(family)),
            Shaping::Advanced,
            None,
        );
        buffer.shape_until_scroll(font_system, false);
        for run in buffer.layout_runs() {
            if run.line_w > 0.0 {
                return run.line_w / 20.0;
            }
        }
        fallback
    }

    /// Shape one row of spans into a positioned [`Buffer`] (width clamps to `px_width`).
    pub fn shape_row(&mut self, spans: &[Span], px_width: f32) -> Buffer {
        let metrics = self.metrics();
        let line_height = self.line_height;
        // Disjoint borrows: the family name is read while the font system is borrowed mutably.
        let Self {
            font_system,
            family,
            ..
        } = self;
        let mut buffer = Buffer::new(font_system, metrics);
        buffer.set_size(font_system, Some(px_width.max(1.0)), Some(line_height));

        let fam = family_attr(family);
        let default = Attrs::new().family(fam);
        let rich: Vec<(&str, Attrs)> = spans
            .iter()
            .map(|s| {
                let mut attrs = Attrs::new()
                    .family(fam)
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
            font_system,
            rich,
            &default,
            Shaping::Advanced,
            None,
        );
        buffer.shape_until_scroll(font_system, false);
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

#[cfg(test)]
mod tests {
    //! Headless shaping tests — cosmic-text shaping needs no GPU device, so the font
    //! fallback chain and grid-advance invariants are verified without a swapchain.

    use super::*;

    fn shape(font_system: &mut FontSystem, text: &str, family: &Option<String>) -> Buffer {
        let metrics = Metrics::new(BASE_FONT_SIZE, BASE_FONT_SIZE * LINE_HEIGHT_RATIO);
        let mut buffer = Buffer::new(font_system, metrics);
        buffer.set_size(font_system, Some(10_000.0), Some(metrics.line_height));
        buffer.set_text(
            font_system,
            text,
            &Attrs::new().family(family_attr(family)),
            Shaping::Advanced,
            None,
        );
        buffer.shape_until_scroll(font_system, false);
        buffer
    }

    fn line_width(buffer: &Buffer) -> f32 {
        buffer.layout_runs().map(|r| r.line_w).fold(0.0, f32::max)
    }

    #[test]
    fn preferred_family_resolves_on_windows() {
        // Consolas ships with every supported Windows, so the chain must resolve.
        let font_system = FontSystem::new();
        let family = select_monospace_family(font_system.db());
        assert!(
            family.is_some(),
            "no preferred monospace family found; PREFERRED_MONOSPACE = {PREFERRED_MONOSPACE:?}"
        );
    }

    #[test]
    fn emoji_falls_back_to_a_glyph() {
        // The preferred monospace family has no emoji coverage; cosmic-text's per-script
        // fallback (Segoe UI Emoji on Windows) must still produce a positive-width glyph
        // rather than a tofu-less empty run.
        let mut font_system = FontSystem::new();
        let family = select_monospace_family(font_system.db());
        let buffer = shape(&mut font_system, "\u{1F642}", &family); // 🙂
        let glyphs: usize = buffer.layout_runs().map(|r| r.glyphs.len()).sum();
        assert!(glyphs >= 1, "emoji produced no glyphs");
        assert!(line_width(&buffer) > 0.0, "emoji shaped to zero width");
    }

    #[test]
    fn ligature_candidates_keep_grid_advance() {
        // With Shaping::Advanced and a ligature-bearing font (Cascadia Code), "->" may
        // shape into a single ligature glyph — but its total advance MUST stay exactly two
        // cells, or the cursor/selection grid drifts off the text (see set_scale).
        let mut font_system = FontSystem::new();
        let family = select_monospace_family(font_system.db());
        let single = line_width(&shape(&mut font_system, "M", &family));
        let arrow = line_width(&shape(&mut font_system, "->", &family));
        assert!(single > 0.0);
        assert!(
            (arrow - 2.0 * single).abs() < 0.01,
            "'->' advance {arrow} != 2 cells ({})",
            2.0 * single
        );
    }

    #[test]
    fn preferred_family_advance_matches_generic_shaping_path() {
        // The advance measured at startup (measure_advance's "MMMM…" run) is the grid's
        // source of truth; per-row shaping must lay out at the same advance per column.
        let mut font_system = FontSystem::new();
        let family = select_monospace_family(font_system.db());
        let twenty = line_width(&shape(&mut font_system, &"M".repeat(20), &family)) / 20.0;
        let two = line_width(&shape(&mut font_system, "MM", &family)) / 2.0;
        assert!(twenty > 0.0);
        assert!(
            (twenty - two).abs() < 0.01,
            "advance differs between runs: {twenty} vs {two}"
        );
    }
}
