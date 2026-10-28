// Adapted from zui crates/gpui_wgpu/src/wgpu_renderer.rs.
use super::*;
use anyhow::Context as _;

impl WgpuRendererCore {
    pub(super) fn prepare_backdrop(
        &mut self,
        size: Size<DevicePixels>,
        blur_count: usize,
    ) -> Result<()> {
        let width = size.width.0 as u32;
        let height = size.height.0 as u32;
        if self
            .resources
            .backdrop_frame
            .as_ref()
            .is_some_and(|frame| frame.texture.width() != width || frame.texture.height() != height)
        {
            self.resources.backdrop_scratch = std::array::from_fn(|_| None);
            self.resources.backdrop_frame = None;
        }
        if blur_count == 0 {
            self.blur_free_frames = self.blur_free_frames.saturating_add(1);
            if self.blur_free_frames >= SCRATCH_RELEASE_AFTER_FRAMES {
                self.resources.backdrop_scratch = std::array::from_fn(|_| None);
                self.resources.backdrop_frame = None;
                self.resources.backdrop_params = None;
            }
            return Ok(());
        }
        self.blur_free_frames = 0;
        let buffer_size = u64::try_from(blur_count)?
            .checked_mul(3)
            .and_then(|slots| slots.checked_mul(self.backdrop_slot_stride))
            .context("backdrop uniform buffer size overflow")?;
        let resources = &mut self.resources;
        anyhow::ensure!(
            buffer_size <= resources.device.limits().max_buffer_size,
            "backdrop uniform buffer size {buffer_size} exceeds device limit"
        );
        if resources
            .backdrop_params
            .as_ref()
            .is_none_or(|buffer| buffer.size() < buffer_size)
        {
            resources.backdrop_params =
                Some(resources.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("backdrop_params"),
                    size: buffer_size,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }));
        }
        if resources
            .backdrop_frame
            .as_ref()
            .is_none_or(|frame| frame.texture.width() != width || frame.texture.height() != height)
        {
            let texture = resources.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("copyable_backdrop_frame"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: self.target_format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let blitter = wgpu::util::TextureBlitter::new(&resources.device, self.target_format);
            resources.backdrop_frame = Some(BackdropFrame {
                texture,
                view,
                blitter,
            });
        }
        Ok(())
    }

    pub(super) fn continue_main_pass<'a>(
        encoder: &'a mut wgpu::CommandEncoder,
        frame_view: &'a wgpu::TextureView,
    ) -> wgpu::RenderPass<'a> {
        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("main_pass_continued"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: frame_view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: None,
            ..Default::default()
        })
    }

    pub(super) fn process_backdrop_blur(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        frame: &wgpu::Texture,
        blur: &BackdropBlur,
        slot: usize,
    ) -> Option<usize> {
        fn valid_bounds(bounds: Bounds<ScaledPixels>) -> bool {
            let x = bounds.origin.x.0;
            let y = bounds.origin.y.0;
            let w = bounds.size.width.0;
            let h = bounds.size.height.0;
            [x, y, w, h, x + w, y + h]
                .iter()
                .all(|value| value.is_finite())
                && w > 0.0
                && h > 0.0
        }
        if !blur.blur_radius.0.is_finite()
            || blur.blur_radius.0 <= 0.0
            || !valid_bounds(blur.bounds)
            || !valid_bounds(blur.content_mask.bounds)
            || ![
                blur.corner_radii.top_left.0,
                blur.corner_radii.top_right.0,
                blur.corner_radii.bottom_right.0,
                blur.corner_radii.bottom_left.0,
            ]
            .iter()
            .all(|value| value.is_finite() && *value >= 0.0)
        {
            return None;
        }
        let visible = blur.bounds.intersect(&blur.content_mask.bounds);
        let drawable_width = frame.width() as i64;
        let drawable_height = frame.height() as i64;
        if visible.is_empty()
            || visible.origin.x.0 >= drawable_width as f32
            || visible.origin.y.0 >= drawable_height as f32
            || visible.origin.x.0 + visible.size.width.0 <= 0.0
            || visible.origin.y.0 + visible.size.height.0 <= 0.0
        {
            return None;
        }
        let sigma = blur.blur_radius.0.clamp(1.0, 64.0);
        let padding = (sigma * 3.0).ceil() + 2.0;
        let x0 = ((visible.origin.x.0 - padding).floor() as i64).max(0);
        let y0 = ((visible.origin.y.0 - padding).floor() as i64).max(0);
        let x1 = ((visible.origin.x.0 + visible.size.width.0 + padding).ceil() as i64)
            .min(drawable_width);
        let y1 = ((visible.origin.y.0 + visible.size.height.0 + padding).ceil() as i64)
            .min(drawable_height);
        if x1 <= x0 || y1 <= y0 {
            return None;
        }
        let downsample = ((sigma / 8.0) as u32).clamp(1, 4);
        let scratch = self.ensure_backdrop_scratch(
            (x1 - x0) as u32,
            (y1 - y0) as u32,
            downsample,
            drawable_width as u32,
            drawable_height as u32,
        );
        let copy_x = x0.min(drawable_width - scratch.width as i64).max(0) as u32;
        let copy_y = y0.min(drawable_height - scratch.height as i64).max(0) as u32;
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: frame,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: copy_x,
                    y: copy_y,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: &scratch.snapshot,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: scratch.width,
                height: scratch.height,
                depth_or_array_layers: 1,
            },
        );

        // Queue writes precede the whole command buffer: each operation needs its own slot.
        let h_offset = self.backdrop_slot_stride * (slot as u64) * 3;
        let v_offset = h_offset + self.backdrop_slot_stride;
        let c_offset = v_offset + self.backdrop_slot_stride;
        let weights = blur_kernel::gaussian_weights(sigma);
        let (horizontal_region, vertical_region) = blur_kernel::blur_regions(
            [
                visible.origin.x.0,
                visible.origin.y.0,
                visible.size.width.0,
                visible.size.height.0,
            ],
            [copy_x, copy_y, scratch.width, scratch.height],
            [scratch.blur_width, scratch.blur_height],
            sigma,
        );
        let resources = &self.resources;
        let buffer = resources
            .backdrop_params
            .as_ref()
            .expect("backdrop buffer prepared");
        let mut params = BlurPassUniform {
            direction: [1.0, 0.0],
            sigma,
            downsample: downsample as f32,
            source_texel_size: [1.0 / scratch.width as f32, 1.0 / scratch.height as f32],
            padding: [0.0; 2],
            weights,
        };
        resources
            .queue
            .write_buffer(buffer, h_offset, bytemuck::bytes_of(&params));
        params.direction = [0.0, 1.0];
        params.source_texel_size[0] = 1.0 / scratch.blur_width as f32;
        resources
            .queue
            .write_buffer(buffer, v_offset, bytemuck::bytes_of(&params));
        resources.queue.write_buffer(
            buffer,
            c_offset,
            bytemuck::bytes_of(&BackdropBlurUniform {
                bounds: blur.bounds.into(),
                corner_radii: [
                    blur.corner_radii.top_left.0,
                    blur.corner_radii.top_right.0,
                    blur.corner_radii.bottom_right.0,
                    blur.corner_radii.bottom_left.0,
                ],
                clip_bounds: blur.content_mask.bounds.into(),
                source_rect: [
                    copy_x as f32,
                    copy_y as f32,
                    scratch.width as f32,
                    scratch.height as f32,
                ],
                clip: blur.clip.into(),
            }),
        );
        let h_group = Self::backdrop_bind_group(
            resources,
            h_offset,
            std::mem::size_of::<BlurPassUniform>() as u64,
            &scratch.snapshot_view,
            &scratch.snapshot_view,
        );
        let v_group = Self::backdrop_bind_group(
            resources,
            v_offset,
            std::mem::size_of::<BlurPassUniform>() as u64,
            &scratch.blur_a_view,
            &scratch.snapshot_view,
        );
        for (view, group, region) in [
            (&scratch.blur_a_view, &h_group, horizontal_region),
            (&scratch.blur_b_view, &v_group, vertical_region),
        ] {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("backdrop_blur_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                ..Default::default()
            });
            pass.set_pipeline(&resources.pipelines.backdrop_blur_pass);
            pass.set_bind_group(0, &resources.globals_bind_group, &[]);
            pass.set_bind_group(1, group, &[]);
            let [x, y, width, height] = region;
            if width > 0 && height > 0 {
                pass.set_scissor_rect(x, y, width, height);
                pass.draw(0..4, 0..1);
            }
        }
        Some((downsample - 1) as usize)
    }

    pub(super) fn draw_backdrop_composite(
        &self,
        slot: usize,
        scratch_index: usize,
        pass: &mut wgpu::RenderPass<'_>,
    ) {
        let resources = &self.resources;
        let scratch = resources.backdrop_scratch[scratch_index]
            .as_ref()
            .expect("backdrop scratch prepared");
        let offset = self.backdrop_slot_stride * (slot as u64 * 3 + 2);
        let group = Self::backdrop_bind_group(
            resources,
            offset,
            std::mem::size_of::<BackdropBlurUniform>() as u64,
            &scratch.blur_b_view,
            &scratch.snapshot_view,
        );
        pass.set_pipeline(&resources.pipelines.backdrop_composite);
        pass.set_bind_group(0, &resources.globals_bind_group, &[]);
        pass.set_bind_group(1, &group, &[]);
        pass.draw(0..4, 0..1);
    }

    fn backdrop_bind_group(
        resources: &WgpuResources,
        offset: u64,
        size: u64,
        source: &wgpu::TextureView,
        snapshot: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        resources
            .device
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("backdrop_bind_group"),
                layout: &resources.bind_group_layouts.backdrop,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                            buffer: resources
                                .backdrop_params
                                .as_ref()
                                .expect("backdrop buffer prepared"),
                            offset,
                            size: NonZeroU64::new(size),
                        }),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(source),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&resources.backdrop_sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(snapshot),
                    },
                ],
            })
    }

    fn ensure_backdrop_scratch(
        &mut self,
        needed_width: u32,
        needed_height: u32,
        downsample: u32,
        drawable_width: u32,
        drawable_height: u32,
    ) -> BackdropScratch {
        let format = self.target_format;
        let resources = &mut self.resources;
        let device = &resources.device;
        let scratch = &mut resources.backdrop_scratch[(downsample - 1) as usize];
        let reusable = scratch.as_ref().is_some_and(|scratch| {
            scratch.width >= needed_width
                && scratch.width <= drawable_width
                && scratch.height >= needed_height
                && scratch.height <= drawable_height
        });
        if !reusable {
            const QUANTUM: u32 = 256;
            let previous = scratch.as_ref();
            let width = needed_width
                .max(previous.map_or(0, |s| s.width))
                .div_ceil(QUANTUM)
                .saturating_mul(QUANTUM)
                .min(drawable_width)
                .max(1);
            let height = needed_height
                .max(previous.map_or(0, |s| s.height))
                .div_ceil(QUANTUM)
                .saturating_mul(QUANTUM)
                .min(drawable_height)
                .max(1);
            let blur_width = width.div_ceil(downsample).max(1);
            let blur_height = height.div_ceil(downsample).max(1);
            let create = |label: &str, width: u32, height: u32, usage| {
                device.create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width,
                        height,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage,
                    view_formats: &[],
                })
            };
            let snapshot = create(
                "backdrop_snapshot",
                width,
                height,
                wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
            );
            let blur_usage =
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
            // Preserve native rows until the vertical pass filters them.
            let blur_a = create("backdrop_blur_a", blur_width, height, blur_usage);
            let blur_b = create("backdrop_blur_b", blur_width, blur_height, blur_usage);
            let view = |texture: &wgpu::Texture| {
                texture.create_view(&wgpu::TextureViewDescriptor::default())
            };
            *scratch = Some(BackdropScratch {
                width,
                height,
                blur_width,
                blur_height,
                snapshot_view: view(&snapshot),
                blur_a_view: view(&blur_a),
                blur_b_view: view(&blur_b),
                snapshot,
            });
        }
        scratch.as_ref().expect("backdrop scratch created").clone()
    }
}
