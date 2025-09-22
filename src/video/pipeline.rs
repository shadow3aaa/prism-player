use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use encase::{ShaderType, UniformBuffer};
use glam::Vec4;
use tessera_ui::{DrawCommand, DrawablePipeline, wgpu};
use uuid::Uuid;

#[derive(ShaderType)]
struct VideoUniforms {
    rect: Vec4, // x, y, w, h (NDC 或者屏幕归一化)
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
    pub receiver: flume::Receiver<Vec<u8>>,
    pub last_frame_time: Instant,
    pub frame_duration: Duration,
    updated: bool,
}

impl VideoTarget {
    fn new(
        gpu: &wgpu::Device,
        config: &wgpu::SurfaceConfiguration,
        receiver: flume::Receiver<Vec<u8>>,
        sample_count: u32,
        width: u32,
        height: u32,
        frame_duration: Duration,
    ) -> Self {
        // 创建纹理
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
                // texture
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
                // sampler
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                // uniforms
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
            last_frame_time: Instant::now(),
            frame_duration,
            updated: false,
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
    pub frame_duration: Duration,
    pub receiver: flume::Receiver<Vec<u8>>,
}

impl PartialEq for VideoCommand {
    fn eq(&self, other: &Self) -> bool {
        // 首先比较 ID，确认是同一个播放目标，其次确保接收器为空（表示缓冲区渲染的帧已经消费完毕）
        self.id == other.id && self.receiver.is_empty()
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
        // 更新视频帧
        for target in self.video_targets.values_mut() {
            if target.last_frame_time.elapsed() >= target.frame_duration
                && let Ok(frame_data) = target.receiver.try_recv()
            {
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
                target.last_frame_time = Instant::now();
                target.updated = true;
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
                // 创建新的 VideoTarget
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
                        cmd.frame_duration,
                    ),
                );
            }

            // 采样视频纹理到绘制目标
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
            render_pass.draw(0..6, 0..1); // 两个三角形矩形
        }
    }
}
