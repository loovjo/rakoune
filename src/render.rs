use std::io::{Result as IOResult, Cursor};

use wgpu::{
    Surface,
    Adapter,
    Device,
    Queue,
    SwapChainDescriptor,
    SwapChain,
    Color,
    RenderPipeline,
    ProgrammableStageDescriptor,
    BlendDescriptor,
    BufferUsage,
};

use winit::{
    dpi::PhysicalSize,
    window::Window,
};

use crate::into_ioerror;
use crate::gpu_primitives::Vertex;
use crate::state::State;

pub struct RenderState {
    surface: Surface,
    adapter: Adapter,
    device: Device,
    queue: Queue,
    sc_desc: SwapChainDescriptor,
    swap_chain: SwapChain,

    render_pipeline: RenderPipeline,
    vertex_buffer: wgpu::Buffer,
}

const VERTEX_SHADER: &[u8] = include_bytes!("../compiled-shaders/shader-vert.spv");
const FRAGMENT_SHADER: &[u8] = include_bytes!("../compiled-shaders/shader-frag.spv");

impl RenderState {
    pub async fn new(window: &Window) -> IOResult<RenderState> {
        let size = window.inner_size();
        let surface = Surface::create(window);

        let adapter = Adapter::request(
            &wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::Default,
                compatible_surface: Some(&surface),
            },
            wgpu::BackendBit::PRIMARY,
        ).await.ok_or(into_ioerror("No adapter available"))?;

        let (device, queue) = adapter.request_device(
            &wgpu::DeviceDescriptor {
                extensions: Default::default(),
                limits: Default::default(),
            }
        ).await;

        let sc_desc = wgpu::SwapChainDescriptor {
            usage: wgpu::TextureUsage::OUTPUT_ATTACHMENT,
            format: wgpu::TextureFormat::Bgra8UnormSrgb,
            width: size.width,
            height: size.height,
            present_mode: wgpu::PresentMode::Fifo,
        };

        let swap_chain = device.create_swap_chain(&surface, &sc_desc);

        let vs_data = wgpu::read_spirv(Cursor::new(VERTEX_SHADER)).map_err(into_ioerror)?;
        let fs_data = wgpu::read_spirv(Cursor::new(FRAGMENT_SHADER)).map_err(into_ioerror)?;

        let vs_module = device.create_shader_module(&vs_data);
        let fs_module = device.create_shader_module(&fs_data);

        let render_pipeline_layout = device.create_pipeline_layout(
            &wgpu::PipelineLayoutDescriptor {
                bind_group_layouts: &[],
            },
        );

        let render_pipeline = device.create_render_pipeline(
            &wgpu::RenderPipelineDescriptor {
                layout: &render_pipeline_layout,
                vertex_stage: ProgrammableStageDescriptor {
                    module: &vs_module,
                    entry_point: "main",
                },
                fragment_stage: Some(ProgrammableStageDescriptor {
                    module: &fs_module,
                    entry_point: "main",
                }),
                rasterization_state: Some(wgpu::RasterizationStateDescriptor {
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: wgpu::CullMode::Back,
                    depth_bias: 0,
                    depth_bias_slope_scale: 0.0,
                    depth_bias_clamp: 0.0,
                }),
                primitive_topology: wgpu::PrimitiveTopology::TriangleList,
                color_states: &[
                    wgpu::ColorStateDescriptor {
                        format: sc_desc.format,
                        color_blend: BlendDescriptor::REPLACE,
                        alpha_blend: BlendDescriptor::REPLACE,
                        write_mask: wgpu::ColorWrite::ALL,
                    }
                ],
                vertex_state: wgpu::VertexStateDescriptor {
                    index_format: wgpu::IndexFormat::Uint32,
                    vertex_buffers: &[
                        Vertex::desc(),
                    ],
                },
                depth_stencil_state: None,
                sample_count: 1,
                sample_mask: !0,
                alpha_to_coverage_enabled: false,
            },
        );

        let vertex_buffer = device.create_buffer_with_data(
            &[0; 1024],
            BufferUsage::VERTEX | BufferUsage::COPY_DST,
        );

        Ok(Self {
            surface, adapter, device, queue, sc_desc, swap_chain, render_pipeline, vertex_buffer,
        })
    }

    pub fn resize(&mut self, into_size: PhysicalSize<u32>) {
        eprintln!("Recreating swapchain!");
        self.sc_desc.width = into_size.width;
        self.sc_desc.height = into_size.height;

        self.swap_chain = self.device.create_swap_chain(&self.surface, &self.sc_desc);
    }

    pub async fn render(&mut self, state: &State) -> IOResult<()> {
        // Upload vertex buffer
        let vertex_buffer_content: &[u8] = bytemuck::cast_slice(&state.verticies);

        // See https://github.com/gfx-rs/wgpu-rs/issues/9#issuecomment-494022784
        // This is a very cheap action since the backing memory is already allocated
        let staging_buffer_mapped = self.device.create_buffer_mapped(
            &wgpu::BufferDescriptor {
                label: Some("Staging buffer"),
                size: 1024,
                usage: BufferUsage::MAP_WRITE | BufferUsage::COPY_SRC | BufferUsage::STORAGE,
            }
        );
        staging_buffer_mapped.data[..vertex_buffer_content.len()].copy_from_slice(vertex_buffer_content);
        let staging_buffer = staging_buffer_mapped.finish();

        let mut stage_upload_encoder = self.device.create_command_encoder(
            &wgpu::CommandEncoderDescriptor {
                label: Some("Staging upload encoder"),
            }
        );

        stage_upload_encoder.copy_buffer_to_buffer(
            &staging_buffer,
            0,
            &self.vertex_buffer,
            0,
            1024,
        );

        let mut encoder = self.device.create_command_encoder(
            &wgpu::CommandEncoderDescriptor {
                label: Some("Render encoder"),
            }
        );

        let current_texture_view = &self.swap_chain.get_next_texture().map_err(|_| into_ioerror("Timeout"))?.view;

        let mut render_pass = encoder.begin_render_pass(
            &wgpu::RenderPassDescriptor {
                color_attachments: &[
                    wgpu::RenderPassColorAttachmentDescriptor {
                        attachment: current_texture_view,
                        resolve_target: None,
                        load_op: wgpu::LoadOp::Clear,
                        store_op: wgpu::StoreOp::Store,
                        clear_color: Color::BLUE,
                    }
                ],
                depth_stencil_attachment: None,
            }
        );

        render_pass.set_pipeline(&self.render_pipeline);
        render_pass.set_vertex_buffer(0, &self.vertex_buffer, 0, 1024);
        render_pass.draw(0..6, 0..1);

        std::mem::drop(render_pass);

        self.queue.submit(&[stage_upload_encoder.finish(), encoder.finish()]);

        Ok(())
    }
}