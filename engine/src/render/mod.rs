//! GPU renderer — plan §5 (interop) / §6 / §8 U6.
//!
//! Phase 1 render model (plan §4 / §5.1): **wgpu owns the swapchain**. C# creates a
//! WinUI 3 `SwapChainPanel`, QIs it for `ISwapChainPanelNative`, and passes the raw
//! `ISwapChainPanel*` across FFI. Rust hands that pointer to wgpu's DX12 backend via
//! [`wgpu::SurfaceTargetUnsafe::SwapChainPanel`]; wgpu performs
//! `CreateSwapChainForComposition` + `ISwapChainPanelNative::SetSwapChain` internally
//! and refcounts the panel.
//!
//! Spike 1 (plan §7.1, GATE): bind the surface, configure it, clear to a color, present —
//! proving the wgpu↔SwapChainPanel composition path before the rest of the renderer
//! (grid/text via glyphon, U6) is built on top.

use std::ffi::c_void;

/// Spike 1 C ABI (`optimus_spike_*`) that lets the WinUI 3 host drive this renderer.
pub mod panel_ffi;

/// Instanced solid-color quads (cell backgrounds, cursor, selection) — plan §8 U6.
pub mod grid;
/// glyphon glyph atlas + text renderer, and cell metrics — plan §8 U6.
pub mod text;
/// The terminal renderer: composes the surface + quad + text layers into a frame.
pub mod terminal;

/// A bound DX12 surface targeting a WinUI 3 `SwapChainPanel`, plus the device/queue
/// needed to render and present into it.
///
/// Owned and used exclusively by the render thread (plan §6: DX12 surfaces are not
/// freely `Send`, so one thread owns the device + surface).
pub struct PanelRenderer {
    _instance: wgpu::Instance,
    pub(crate) surface: wgpu::Surface<'static>,
    pub(crate) device: wgpu::Device,
    pub(crate) queue: wgpu::Queue,
    pub(crate) config: wgpu::SurfaceConfiguration,
    /// Composition (= DPI) scale of the hosting `SwapChainPanel`. The panel composites the
    /// swapchain magnified by this factor; we apply its inverse as a swapchain matrix
    /// transform so physical surface pixels map 1:1 to screen pixels (plan §5.2). Re-asserted
    /// after every `configure`, since the transform belongs to the swapchain object and is
    /// dropped whenever the swapchain is recreated.
    composition_scale: f32,
}

impl PanelRenderer {
    /// Bind a wgpu DX12 surface to the WinUI 3 `SwapChainPanel` behind `panel` and
    /// configure it at `width`×`height` physical pixels.
    ///
    /// `panel` is a raw `ISwapChainPanel*` (COM) obtained on the C# side. wgpu refcounts
    /// it for the lifetime of the surface.
    ///
    /// `scale` is the panel's composition (DPI) scale; its inverse is applied as a swapchain
    /// matrix transform so the panel composites the surface 1:1 (plan §5.2).
    ///
    /// # Safety
    /// `panel` must be a valid, live `ISwapChainPanel*` pointer (e.g. from
    /// `ISwapChainPanelNative` on a real `SwapChainPanel`). It must remain valid until
    /// this `PanelRenderer` is dropped.
    pub unsafe fn new(
        panel: *mut c_void,
        width: u32,
        height: u32,
        scale: f32,
    ) -> Result<Self, RenderError> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::DX12,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });

        // SAFETY: forwarded from this function's contract — `panel` is a valid ISwapChainPanel*.
        let surface = unsafe {
            instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::SwapChainPanel(panel))?
        };

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: Some(&surface),
        }))
        .map_err(|_| RenderError::NoAdapter)?;

        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("optimus-engine device"),
                required_features: wgpu::Features::empty(),
                // Use the adapter's real limits, not downlevel_defaults(): the latter caps
                // max_texture_dimension_2d at 2048, which makes Surface::configure panic for any
                // window larger than 2048px in either dimension (e.g. a 200%/4K monitor). DX12
                // adapters support 16384, so adopt whatever this adapter actually offers.
                required_limits: adapter.limits(),
                memory_hints: wgpu::MemoryHints::Performance,
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                trace: wgpu::Trace::Off,
            }))?;

        let caps = surface.get_capabilities(&adapter);
        // Prefer a NON-sRGB (linear UNORM) format. Glyph anti-aliasing coverage must be blended in
        // perceptual (sRGB-encoded) space — the space grayscale/ClearType AA is tuned for. An sRGB
        // surface makes the hardware blend in *linear* space, which makes light-on-dark text look
        // heavy and soft ("blurry"). With a UNORM surface we write already-sRGB-encoded colors and
        // blending happens in that encoded space, giving crisp terminal text. All color writers
        // below therefore emit sRGB-encoded values (not linearized).
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| !f.is_srgb())
            .unwrap_or(caps.formats[0]);

        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: width.max(1),
            height: height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            // 1 (not the default 2): a terminal favors input-to-photon latency over throughput,
            // so we let the GPU queue at most one frame ahead before blocking the next present.
            desired_maximum_frame_latency: 1,
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
            view_formats: vec![],
        };
        surface.configure(&device, &config);

        let renderer = Self {
            _instance: instance,
            surface,
            device,
            queue,
            config,
            composition_scale: if scale > 0.0 { scale } else { 1.0 },
        };
        renderer.apply_composition_transform();

        Ok(renderer)
    }

    /// The surface's texture format (sRGB) — needed to build the quad/text pipelines.
    pub(crate) fn format(&self) -> wgpu::TextureFormat {
        self.config.format
    }

    /// Current surface size in physical pixels.
    pub(crate) fn size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }

    /// Resize the surface to `width`×`height` physical pixels (driven by `SizeChanged` /
    /// `CompositionScaleChanged` on the C# side — plan §5.2).
    pub fn resize(&mut self, width: u32, height: u32) {
        self.config.width = width.max(1);
        self.config.height = height.max(1);
        self.reconfigure();
    }

    /// Update the panel's composition (DPI) scale and re-assert the inverse swapchain
    /// transform. Called on `CompositionScaleChanged` (e.g. dragging the window between
    /// monitors of different DPI — plan §5.2).
    pub fn set_composition_scale(&mut self, scale: f32) {
        if scale > 0.0 {
            self.composition_scale = scale;
        }
        self.apply_composition_transform();
    }

    /// Recreate the swapchain at the current config, then re-assert the inverse
    /// composition-scale transform. The transform is a property of the swapchain object, so
    /// it is lost whenever the swapchain is recreated (resize, outdated/lost recovery) and
    /// must be set again after every `configure`.
    pub(crate) fn reconfigure(&self) {
        self.surface.configure(&self.device, &self.config);
        self.apply_composition_transform();
    }

    /// Apply `1/composition_scale` as the swapchain's presentation matrix transform.
    ///
    /// A `SwapChainPanel` composites its DXGI swapchain magnified by the panel's
    /// composition scale (which WinUI keeps equal to the DPI scale). wgpu owns the
    /// swapchain and never sets this transform — and isn't even told the scale — so without
    /// correction the rendered surface is drawn `scale×` too large, anchored at the
    /// top-left. Text, cursor, and selection stay mutually aligned (all magnified) but
    /// drift down-and-right of the OS hardware cursor, worse the farther from the origin.
    /// Setting the inverse transform makes physical surface pixels composite 1:1 (plan §5.2).
    fn apply_composition_transform(&self) {
        use windows::core::Interface; // brings `IDXGISwapChain3::cast` into scope
        use windows::Win32::Graphics::Dxgi::{DXGI_MATRIX_3X2_F, IDXGISwapChain2};

        let scale = self.composition_scale;
        if !(scale > 0.0) {
            return;
        }

        // SAFETY: `as_hal` yields the live DX12 surface wgpu created for this panel; we only
        // read its swapchain and set a presentation transform — we never destroy the
        // resource, and the guard keeps the surface borrowed for the duration of the call.
        unsafe {
            let Some(hal_surface) = self.surface.as_hal::<wgpu::hal::dx12::Api>() else {
                return;
            };
            // wgpu creates the swapchain lazily on `configure`; bail quietly if it isn't up yet.
            let Some(swap_chain) = hal_surface.swap_chain() else {
                return;
            };
            // IDXGISwapChain3 (from CreateSwapChainForComposition) → IDXGISwapChain2 for
            // SetMatrixTransform. wgpu-hal and the engine share one `windows 0.62` crate, so
            // this COM interface is directly interoperable (no raw-pointer bridging).
            let Ok(swap_chain2) = swap_chain.cast::<IDXGISwapChain2>() else {
                return;
            };
            let inv = 1.0 / scale;
            let transform = DXGI_MATRIX_3X2_F {
                _11: inv,
                _12: 0.0,
                _21: 0.0,
                _22: inv,
                _31: 0.0,
                _32: 0.0,
            };
            let _ = swap_chain2.SetMatrixTransform(&transform);
        }
    }

    /// Clear the surface to `(r, g, b, a)` and present. Spike 1's "render one frame" step;
    /// the grid/text passes (U6) will add draw calls before `present`.
    pub fn clear_and_present(&mut self, r: f64, g: f64, b: f64, a: f64) -> Result<(), RenderError> {
        // `get_current_texture` returns a status enum (not a Result) in wgpu 29.
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t)
            | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            // Surface no longer matches the panel (resize/DPI/device change): reconfigure and let
            // the caller retry next tick rather than drawing into a stale buffer.
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.reconfigure();
                return Err(RenderError::FrameUnavailable(
                    "surface outdated/lost; reconfigured",
                ));
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
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("optimus-engine clear encoder"),
            });
        {
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("optimus-engine clear pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color { r, g, b, a }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
        }
        self.queue.submit(std::iter::once(encoder.finish()));
        frame.present();
        Ok(())
    }
}

/// Errors from binding or driving the GPU surface.
#[derive(Debug)]
pub enum RenderError {
    /// `create_surface_unsafe` failed (bad panel pointer, no DX12, etc.).
    CreateSurface(wgpu::CreateSurfaceError),
    /// No compatible DX12 adapter was found.
    NoAdapter,
    /// Device/queue request failed.
    RequestDevice(wgpu::RequestDeviceError),
    /// The next swapchain frame could not be acquired this tick (timeout/occluded/
    /// outdated/lost/validation). Non-fatal — the caller skips the frame and retries.
    FrameUnavailable(&'static str),
    /// glyphon text atlas prepare/render failure.
    Text(String),
}

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RenderError::CreateSurface(e) => write!(f, "create_surface failed: {e}"),
            RenderError::NoAdapter => write!(f, "no compatible DX12 adapter"),
            RenderError::RequestDevice(e) => write!(f, "request_device failed: {e}"),
            RenderError::FrameUnavailable(why) => write!(f, "frame unavailable: {why}"),
            RenderError::Text(e) => write!(f, "text renderer: {e}"),
        }
    }
}

impl std::error::Error for RenderError {}

impl From<wgpu::CreateSurfaceError> for RenderError {
    fn from(e: wgpu::CreateSurfaceError) -> Self {
        RenderError::CreateSurface(e)
    }
}
impl From<wgpu::RequestDeviceError> for RenderError {
    fn from(e: wgpu::RequestDeviceError) -> Self {
        RenderError::RequestDevice(e)
    }
}
