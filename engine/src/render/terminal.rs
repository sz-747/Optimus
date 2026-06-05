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

pub struct TerminalRenderer {
    panel: PanelRenderer,
    quads: QuadLayer,
    text: TextLayer,
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
        let panel = unsafe { PanelRenderer::new(panel_ptr, width, height)? };
        let quads = QuadLayer::new(&panel.device, panel.format(), width, height);
        let text = TextLayer::new(&panel.device, &panel.queue, panel.format(), dpi_scale);
        Ok(Self { panel, quads, text })
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.panel.resize(width, height);
        self.quads.set_resolution(&self.panel.queue, width, height);
    }

    pub fn set_scale(&mut self, dpi_scale: f32) {
        self.text.set_scale(dpi_scale);
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
    ) -> Result<(), RenderError> {
        let (cell_w, cell_h) = self.text.cell_size();
        let (surf_w, surf_h) = self.panel.size();

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
        // (buffer, top_y) per row; kept alive until after prepare().
        let mut row_buffers: Vec<(glyphon::Buffer, f32)> = Vec::with_capacity(lines.len());

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
                        lin(cell_bg),
                    ));
                }

                // Clear continuation columns of a wide cell so glyphs aren't duplicated.
                for w in 1..cell.width() {
                    if col + w < cols {
                        glyphs[col + w] = String::new();
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
                    lin(default_fg),
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

            let buffer = self.text.shape_row(&spans, surf_w as f32);
            row_buffers.push((buffer, y));
        }

        // Build text areas referencing the row buffers.
        let bounds = TextLayer::full_bounds(surf_w, surf_h);
        let areas: Vec<TextArea> = row_buffers
            .iter()
            .map(|(buffer, top)| TextArea {
                buffer,
                left: 0.0,
                top: *top,
                scale: 1.0,
                bounds,
                default_color: Color::rgba(
                    default_fg_u8[0],
                    default_fg_u8[1],
                    default_fg_u8[2],
                    default_fg_u8[3],
                ),
                custom_glyphs: &[],
            })
            .collect();

        self.quads
            .upload(&self.panel.device, &self.panel.queue, &quads);
        self.text
            .prepare(&self.panel.device, &self.panel.queue, surf_w, surf_h, areas)?;

        // Acquire the frame and draw: clear → backgrounds/cursor → glyphs.
        let frame = match self.panel.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) | wgpu::CurrentSurfaceTexture::Suboptimal(t) => {
                t
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.panel.surface.configure(&self.panel.device, &self.panel.config);
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
        let mut encoder = self
            .panel
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("cmux frame encoder"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("cmux terminal pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(srgba_to_wgpu_linear(default_bg)),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            self.quads.draw(&mut pass, quads.len() as u32);
            self.text.render(&mut pass)?;
        }
        self.panel.queue.submit(std::iter::once(encoder.finish()));
        frame.present();
        Ok(())
    }
}

/// sRGB 0–1 tuple → 8-bit sRGB components (for glyphon text colors).
fn srgb_u8(c: SrgbaTuple) -> [u8; 4] {
    let (r, g, b, a) = c.to_srgb_u8();
    [r, g, b, a]
}

/// sRGB tuple → linear RGBA (for quad colors written to the sRGB surface).
fn lin(c: SrgbaTuple) -> [f32; 4] {
    let l = c.to_linear();
    [l.0, l.1, l.2, l.3]
}

/// sRGB tuple → wgpu clear color (linear).
fn srgba_to_wgpu_linear(c: SrgbaTuple) -> wgpu::Color {
    let l = c.to_linear();
    wgpu::Color {
        r: l.0 as f64,
        g: l.1 as f64,
        b: l.2 as f64,
        a: l.3 as f64,
    }
}
