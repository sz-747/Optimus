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

/// A bound DX12 surface targeting a WinUI 3 `SwapChainPanel`, plus the device/queue
/// needed to render and present into it.
///
/// Owned and used exclusively by the render thread (plan §6: DX12 surfaces are not
/// freely `Send`, so one thread owns the device + surface).
pub struct PanelRenderer {
    _instance: wgpu::Instance,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
}

impl PanelRenderer {
    /// Bind a wgpu DX12 surface to the WinUI 3 `SwapChainPanel` behind `panel` and
    /// configure it at `width`×`height` physical pixels.
    ///
    /// `panel` is a raw `ISwapChainPanel*` (COM) obtained on the C# side. wgpu refcounts
    /// it for the lifetime of the surface.
    ///
    /// # Safety
    /// `panel` must be a valid, live `ISwapChainPanel*` pointer (e.g. from
    /// `ISwapChainPanelNative` on a real `SwapChainPanel`). It must remain valid until
    /// this `PanelRenderer` is dropped.
    pub unsafe fn new(panel: *mut c_void, width: u32, height: u32) -> Result<Self, RenderError> {
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
                label: Some("cmux-engine device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::downlevel_defaults(),
                memory_hints: wgpu::MemoryHints::Performance,
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                trace: wgpu::Trace::Off,
            }))?;

        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(caps.formats[0]);

        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: width.max(1),
            height: height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
            view_formats: vec![],
        };
        surface.configure(&device, &config);

        Ok(Self {
            _instance: instance,
            surface,
            device,
            queue,
            config,
        })
    }

    /// Resize the surface to `width`×`height` physical pixels (driven by `SizeChanged` /
    /// `CompositionScaleChanged` on the C# side — plan §5.2).
    pub fn resize(&mut self, width: u32, height: u32) {
        self.config.width = width.max(1);
        self.config.height = height.max(1);
        self.surface.configure(&self.device, &self.config);
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
                self.surface.configure(&self.device, &self.config);
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
                label: Some("cmux-engine clear encoder"),
            });
        {
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("cmux-engine clear pass"),
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
}

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RenderError::CreateSurface(e) => write!(f, "create_surface failed: {e}"),
            RenderError::NoAdapter => write!(f, "no compatible DX12 adapter"),
            RenderError::RequestDevice(e) => write!(f, "request_device failed: {e}"),
            RenderError::FrameUnavailable(why) => write!(f, "frame unavailable: {why}"),
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
