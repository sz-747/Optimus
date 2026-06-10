//! Instanced solid-color quads (plan §8 U6): cell backgrounds, the block cursor, and the
//! selection highlight. One pipeline, one instance buffer; each instance is a pixel-space
//! rectangle plus an RGBA color. The vertex shader expands six corners per instance and maps
//! pixel coordinates to NDC using the surface resolution (a uniform).
//!
//! Colors are written **sRGB-encoded**: the surface is a linear UNORM format (see
//! `PanelRenderer::new`), so the shader's output is composited verbatim with no
//! encoding step. Callers pass already-sRGB-encoded RGBA (see `quad_color` at the call
//! site) — the space glyph anti-aliasing is tuned for, which keeps light-on-dark text crisp.

use wgpu::util::DeviceExt;

/// One quad: position + size in physical pixels, color in sRGB-encoded RGBA.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct QuadInstance {
    pub pos: [f32; 2],
    pub size: [f32; 2],
    pub color: [f32; 4],
}

impl QuadInstance {
    pub fn new(x: f32, y: f32, w: f32, h: f32, color: [f32; 4]) -> Self {
        Self {
            pos: [x, y],
            size: [w, h],
            color,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Globals {
    resolution: [f32; 2],
    _pad: [f32; 2],
}

/// The quad render pipeline + its dynamically-grown instance buffer.
pub struct QuadLayer {
    pipeline: wgpu::RenderPipeline,
    globals_buf: wgpu::Buffer,
    globals_bind: wgpu::BindGroup,
    instances: wgpu::Buffer,
    instance_capacity: u32,
}

const SHADER: &str = r#"
struct Globals { resolution: vec2<f32>, _pad: vec2<f32> };
@group(0) @binding(0) var<uniform> globals: Globals;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_main(
    @builtin(vertex_index) vid: u32,
    @location(0) inst_pos: vec2<f32>,
    @location(1) inst_size: vec2<f32>,
    @location(2) inst_color: vec4<f32>,
) -> VsOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
    );
    let c = corners[vid];
    let px = inst_pos + c * inst_size;
    let ndc = vec2<f32>(
        px.x / globals.resolution.x * 2.0 - 1.0,
        1.0 - px.y / globals.resolution.y * 2.0,
    );
    var out: VsOut;
    out.clip = vec4<f32>(ndc, 0.0, 1.0);
    out.color = inst_color;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return in.color;
}
"#;

impl QuadLayer {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat, width: u32, height: u32) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("optimus quad shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });

        let globals_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("optimus quad globals"),
            contents: bytemuck::bytes_of(&Globals {
                resolution: [width.max(1) as f32, height.max(1) as f32],
                _pad: [0.0, 0.0],
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("optimus quad globals layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let globals_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("optimus quad globals bind"),
            layout: &bind_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals_buf.as_entire_binding(),
            }],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("optimus quad pipeline layout"),
            bind_group_layouts: &[Some(&bind_layout)],
            immediate_size: 0,
        });

        let instance_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<QuadInstance>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &[
                wgpu::VertexAttribute {
                    offset: 0,
                    shader_location: 0,
                    format: wgpu::VertexFormat::Float32x2,
                },
                wgpu::VertexAttribute {
                    offset: 8,
                    shader_location: 1,
                    format: wgpu::VertexFormat::Float32x2,
                },
                wgpu::VertexAttribute {
                    offset: 16,
                    shader_location: 2,
                    format: wgpu::VertexFormat::Float32x4,
                },
            ],
        };

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("optimus quad pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[instance_layout],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let instance_capacity = 1024;
        let instances = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("optimus quad instances"),
            size: instance_capacity as u64 * std::mem::size_of::<QuadInstance>() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            pipeline,
            globals_buf,
            globals_bind,
            instances,
            instance_capacity,
        }
    }

    /// Update the resolution uniform after a surface resize.
    pub fn set_resolution(&self, queue: &wgpu::Queue, width: u32, height: u32) {
        queue.write_buffer(
            &self.globals_buf,
            0,
            bytemuck::bytes_of(&Globals {
                resolution: [width.max(1) as f32, height.max(1) as f32],
                _pad: [0.0, 0.0],
            }),
        );
    }

    /// Upload this frame's instances, growing the buffer if needed.
    pub fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, instances: &[QuadInstance]) {
        if instances.is_empty() {
            return;
        }
        if instances.len() as u32 > self.instance_capacity {
            let mut cap = self.instance_capacity.max(1);
            while (instances.len() as u32) > cap {
                cap *= 2;
            }
            self.instances = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("optimus quad instances"),
                size: cap as u64 * std::mem::size_of::<QuadInstance>() as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.instance_capacity = cap;
        }
        queue.write_buffer(&self.instances, 0, bytemuck::cast_slice(instances));
    }

    /// Draw `count` instances into the pass (call after [`Self::upload`]).
    pub fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, count: u32) {
        if count == 0 {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.globals_bind, &[]);
        pass.set_vertex_buffer(0, self.instances.slice(..));
        pass.draw(0..6, 0..count);
    }
}
