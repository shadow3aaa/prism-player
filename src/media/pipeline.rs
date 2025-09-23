use std::{collections::HashMap, time::Instant};

use crate::media::clock::GlobalClock;
use encase::{ShaderType, UniformBuffer};
use glam::Vec4;
use tessera_ui::{DrawCommand, DrawablePipeline, wgpu};
use uuid::Uuid;

#[derive(ShaderType)]
struct VideoUniforms {
    rect: Vec4, // x, y, w, h (normalized device coords or screen-normalized)
}

struct VideoResources {
    uniform_buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    #[allow(unused)]
    bind_group_layout: wgpu::BindGroupLayout,
    texture_view: wgpu::TextureView,
    #[allow(unused)]
    sampler: wgpu::Sampler,
    pipeline: wgpu::RenderPipeline,
}

struct VideoTarget {
    pub resources: VideoResources,
    pub receiver: flume::Receiver<(Vec<u8>, f64)>,
    updated: bool,
    // scheduling driven by presentation timestamps (PTS)
    pub first_pts: Option<f64>,
    pub first_instant: Option<Instant>,
    pub last_pts_seconds: Option<f64>,
    // per-target clock for independent timing/control
    pub clock: GlobalClock,
    // single-frame slot to avoid pipeline-side buffering
    pub next_frame_slot: Option<(Vec<u8>, f64)>,
}

impl VideoTarget {
    fn new(
        gpu: &wgpu::Device,
        config: &wgpu::SurfaceConfiguration,
        receiver: flume::Receiver<(Vec<u8>, f64)>,
        sample_count: u32,
        width: u32,
        height: u32,
        clock: GlobalClock,
    ) -> Self {
        // create texture used as the video render target
        let texture = gpu.create_texture(&wgpu::TextureDescriptor {
            label: Some("video texture"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let texture_view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = gpu.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let bind_group_layout = gpu.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("video bind group layout"),
            entries: &[
                // texture binding for sampling video pixels
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    },
                    count: None,
                },
                // sampler for texture sampling
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                // uniform buffer for vertex and fragment shaders
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let uniform_buffer = gpu.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Video Uniform Buffer"),
            size: VideoUniforms::min_size().get(),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_group = gpu.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("video bind group"),
            layout: &bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&texture_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: uniform_buffer.as_entire_binding(),
                },
            ],
        });

        let shader = gpu.create_shader_module(wgpu::include_wgsl!("video.wgsl"));
        let pipeline_layout = gpu.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("video pipeline layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });
        let pipeline = gpu.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("video pipeline"),
            cache: None,
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState {
                count: sample_count,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            multiview: None,
        });

        let resources = VideoResources {
            pipeline,
            sampler,
            texture_view,
            uniform_buffer,
            bind_group,
            bind_group_layout,
        };

        Self {
            resources,
            receiver,
            updated: false,
            first_pts: None,
            first_instant: None,
            clock,
            next_frame_slot: None,
            last_pts_seconds: None,
        }
    }
}

pub struct VideoPipeline {
    video_targets: HashMap<Uuid, VideoTarget>,
    sample_count: u32,
}

impl VideoPipeline {
    pub fn new(sample_count: u32) -> Self {
        Self {
            video_targets: HashMap::new(),
            sample_count,
        }
    }
}

#[derive(Clone)]
pub struct VideoCommand {
    pub id: Uuid,
    pub width: u32,
    pub height: u32,
    pub receiver: flume::Receiver<(Vec<u8>, f64)>,
    pub clock: GlobalClock,
}

impl PartialEq for VideoCommand {
    fn eq(&self, other: &Self) -> bool {
        // compare id to ensure same playback target; require clock to be paused to avoid race conditions
        self.id == other.id && self.clock.is_paused()
    }
}

impl DrawCommand for VideoCommand {}

impl DrawablePipeline<VideoCommand> for VideoPipeline {
    fn begin_frame(
        &mut self,
        _gpu: &tessera_ui::wgpu::Device,
        gpu_queue: &tessera_ui::wgpu::Queue,
        _config: &tessera_ui::wgpu::SurfaceConfiguration,
    ) {
        // update video frames based on shared clock and single-frame slot to avoid buffering
        const TOLERANCE_SHOW: f64 = 0.03; // seconds
        const DROP_THRESHOLD: f64 = 0.15; // seconds

        for target in self.video_targets.values_mut() {
            let now = target.clock.now();

            // fill single-frame slot to hold the next frame for scheduling decisions
            if target.next_frame_slot.is_none()
                && let Ok((frame_data, pts)) = target.receiver.try_recv()
            {
                target.next_frame_slot = Some((frame_data, pts));
            }

            // Evaluate slot by temporarily taking it to avoid simultaneous borrows
            if let Some(slot) = target.next_frame_slot.take() {
                let (frame_data, pts_seconds) = slot;
                // decide whether to show, drop, or wait for the correct display time
                if pts_seconds <= now + TOLERANCE_SHOW {
                    // show frame
                    gpu_queue.write_texture(
                        wgpu::TexelCopyTextureInfo {
                            texture: target.resources.texture_view.texture(),
                            mip_level: 0,
                            origin: wgpu::Origin3d::ZERO,
                            aspect: wgpu::TextureAspect::All,
                        },
                        &frame_data,
                        wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(
                                4 * target.resources.texture_view.texture().size().width,
                            ),
                            rows_per_image: None,
                        },
                        wgpu::Extent3d {
                            width: target.resources.texture_view.texture().size().width,
                            height: target.resources.texture_view.texture().size().height,
                            depth_or_array_layers: 1,
                        },
                    );
                    target.updated = true;
                    target.last_pts_seconds = Some(pts_seconds);
                    if target.first_pts.is_none() {
                        target.first_pts = Some(pts_seconds);
                        target.first_instant = Some(Instant::now());
                    }
                } else if pts_seconds < now - DROP_THRESHOLD {
                    // drop stale frame to avoid excessive latency
                } else {
                    // future frame: put it back and wait until its presentation time
                    target.next_frame_slot = Some((frame_data, pts_seconds));
                }
            }
        }
    }

    fn draw(
        &mut self,
        gpu: &tessera_ui::wgpu::Device,
        gpu_queue: &tessera_ui::wgpu::Queue,
        config: &tessera_ui::wgpu::SurfaceConfiguration,
        render_pass: &mut tessera_ui::wgpu::RenderPass<'_>,
        commands: &[(&VideoCommand, tessera_ui::PxSize, tessera_ui::PxPosition)],
        _scene_texture_view: &tessera_ui::wgpu::TextureView,
        _clip_rect: Option<tessera_ui::PxRect>,
    ) {
        for (cmd, size, pos) in commands {
            if !self.video_targets.contains_key(&cmd.id) {
                // create a new VideoTarget to manage per-target resources and timing
                let rx = cmd.receiver.clone();
                self.video_targets.insert(
                    cmd.id,
                    VideoTarget::new(
                        gpu,
                        config,
                        rx,
                        self.sample_count,
                        cmd.width,
                        cmd.height,
                        cmd.clock.clone(),
                    ),
                );
            }

            // sample video texture into the render target for drawing
            let target = self.video_targets.get_mut(&cmd.id).unwrap();
            render_pass.set_pipeline(&target.resources.pipeline);
            let uniforms = VideoUniforms {
                rect: Vec4::new(
                    pos.x.0 as f32 / config.width as f32,
                    pos.y.0 as f32 / config.height as f32,
                    size.width.0 as f32 / config.width as f32,
                    size.height.0 as f32 / config.height as f32,
                ),
            };
            let mut buffer = UniformBuffer::new(Vec::new());
            buffer.write(&uniforms).unwrap();
            gpu_queue.write_buffer(&target.resources.uniform_buffer, 0, &buffer.into_inner());
            render_pass.set_bind_group(0, &target.resources.bind_group, &[]);
            render_pass.draw(0..6, 0..1); // two triangles forming a rectangle
        }
    }
}
