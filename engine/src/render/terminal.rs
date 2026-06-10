//! The terminal renderer (plan §8 U6): composes the surface, the instanced-quad layer
//! (backgrounds + cursor), and the glyphon text layer into one frame driven by a
//! `wezterm-term` `Screen`.
//!
//! Cells are placed on a fixed grid (`cell_width` × `line_height`, both physical pixels).
//! Each visible row is rasterized as: per-cell background quads, then a block cursor quad,
//! then one shaped text buffer (same-style cell runs coalesced into colored spans) drawn on
//! top. The grid dimensions reported by [`TerminalRenderer::cell_size`] are the engine's
//! source of truth for computing cols/rows on resize.

use std::ffi::c_void;

use glyphon::{Color, TextArea};
use termwiz::color::{ColorAttribute, SrgbaTuple};
use termwiz::surface::CursorVisibility;
use wezterm_term::Terminal;

use super::grid::{QuadInstance, QuadLayer};
use super::text::{Span, TextLayer};
use super::{PanelRenderer, RenderError};

/// Selection highlight color, sRGB-encoded RGBA (translucent — drawn behind glyphs, written
/// verbatim to the UNORM surface like the other quad colors).
const SELECTION_COLOR: [f32; 4] = [0.20, 0.40, 0.85, 0.40];

/// A normalized text selection in stable-row coordinates (survives scrolling/output).
///
/// `start` precedes `end` in (row, col) order; `end_col` is inclusive (the last selected cell).
#[derive(Clone, Copy)]
pub struct SelectionSpan {
    pub start_row: i64,
    pub start_col: usize,
    pub end_row: i64,
    pub end_col: usize,
}

impl SelectionSpan {
    /// The exclusive column range selected on stable row `stable` (clamped by the caller to
    /// the grid width). The inclusive `end_col` becomes an exclusive `+1`.
    pub fn column_range(&self, stable: i64, cols: usize) -> (usize, usize) {
        if self.start_row == self.end_row {
            (self.start_col, self.end_col + 1)
        } else if stable == self.start_row {
            (self.start_col, cols)
        } else if stable == self.end_row {
            (0, self.end_col + 1)
        } else {
            (0, cols)
        }
    }
}

/// A shaped row buffer retained across frames, tagged with a key over the row's visible
/// appearance. Reused when the key is unchanged so only rows that actually changed are
/// re-shaped — text shaping is the dominant per-frame cost (plan §8 U6).
struct CachedRow {
    key: u64,
    buffer: glyphon::Buffer,
}

pub struct TerminalRenderer {
    panel: PanelRenderer,
    quads: QuadLayer,
    text: TextLayer,
    /// Shaped-buffer cache, indexed by viewport row (0 = top visible row). Slots are `None`
    /// only transiently while a frame is being built. Cleared on resize/DPI change.
    row_cache: Vec<Option<CachedRow>>,
}

impl TerminalRenderer {
    /// # Safety
    /// `panel_ptr` must be a valid, live `ISwapChainPanel*` for the renderer's lifetime.
    pub unsafe fn new(
        panel_ptr: *mut c_void,
        width: u32,
        height: u32,
        dpi_scale: f32,
    ) -> Result<Self, RenderError> {
        let panel = unsafe { PanelRenderer::new(panel_ptr, width, height, dpi_scale)? };
        let quads = QuadLayer::new(&panel.device, panel.format(), width, height);
        let text = TextLayer::new(&panel.device, &panel.queue, panel.format(), dpi_scale);
        Ok(Self {
            panel,
            quads,
            text,
            row_cache: Vec::new(),
        })
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.panel.resize(width, height);
        self.quads.set_resolution(&self.panel.queue, width, height);
        // Shaping width + geometry changed: every cached row is stale.
        self.row_cache.clear();
    }

    pub fn set_scale(&mut self, dpi_scale: f32) {
        self.text.set_scale(dpi_scale);
        // Cancel the panel's composition-scale magnification at the new scale (plan §5.2).
        self.panel.set_composition_scale(dpi_scale);
        // Font size changed: re-shape every row at the new metrics.
        self.row_cache.clear();
    }

    /// Physical-pixel cell size (advance width, line height) — used to derive cols/rows.
    pub fn cell_size(&self) -> (f32, f32) {
        self.text.cell_size()
    }

    /// Render one frame from the terminal's current screen.
    ///
    /// `scroll_offset` is how many lines the viewport is scrolled back from the bottom;
    /// `cursor_blink_on` gates the block cursor (blink phase).
    pub fn render(
        &mut self,
        terminal: &Terminal,
        scroll_offset: usize,
        cursor_blink_on: bool,
        selection: Option<SelectionSpan>,
    ) -> Result<(), RenderError> {
        // Disjoint borrows: the row cache is mutated while `text` shapes and `panel` presents.
        let Self {
            panel,
            quads: quad_layer,
            text,
            row_cache,
        } = self;

        let (cell_w, cell_h) = text.cell_size();
        let (surf_w, surf_h) = panel.size();

        let palette = terminal.palette();
        let default_fg = palette.resolve_fg(ColorAttribute::Default);
        let default_bg = palette.resolve_bg(ColorAttribute::Default);
        let default_fg_u8 = srgb_u8(default_fg);
        let default_bg_u8 = srgb_u8(default_bg);

        let screen = terminal.screen();
        let cols = screen.physical_cols;
        let visible = screen.physical_rows;
        let total = screen.scrollback_rows();
        let bottom = total.saturating_sub(scroll_offset);
        let start = bottom.saturating_sub(visible);
        let lines = screen.lines_in_phys_range(start..bottom);

        // Cursor (only when at the live bottom and not hidden).
        let cursor = terminal.cursor_pos();
        let cursor_visible = scroll_offset == 0
            && cursor_blink_on
            && matches!(cursor.visibility, CursorVisibility::Visible);
        let cursor_col = cursor.x;
        let cursor_row = cursor.y.max(0) as usize;

        let mut quads: Vec<QuadInstance> = Vec::new();

        // Reuse last frame's shaped buffers; only re-shape rows whose appearance changed. The
        // quads (backgrounds, cursor block, selection) are cheap and rebuilt every frame.
        let mut old_cache: Vec<Option<CachedRow>> = std::mem::take(row_cache);
        let mut new_cache: Vec<Option<CachedRow>> = Vec::with_capacity(lines.len());

        for (r, line) in lines.iter().enumerate() {
            let y = r as f32 * cell_h;

            // Per-column cell snapshot (default-filled so gaps stay aligned).
            let mut glyphs: Vec<String> = vec![String::from(" "); cols];
            let mut fg: Vec<[u8; 4]> = vec![default_fg_u8; cols];
            let mut bold: Vec<bool> = vec![false; cols];
            let mut italic: Vec<bool> = vec![false; cols];

            for cell in line.visible_cells() {
                let col = cell.cell_index();
                if col >= cols {
                    continue;
                }
                let attrs = cell.attrs();
                let mut cell_fg = palette.resolve_fg(attrs.foreground());
                let mut cell_bg = palette.resolve_bg(attrs.background());
                if attrs.reverse() {
                    std::mem::swap(&mut cell_fg, &mut cell_bg);
                }
                let cell_fg_u8 = srgb_u8(cell_fg);
                let cell_bg_u8 = srgb_u8(cell_bg);
                let is_bold = matches!(attrs.intensity(), wezterm_term::Intensity::Bold);

                glyphs[col] = cell.str().to_string();
                fg[col] = cell_fg_u8;
                bold[col] = is_bold;
                italic[col] = attrs.italic();

                // Background quad when not the default background.
                if cell_bg_u8 != default_bg_u8 {
                    let w = cell.width().max(1) as f32;
                    quads.push(QuadInstance::new(
                        col as f32 * cell_w,
                        y,
                        cell_w * w,
                        cell_h,
                        quad_color(cell_bg),
                    ));
                }

                // Clear continuation columns of a wide cell so glyphs aren't duplicated.
                for w in 1..cell.width() {
                    if col + w < cols {
                        glyphs[col + w] = String::new();
                    }
                }
            }

            // Selection highlight (translucent, drawn over backgrounds and under glyphs).
            if let Some(sel) = &selection {
                let stable = screen.phys_to_stable_row_index(start + r) as i64;
                if stable >= sel.start_row && stable <= sel.end_row {
                    let (c0, c1) = sel.column_range(stable, cols);
                    let c1 = c1.min(cols);
                    if c1 > c0 {
                        quads.push(QuadInstance::new(
                            c0 as f32 * cell_w,
                            y,
                            (c1 - c0) as f32 * cell_w,
                            cell_h,
                            SELECTION_COLOR,
                        ));
                    }
                }
            }

            // Block cursor: a foreground-colored quad with the glyph re-colored to the bg.
            if cursor_visible && r == cursor_row && cursor_col < cols {
                quads.push(QuadInstance::new(
                    cursor_col as f32 * cell_w,
                    y,
                    cell_w,
                    cell_h,
                    quad_color(default_fg),
                ));
                fg[cursor_col] = default_bg_u8;
            }

            // Coalesce adjacent same-style columns into spans.
            let mut spans: Vec<Span> = Vec::new();
            let mut run = String::new();
            let mut run_fg = fg.first().copied().unwrap_or(default_fg_u8);
            let mut run_bold = bold.first().copied().unwrap_or(false);
            let mut run_italic = italic.first().copied().unwrap_or(false);
            for col in 0..cols {
                if glyphs[col].is_empty() {
                    continue; // wide-cell continuation
                }
                let same = fg[col] == run_fg && bold[col] == run_bold && italic[col] == run_italic;
                if !same && !run.is_empty() {
                    spans.push(Span {
                        text: std::mem::take(&mut run),
                        color: run_fg,
                        bold: run_bold,
                        italic: run_italic,
                    });
                }
                if run.is_empty() {
                    run_fg = fg[col];
                    run_bold = bold[col];
                    run_italic = italic[col];
                }
                run.push_str(&glyphs[col]);
            }
            if !run.is_empty() {
                spans.push(Span {
                    text: run,
                    color: run_fg,
                    bold: run_bold,
                    italic: run_italic,
                });
            }

            // Reuse the previous frame's buffer for this row if its appearance is unchanged;
            // otherwise shape it now. (`matches!` ends the immutable borrow before the `take`.)
            let key = row_appearance_key(&spans, surf_w);
            let reuse = if matches!(old_cache.get(r), Some(Some(c)) if c.key == key) {
                old_cache[r].take()
            } else {
                None
            };
            let cached = reuse.unwrap_or_else(|| CachedRow {
                key,
                buffer: text.shape_row(&spans, surf_w as f32),
            });
            new_cache.push(Some(cached));
        }

        // Build text areas referencing the (reused or freshly shaped) row buffers.
        let bounds = TextLayer::full_bounds(surf_w, surf_h);
        let areas: Vec<TextArea> = new_cache
            .iter()
            .enumerate()
            .map(|(r, slot)| {
                let buffer = &slot
                    .as_ref()
                    .expect("every row slot is populated above")
                    .buffer;
                TextArea {
                    buffer,
                    left: 0.0,
                    top: r as f32 * cell_h,
                    scale: 1.0,
                    bounds,
                    default_color: Color::rgba(
                        default_fg_u8[0],
                        default_fg_u8[1],
                        default_fg_u8[2],
                        default_fg_u8[3],
                    ),
                    custom_glyphs: &[],
                }
            })
            .collect();

        quad_layer.upload(&panel.device, &panel.queue, &quads);
        text.prepare(&panel.device, &panel.queue, surf_w, surf_h, areas)?;

        // Retain the shaped buffers for next frame's reuse (`areas` was consumed by `prepare`).
        *row_cache = new_cache;

        // Acquire the frame and draw: clear → backgrounds/cursor → glyphs.
        let frame = match panel.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) | wgpu::CurrentSurfaceTexture::Suboptimal(t) => {
                t
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                panel.reconfigure();
                return Err(RenderError::FrameUnavailable("surface outdated/lost; reconfigured"));
            }
            wgpu::CurrentSurfaceTexture::Timeout => {
                return Err(RenderError::FrameUnavailable("acquire timeout"))
            }
            wgpu::CurrentSurfaceTexture::Occluded => {
                return Err(RenderError::FrameUnavailable("surface occluded"))
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                return Err(RenderError::FrameUnavailable("validation error"))
            }
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = panel
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("optimus frame encoder"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("optimus terminal pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(srgba_to_wgpu_clear(default_bg)),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            quad_layer.draw(&mut pass, quads.len() as u32);
            text.render(&mut pass)?;
        }
        panel.queue.submit(std::iter::once(encoder.finish()));
        frame.present();
        Ok(())
    }
}

/// A stable key over a row's *visible* appearance — the coalesced spans (text + per-span
/// color/bold/italic), salted with the surface width that shaping depends on. Two frames whose
/// row keys match render identically, so the shaped buffer can be reused without re-shaping (the
/// dominant per-frame cost). The font size is not in the key because a DPI change clears the
/// whole cache. Background color and the cursor block are quads (not shaped), so they are not
/// part of the key; the cursor's glyph recolor *is*, via the span colors.
fn row_appearance_key(spans: &[Span], surf_w: u32) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    surf_w.hash(&mut h);
    spans.len().hash(&mut h);
    for s in spans {
        s.text.hash(&mut h);
        s.color.hash(&mut h);
        s.bold.hash(&mut h);
        s.italic.hash(&mut h);
    }
    h.finish()
}

/// sRGB 0–1 tuple → 8-bit sRGB components (for glyphon text colors).
fn srgb_u8(c: SrgbaTuple) -> [u8; 4] {
    let (r, g, b, a) = c.to_srgb_u8();
    [r, g, b, a]
}

/// sRGB tuple → sRGB-encoded RGBA for quad colors. The surface is a linear UNORM format (see
/// `render::mod`), so we write the gamma-encoded components verbatim — the `SrgbaTuple` already
/// holds sRGB-encoded 0–1 values. Writing linear here would wash colors out, and (more visibly)
/// blend glyph AA in the wrong space.
fn quad_color(c: SrgbaTuple) -> [f32; 4] {
    [c.0, c.1, c.2, c.3]
}

/// sRGB tuple → wgpu clear color, sRGB-encoded (the surface is UNORM, written verbatim).
fn srgba_to_wgpu_clear(c: SrgbaTuple) -> wgpu::Color {
    wgpu::Color {
        r: c.0 as f64,
        g: c.1 as f64,
        b: c.2 as f64,
        a: c.3 as f64,
    }
}
