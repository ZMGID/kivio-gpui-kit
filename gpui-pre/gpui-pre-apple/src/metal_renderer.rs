use crate::metal_atlas::MetalAtlas;
use anyhow::{Context as _, Result};
use block2::RcBlock;
use core_graphics::geometry::CGSize;
use gpui::{
    AtlasTextureId, BackdropBlur, Background, Bounds, ContentMask, DevicePixels, PaintSurface,
    Path, Point, PrimitiveBatch, RoundedClipParams, ScaledPixels, Scene, Size, point, size,
};
#[cfg(any(test, feature = "bench-support", feature = "test-support"))]
use image::RgbaImage;
use objc2::{class, msg_send, rc::Retained, runtime::AnyObject};

use core_graphics::color_space::{CGColorSpace, kCGColorSpaceDisplayP3};
use foreign_types::{ForeignType, ForeignTypeRef};
use metal::{
    CAMetalLayer, CommandQueue, MTLGPUFamily, MTLPixelFormat, MTLResourceOptions, NSRange,
    NSUInteger,
};
use objc2_core_foundation::CFRetained;
use objc2_core_video::{
    CVMetalTexture, CVMetalTextureCache, CVMetalTextureGetTexture, CVPixelBufferGetHeight,
    CVPixelBufferGetHeightOfPlane, CVPixelBufferGetPixelFormatType, CVPixelBufferGetWidth,
    CVPixelBufferGetWidthOfPlane, kCVPixelFormatType_420YpCbCr8BiPlanarFullRange, kCVReturnSuccess,
};
use parking_lot::Mutex;

use std::{cell::Cell, ffi::c_void, mem, mem::MaybeUninit, ops::Range, ptr, slice, sync::Arc};

#[link(name = "MetalPerformanceShaders", kind = "framework")]
unsafe extern "C" {}

// Exported to metal
pub(crate) type PointF = gpui::Point<f32>;

#[cfg(not(feature = "runtime_shaders"))]
const SHADERS_METALLIB: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/shaders.metallib"));
#[cfg(feature = "runtime_shaders")]
const SHADERS_SOURCE_FILE: &str = include_str!(concat!(env!("OUT_DIR"), "/stitched_shaders.metal"));
// Use 4x MSAA, all devices support it.
// https://developer.apple.com/documentation/metal/mtldevice/1433355-supportstexturesamplecount
const PATH_SAMPLE_COUNT: u32 = 4;
/// Metal requires the offset a buffer is bound at to be 256-byte aligned.
const INSTANCE_BUFFER_ALIGNMENT: usize = 256;
const MAX_INSTANCE_BUFFER_SIZE: usize = 256 * 1024 * 1024;
const BACKDROP_RELEASE_AFTER_FRAMES: u32 = 30;

pub type Context = Arc<Mutex<InstanceBufferPool>>;
pub type Renderer = MetalRenderer;

pub unsafe fn new_renderer(
    context: self::Context,
    _native_window: *mut c_void,
    _native_view: *mut c_void,
    _bounds: gpui::Size<f32>,
    transparent: bool,
    use_display_p3: bool,
) -> Renderer {
    MetalRenderer::new(context, transparent, use_display_p3)
}

pub struct InstanceBufferPool {
    buffer_size: usize,
    buffers: Vec<metal::Buffer>,
}

impl Default for InstanceBufferPool {
    fn default() -> Self {
        Self {
            buffer_size: 2 * 1024 * 1024,
            buffers: Vec::new(),
        }
    }
}

pub(crate) struct InstanceBuffer {
    metal_buffer: metal::Buffer,
    size: usize,
}

impl InstanceBufferPool {
    pub(crate) fn reset(&mut self, buffer_size: usize) {
        self.buffer_size = buffer_size;
        self.buffers.clear();
    }

    pub(crate) fn acquire(
        &mut self,
        device: &metal::Device,
        unified_memory: bool,
    ) -> InstanceBuffer {
        let buffer = self.buffers.pop().unwrap_or_else(|| {
            let options = if unified_memory {
                MTLResourceOptions::StorageModeShared
                    // Buffers are write only which can benefit from the combined cache
                    // https://developer.apple.com/documentation/metal/mtlresourceoptions/cpucachemodewritecombined
                    | MTLResourceOptions::CPUCacheModeWriteCombined
            } else {
                MTLResourceOptions::StorageModeManaged
            };

            device.new_buffer(self.buffer_size as u64, options)
        });
        InstanceBuffer {
            metal_buffer: buffer,
            size: self.buffer_size,
        }
    }

    pub(crate) fn release(&mut self, buffer: InstanceBuffer) {
        if buffer.size == self.buffer_size {
            self.buffers.push(buffer.metal_buffer)
        }
    }
}

pub struct MetalRenderer {
    device: metal::Device,
    layer: Option<metal::MetalLayer>,
    restore_framebuffer_only: bool,
    is_apple_gpu: bool,
    is_unified_memory: bool,
    presents_with_transaction: bool,
    /// For headless rendering, tracks whether output should be opaque
    opaque: bool,
    command_queue: CommandQueue,
    paths_rasterization_pipeline_state: metal::RenderPipelineState,
    path_sprites_pipeline_state: metal::RenderPipelineState,
    shadows_pipeline_state: metal::RenderPipelineState,
    backdrop_blur_pipeline_state: metal::RenderPipelineState,
    backdrop_textures: Vec<BackdropTextures>,
    backdrop_kernels: Vec<(f32, Retained<AnyObject>)>,
    blur_free_frames: u32,
    quads_pipeline_state: metal::RenderPipelineState,
    underlines_pipeline_state: metal::RenderPipelineState,
    monochrome_sprites_pipeline_state: metal::RenderPipelineState,
    polychrome_sprites_pipeline_state: metal::RenderPipelineState,
    surfaces_pipeline_state: metal::RenderPipelineState,
    unit_vertices: metal::Buffer,
    #[allow(clippy::arc_with_non_send_sync)]
    instance_buffer_pool: Arc<Mutex<InstanceBufferPool>>,
    sprite_atlas: Arc<MetalAtlas>,
    core_video_texture_cache: CFRetained<CVMetalTextureCache>,
    path_intermediate_texture: Option<metal::Texture>,
    path_intermediate_msaa_texture: Option<metal::Texture>,
    path_sample_count: u32,
    /// Offscreen render target reused across `render_scene` calls when
    /// rendering headlessly without reading pixels back.
    #[cfg(any(test, feature = "bench-support", feature = "test-support"))]
    headless_render_target: Option<metal::Texture>,
}

struct BackdropTextures {
    scratch: metal::Texture,
    blurred: metal::Texture,
}

impl BackdropTextures {
    fn bytes(&self) -> u64 {
        self.scratch.width() * self.scratch.height() * 8
    }
}

#[repr(C)]
pub struct PathRasterizationVertex {
    pub xy_position: Point<ScaledPixels>,
    pub st_position: Point<f32>,
    pub color: Background,
    pub bounds: Bounds<ScaledPixels>,
}

impl MetalRenderer {
    /// Creates a new MetalRenderer with a CAMetalLayer for window-based rendering.
    pub fn new(
        instance_buffer_pool: Arc<Mutex<InstanceBufferPool>>,
        transparent: bool,
        use_display_p3: bool,
    ) -> Self {
        let device = Self::create_device();
        let layer = metal::MetalLayer::new();
        Self::configure_layer(&layer, &device, transparent, use_display_p3);
        Self::new_internal(device, Some(layer), !transparent, instance_buffer_pool)
    }

    /// Creates a renderer for a CAMetalLayer owned by a platform view, such as
    /// the backing layer UIKit creates for a view whose `layerClass` is
    /// `CAMetalLayer`. The renderer retains the layer.
    pub fn from_layer(
        instance_buffer_pool: Arc<Mutex<InstanceBufferPool>>,
        layer: &objc2_quartz_core::CAMetalLayer,
        transparent: bool,
    ) -> Self {
        let device = Self::create_device();
        // Both types bind the same Objective-C class, so this only changes which
        // Rust wrapper views the live layer. `to_owned` retains it.
        let layer = unsafe {
            metal::MetalLayerRef::from_ptr(ptr::from_ref(layer).cast_mut().cast::<CAMetalLayer>())
        }
        .to_owned();
        Self::configure_layer(&layer, &device, transparent, false);
        Self::new_internal(device, Some(layer), !transparent, instance_buffer_pool)
    }

    fn configure_layer(
        layer: &metal::MetalLayerRef,
        device: &metal::DeviceRef,
        transparent: bool,
        use_display_p3: bool,
    ) {
        layer.set_device(device);
        layer.set_pixel_format(MTLPixelFormat::BGRA8Unorm);
        // metal-rs doesn't bind these setters, so view the same object through
        // objc2's typed CAMetalLayer binding.
        let objc2_layer: &objc2_quartz_core::CAMetalLayer = unsafe { &*layer.as_ptr().cast() };
        if use_display_p3 {
            // SAFETY: The constant is extern, from Core Graphics
            let display_p3_name = unsafe { kCGColorSpaceDisplayP3 };
            if let Some(display_p3_color_space) = CGColorSpace::create_with_name(display_p3_name) {
                // SAFETY: `layer` is a CAMetalLayer and the color space remains valid for the call.
                // 'metal-rs' does not expose CAMetalLayer's typed `colorspace` setter.
                // Could be replaced with a native binding via objc2-* in future
                unsafe {
                    let _: () = objc2::msg_send![
                        objc2_layer,
                        setColorspace: display_p3_color_space.as_ptr().cast::<c_void>()
                    ];
                }
            } else {
                log::error!("unable to create the Display P3 color space");
            }
        }
        // Support direct-to-display rendering if the window is not transparent
        // https://developer.apple.com/documentation/metal/managing-your-game-window-for-metal-in-macos
        layer.set_opaque(!transparent);
        layer.set_maximum_drawable_count(3);
        // Preserve direct-to-display outside blur frames, except when screenshots
        // require readable drawables in debug builds.
        #[cfg(all(feature = "test-support", debug_assertions))]
        layer.set_framebuffer_only(false);
        objc2_layer.setAllowsNextDrawableTimeout(false);
        objc2_layer.setNeedsDisplayOnBoundsChange(true);
        // UIKit sizes a view's backing layer itself; only AppKit-hosted
        // layers need to track their superlayer's bounds.
        #[cfg(target_os = "macos")]
        objc2_layer.setAutoresizingMask(
            objc2_quartz_core::CAAutoresizingMask::LayerWidthSizable
                | objc2_quartz_core::CAAutoresizingMask::LayerHeightSizable,
        );
    }

    /// Creates a new headless MetalRenderer for offscreen rendering without a window.
    ///
    /// This renderer can render scenes to images without requiring a CAMetalLayer,
    /// window, or AppKit. Use `render_scene_to_image()` to render scenes.
    #[cfg(any(test, feature = "bench-support", feature = "test-support"))]
    pub fn new_headless(instance_buffer_pool: Arc<Mutex<InstanceBufferPool>>) -> Self {
        let device = Self::create_device();
        Self::new_internal(device, None, true, instance_buffer_pool)
    }

    #[cfg(target_os = "macos")]
    fn create_device() -> metal::Device {
        // Prefer low‐power integrated GPUs on Intel Mac. On Apple
        // Silicon, there is only ever one GPU, so this is equivalent to
        // `metal::Device::system_default()`.
        if let Some(d) = metal::Device::all()
            .into_iter()
            .min_by_key(|d| (d.is_removable(), !d.is_low_power()))
        {
            d
        } else {
            // For some reason `all()` can return an empty list, see https://github.com/zed-industries/zed/issues/37689
            // In that case, we fall back to the system default device.
            log::error!(
                "Unable to enumerate Metal devices; attempting to use system default device"
            );
            Self::system_default_device()
        }
    }

    #[cfg(target_os = "ios")]
    fn create_device() -> metal::Device {
        Self::system_default_device()
    }

    fn system_default_device() -> metal::Device {
        metal::Device::system_default().unwrap_or_else(|| {
            log::error!("unable to access a compatible graphics device");
            std::process::exit(1);
        })
    }

    fn new_internal(
        device: metal::Device,
        layer: Option<metal::MetalLayer>,
        opaque: bool,
        instance_buffer_pool: Arc<Mutex<InstanceBufferPool>>,
    ) -> Self {
        #[cfg(feature = "runtime_shaders")]
        let library = device
            .new_library_with_source(&SHADERS_SOURCE_FILE, &metal::CompileOptions::new())
            .expect("error building metal library");
        #[cfg(not(feature = "runtime_shaders"))]
        let library = device
            .new_library_with_data(SHADERS_METALLIB)
            .expect("error building metal library");

        fn to_float2_bits(point: PointF) -> u64 {
            let mut output = point.y.to_bits() as u64;
            output <<= 32;
            output |= point.x.to_bits() as u64;
            output
        }

        // Shared memory can be used only if CPU and GPU share the same memory space.
        // https://developer.apple.com/documentation/metal/setting-resource-storage-modes
        // iOS does not support managed resources. Its simulator may report a
        // non-unified host GPU even though resources must still use shared storage.
        let is_unified_memory = cfg!(target_os = "ios") || device.has_unified_memory();
        // Apple GPU families support memoryless textures, which can significantly reduce
        // memory usage by keeping render targets in on-chip tile memory instead of
        // allocating backing store in system memory.
        // https://developer.apple.com/documentation/metal/mtlgpufamily
        let is_apple_gpu = device.supports_family(MTLGPUFamily::Apple1);

        let unit_vertices = [
            to_float2_bits(point(0., 0.)),
            to_float2_bits(point(1., 0.)),
            to_float2_bits(point(0., 1.)),
            to_float2_bits(point(0., 1.)),
            to_float2_bits(point(1., 0.)),
            to_float2_bits(point(1., 1.)),
        ];
        let unit_vertices = device.new_buffer_with_data(
            unit_vertices.as_ptr() as *const c_void,
            mem::size_of_val(&unit_vertices) as u64,
            if is_unified_memory {
                MTLResourceOptions::StorageModeShared
                    | MTLResourceOptions::CPUCacheModeWriteCombined
            } else {
                MTLResourceOptions::StorageModeManaged
            },
        );

        let paths_rasterization_pipeline_state = build_path_rasterization_pipeline_state(
            &device,
            &library,
            "paths_rasterization",
            "path_rasterization_vertex",
            "path_rasterization_fragment",
            MTLPixelFormat::BGRA8Unorm,
            PATH_SAMPLE_COUNT,
        );
        let path_sprites_pipeline_state = build_path_sprite_pipeline_state(
            &device,
            &library,
            "path_sprites",
            "path_sprite_vertex",
            "path_sprite_fragment",
            MTLPixelFormat::BGRA8Unorm,
        );
        let shadows_pipeline_state = build_pipeline_state(
            &device,
            &library,
            "shadows",
            "shadow_vertex",
            "shadow_fragment",
            MTLPixelFormat::BGRA8Unorm,
        );
        let backdrop_blur_pipeline_state = build_pipeline_state_no_blend(
            &device,
            &library,
            "backdrop_blur",
            "backdrop_blur_vertex",
            "backdrop_blur_fragment",
            MTLPixelFormat::BGRA8Unorm,
        );
        let quads_pipeline_state = build_pipeline_state(
            &device,
            &library,
            "quads",
            "quad_vertex",
            "quad_fragment",
            MTLPixelFormat::BGRA8Unorm,
        );
        let underlines_pipeline_state = build_pipeline_state(
            &device,
            &library,
            "underlines",
            "underline_vertex",
            "underline_fragment",
            MTLPixelFormat::BGRA8Unorm,
        );
        let monochrome_sprites_pipeline_state = build_pipeline_state(
            &device,
            &library,
            "monochrome_sprites",
            "monochrome_sprite_vertex",
            "monochrome_sprite_fragment",
            MTLPixelFormat::BGRA8Unorm,
        );
        let polychrome_sprites_pipeline_state = build_pipeline_state(
            &device,
            &library,
            "polychrome_sprites",
            "polychrome_sprite_vertex",
            "polychrome_sprite_fragment",
            MTLPixelFormat::BGRA8Unorm,
        );
        let surfaces_pipeline_state = build_pipeline_state(
            &device,
            &library,
            "surfaces",
            "surface_vertex",
            "surface_fragment",
            MTLPixelFormat::BGRA8Unorm,
        );

        let command_queue = device.new_command_queue();
        let supports_shared_storage = cfg!(target_os = "ios") || is_apple_gpu;
        let sprite_atlas = Arc::new(MetalAtlas::new(device.clone(), supports_shared_storage));
        // metal and objc2-metal expose the same Objective-C device; the borrowed
        // protocol object remains valid while `device` owns it.
        let core_video_texture_cache = unsafe {
            let mut cache = ptr::null_mut();
            let status = CVMetalTextureCache::create(
                None,
                None,
                &*device
                    .as_ptr()
                    .cast::<objc2::runtime::ProtocolObject<dyn objc2_metal::MTLDevice>>(),
                None,
                ptr::NonNull::from(&mut cache),
            );
            assert_eq!(
                status, kCVReturnSuccess,
                "failed to create CoreVideo Metal texture cache"
            );
            CFRetained::from_raw(ptr::NonNull::new(cache).expect("CoreVideo returned a null cache"))
        };

        Self {
            device,
            layer,
            restore_framebuffer_only: false,
            presents_with_transaction: false,
            is_apple_gpu,
            is_unified_memory,
            opaque,
            command_queue,
            paths_rasterization_pipeline_state,
            path_sprites_pipeline_state,
            shadows_pipeline_state,
            backdrop_blur_pipeline_state,
            backdrop_textures: Vec::new(),
            backdrop_kernels: Vec::new(),
            blur_free_frames: 0,
            quads_pipeline_state,
            underlines_pipeline_state,
            monochrome_sprites_pipeline_state,
            polychrome_sprites_pipeline_state,
            surfaces_pipeline_state,
            unit_vertices,
            instance_buffer_pool,
            sprite_atlas,
            core_video_texture_cache,
            path_intermediate_texture: None,
            path_intermediate_msaa_texture: None,
            path_sample_count: PATH_SAMPLE_COUNT,
            #[cfg(any(test, feature = "bench-support", feature = "test-support"))]
            headless_render_target: None,
        }
    }

    pub fn layer(&self) -> Option<&metal::MetalLayerRef> {
        self.layer.as_ref().map(|l| l.as_ref())
    }

    pub fn layer_ptr(&self) -> *mut CAMetalLayer {
        self.layer
            .as_ref()
            .map(|l| l.as_ptr())
            .unwrap_or(ptr::null_mut())
    }

    pub fn sprite_atlas(&self) -> &Arc<MetalAtlas> {
        &self.sprite_atlas
    }

    pub fn set_presents_with_transaction(&mut self, presents_with_transaction: bool) {
        self.presents_with_transaction = presents_with_transaction;
        if let Some(layer) = &self.layer {
            layer.set_presents_with_transaction(presents_with_transaction);
        }
    }

    pub fn update_drawable_size(&mut self, size: Size<DevicePixels>) {
        if let Some(layer) = &self.layer {
            layer.set_drawable_size(CGSize::new(size.width.0 as f64, size.height.0 as f64));
        }
        self.update_path_intermediate_textures(size);
    }

    fn update_path_intermediate_textures(&mut self, size: Size<DevicePixels>) {
        // We are uncertain when this happens, but sometimes size can be 0 here. Most likely before
        // the layout pass on window creation. Zero-sized texture creation causes SIGABRT.
        // https://github.com/zed-industries/zed/issues/36229
        if size.width.0 <= 0 || size.height.0 <= 0 {
            self.path_intermediate_texture = None;
            self.path_intermediate_msaa_texture = None;
            return;
        }

        let texture_descriptor = metal::TextureDescriptor::new();
        texture_descriptor.set_width(size.width.0 as u64);
        texture_descriptor.set_height(size.height.0 as u64);
        texture_descriptor.set_pixel_format(metal::MTLPixelFormat::BGRA8Unorm);
        texture_descriptor.set_storage_mode(metal::MTLStorageMode::Private);
        texture_descriptor
            .set_usage(metal::MTLTextureUsage::RenderTarget | metal::MTLTextureUsage::ShaderRead);
        self.path_intermediate_texture = Some(self.device.new_texture(&texture_descriptor));

        if self.path_sample_count > 1 {
            // https://developer.apple.com/documentation/metal/choosing-a-resource-storage-mode-for-apple-gpus
            // Rendering MSAA textures are done in a single pass, so we can use memory-less storage on Apple Silicon
            let storage_mode = if self.is_apple_gpu {
                metal::MTLStorageMode::Memoryless
            } else {
                metal::MTLStorageMode::Private
            };

            let msaa_descriptor = texture_descriptor;
            msaa_descriptor.set_texture_type(metal::MTLTextureType::D2Multisample);
            msaa_descriptor.set_storage_mode(storage_mode);
            msaa_descriptor.set_sample_count(self.path_sample_count as _);
            self.path_intermediate_msaa_texture = Some(self.device.new_texture(&msaa_descriptor));
        } else {
            self.path_intermediate_msaa_texture = None;
        }
    }

    pub fn update_transparency(&mut self, transparent: bool) {
        self.opaque = !transparent;
        if let Some(layer) = &self.layer {
            layer.set_opaque(!transparent);
        }
    }

    pub fn destroy(&self) {
        // nothing to do
    }

    pub fn draw(&mut self, scene: &Scene) {
        let layer = match &self.layer {
            Some(l) => l.clone(),
            None => {
                log::error!(
                    "draw() called on headless renderer - use render_scene_to_image() instead"
                );
                return;
            }
        };
        if scene.backdrop_blurs.is_empty() {
            if self.restore_framebuffer_only {
                layer.set_framebuffer_only(true);
                self.restore_framebuffer_only = false;
            }
        } else if layer.framebuffer_only() {
            layer.set_framebuffer_only(false);
            self.restore_framebuffer_only = true;
        }
        let viewport_size = layer.drawable_size();
        let viewport_size: Size<DevicePixels> = size(
            (viewport_size.width.ceil() as i32).into(),
            (viewport_size.height.ceil() as i32).into(),
        );
        let drawable = if let Some(drawable) = layer.next_drawable() {
            drawable
        } else {
            log::error!(
                "failed to retrieve next drawable, drawable size: {:?}",
                viewport_size
            );
            return;
        };

        let command_buffer = match self.render_frame(scene, drawable.texture(), viewport_size) {
            Ok(command_buffer) => command_buffer,
            Err(error) => {
                log::error!("failed to render: {error:#}");
                return;
            }
        };

        if self.presents_with_transaction {
            command_buffer.commit();
            command_buffer.wait_until_scheduled();
            drawable.present();
        } else {
            command_buffer.present_drawable(drawable);
            command_buffer.commit();
        }
    }

    fn render_frame(
        &mut self,
        scene: &Scene,
        texture: &metal::TextureRef,
        viewport_size: Size<DevicePixels>,
    ) -> Result<metal::CommandBuffer> {
        let mut writer = InstanceBufferWriter::new(
            &self.device,
            &self.instance_buffer_pool,
            self.is_unified_memory,
        );
        let instance_bindings = write_instances(scene, &mut writer).with_context(|| {
            format!(
                "scene too large: {} paths, {} shadows, {} quads, {} underlines, {} mono, {} poly, {} surfaces",
                scene.paths.len(),
                scene.shadows.len(),
                scene.quads.len(),
                scene.underlines.len(),
                scene.monochrome_sprites.len(),
                scene.polychrome_sprites.len(),
                scene.surfaces.len(),
            )
        })?;
        let command_buffer = self.draw_primitives_to_texture(
            scene,
            &instance_bindings,
            &mut writer,
            texture,
            viewport_size,
        )?;

        let instance_buffer_pool = self.instance_buffer_pool.clone();
        let instance_buffer = Cell::new(Some(writer.finish()));
        let block = RcBlock::new(move |_: ptr::NonNull<AnyObject>| {
            if let Some(instance_buffer) = instance_buffer.take() {
                instance_buffer_pool.lock().release(instance_buffer);
            }
        });
        // SAFETY: Both pointee types are opaque views of the same Objective-C block pointer ABI.
        unsafe {
            command_buffer.add_completed_handler(&*RcBlock::as_ptr(&block).cast());
        }

        Ok(command_buffer)
    }

    /// Renders the scene to a texture and returns the pixel data as an RGBA image.
    /// This does not present the frame to screen - useful for visual testing
    /// where we want to capture what would be rendered without displaying it.
    ///
    /// Note: This requires a layer-backed renderer. For headless rendering,
    /// use `render_scene_to_image()` instead.
    #[cfg(any(test, feature = "test-support"))]
    pub fn render_to_image(&mut self, scene: &Scene) -> Result<RgbaImage> {
        let layer = self
            .layer
            .clone()
            .ok_or_else(|| anyhow::anyhow!("render_to_image requires a layer-backed renderer"))?;
        // Reading the result requires a non-framebuffer-only drawable.
        let framebuffer_only = layer.framebuffer_only();
        if framebuffer_only {
            layer.set_framebuffer_only(false);
        }
        let result = (|| {
            let viewport_size = layer.drawable_size();
            let viewport_size: Size<DevicePixels> = size(
                (viewport_size.width.ceil() as i32).into(),
                (viewport_size.height.ceil() as i32).into(),
            );
            let drawable = layer
                .next_drawable()
                .ok_or_else(|| anyhow::anyhow!("Failed to get drawable for render_to_image"))?;
            let command_buffer = self.render_frame(scene, drawable.texture(), viewport_size)?;
            command_buffer.commit();
            command_buffer.wait_until_completed();
            read_texture_to_image(drawable.texture())
        })();
        if framebuffer_only {
            layer.set_framebuffer_only(true);
        }
        result
    }

    /// Renders a scene to an image without requiring a window or CAMetalLayer.
    ///
    /// This is the primary method for headless rendering. It creates an offscreen
    /// texture, renders the scene to it, and returns the pixel data as an RGBA image.
    #[cfg(any(test, feature = "bench-support", feature = "test-support"))]
    pub fn render_scene_to_image(
        &mut self,
        scene: &Scene,
        size: Size<DevicePixels>,
    ) -> Result<RgbaImage> {
        if size.width.0 <= 0 || size.height.0 <= 0 {
            anyhow::bail!("Invalid size for render_scene_to_image: {:?}", size);
        }

        // Headless callers do not have a Cocoa event-loop pool to release
        // autoreleased command buffers and render-pass descriptors.
        objc2::rc::autoreleasepool(|_| {
            // Update path intermediate textures for this size
            self.update_path_intermediate_textures(size);

            // Create an offscreen texture as render target
            let texture_descriptor = metal::TextureDescriptor::new();
            texture_descriptor.set_width(size.width.0 as u64);
            texture_descriptor.set_height(size.height.0 as u64);
            texture_descriptor.set_pixel_format(MTLPixelFormat::BGRA8Unorm);
            texture_descriptor.set_usage(
                metal::MTLTextureUsage::RenderTarget | metal::MTLTextureUsage::ShaderRead,
            );
            // Like the atlas, only Apple GPUs can create shared textures on macOS;
            // Intel Macs cannot, even with unified memory. iOS has no managed storage.
            let uses_shared_storage = cfg!(target_os = "ios") || self.is_apple_gpu;
            texture_descriptor.set_storage_mode(if uses_shared_storage {
                metal::MTLStorageMode::Shared
            } else {
                metal::MTLStorageMode::Managed
            });
            let target_texture = self.device.new_texture(&texture_descriptor);

            let command_buffer = self.render_frame(scene, &target_texture, size)?;

            // Managed textures require an explicit blit synchronize before the CPU
            // can read back the rendered data. Without this, get_bytes returns
            // stale zeros.
            if !uses_shared_storage {
                let blit = command_buffer.new_blit_command_encoder();
                blit.synchronize_resource(&target_texture);
                blit.end_encoding();
            }

            // Commit and wait for completion
            command_buffer.commit();
            command_buffer.wait_until_completed();

            read_texture_to_image(&target_texture)
        })
    }

    /// Renders a scene to a reused offscreen texture without reading pixels
    /// back or blocking on GPU completion.
    ///
    /// This mirrors the CPU cost of presenting a frame to a window (scene
    /// encoding, instance buffer writes, command submission) and is used by
    /// headless benchmark rendering, where the produced pixels are never
    /// inspected.
    #[cfg(any(test, feature = "bench-support", feature = "test-support"))]
    pub fn render_scene(&mut self, scene: &Scene, size: Size<DevicePixels>) -> Result<()> {
        if size.width.0 <= 0 || size.height.0 <= 0 {
            anyhow::bail!("Invalid size for render_scene: {:?}", size);
        }

        objc2::rc::autoreleasepool(|_| {
            self.update_path_intermediate_textures(size);

            let needs_new_target = self.headless_render_target.as_ref().is_none_or(|texture| {
                texture.width() != size.width.0 as u64 || texture.height() != size.height.0 as u64
            });
            if needs_new_target {
                let texture_descriptor = metal::TextureDescriptor::new();
                texture_descriptor.set_width(size.width.0 as u64);
                texture_descriptor.set_height(size.height.0 as u64);
                texture_descriptor.set_pixel_format(MTLPixelFormat::BGRA8Unorm);
                texture_descriptor.set_usage(
                    metal::MTLTextureUsage::RenderTarget | metal::MTLTextureUsage::ShaderRead,
                );
                texture_descriptor.set_storage_mode(metal::MTLStorageMode::Private);
                self.headless_render_target = Some(self.device.new_texture(&texture_descriptor));
            }
            let target_texture = self
                .headless_render_target
                .clone()
                .expect("just ensured the render target exists");

            let command_buffer = self.render_frame(scene, &target_texture, size)?;

            // Commit without waiting, mirroring presentation to a real window where
            // the CPU doesn't block on the GPU.
            command_buffer.commit();
            Ok(())
        })
    }

    fn draw_primitives_to_texture(
        &mut self,
        scene: &Scene,
        instance_bindings: &InstanceBindings,
        writer: &mut InstanceBufferWriter,
        texture: &metal::TextureRef,
        viewport_size: Size<DevicePixels>,
    ) -> Result<metal::CommandBuffer> {
        let command_queue = self.command_queue.clone();
        let command_buffer = command_queue.new_command_buffer();
        let alpha = if self.opaque { 1. } else { 0. };
        self.backdrop_textures.retain(|pair| {
            pair.scratch.width() <= texture.width()
                && pair.scratch.height() <= texture.height()
                && pair.scratch.pixel_format() == texture.pixel_format()
        });
        if scene.backdrop_blurs.is_empty() {
            self.blur_free_frames = self.blur_free_frames.saturating_add(1);
            if self.blur_free_frames >= BACKDROP_RELEASE_AFTER_FRAMES {
                self.backdrop_textures.clear();
                self.backdrop_kernels.clear();
            }
        } else {
            self.blur_free_frames = 0;
        }

        let mut command_encoder = new_command_encoder_for_texture(
            command_buffer,
            texture,
            viewport_size,
            Some(metal::MTLClearColor::new(0., 0., 0., alpha)),
        );

        let mut pending_blurs = scene.backdrop_blurs.iter().enumerate().peekable();
        for batch in scene.batches() {
            while pending_blurs
                .peek()
                .is_some_and(|(_, blur)| blur.order <= batch.first_order(scene))
            {
                let (index, blur) = pending_blurs.next().unwrap();
                command_encoder = self.draw_backdrop_blur(
                    blur,
                    index,
                    instance_bindings,
                    texture,
                    viewport_size,
                    command_buffer,
                    command_encoder,
                );
            }
            match batch {
                PrimitiveBatch::Shadows(range) => {
                    self.draw_shadows(range, instance_bindings, viewport_size, command_encoder)
                }
                PrimitiveBatch::Quads(range) => {
                    self.draw_quads(range, instance_bindings, viewport_size, command_encoder)
                }
                PrimitiveBatch::Paths(range) => {
                    let mut paths = &scene.paths[range];
                    while let Some(first) = paths.first() {
                        // A clipped path must not sample another path's rasterized pixels.
                        // Preserve the existing fast batch for consecutive unclipped paths.
                        let count = if first.clip.right > first.clip.left {
                            1
                        } else {
                            paths
                                .iter()
                                .position(|path| path.clip.right > path.clip.left)
                                .unwrap_or(paths.len())
                        };
                        let (group, remaining) = paths.split_at(count);
                        command_encoder.end_encoding();

                        let did_draw = self.draw_paths_to_intermediate(
                            group,
                            writer,
                            viewport_size,
                            command_buffer,
                        )?;

                        command_encoder = new_command_encoder_for_texture(
                            command_buffer,
                            texture,
                            viewport_size,
                            None,
                        );

                        if did_draw {
                            if let Err(error) = self.draw_paths_from_intermediate(
                                group,
                                writer,
                                viewport_size,
                                command_encoder,
                            ) {
                                command_encoder.end_encoding();
                                return Err(error);
                            }
                        }
                        paths = remaining;
                    }
                }
                PrimitiveBatch::Underlines(range) => {
                    self.draw_underlines(range, instance_bindings, viewport_size, command_encoder)
                }
                PrimitiveBatch::MonochromeSprites { texture_id, range } => self
                    .draw_monochrome_sprites(
                        texture_id,
                        range,
                        instance_bindings,
                        viewport_size,
                        command_encoder,
                    ),
                PrimitiveBatch::PolychromeSprites { texture_id, range } => self
                    .draw_polychrome_sprites(
                        texture_id,
                        range,
                        instance_bindings,
                        viewport_size,
                        command_encoder,
                    ),
                PrimitiveBatch::Surfaces(range) => self.draw_surfaces(
                    &scene.surfaces[range.clone()],
                    range.start,
                    instance_bindings,
                    viewport_size,
                    command_encoder,
                ),
                PrimitiveBatch::SubpixelSprites { .. } => unreachable!(),
            }
        }
        for (index, blur) in pending_blurs {
            command_encoder = self.draw_backdrop_blur(
                blur,
                index,
                instance_bindings,
                texture,
                viewport_size,
                command_buffer,
                command_encoder,
            );
        }

        command_encoder.end_encoding();

        Ok(command_buffer.to_owned())
    }

    fn draw_backdrop_blur<'a>(
        &mut self,
        blur: &BackdropBlur,
        index: usize,
        bindings: &InstanceBindings,
        texture: &'a metal::TextureRef,
        viewport_size: Size<DevicePixels>,
        command_buffer: &'a metal::CommandBufferRef,
        command_encoder: &'a metal::RenderCommandEncoderRef,
    ) -> &'a metal::RenderCommandEncoderRef {
        let Some((x, y, width, height, sigma)) =
            backdrop_region(blur, texture.width(), texture.height())
        else {
            return command_encoder;
        };

        command_encoder.end_encoding();
        let (scratch, blurred) = self.ensure_backdrop_scratch(
            width,
            height,
            texture.width(),
            texture.height(),
            texture.pixel_format(),
        );
        // Fill the entire cached texture so the Gaussian never samples stale pixels.
        let copy_x = x.min(texture.width() - scratch.width());
        let copy_y = y.min(texture.height() - scratch.height());
        let blit = command_buffer.new_blit_command_encoder();
        blit.copy_from_texture(
            texture,
            0,
            0,
            metal::MTLOrigin {
                x: copy_x,
                y: copy_y,
                z: 0,
            },
            metal::MTLSize {
                width: scratch.width(),
                height: scratch.height(),
                depth: 1,
            },
            &scratch,
            0,
            0,
            metal::MTLOrigin { x: 0, y: 0, z: 0 },
        );
        blit.end_encoding();

        let kernel = self.ensure_gaussian_kernel(sigma);
        // SAFETY: these are the retained Metal/MPS objects used by this command buffer.
        unsafe {
            let _: () = msg_send![
                kernel,
                encodeToCommandBuffer: command_buffer.as_ptr().cast::<AnyObject>(),
                sourceTexture: scratch.as_ptr().cast::<AnyObject>(),
                destinationTexture: blurred.as_ptr().cast::<AnyObject>()
            ];
        }
        let command_encoder =
            new_command_encoder_for_texture(command_buffer, texture, viewport_size, None);
        let source_rect = [
            copy_x as f32,
            copy_y as f32,
            scratch.width() as f32,
            scratch.height() as f32,
        ];
        command_encoder.set_render_pipeline_state(&self.backdrop_blur_pipeline_state);
        command_encoder.set_vertex_buffer(
            BackdropBlurInputIndex::Vertices as u64,
            Some(&self.unit_vertices),
            0,
        );
        command_encoder.set_vertex_buffer(
            BackdropBlurInputIndex::Blurs as u64,
            Some(&bindings.backdrop_blurs.buffer),
            bindings.backdrop_blurs.offset as u64,
        );
        command_encoder.set_fragment_buffer(
            BackdropBlurInputIndex::Blurs as u64,
            Some(&bindings.backdrop_blurs.buffer),
            bindings.backdrop_blurs.offset as u64,
        );
        command_encoder.set_vertex_bytes(
            BackdropBlurInputIndex::ViewportSize as u64,
            mem::size_of_val(&viewport_size) as u64,
            &viewport_size as *const Size<DevicePixels> as *const _,
        );
        command_encoder.set_fragment_bytes(
            BackdropBlurInputIndex::SourceRect as u64,
            mem::size_of_val(&source_rect) as u64,
            source_rect.as_ptr() as *const _,
        );
        command_encoder
            .set_fragment_texture(BackdropBlurInputIndex::SourceTexture as u64, Some(&blurred));
        command_encoder.set_fragment_texture(
            BackdropBlurInputIndex::SnapshotTexture as u64,
            Some(&scratch),
        );
        command_encoder.draw_primitives_instanced_base_instance(
            metal::MTLPrimitiveType::Triangle,
            0,
            6,
            1,
            index as u64,
        );
        command_encoder
    }

    fn ensure_backdrop_scratch(
        &mut self,
        needed_width: u64,
        needed_height: u64,
        drawable_width: u64,
        drawable_height: u64,
        format: MTLPixelFormat,
    ) -> (metal::Texture, metal::Texture) {
        const QUANTUM: u64 = 64;
        let width = (needed_width.div_ceil(QUANTUM) * QUANTUM).min(drawable_width);
        let height = (needed_height.div_ceil(QUANTUM) * QUANTUM).min(drawable_height);
        self.backdrop_textures.retain(|pair| {
            pair.scratch.width() <= drawable_width
                && pair.scratch.height() <= drawable_height
                && pair.scratch.pixel_format() == format
        });
        if let Some(index) = self.backdrop_textures.iter().position(|pair| {
            pair.scratch.width() == width
                && pair.scratch.height() == height
                && pair.scratch.pixel_format() == format
        }) {
            let pair = self.backdrop_textures.remove(index);
            self.backdrop_textures.push(pair);
        } else {
            let descriptor = metal::TextureDescriptor::new();
            descriptor.set_texture_type(metal::MTLTextureType::D2);
            descriptor.set_pixel_format(format);
            descriptor.set_width(width);
            descriptor.set_height(height);
            descriptor.set_storage_mode(metal::MTLStorageMode::Private);
            descriptor.set_usage(metal::MTLTextureUsage::ShaderRead);
            let scratch = self.device.new_texture(&descriptor);
            descriptor.set_usage(
                metal::MTLTextureUsage::ShaderRead | metal::MTLTextureUsage::ShaderWrite,
            );
            let blurred = self.device.new_texture(&descriptor);
            self.backdrop_textures
                .push(BackdropTextures { scratch, blurred });
        }
        let budget = (drawable_width * drawable_height * 16)
            .min(32 * 1024 * 1024)
            .max(width * height * 8);
        while self.backdrop_textures.len() > 1
            && (self.backdrop_textures.len() > 4
                || self
                    .backdrop_textures
                    .iter()
                    .map(BackdropTextures::bytes)
                    .sum::<u64>()
                    > budget)
        {
            self.backdrop_textures.remove(0);
        }
        let pair = self.backdrop_textures.last().unwrap();
        (pair.scratch.clone(), pair.blurred.clone())
    }

    fn ensure_gaussian_kernel(&mut self, sigma: f32) -> *mut AnyObject {
        if let Some(index) = self
            .backdrop_kernels
            .iter()
            .position(|(cached, _)| (*cached - sigma).abs() < 0.01)
        {
            let entry = self.backdrop_kernels.remove(index);
            self.backdrop_kernels.push(entry);
        } else {
            if self.backdrop_kernels.len() >= 4 {
                self.backdrop_kernels.remove(0);
            }
            // SAFETY: MPS owns the initialized kernel; Retained releases it on eviction.
            let kernel = unsafe {
                let allocated: *mut AnyObject = msg_send![class!(MPSImageGaussianBlur), alloc];
                let kernel: *mut AnyObject = msg_send![
                    allocated,
                    initWithDevice: self.device.as_ptr().cast::<AnyObject>(),
                    sigma: sigma
                ];
                let kernel =
                    Retained::from_raw(kernel).expect("failed to create MPS Gaussian blur");
                let _: () = msg_send![&*kernel, setEdgeMode: 1u64];
                kernel
            };
            self.backdrop_kernels.push((sigma, kernel));
        }
        Retained::as_ptr(&self.backdrop_kernels.last().unwrap().1).cast_mut()
    }

    fn draw_paths_to_intermediate(
        &self,
        paths: &[Path<ScaledPixels>],
        writer: &mut InstanceBufferWriter,
        viewport_size: Size<DevicePixels>,
        command_buffer: &metal::CommandBufferRef,
    ) -> Result<bool> {
        if paths.is_empty() {
            return Ok(false);
        }
        let intermediate_texture = self
            .path_intermediate_texture
            .as_ref()
            .context("missing path intermediate texture")?;

        let mut vertices = Vec::new();
        for path in paths {
            vertices.extend(path.vertices.iter().map(|v| PathRasterizationVertex {
                xy_position: v.xy_position,
                st_position: v.st_position,
                color: path.color,
                bounds: path.bounds.intersect(&path.content_mask.bounds),
            }));
        }
        let vertex_instance_bindings = writer.write(&vertices)?;

        let render_pass_descriptor = metal::RenderPassDescriptor::new();
        let color_attachment = render_pass_descriptor
            .color_attachments()
            .object_at(0)
            .unwrap();
        color_attachment.set_load_action(metal::MTLLoadAction::Clear);
        color_attachment.set_clear_color(metal::MTLClearColor::new(0., 0., 0., 0.));

        if let Some(msaa_texture) = &self.path_intermediate_msaa_texture {
            color_attachment.set_texture(Some(msaa_texture));
            color_attachment.set_resolve_texture(Some(intermediate_texture));
            color_attachment.set_store_action(metal::MTLStoreAction::MultisampleResolve);
        } else {
            color_attachment.set_texture(Some(intermediate_texture));
            color_attachment.set_store_action(metal::MTLStoreAction::Store);
        }

        let command_encoder = command_buffer.new_render_command_encoder(render_pass_descriptor);
        command_encoder.set_render_pipeline_state(&self.paths_rasterization_pipeline_state);
        command_encoder.set_vertex_buffer(
            PathRasterizationInputIndex::Vertices as u64,
            Some(&vertex_instance_bindings.buffer),
            vertex_instance_bindings.offset as u64,
        );
        command_encoder.set_vertex_bytes(
            PathRasterizationInputIndex::ViewportSize as u64,
            mem::size_of_val(&viewport_size) as u64,
            &viewport_size as *const Size<DevicePixels> as *const _,
        );
        command_encoder.set_fragment_buffer(
            PathRasterizationInputIndex::Vertices as u64,
            Some(&vertex_instance_bindings.buffer),
            vertex_instance_bindings.offset as u64,
        );
        command_encoder.draw_primitives(
            metal::MTLPrimitiveType::Triangle,
            0,
            vertices.len() as u64,
        );

        command_encoder.end_encoding();
        Ok(true)
    }

    fn draw_shadows(
        &self,
        shadows: Range<usize>,
        instance_bindings: &InstanceBindings,
        viewport_size: Size<DevicePixels>,
        command_encoder: &metal::RenderCommandEncoderRef,
    ) {
        if shadows.is_empty() {
            return;
        }

        command_encoder.set_render_pipeline_state(&self.shadows_pipeline_state);
        command_encoder.set_vertex_buffer(
            ShadowInputIndex::Vertices as u64,
            Some(&self.unit_vertices),
            0,
        );
        command_encoder.set_vertex_buffer(
            ShadowInputIndex::Shadows as u64,
            Some(&instance_bindings.shadows.buffer),
            instance_bindings.shadows.offset as u64,
        );
        command_encoder.set_fragment_buffer(
            ShadowInputIndex::Shadows as u64,
            Some(&instance_bindings.shadows.buffer),
            instance_bindings.shadows.offset as u64,
        );
        command_encoder.set_vertex_bytes(
            ShadowInputIndex::ViewportSize as u64,
            mem::size_of_val(&viewport_size) as u64,
            &viewport_size as *const Size<DevicePixels> as *const _,
        );

        command_encoder.draw_primitives_instanced_base_instance(
            metal::MTLPrimitiveType::Triangle,
            0,
            6,
            shadows.len() as u64,
            shadows.start as u64,
        );
    }

    fn draw_quads(
        &self,
        quads: Range<usize>,
        instance_bindings: &InstanceBindings,
        viewport_size: Size<DevicePixels>,
        command_encoder: &metal::RenderCommandEncoderRef,
    ) {
        if quads.is_empty() {
            return;
        }

        command_encoder.set_render_pipeline_state(&self.quads_pipeline_state);
        command_encoder.set_vertex_buffer(
            QuadInputIndex::Vertices as u64,
            Some(&self.unit_vertices),
            0,
        );
        command_encoder.set_vertex_buffer(
            QuadInputIndex::Quads as u64,
            Some(&instance_bindings.quads.buffer),
            instance_bindings.quads.offset as u64,
        );
        command_encoder.set_fragment_buffer(
            QuadInputIndex::Quads as u64,
            Some(&instance_bindings.quads.buffer),
            instance_bindings.quads.offset as u64,
        );
        command_encoder.set_vertex_bytes(
            QuadInputIndex::ViewportSize as u64,
            mem::size_of_val(&viewport_size) as u64,
            &viewport_size as *const Size<DevicePixels> as *const _,
        );

        command_encoder.draw_primitives_instanced_base_instance(
            metal::MTLPrimitiveType::Triangle,
            0,
            6,
            quads.len() as u64,
            quads.start as u64,
        );
    }

    fn draw_paths_from_intermediate(
        &self,
        paths: &[Path<ScaledPixels>],
        writer: &mut InstanceBufferWriter,
        viewport_size: Size<DevicePixels>,
        command_encoder: &metal::RenderCommandEncoderRef,
    ) -> Result<()> {
        let Some(first_path) = paths.first() else {
            return Ok(());
        };
        let intermediate_texture = self
            .path_intermediate_texture
            .as_ref()
            .context("missing path intermediate texture")?;

        command_encoder.set_render_pipeline_state(&self.path_sprites_pipeline_state);
        command_encoder.set_vertex_buffer(
            SpriteInputIndex::Vertices as u64,
            Some(&self.unit_vertices),
            0,
        );
        command_encoder.set_vertex_bytes(
            SpriteInputIndex::ViewportSize as u64,
            mem::size_of_val(&viewport_size) as u64,
            &viewport_size as *const Size<DevicePixels> as *const _,
        );

        command_encoder.set_fragment_texture(
            SpriteInputIndex::AtlasTexture as u64,
            Some(intermediate_texture),
        );

        // Clipped paths arrive individually. Unclipped batches retain the
        // existing per-bound copy at one order, or one union across orders.
        let sprites;
        if paths.last().unwrap().order == first_path.order {
            sprites = paths
                .iter()
                .map(|path| PathSprite {
                    bounds: path.clipped_bounds(),
                    clip: path.clip,
                })
                .collect();
        } else {
            let mut bounds = first_path.clipped_bounds();
            for path in paths.iter().skip(1) {
                bounds = bounds.union(&path.clipped_bounds());
            }
            sprites = vec![PathSprite {
                bounds,
                clip: RoundedClipParams::default(),
            }];
        }

        let sprite_instance_bindings = writer.write(&sprites)?;
        command_encoder.set_vertex_buffer(
            SpriteInputIndex::Sprites as u64,
            Some(&sprite_instance_bindings.buffer),
            sprite_instance_bindings.offset as u64,
        );
        command_encoder.set_fragment_buffer(
            SpriteInputIndex::Sprites as u64,
            Some(&sprite_instance_bindings.buffer),
            sprite_instance_bindings.offset as u64,
        );

        command_encoder.draw_primitives_instanced(
            metal::MTLPrimitiveType::Triangle,
            0,
            6,
            sprites.len() as u64,
        );
        Ok(())
    }

    fn draw_underlines(
        &self,
        underlines: Range<usize>,
        instance_bindings: &InstanceBindings,
        viewport_size: Size<DevicePixels>,
        command_encoder: &metal::RenderCommandEncoderRef,
    ) {
        if underlines.is_empty() {
            return;
        }

        command_encoder.set_render_pipeline_state(&self.underlines_pipeline_state);
        command_encoder.set_vertex_buffer(
            UnderlineInputIndex::Vertices as u64,
            Some(&self.unit_vertices),
            0,
        );
        command_encoder.set_vertex_buffer(
            UnderlineInputIndex::Underlines as u64,
            Some(&instance_bindings.underlines.buffer),
            instance_bindings.underlines.offset as u64,
        );
        command_encoder.set_fragment_buffer(
            UnderlineInputIndex::Underlines as u64,
            Some(&instance_bindings.underlines.buffer),
            instance_bindings.underlines.offset as u64,
        );
        command_encoder.set_vertex_bytes(
            UnderlineInputIndex::ViewportSize as u64,
            mem::size_of_val(&viewport_size) as u64,
            &viewport_size as *const Size<DevicePixels> as *const _,
        );

        command_encoder.draw_primitives_instanced_base_instance(
            metal::MTLPrimitiveType::Triangle,
            0,
            6,
            underlines.len() as u64,
            underlines.start as u64,
        );
    }

    fn draw_monochrome_sprites(
        &self,
        texture_id: AtlasTextureId,
        sprites: Range<usize>,
        instance_bindings: &InstanceBindings,
        viewport_size: Size<DevicePixels>,
        command_encoder: &metal::RenderCommandEncoderRef,
    ) {
        if sprites.is_empty() {
            return;
        }

        let Some(texture) = self.sprite_atlas.metal_texture(texture_id) else {
            return;
        };
        let texture_size = size(
            DevicePixels(texture.width() as i32),
            DevicePixels(texture.height() as i32),
        );
        command_encoder.set_render_pipeline_state(&self.monochrome_sprites_pipeline_state);
        command_encoder.set_vertex_buffer(
            SpriteInputIndex::Vertices as u64,
            Some(&self.unit_vertices),
            0,
        );
        command_encoder.set_vertex_buffer(
            SpriteInputIndex::Sprites as u64,
            Some(&instance_bindings.monochrome_sprites.buffer),
            instance_bindings.monochrome_sprites.offset as u64,
        );
        command_encoder.set_vertex_bytes(
            SpriteInputIndex::ViewportSize as u64,
            mem::size_of_val(&viewport_size) as u64,
            &viewport_size as *const Size<DevicePixels> as *const _,
        );
        command_encoder.set_vertex_bytes(
            SpriteInputIndex::AtlasTextureSize as u64,
            mem::size_of_val(&texture_size) as u64,
            &texture_size as *const Size<DevicePixels> as *const _,
        );
        command_encoder.set_fragment_buffer(
            SpriteInputIndex::Sprites as u64,
            Some(&instance_bindings.monochrome_sprites.buffer),
            instance_bindings.monochrome_sprites.offset as u64,
        );
        command_encoder.set_fragment_texture(SpriteInputIndex::AtlasTexture as u64, Some(&texture));

        command_encoder.draw_primitives_instanced_base_instance(
            metal::MTLPrimitiveType::Triangle,
            0,
            6,
            sprites.len() as u64,
            sprites.start as u64,
        );
    }

    fn draw_polychrome_sprites(
        &self,
        texture_id: AtlasTextureId,
        sprites: Range<usize>,
        instance_bindings: &InstanceBindings,
        viewport_size: Size<DevicePixels>,
        command_encoder: &metal::RenderCommandEncoderRef,
    ) {
        if sprites.is_empty() {
            return;
        }

        let Some(texture) = self.sprite_atlas.metal_texture(texture_id) else {
            return;
        };
        let texture_size = size(
            DevicePixels(texture.width() as i32),
            DevicePixels(texture.height() as i32),
        );
        command_encoder.set_render_pipeline_state(&self.polychrome_sprites_pipeline_state);
        command_encoder.set_vertex_buffer(
            SpriteInputIndex::Vertices as u64,
            Some(&self.unit_vertices),
            0,
        );
        command_encoder.set_vertex_buffer(
            SpriteInputIndex::Sprites as u64,
            Some(&instance_bindings.polychrome_sprites.buffer),
            instance_bindings.polychrome_sprites.offset as u64,
        );
        command_encoder.set_vertex_bytes(
            SpriteInputIndex::ViewportSize as u64,
            mem::size_of_val(&viewport_size) as u64,
            &viewport_size as *const Size<DevicePixels> as *const _,
        );
        command_encoder.set_vertex_bytes(
            SpriteInputIndex::AtlasTextureSize as u64,
            mem::size_of_val(&texture_size) as u64,
            &texture_size as *const Size<DevicePixels> as *const _,
        );
        command_encoder.set_fragment_buffer(
            SpriteInputIndex::Sprites as u64,
            Some(&instance_bindings.polychrome_sprites.buffer),
            instance_bindings.polychrome_sprites.offset as u64,
        );
        command_encoder.set_fragment_texture(SpriteInputIndex::AtlasTexture as u64, Some(&texture));

        command_encoder.draw_primitives_instanced_base_instance(
            metal::MTLPrimitiveType::Triangle,
            0,
            6,
            sprites.len() as u64,
            sprites.start as u64,
        );
    }

    fn draw_surfaces(
        &mut self,
        surfaces: &[PaintSurface],
        first_surface: usize,
        instance_bindings: &InstanceBindings,
        viewport_size: Size<DevicePixels>,
        command_encoder: &metal::RenderCommandEncoderRef,
    ) {
        if surfaces.is_empty() {
            return;
        }

        command_encoder.set_render_pipeline_state(&self.surfaces_pipeline_state);
        command_encoder.set_vertex_buffer(
            SurfaceInputIndex::Vertices as u64,
            Some(&self.unit_vertices),
            0,
        );
        command_encoder.set_vertex_buffer(
            SurfaceInputIndex::Surfaces as u64,
            Some(&instance_bindings.surfaces.buffer),
            instance_bindings.surfaces.offset as u64,
        );
        command_encoder.set_fragment_buffer(
            SurfaceInputIndex::Surfaces as u64,
            Some(&instance_bindings.surfaces.buffer),
            instance_bindings.surfaces.offset as u64,
        );
        command_encoder.set_vertex_bytes(
            SurfaceInputIndex::ViewportSize as u64,
            mem::size_of_val(&viewport_size) as u64,
            &viewport_size as *const Size<DevicePixels> as *const _,
        );

        for (index, surface) in surfaces.iter().enumerate() {
            let image_buffer = &surface.image_buffer;
            let texture_size = size(
                DevicePixels::from(CVPixelBufferGetWidth(image_buffer) as i32),
                DevicePixels::from(CVPixelBufferGetHeight(image_buffer) as i32),
            );

            assert_eq!(
                CVPixelBufferGetPixelFormatType(image_buffer),
                kCVPixelFormatType_420YpCbCr8BiPlanarFullRange
            );

            let plane_texture = |plane, format| unsafe {
                let mut texture = ptr::null_mut();
                let status = CVMetalTextureCache::create_texture_from_image(
                    None,
                    &self.core_video_texture_cache,
                    image_buffer,
                    None,
                    format,
                    CVPixelBufferGetWidthOfPlane(image_buffer, plane),
                    CVPixelBufferGetHeightOfPlane(image_buffer, plane),
                    plane,
                    ptr::NonNull::from(&mut texture),
                );
                assert_eq!(
                    status, kCVReturnSuccess,
                    "failed to create CoreVideo Metal texture"
                );
                CFRetained::<CVMetalTexture>::from_raw(
                    ptr::NonNull::new(texture).expect("CoreVideo returned a null texture"),
                )
            };
            let y_texture = plane_texture(0, objc2_metal::MTLPixelFormat::R8Unorm);
            let cb_cr_texture = plane_texture(1, objc2_metal::MTLPixelFormat::RG8Unorm);

            command_encoder.set_vertex_bytes(
                SurfaceInputIndex::TextureSize as u64,
                mem::size_of_val(&texture_size) as u64,
                &texture_size as *const Size<DevicePixels> as *const _,
            );
            command_encoder.set_fragment_texture(SurfaceInputIndex::YTexture as u64, unsafe {
                let texture = CVMetalTextureGetTexture(&y_texture)
                    .expect("CoreVideo returned no Metal texture");
                Some(metal::TextureRef::from_ptr(
                    objc2::rc::Retained::as_ptr(&texture) as *mut _,
                ))
            });
            command_encoder.set_fragment_texture(SurfaceInputIndex::CbCrTexture as u64, unsafe {
                let texture = CVMetalTextureGetTexture(&cb_cr_texture)
                    .expect("CoreVideo returned no Metal texture");
                Some(metal::TextureRef::from_ptr(
                    objc2::rc::Retained::as_ptr(&texture) as *mut _,
                ))
            });

            command_encoder.draw_primitives_instanced_base_instance(
                metal::MTLPrimitiveType::Triangle,
                0,
                6,
                1,
                (first_surface + index) as u64,
            );
        }
    }
}

fn backdrop_region(
    blur: &BackdropBlur,
    drawable_width: u64,
    drawable_height: u64,
) -> Option<(u64, u64, u64, u64, f32)> {
    if !blur.blur_radius.0.is_finite()
        || blur.blur_radius.0 <= 0.0
        || drawable_width == 0
        || drawable_height == 0
    {
        return None;
    }
    if [
        blur.corner_radii.top_left.0,
        blur.corner_radii.top_right.0,
        blur.corner_radii.bottom_right.0,
        blur.corner_radii.bottom_left.0,
    ]
    .iter()
    .any(|radius| !radius.is_finite() || *radius < 0.0)
    {
        return None;
    }
    for bounds in [blur.bounds, blur.content_mask.bounds] {
        let x = bounds.origin.x.0;
        let y = bounds.origin.y.0;
        let width = bounds.size.width.0;
        let height = bounds.size.height.0;
        if ![x, y, width, height, x + width, y + height]
            .iter()
            .all(|value| value.is_finite())
            || width <= 0.0
            || height <= 0.0
        {
            return None;
        }
    }
    let visible = blur.bounds.intersect(&blur.content_mask.bounds);
    let x = visible.origin.x.0;
    let y = visible.origin.y.0;
    let right = x + visible.size.width.0;
    let bottom = y + visible.size.height.0;
    if visible.is_empty()
        || right <= 0.0
        || bottom <= 0.0
        || x >= drawable_width as f32
        || y >= drawable_height as f32
    {
        return None;
    }
    let sigma = blur.blur_radius.0.clamp(1.0, 64.0);
    let padding = (sigma * 3.0).ceil() + 2.0;
    let x0 = (x - padding).floor().max(0.0) as u64;
    let y0 = (y - padding).floor().max(0.0) as u64;
    let x1 = (right + padding).ceil().min(drawable_width as f32) as u64;
    let y1 = (bottom + padding).ceil().min(drawable_height as f32) as u64;
    Some((x0, y0, x1 - x0, y1 - y0, sigma))
}

fn new_command_encoder_for_texture<'a>(
    command_buffer: &'a metal::CommandBufferRef,
    texture: &'a metal::TextureRef,
    viewport_size: Size<DevicePixels>,
    clear_color: Option<metal::MTLClearColor>,
) -> &'a metal::RenderCommandEncoderRef {
    let render_pass_descriptor = metal::RenderPassDescriptor::new();
    let color_attachment = render_pass_descriptor
        .color_attachments()
        .object_at(0)
        .unwrap();
    color_attachment.set_texture(Some(texture));
    color_attachment.set_store_action(metal::MTLStoreAction::Store);
    if let Some(clear_color) = clear_color {
        color_attachment.set_load_action(metal::MTLLoadAction::Clear);
        color_attachment.set_clear_color(clear_color);
    } else {
        color_attachment.set_load_action(metal::MTLLoadAction::Load);
    }

    let command_encoder = command_buffer.new_render_command_encoder(render_pass_descriptor);
    command_encoder.set_viewport(metal::MTLViewport {
        originX: 0.0,
        originY: 0.0,
        width: i32::from(viewport_size.width) as f64,
        height: i32::from(viewport_size.height) as f64,
        znear: 0.0,
        zfar: 1.0,
    });
    command_encoder
}

#[cfg(any(test, feature = "bench-support", feature = "test-support"))]
fn read_texture_to_image(texture: &metal::TextureRef) -> Result<RgbaImage> {
    let width = texture.width() as u32;
    let height = texture.height() as u32;
    let bytes_per_row = width as usize * 4;
    let mut pixels = vec![0u8; height as usize * bytes_per_row];

    let region = metal::MTLRegion {
        origin: metal::MTLOrigin { x: 0, y: 0, z: 0 },
        size: metal::MTLSize {
            width: width as u64,
            height: height as u64,
            depth: 1,
        },
    };
    texture.get_bytes(
        pixels.as_mut_ptr() as *mut std::ffi::c_void,
        bytes_per_row as u64,
        region,
        0,
    );

    // Convert BGRA to RGBA (swap B and R channels)
    for chunk in pixels.chunks_exact_mut(4) {
        chunk.swap(0, 2);
    }

    RgbaImage::from_raw(width, height, pixels).context("failed to create RgbaImage from pixel data")
}

fn build_pipeline_state_no_blend(
    device: &metal::DeviceRef,
    library: &metal::LibraryRef,
    label: &str,
    vertex_fn_name: &str,
    fragment_fn_name: &str,
    pixel_format: metal::MTLPixelFormat,
) -> metal::RenderPipelineState {
    let vertex_fn = library
        .get_function(vertex_fn_name, None)
        .expect("error locating vertex function");
    let fragment_fn = library
        .get_function(fragment_fn_name, None)
        .expect("error locating fragment function");
    let descriptor = metal::RenderPipelineDescriptor::new();
    descriptor.set_label(label);
    descriptor.set_vertex_function(Some(vertex_fn.as_ref()));
    descriptor.set_fragment_function(Some(fragment_fn.as_ref()));
    let color_attachment = descriptor.color_attachments().object_at(0).unwrap();
    color_attachment.set_pixel_format(pixel_format);
    color_attachment.set_blending_enabled(false);
    device
        .new_render_pipeline_state(&descriptor)
        .expect("could not create backdrop blur pipeline state")
}

fn build_pipeline_state(
    device: &metal::DeviceRef,
    library: &metal::LibraryRef,
    label: &str,
    vertex_fn_name: &str,
    fragment_fn_name: &str,
    pixel_format: metal::MTLPixelFormat,
) -> metal::RenderPipelineState {
    let vertex_fn = library
        .get_function(vertex_fn_name, None)
        .expect("error locating vertex function");
    let fragment_fn = library
        .get_function(fragment_fn_name, None)
        .expect("error locating fragment function");

    let descriptor = metal::RenderPipelineDescriptor::new();
    descriptor.set_label(label);
    descriptor.set_vertex_function(Some(vertex_fn.as_ref()));
    descriptor.set_fragment_function(Some(fragment_fn.as_ref()));
    let color_attachment = descriptor.color_attachments().object_at(0).unwrap();
    color_attachment.set_pixel_format(pixel_format);
    color_attachment.set_blending_enabled(true);
    color_attachment.set_rgb_blend_operation(metal::MTLBlendOperation::Add);
    color_attachment.set_alpha_blend_operation(metal::MTLBlendOperation::Add);
    color_attachment.set_source_rgb_blend_factor(metal::MTLBlendFactor::SourceAlpha);
    color_attachment.set_source_alpha_blend_factor(metal::MTLBlendFactor::One);
    color_attachment.set_destination_rgb_blend_factor(metal::MTLBlendFactor::OneMinusSourceAlpha);
    color_attachment.set_destination_alpha_blend_factor(metal::MTLBlendFactor::OneMinusSourceAlpha);

    device
        .new_render_pipeline_state(&descriptor)
        .expect("could not create render pipeline state")
}

fn build_path_sprite_pipeline_state(
    device: &metal::DeviceRef,
    library: &metal::LibraryRef,
    label: &str,
    vertex_fn_name: &str,
    fragment_fn_name: &str,
    pixel_format: metal::MTLPixelFormat,
) -> metal::RenderPipelineState {
    let vertex_fn = library
        .get_function(vertex_fn_name, None)
        .expect("error locating vertex function");
    let fragment_fn = library
        .get_function(fragment_fn_name, None)
        .expect("error locating fragment function");

    let descriptor = metal::RenderPipelineDescriptor::new();
    descriptor.set_label(label);
    descriptor.set_vertex_function(Some(vertex_fn.as_ref()));
    descriptor.set_fragment_function(Some(fragment_fn.as_ref()));
    let color_attachment = descriptor.color_attachments().object_at(0).unwrap();
    color_attachment.set_pixel_format(pixel_format);
    color_attachment.set_blending_enabled(true);
    color_attachment.set_rgb_blend_operation(metal::MTLBlendOperation::Add);
    color_attachment.set_alpha_blend_operation(metal::MTLBlendOperation::Add);
    color_attachment.set_source_rgb_blend_factor(metal::MTLBlendFactor::One);
    color_attachment.set_source_alpha_blend_factor(metal::MTLBlendFactor::One);
    color_attachment.set_destination_rgb_blend_factor(metal::MTLBlendFactor::OneMinusSourceAlpha);
    color_attachment.set_destination_alpha_blend_factor(metal::MTLBlendFactor::OneMinusSourceAlpha);

    device
        .new_render_pipeline_state(&descriptor)
        .expect("could not create render pipeline state")
}

fn build_path_rasterization_pipeline_state(
    device: &metal::DeviceRef,
    library: &metal::LibraryRef,
    label: &str,
    vertex_fn_name: &str,
    fragment_fn_name: &str,
    pixel_format: metal::MTLPixelFormat,
    path_sample_count: u32,
) -> metal::RenderPipelineState {
    let vertex_fn = library
        .get_function(vertex_fn_name, None)
        .expect("error locating vertex function");
    let fragment_fn = library
        .get_function(fragment_fn_name, None)
        .expect("error locating fragment function");

    let descriptor = metal::RenderPipelineDescriptor::new();
    descriptor.set_label(label);
    descriptor.set_vertex_function(Some(vertex_fn.as_ref()));
    descriptor.set_fragment_function(Some(fragment_fn.as_ref()));
    if path_sample_count > 1 {
        descriptor.set_raster_sample_count(path_sample_count as _);
        descriptor.set_alpha_to_coverage_enabled(false);
    }
    let color_attachment = descriptor.color_attachments().object_at(0).unwrap();
    color_attachment.set_pixel_format(pixel_format);
    color_attachment.set_blending_enabled(true);
    color_attachment.set_rgb_blend_operation(metal::MTLBlendOperation::Add);
    color_attachment.set_alpha_blend_operation(metal::MTLBlendOperation::Add);
    color_attachment.set_source_rgb_blend_factor(metal::MTLBlendFactor::One);
    color_attachment.set_source_alpha_blend_factor(metal::MTLBlendFactor::One);
    color_attachment.set_destination_rgb_blend_factor(metal::MTLBlendFactor::OneMinusSourceAlpha);
    color_attachment.set_destination_alpha_blend_factor(metal::MTLBlendFactor::OneMinusSourceAlpha);

    device
        .new_render_pipeline_state(&descriptor)
        .expect("could not create render pipeline state")
}

#[derive(Clone)]
struct InstanceBinding {
    buffer: metal::Buffer,
    offset: usize,
}

struct InstanceBindings {
    quads: InstanceBinding,
    shadows: InstanceBinding,
    backdrop_blurs: InstanceBinding,
    underlines: InstanceBinding,
    monochrome_sprites: InstanceBinding,
    polychrome_sprites: InstanceBinding,
    surfaces: InstanceBinding,
}

fn write_instances(scene: &Scene, writer: &mut InstanceBufferWriter) -> Result<InstanceBindings> {
    Ok(InstanceBindings {
        quads: writer.write(&scene.quads)?,
        shadows: writer.write(&scene.shadows)?,
        backdrop_blurs: writer.write(&scene.backdrop_blurs)?,
        underlines: writer.write(&scene.underlines)?,
        monochrome_sprites: writer.write(&scene.monochrome_sprites)?,
        polychrome_sprites: writer.write(&scene.polychrome_sprites)?,
        surfaces: writer.write_iter(scene.surfaces.iter().map(|surface| SurfaceBounds {
            bounds: surface.bounds,
            content_mask: surface.content_mask,
            clip: surface.clip,
        }))?,
    })
}

struct InstanceBufferWriter {
    device: metal::Device,
    pool: Arc<Mutex<InstanceBufferPool>>,
    unified_memory: bool,
    filled: Vec<(InstanceBuffer, usize)>,
    current: InstanceBuffer,
    offset: usize,
}

impl InstanceBufferWriter {
    fn new(
        device: &metal::Device,
        pool: &Arc<Mutex<InstanceBufferPool>>,
        unified_memory: bool,
    ) -> Self {
        let current = pool.lock().acquire(device, unified_memory);
        Self {
            device: device.clone(),
            pool: pool.clone(),
            unified_memory,
            filled: Vec::new(),
            current,
            offset: 0,
        }
    }

    fn allocate<T>(&mut self, count: usize) -> Result<(InstanceBinding, &mut [MaybeUninit<T>])> {
        let size = mem::size_of::<T>() * count;
        let mut offset = self.offset.next_multiple_of(INSTANCE_BUFFER_ALIGNMENT);
        if offset + size > self.current.size {
            self.grow(size)?;
            offset = 0;
        }
        self.offset = offset + size;

        let binding = InstanceBinding {
            buffer: self.current.metal_buffer.clone(),
            offset,
        };
        // Safety: the reservation lies within a buffer this frame owns
        // exclusively, and never overlaps one handed out earlier.
        let values = unsafe {
            let start = (self.current.metal_buffer.contents() as *mut u8).add(offset);
            slice::from_raw_parts_mut(start.cast::<MaybeUninit<T>>(), count)
        };
        Ok((binding, values))
    }

    fn write<T>(&mut self, values: &[T]) -> Result<InstanceBinding> {
        let (binding, destination) = self.allocate::<T>(values.len())?;
        unsafe {
            ptr::copy_nonoverlapping(
                values.as_ptr(),
                destination.as_mut_ptr().cast::<T>(),
                values.len(),
            );
        }
        Ok(binding)
    }

    fn write_iter<T>(
        &mut self,
        values: impl ExactSizeIterator<Item = T>,
    ) -> Result<InstanceBinding> {
        let (binding, destination) = self.allocate::<T>(values.len())?;
        for (slot, value) in destination.iter_mut().zip(values) {
            slot.write(value);
        }
        Ok(binding)
    }

    fn grow(&mut self, required: usize) -> Result<()> {
        let mut pool = self.pool.lock();
        let buffer_size = (pool.buffer_size * 2)
            .max(required.next_power_of_two())
            .min(MAX_INSTANCE_BUFFER_SIZE);
        anyhow::ensure!(
            buffer_size >= required,
            "instance buffer needs {required} bytes, above the maximum of {MAX_INSTANCE_BUFFER_SIZE}"
        );
        anyhow::ensure!(
            buffer_size > self.current.size,
            "frame instance data exceeds the {MAX_INSTANCE_BUFFER_SIZE}-byte maximum"
        );
        if buffer_size != pool.buffer_size {
            log::info!("increased instance buffer size to {buffer_size}");
            pool.reset(buffer_size);
        }
        let buffer = pool.acquire(&self.device, self.unified_memory);
        drop(pool);

        let filled = mem::replace(&mut self.current, buffer);
        self.filled.push((filled, self.offset));
        self.offset = 0;
        Ok(())
    }

    fn finish(self) -> InstanceBuffer {
        let Self {
            unified_memory,
            filled,
            current,
            offset,
            ..
        } = self;

        if !unified_memory {
            for (buffer, written) in &filled {
                if *written == 0 {
                    continue;
                }
                buffer.metal_buffer.did_modify_range(NSRange {
                    location: 0,
                    length: *written as NSUInteger,
                });
            }
            if offset > 0 {
                current.metal_buffer.did_modify_range(NSRange {
                    location: 0,
                    length: offset as NSUInteger,
                });
            }
        }

        // Metal retains encoded resources until the command buffer completes.
        // Only the final, largest buffer is worth keeping in the pool.
        drop(filled);
        current
    }
}

#[repr(C)]
enum ShadowInputIndex {
    Vertices = 0,
    Shadows = 1,
    ViewportSize = 2,
}

#[repr(C)]
enum BackdropBlurInputIndex {
    Vertices = 0,
    Blurs = 1,
    ViewportSize = 2,
    SourceTexture = 3,
    SourceRect = 4,
    SnapshotTexture = 5,
}

#[repr(C)]
enum QuadInputIndex {
    Vertices = 0,
    Quads = 1,
    ViewportSize = 2,
}

#[repr(C)]
enum UnderlineInputIndex {
    Vertices = 0,
    Underlines = 1,
    ViewportSize = 2,
}

#[repr(C)]
enum SpriteInputIndex {
    Vertices = 0,
    Sprites = 1,
    ViewportSize = 2,
    AtlasTextureSize = 3,
    AtlasTexture = 4,
}

#[repr(C)]
enum SurfaceInputIndex {
    Vertices = 0,
    Surfaces = 1,
    ViewportSize = 2,
    TextureSize = 3,
    YTexture = 4,
    CbCrTexture = 5,
}

#[repr(C)]
enum PathRasterizationInputIndex {
    Vertices = 0,
    ViewportSize = 1,
}

#[derive(Clone, Debug, PartialEq)]
#[repr(C)]
pub struct PathSprite {
    pub bounds: Bounds<ScaledPixels>,
    pub clip: RoundedClipParams,
}

#[derive(Clone, Debug, PartialEq)]
#[repr(C)]
pub struct SurfaceBounds {
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub clip: RoundedClipParams,
}

#[cfg(any(test, feature = "bench-support", feature = "test-support"))]
pub struct MetalHeadlessRenderer {
    renderer: MetalRenderer,
}

#[cfg(any(test, feature = "bench-support", feature = "test-support"))]
impl MetalHeadlessRenderer {
    pub fn new() -> Self {
        let instance_buffer_pool = Arc::new(Mutex::new(InstanceBufferPool::default()));
        let renderer = MetalRenderer::new_headless(instance_buffer_pool);
        Self { renderer }
    }
}

#[cfg(any(test, feature = "bench-support", feature = "test-support"))]
impl gpui::PlatformHeadlessRenderer for MetalHeadlessRenderer {
    fn render_scene_to_image(
        &mut self,
        scene: &Scene,
        size: Size<DevicePixels>,
    ) -> anyhow::Result<image::RgbaImage> {
        self.renderer.render_scene_to_image(scene, size)
    }

    fn render_scene(&mut self, scene: &Scene, size: Size<DevicePixels>) -> anyhow::Result<()> {
        self.renderer.render_scene(scene, size)
    }

    fn sprite_atlas(&self) -> Arc<dyn gpui::PlatformAtlas> {
        self.renderer.sprite_atlas().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2_core_foundation::{CFDictionary, CFType};
    use objc2_core_video::{
        CVPixelBufferCreate, CVPixelBufferGetBaseAddressOfPlane,
        CVPixelBufferGetBytesPerRowOfPlane, CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags,
        CVPixelBufferUnlockBaseAddress, kCVPixelBufferIOSurfacePropertiesKey,
    };

    #[test]
    fn render_core_video_surface() {
        let attributes = CFDictionary::from_slices(
            &[unsafe { kCVPixelBufferIOSurfacePropertiesKey }],
            &[&*CFDictionary::<CFType, CFType>::empty()],
        );
        let mut image_buffer = ptr::null_mut();
        assert_eq!(
            unsafe {
                CVPixelBufferCreate(
                    None,
                    16,
                    16,
                    kCVPixelFormatType_420YpCbCr8BiPlanarFullRange,
                    Some(attributes.as_opaque()),
                    ptr::NonNull::from(&mut image_buffer),
                )
            },
            kCVReturnSuccess
        );
        let image_buffer =
            unsafe { CFRetained::from_raw(ptr::NonNull::new(image_buffer).unwrap()) };
        unsafe {
            assert_eq!(
                CVPixelBufferLockBaseAddress(&image_buffer, CVPixelBufferLockFlags::empty()),
                kCVReturnSuccess
            );
            for plane in 0..2 {
                let bytes = CVPixelBufferGetBaseAddressOfPlane(&image_buffer, plane).cast::<u8>();
                let length = CVPixelBufferGetBytesPerRowOfPlane(&image_buffer, plane)
                    * CVPixelBufferGetHeightOfPlane(&image_buffer, plane);
                slice::from_raw_parts_mut(bytes, length).fill(128);
            }
            assert_eq!(
                CVPixelBufferUnlockBaseAddress(&image_buffer, CVPixelBufferLockFlags::empty()),
                kCVReturnSuccess
            );
        }

        let bounds = Bounds {
            origin: Point::default(),
            size: size(ScaledPixels(16.0), ScaledPixels(16.0)),
        };
        let mut scene = Scene::default();
        scene.insert_primitive(PaintSurface {
            order: 0,
            bounds,
            content_mask: ContentMask { bounds },
            clip: RoundedClipParams::default(),
            image_buffer: image_buffer.clone(),
        });
        drop(image_buffer);
        scene.finish();

        let mut renderer = MetalRenderer::new_headless(Arc::new(Mutex::new(Default::default())));
        let image = renderer
            .render_scene_to_image(&scene, size(DevicePixels(16), DevicePixels(16)))
            .unwrap();
        for image::Rgba([red, green, blue, alpha]) in image.pixels() {
            assert!(red.abs_diff(128) <= 2);
            assert!(green.abs_diff(128) <= 2);
            assert!(blue.abs_diff(128) <= 2);
            assert_eq!(*alpha, 255);
        }

        renderer.opaque = false;
        scene.surfaces[0].clip = RoundedClipParams {
            left: 0.,
            top: 0.,
            right: 16.,
            bottom: 16.,
            top_left: 6.,
            top_right: 0.,
            bottom_right: 6.,
            bottom_left: 0.,
        };
        let clipped = renderer
            .render_scene_to_image(&scene, size(DevicePixels(16), DevicePixels(16)))
            .unwrap();
        assert_eq!(clipped.get_pixel(0, 0)[3], 0);
        assert_eq!(clipped.get_pixel(15, 15)[3], 0);
        assert_eq!(clipped.get_pixel(15, 0)[3], 255);
        assert_eq!(clipped.get_pixel(0, 15)[3], 255);
        assert_eq!(clipped.get_pixel(8, 8)[3], 255);
        let edge = clipped.get_pixel(1, 1);
        assert!(edge[3] > 0 && edge[3] < 255, "surface edge: {edge:?}");
        for channel in 0..3 {
            assert!((edge[channel] as i32 * 2 - edge[3] as i32).abs() <= 3);
        }
    }
}

#[cfg(test)]
mod backdrop_blur_tests {
    use super::*;
    use gpui::{
        Corners, DrawOrder, EdgeFadeParams, Edges, FontId, GlyphId, ImageAlphaMaskParams, ImageId,
        MonochromeSprite, PlatformAtlas, PolychromeSprite, Quad, RenderGlyphParams,
        RenderImageParams, RenderSvgParams, Shadow, TransformationMatrix, Underline, px, rgba,
        solid_background,
    };
    use std::borrow::Cow;

    fn bounds(x: f32, y: f32, width: f32, height: f32) -> Bounds<ScaledPixels> {
        Bounds {
            origin: point(ScaledPixels(x), ScaledPixels(y)),
            size: size(ScaledPixels(width), ScaledPixels(height)),
        }
    }

    fn rounded_clip() -> RoundedClipParams {
        RoundedClipParams {
            left: 10.,
            top: 10.,
            right: 50.,
            bottom: 50.,
            top_left: 12.,
            top_right: 0.,
            bottom_right: 4.,
            bottom_left: 0.,
        }
    }

    fn quad(order: DrawOrder, bounds: Bounds<ScaledPixels>, color: u32) -> Quad {
        Quad {
            order,
            bounds,
            content_mask: ContentMask { bounds },
            background: solid_background(rgba(color)),
            ..Default::default()
        }
    }

    fn blur(order: DrawOrder, bounds: Bounds<ScaledPixels>, radius: f32) -> BackdropBlur {
        BackdropBlur {
            order,
            blur_radius: ScaledPixels(radius),
            bounds,
            content_mask: ContentMask { bounds },
            corner_radii: Corners::default(),
            clip: RoundedClipParams::default(),
        }
    }

    fn renderer() -> MetalRenderer {
        MetalRenderer::new_headless(Arc::new(Mutex::new(InstanceBufferPool::default())))
    }

    fn render(
        renderer: &mut MetalRenderer,
        scene: &mut Scene,
        width: i32,
        height: i32,
    ) -> RgbaImage {
        scene.finish();
        renderer
            .render_scene_to_image(scene, size(DevicePixels(width), DevicePixels(height)))
            .expect("Metal scene rendering failed")
    }

    #[test]
    fn backdrop_shader_bindings_match_scene_layout() {
        assert_eq!(mem::size_of::<BackdropBlur>(), 88);
        assert_eq!(mem::offset_of!(BackdropBlur, order), 0);
        assert_eq!(mem::offset_of!(BackdropBlur, blur_radius), 4);
        assert_eq!(mem::offset_of!(BackdropBlur, bounds), 8);
        assert_eq!(mem::offset_of!(BackdropBlur, content_mask), 24);
        assert_eq!(mem::offset_of!(BackdropBlur, corner_radii), 40);
        assert_eq!(mem::offset_of!(BackdropBlur, clip), 56);
        let header = include_str!(concat!(env!("OUT_DIR"), "/scene.h"));
        let definition = header
            .split("struct BackdropBlur {")
            .nth(1)
            .expect("BackdropBlur missing from generated Metal header")
            .split('}')
            .next()
            .unwrap();
        let offsets = [
            "order",
            "blur_radius",
            "bounds",
            "content_mask",
            "corner_radii",
            "clip",
        ]
        .map(|field| definition.find(field).expect("missing backdrop field"));
        assert!(offsets.windows(2).all(|pair| pair[0] < pair[1]));
        for (index, name) in [
            (BackdropBlurInputIndex::Vertices as u32, "Vertices"),
            (BackdropBlurInputIndex::Blurs as u32, "Blurs"),
            (BackdropBlurInputIndex::ViewportSize as u32, "ViewportSize"),
            (
                BackdropBlurInputIndex::SourceTexture as u32,
                "SourceTexture",
            ),
            (BackdropBlurInputIndex::SourceRect as u32, "SourceRect"),
            (
                BackdropBlurInputIndex::SnapshotTexture as u32,
                "SnapshotTexture",
            ),
        ] {
            assert!(header.contains(&format!("BackdropBlurInputIndex_{name} = {index}")));
        }
        assert!(header.contains("EdgeFadeParams"));
    }

    #[test]
    fn fade_shader_bindings_match_primitive_layouts() {
        assert_eq!(mem::size_of::<RoundedClipParams>(), 32);
        assert_eq!(
            [
                mem::offset_of!(RoundedClipParams, left),
                mem::offset_of!(RoundedClipParams, top),
                mem::offset_of!(RoundedClipParams, right),
                mem::offset_of!(RoundedClipParams, bottom),
                mem::offset_of!(RoundedClipParams, top_left),
                mem::offset_of!(RoundedClipParams, top_right),
                mem::offset_of!(RoundedClipParams, bottom_right),
                mem::offset_of!(RoundedClipParams, bottom_left),
            ],
            [0, 4, 8, 12, 16, 20, 24, 28]
        );
        assert_eq!(mem::size_of::<EdgeFadeParams>(), 64);
        assert_eq!(mem::offset_of!(EdgeFadeParams, clip), 32);
        assert_eq!(mem::size_of::<Shadow>(), 144);
        assert_eq!(mem::offset_of!(Shadow, clip), 112);
        assert_eq!(mem::size_of::<Underline>(), 96);
        assert_eq!(mem::offset_of!(Underline, clip), 64);
        assert_eq!(mem::size_of::<SurfaceBounds>(), 64);
        assert_eq!(mem::offset_of!(SurfaceBounds, clip), 32);
        assert_eq!(mem::size_of::<PathSprite>(), 48);
        assert_eq!(mem::offset_of!(PathSprite, clip), 16);
        assert_eq!(mem::size_of::<Quad>(), 224);
        assert_eq!(mem::size_of::<MonochromeSprite>(), 176);
        assert_eq!(mem::size_of::<PolychromeSprite>(), 200);
        assert_eq!(mem::size_of::<ImageAlphaMaskParams>(), 40);
        assert_eq!(
            [
                mem::offset_of!(ImageAlphaMaskParams, bounds),
                mem::offset_of!(ImageAlphaMaskParams, radius),
                mem::offset_of!(ImageAlphaMaskParams, feather),
                mem::offset_of!(ImageAlphaMaskParams, clearance),
                mem::offset_of!(ImageAlphaMaskParams, bottom_y),
                mem::offset_of!(ImageAlphaMaskParams, bottom_feather),
                mem::offset_of!(ImageAlphaMaskParams, pad),
            ],
            [0, 16, 20, 24, 28, 32, 36]
        );
        assert_eq!(mem::offset_of!(PolychromeSprite, alpha_mask), 160);
        assert_eq!(mem::offset_of!(Quad, fade), 160);
        assert_eq!(mem::offset_of!(MonochromeSprite, fade), 112);
        assert_eq!(mem::offset_of!(gpui::PolychromeSprite, fade), 96);
        let header = include_str!(concat!(env!("OUT_DIR"), "/scene.h"));
        for (name, fields) in [
            (
                "Quad",
                &[
                    "order",
                    "border_style",
                    "bounds",
                    "content_mask",
                    "background",
                    "border_color",
                    "corner_radii",
                    "border_widths",
                    "fade",
                ][..],
            ),
            (
                "MonochromeSprite",
                &[
                    "order",
                    "pad",
                    "tile",
                    "bounds",
                    "content_mask",
                    "color",
                    "transformation",
                    "fade",
                ][..],
            ),
            (
                "PolychromeSprite",
                &[
                    "order",
                    "pad",
                    "tile",
                    "grayscale",
                    "opacity",
                    "bounds",
                    "content_mask",
                    "corner_radii",
                    "fade",
                    "alpha_mask",
                ][..],
            ),
        ] {
            let definition = header
                .split(&format!("struct {name} {{"))
                .nth(1)
                .expect("primitive missing from generated Metal header")
                .split('}')
                .next()
                .unwrap();
            let positions: Vec<_> = fields
                .iter()
                .map(|field| {
                    definition
                        .find(&format!(" {field};"))
                        .expect("missing primitive field")
                })
                .collect();
            assert!(positions.windows(2).all(|pair| pair[0] < pair[1]), "{name}");
            assert!(definition.contains("EdgeFadeParams fade;"), "{name}");
            if name == "PolychromeSprite" {
                assert!(definition.contains("ImageAlphaMaskParams alpha_mask;"));
            }
        }
        let definition = header
            .split("struct ImageAlphaMaskParams {")
            .nth(1)
            .expect("ImageAlphaMaskParams missing from generated Metal header")
            .split('}')
            .next()
            .unwrap();
        let positions = [
            "bounds",
            "radius",
            "feather",
            "clearance",
            "bottom_y",
            "bottom_feather",
            "pad",
        ]
        .map(|field| definition.find(&format!(" {field};")).unwrap());
        assert!(positions.windows(2).all(|pair| pair[0] < pair[1]));
        let definition = header
            .split("struct EdgeFadeParams {")
            .nth(1)
            .unwrap()
            .split('}')
            .next()
            .unwrap();
        let positions = [
            "top_y",
            "bottom_y",
            "band_top",
            "band_bottom",
            "left_x",
            "right_x",
            "band_left",
            "band_right",
            "clip",
        ]
        .map(|field| definition.find(&format!(" {field};")).unwrap());
        assert!(positions.windows(2).all(|pair| pair[0] < pair[1]));
        let definition = header
            .split("struct RoundedClipParams {")
            .nth(1)
            .expect("RoundedClipParams missing from generated Metal header")
            .split('}')
            .next()
            .unwrap();
        let positions = [
            "left",
            "top",
            "right",
            "bottom",
            "top_left",
            "top_right",
            "bottom_right",
            "bottom_left",
        ]
        .map(|field| definition.find(&format!(" {field};")).unwrap());
        assert!(positions.windows(2).all(|pair| pair[0] < pair[1]));
        for (name, preceding) in [
            ("Shadow", "pad"),
            ("Underline", "wavy"),
            ("PathSprite", "bounds"),
            ("SurfaceBounds", "content_mask"),
        ] {
            let definition = header
                .split(&format!("struct {name} {{"))
                .nth(1)
                .unwrap()
                .split('}')
                .next()
                .unwrap();
            let preceding = definition.find(&format!(" {preceding};")).unwrap();
            let clip = definition.find("RoundedClipParams clip;").unwrap();
            assert!(preceding < clip, "{name}");
        }
    }

    #[test]
    fn backdrop_region_checks_geometry_and_clamps_sigma() {
        let mut region = blur(0, bounds(-10., 5., 30., 20.), 0.25);
        assert_eq!(backdrop_region(&region, 100, 100), Some((0, 0, 25, 30, 1.)));
        region.blur_radius = ScaledPixels(1000.);
        assert_eq!(
            backdrop_region(&region, 100, 100),
            Some((0, 0, 100, 100, 64.))
        );
        for radius in [0., -1., f32::NAN, f32::INFINITY] {
            region.blur_radius = ScaledPixels(radius);
            assert!(backdrop_region(&region, 100, 100).is_none());
        }
        region.blur_radius = ScaledPixels(4.);
        region.corner_radii.top_left = ScaledPixels(-1.);
        assert!(backdrop_region(&region, 100, 100).is_none());
        region.corner_radii.top_left = ScaledPixels(f32::NAN);
        assert!(backdrop_region(&region, 100, 100).is_none());
        region.corner_radii = Corners::default();
        region.bounds = bounds(f32::INFINITY, 0., 20., 20.);
        assert!(backdrop_region(&region, 100, 100).is_none());
        region.bounds = bounds(101., 0., 20., 20.);
        region.content_mask.bounds = region.bounds;
        assert!(backdrop_region(&region, 100, 100).is_none());
    }

    #[test]
    fn backdrop_obeys_order_mask_corners_and_foreground() {
        let mut renderer = renderer();
        let mut scene = Scene::default();
        scene
            .quads
            .push(quad(0, bounds(64., 0., 64., 96.), 0xffffffff));
        let mut panel = blur(1, bounds(56., 16., 32., 64.), 8.);
        panel.content_mask.bounds = bounds(60., 16., 8., 64.);
        panel.corner_radii = Corners::all(ScaledPixels(12.));
        scene.backdrop_blurs.push(panel);
        scene
            .quads
            .push(quad(1, bounds(63., 40., 3., 16.), 0xff0000ff));
        let image = render(&mut renderer, &mut scene, 128, 96);
        assert_eq!(image.get_pixel(58, 48)[0], 0, "outside content mask");
        assert_eq!(image.get_pixel(69, 48)[0], 255, "outside content mask");
        assert_eq!(image.get_pixel(60, 16)[0], 0, "outside rounded corner");
        assert!(image.get_pixel(60, 48)[0] > 30, "blurred edge");
        assert_eq!(
            *image.get_pixel(64, 48),
            image::Rgba([255, 0, 0, 255]),
            "foreground remains sharp"
        );
    }

    #[test]
    fn nested_backdrops_snapshot_the_previous_composite() {
        fn scene(nested: bool) -> Scene {
            let mut scene = Scene::default();
            scene
                .quads
                .push(quad(0, bounds(64., 0., 64., 96.), 0xffffffff));
            scene
                .backdrop_blurs
                .push(blur(1, bounds(24., 16., 80., 64.), 8.));
            scene
                .quads
                .push(quad(2, bounds(55., 24., 4., 48.), 0xffffffff));
            if nested {
                scene
                    .backdrop_blurs
                    .push(blur(3, bounds(48., 24., 32., 48.), 4.));
            }
            scene
                .quads
                .push(quad(4, bounds(70., 40., 2., 16.), 0xff0000ff));
            scene
        }
        let mut renderer = renderer();
        let sharp_stripe = render(&mut renderer, &mut scene(false), 128, 96);
        let nested = render(&mut renderer, &mut scene(true), 128, 96);
        assert_eq!(sharp_stripe.get_pixel(56, 48)[0], 255);
        assert!(
            nested.get_pixel(56, 48)[0] < 230,
            "child blurs parent foreground"
        );
        assert!(
            nested.get_pixel(53, 48)[0] > sharp_stripe.get_pixel(53, 48)[0],
            "child samples stripe"
        );
        assert_eq!(*nested.get_pixel(70, 48), image::Rgba([255, 0, 0, 255]));
    }

    #[test]
    fn transparent_backdrop_replaces_premultiplied_pixels() {
        let mut renderer = renderer();
        renderer.opaque = false;
        let mut scene = Scene::default();
        scene
            .quads
            .push(quad(0, bounds(0., 0., 64., 96.), 0xffffff80));
        scene
            .backdrop_blurs
            .push(blur(1, bounds(0., 0., 128., 96.), 8.));
        let image = render(&mut renderer, &mut scene, 128, 96);
        let flat = image.get_pixel(8, 48);
        assert!(
            (125..=130).contains(&flat[3]),
            "alpha must not accumulate: {flat:?}"
        );
        let edge = image.get_pixel(64, 48);
        assert!(
            (35..=95).contains(&edge[3]),
            "alpha must be blurred: {edge:?}"
        );
        assert!(
            edge[0].abs_diff(edge[3]) <= 2,
            "premultiplied color must survive"
        );
    }

    #[test]
    fn moving_backdrop_reuses_scratch_without_shifting_the_snapshot() {
        fn gradient(panel_x: Option<f32>) -> Scene {
            let mut scene = Scene::default();
            for column in 0..128u32 {
                let gray = column * 2;
                let color = (gray << 24) | (gray << 16) | (gray << 8) | 255;
                scene
                    .quads
                    .push(quad(0, bounds(column as f32 * 4., 0., 4., 128.), color));
            }
            if let Some(x) = panel_x {
                scene
                    .backdrop_blurs
                    .push(blur(1, bounds(x, 32., 64., 64.), 4.));
            }
            scene
        }
        let mut renderer = renderer();
        let baseline = render(&mut renderer, &mut gradient(None), 512, 128);
        let first = render(&mut renderer, &mut gradient(Some(144.)), 512, 128);
        assert_eq!(renderer.backdrop_textures.len(), 1);
        let scratch = renderer.backdrop_textures[0].scratch.as_ptr();
        let second = render(&mut renderer, &mut gradient(Some(320.)), 512, 128);
        assert_eq!(renderer.backdrop_textures[0].scratch.as_ptr(), scratch);
        for (image, start) in [(&first, 152), (&second, 328)] {
            for x in start..start + 48 {
                let before = baseline.get_pixel(x, 64)[0];
                let after = image.get_pixel(x, 64)[0];
                assert!(
                    before.abs_diff(after) <= 3,
                    "snapshot shifted at x={x}: {before} vs {after}"
                );
            }
        }
    }

    #[test]
    fn blur_without_batches_runs_and_scratch_releases_after_resize() {
        let mut renderer = renderer();
        let mut scene = Scene::default();
        scene
            .backdrop_blurs
            .push(blur(0, bounds(0., 0., 24., 20.), 4.));
        let image = render(&mut renderer, &mut scene, 24, 20);
        assert_eq!(*image.get_pixel(10, 10), image::Rgba([0, 0, 0, 255]));
        assert_eq!(
            renderer.backdrop_textures.len(),
            1,
            "blur must execute even without batches"
        );
        render(&mut renderer, &mut scene, 13, 9);
        assert!(
            renderer
                .backdrop_textures
                .iter()
                .all(|pair| pair.scratch.width() <= 13 && pair.scratch.height() <= 9)
        );
        let mut plain = Scene::default();
        for _ in 0..BACKDROP_RELEASE_AFTER_FRAMES {
            render(&mut renderer, &mut plain, 13, 9);
        }
        assert!(renderer.backdrop_textures.is_empty());
        assert!(renderer.backdrop_kernels.is_empty());
    }

    #[test]
    fn edge_fade_scales_final_translucent_quad_and_sprite() {
        let mut renderer = renderer();
        renderer.opaque = false;
        let tile_size = size(DevicePixels(100), DevicePixels(40));
        let tile = renderer
            .sprite_atlas()
            .get_or_insert_with(
                RenderSvgParams {
                    path: "metal-fade-regression".into(),
                    size: tile_size,
                }
                .into(),
                &mut || Ok(Some((tile_size, Cow::Owned(vec![255; 100 * 40])))),
            )
            .unwrap()
            .unwrap();
        for sprite in [false, true] {
            let mut images = Vec::new();
            for faded in [false, true] {
                let fade = if faded {
                    EdgeFadeParams {
                        left_x: 10.,
                        band_left: 32.,
                        ..Default::default()
                    }
                } else {
                    EdgeFadeParams::default()
                };
                let mut scene = Scene::default();
                if sprite {
                    scene.monochrome_sprites.push(MonochromeSprite {
                        order: 0,
                        pad: 0,
                        tile,
                        bounds: bounds(0., 0., 100., 40.),
                        content_mask: ContentMask {
                            bounds: bounds(0., 0., 100., 40.),
                        },
                        color: rgba(0x80ff0080).into(),
                        transformation: TransformationMatrix::unit(),
                        fade,
                    });
                } else {
                    let mut panel = quad(0, bounds(0., 0., 100., 40.), 0xff000080);
                    panel.border_color = rgba(0x00ff0080).into();
                    panel.border_widths = Edges::all(ScaledPixels(6.));
                    panel.fade = fade;
                    scene.quads.push(panel);
                }
                images.push(render(&mut renderer, &mut scene, 100, 40));
            }
            for y in [1, 20] {
                for x in [5, 26, 60] {
                    let factor = (((x as f32 + 0.5 - 10.) / 32.).clamp(0., 1.)).powi(2);
                    for channel in 0..4 {
                        let expected = images[0].get_pixel(x, y)[channel] as f32 * factor;
                        let actual = images[1].get_pixel(x, y)[channel] as f32;
                        assert!(
                            (actual - expected).abs() <= 2.,
                            "sprite={sprite} x={x} y={y} channel={channel}: {actual} vs {expected}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn rounded_clip_combines_with_fade_and_alpha_for_quads_glyphs_and_images() {
        let mut renderer = renderer();
        renderer.opaque = false;
        let tile_size = size(DevicePixels(64), DevicePixels(64));
        let glyph = renderer
            .sprite_atlas()
            .get_or_insert_with(
                RenderGlyphParams {
                    font_id: FontId(0),
                    glyph_id: GlyphId(1),
                    font_size: px(64.),
                    subpixel_variant: point(0, 0),
                    scale_factor: 1.,
                    is_emoji: false,
                    subpixel_rendering: false,
                    dilation: 0,
                }
                .into(),
                &mut || Ok(Some((tile_size, Cow::Owned(vec![255; 64 * 64])))),
            )
            .unwrap()
            .unwrap();
        let image = renderer
            .sprite_atlas()
            .get_or_insert_with(
                RenderImageParams {
                    image_id: ImageId(739),
                    frame_index: 0,
                }
                .into(),
                &mut || {
                    Ok(Some((
                        tile_size,
                        Cow::Owned([255u8, 255, 255, 160].repeat(64 * 64)),
                    )))
                },
            )
            .unwrap()
            .unwrap();

        for kind in 0..4 {
            let mut images = Vec::new();
            for (clipped, faded) in [(false, false), (true, false), (true, true)] {
                let fade = EdgeFadeParams {
                    band_left: if faded { 32. } else { 0. },
                    clip: if clipped {
                        rounded_clip()
                    } else {
                        Default::default()
                    },
                    ..Default::default()
                };
                let mut scene = Scene::default();
                match kind {
                    0 | 3 => {
                        let mut panel = quad(0, bounds(0., 0., 64., 64.), 0xffffff80);
                        if kind == 3 {
                            panel.border_color = rgba(0x00ff0080).into();
                            panel.border_widths = Edges::all(ScaledPixels(16.));
                        }
                        panel.fade = fade;
                        scene.quads.push(panel);
                    }
                    1 => scene.monochrome_sprites.push(MonochromeSprite {
                        order: 0,
                        pad: 0,
                        tile: glyph,
                        bounds: bounds(0., 0., 56., 56.),
                        content_mask: ContentMask {
                            bounds: bounds(0., 0., 64., 64.),
                        },
                        color: rgba(0xffffff80).into(),
                        transformation: TransformationMatrix {
                            translation: [8., 8.],
                            ..TransformationMatrix::unit()
                        },
                        fade,
                    }),
                    _ => scene.polychrome_sprites.push(PolychromeSprite {
                        order: 0,
                        pad: 0,
                        tile: image,
                        grayscale: false.into(),
                        opacity: 0.75,
                        bounds: bounds(0., 0., 64., 64.),
                        content_mask: ContentMask {
                            bounds: bounds(0., 0., 64., 64.),
                        },
                        corner_radii: Corners::default(),
                        fade,
                        alpha_mask: Default::default(),
                    }),
                }
                images.push(render(&mut renderer, &mut scene, 64, 64));
            }

            assert!(images[0].get_pixel(8, 30)[3] > 100, "kind={kind}");
            assert_eq!(images[1].get_pixel(8, 30)[3], 0, "kind={kind}");
            assert_eq!(images[1].get_pixel(10, 10)[3], 0, "kind={kind}");
            assert_eq!(images[1].get_pixel(49, 49)[3], 0, "kind={kind}");
            for (x, y) in [(49, 10), (10, 49), (30, 30)] {
                assert_eq!(
                    images[1].get_pixel(x, y),
                    images[0].get_pixel(x, y),
                    "kind={kind}"
                );
            }
            for (x, y, coverage) in [(13, 13, 0.479_184_72), (30, 30, 1.)] {
                let fade = ((x as f32 + 0.5) / 32.).clamp(0., 1.).powi(2);
                for channel in 0..4 {
                    let baseline = images[0].get_pixel(x, y)[channel] as f32;
                    let clipped = images[1].get_pixel(x, y)[channel] as f32;
                    let faded = images[2].get_pixel(x, y)[channel] as f32;
                    assert!(
                        (clipped - baseline * coverage).abs() <= 2.,
                        "kind={kind} channel={channel}"
                    );
                    assert!(
                        (faded - baseline * coverage * fade).abs() <= 2.,
                        "kind={kind} channel={channel}"
                    );
                }
            }
        }
    }

    fn solid_path(order: DrawOrder, color: u32, clip: RoundedClipParams) -> Path<ScaledPixels> {
        let mut path = Path::new(point(px(0.), px(0.)));
        path.push_triangle(
            (
                point(px(0.), px(0.)),
                point(px(128.), px(0.)),
                point(px(0.), px(128.)),
            ),
            (point(0., 1.), point(0., 1.), point(0., 1.)),
        );
        let mut path = path.scale(1.);
        path.order = order;
        path.content_mask = ContentMask {
            bounds: bounds(0., 0., 64., 64.),
        };
        path.color = solid_background(rgba(color));
        path.clip = clip;
        path
    }

    #[test]
    fn rounded_clip_applies_to_shadow_underlines_and_paths() {
        let mut renderer = renderer();
        renderer.opaque = false;
        let viewport = bounds(0., 0., 64., 64.);
        for kind in 0..5 {
            let mut images = Vec::new();
            for clipped in [false, true] {
                let clip = if clipped {
                    rounded_clip()
                } else {
                    Default::default()
                };
                let mut scene = Scene::default();
                match kind {
                    0 | 4 => scene.shadows.push(Shadow {
                        order: 0,
                        blur_radius: ScaledPixels(0.),
                        bounds: if kind == 4 {
                            bounds(100., 100., 1., 1.)
                        } else {
                            viewport
                        },
                        corner_radii: Corners::default(),
                        content_mask: ContentMask { bounds: viewport },
                        color: rgba(0xffffff80).into(),
                        element_bounds: viewport,
                        element_corner_radii: Corners::default(),
                        inset: if kind == 4 { 1 } else { 0 },
                        pad: 0,
                        clip,
                    }),
                    1 | 3 => scene.underlines.push(Underline {
                        order: 0,
                        pad: 0,
                        bounds: if kind == 3 {
                            bounds(0., 12., 64., 4.)
                        } else {
                            viewport
                        },
                        content_mask: ContentMask { bounds: viewport },
                        color: rgba(0xffffff80).into(),
                        thickness: ScaledPixels(4.),
                        wavy: (kind == 3).into(),
                        clip,
                    }),
                    _ => scene.paths.push(solid_path(0, 0xffffff80, clip)),
                }
                images.push(render(&mut renderer, &mut scene, 64, 64));
            }
            assert!(images[0].get_pixel(13, 13)[3] > 100, "kind={kind}");
            assert_eq!(images[1].get_pixel(10, 13)[3], 0, "kind={kind}");
            assert_eq!(
                images[1].get_pixel(49, 13),
                images[0].get_pixel(49, 13),
                "kind={kind}"
            );
            for channel in 0..4 {
                let baseline = images[0].get_pixel(13, 13)[channel] as f32;
                let clipped = images[1].get_pixel(13, 13)[channel] as f32;
                assert!(
                    (clipped - baseline * 0.479_184_72).abs() <= 3.,
                    "kind={kind} channel={channel}"
                );
            }
        }
    }

    #[test]
    fn path_clip_applies_once_after_overlapping_triangles_are_rasterized() {
        let mut renderer = renderer();
        renderer.opaque = false;
        let mut images = Vec::new();
        for clipped in [false, true] {
            let mut path = Path::new(point(px(0.), px(0.)));
            for _ in 0..2 {
                path.push_triangle(
                    (
                        point(px(0.), px(0.)),
                        point(px(128.), px(0.)),
                        point(px(0.), px(128.)),
                    ),
                    (point(0., 1.), point(0., 1.), point(0., 1.)),
                );
            }
            let mut path = path.scale(1.);
            path.content_mask = ContentMask {
                bounds: bounds(0., 0., 64., 64.),
            };
            path.color = solid_background(rgba(0xffffff80));
            if clipped {
                path.clip = rounded_clip();
            }
            let mut scene = Scene::default();
            scene.paths.push(path);
            images.push(render(&mut renderer, &mut scene, 64, 64));
        }
        let baseline = images[0].get_pixel(13, 13);
        let clipped = images[1].get_pixel(13, 13);
        assert!(
            (188..=194).contains(&baseline[3]),
            "overlapping triangles: {baseline:?}"
        );
        for channel in 0..4 {
            assert!(
                (clipped[channel] as f32 - baseline[channel] as f32 * 0.479_184_72).abs() <= 3.
            );
        }
    }

    #[test]
    fn same_layer_paths_do_not_sample_each_others_clips() {
        let mut renderer = renderer();
        renderer.opaque = false;
        let viewport = bounds(0., 0., 64., 64.);
        let mut square_clip = rounded_clip();
        square_clip.top_left = 0.;
        square_clip.bottom_right = 0.;
        let mut scene = Scene::default();
        scene.push_layer(viewport);
        for (vertices, color, clip) in [
            (
                (
                    point(px(0.), px(0.)),
                    point(px(64.), px(0.)),
                    point(px(0.), px(64.)),
                ),
                0xff000080,
                rounded_clip(),
            ),
            (
                (
                    point(px(64.), px(64.)),
                    point(px(64.), px(0.)),
                    point(px(0.), px(64.)),
                ),
                0x00ff0080,
                square_clip,
            ),
        ] {
            let mut path = Path::new(vertices.0);
            path.push_triangle(vertices, (point(0., 1.), point(0., 1.), point(0., 1.)));
            let mut path = path.scale(1.);
            path.content_mask = ContentMask { bounds: viewport };
            path.color = solid_background(rgba(color));
            path.clip = clip;
            scene.insert_primitive(path);
        }
        scene.pop_layer();
        assert_eq!(scene.paths.len(), 2);
        assert_eq!(scene.paths[0].order, scene.paths[1].order);

        let image = render(&mut renderer, &mut scene, 64, 64);
        assert_eq!(*image.get_pixel(10, 10), image::Rgba([0, 0, 0, 0]));
        let arc = image.get_pixel(13, 13);
        assert!((59..=64).contains(&arc[0]) && arc[1] == 0 && (59..=64).contains(&arc[3]));
        let red = image.get_pixel(30, 10);
        assert!((126..=130).contains(&red[0]) && red[1] == 0 && (126..=130).contains(&red[3]));
        for (x, y) in [(40, 40), (49, 49)] {
            let green = image.get_pixel(x, y);
            assert!(
                green[0] == 0 && (126..=130).contains(&green[1]) && (126..=130).contains(&green[3])
            );
        }
    }

    #[test]
    fn fractional_rounded_path_clip_covers_msaa_edge_once() {
        let mut renderer = renderer();
        renderer.opaque = false;
        let clip = RoundedClipParams {
            left: 8.75,
            top: 8.,
            right: 40.25,
            bottom: 40.,
            top_left: 6.,
            top_right: 6.,
            bottom_right: 6.,
            bottom_left: 6.,
        };
        let mut images = Vec::new();
        for clipped in [false, true] {
            let mut path = solid_path(
                0,
                0xffffff80,
                if clipped { clip } else { Default::default() },
            );
            let second_triangle = path.vertices.clone();
            path.vertices.extend(second_triangle);
            // The CPU path mask covers the fractional clip outward.
            path.content_mask = ContentMask {
                bounds: bounds(8., 8., 33., 32.),
            };
            let mut scene = Scene::default();
            scene.paths.push(path);
            images.push(render(&mut renderer, &mut scene, 64, 64));
        }
        assert_eq!(images[1].get_pixel(7, 20)[3], 0);
        assert!((188..=194).contains(&images[0].get_pixel(8, 20)[3]));
        for x in [8, 40] {
            for channel in 0..4 {
                let baseline = images[0].get_pixel(x, 20)[channel] as f32;
                let clipped = images[1].get_pixel(x, 20)[channel] as f32;
                assert!(
                    (clipped - baseline * 0.25).abs() <= 3.,
                    "x={x} channel={channel}"
                );
            }
        }
        assert_eq!(images[1].get_pixel(9, 20), images[0].get_pixel(9, 20));
    }

    #[test]
    fn rounded_clip_preserves_path_and_foreground_order() {
        let mut renderer = renderer();
        renderer.opaque = false;
        let mut scene = Scene::default();
        scene.paths.push(solid_path(0, 0xff000080, rounded_clip()));
        scene
            .paths
            .push(solid_path(1, 0x00ff0080, Default::default()));
        scene
            .quads
            .push(quad(2, bounds(30., 30., 4., 4.), 0x0000ffff));
        let image = render(&mut renderer, &mut scene, 64, 64);
        let edge = image.get_pixel(13, 13);
        assert!((28..=34).contains(&edge[0]), "clipped red path: {edge:?}");
        assert!(
            (126..=130).contains(&edge[1]),
            "unclipped green path: {edge:?}"
        );
        assert!((156..=162).contains(&edge[3]), "composited alpha: {edge:?}");
        assert_eq!(*image.get_pixel(30, 30), image::Rgba([0, 0, 255, 255]));
    }

    #[test]
    fn rounded_backdrop_mixes_blur_with_unmodified_transparent_snapshot() {
        let mut renderer = renderer();
        renderer.opaque = false;
        let mut images = Vec::new();
        for (with_blur, clipped) in [(false, false), (true, false), (true, true)] {
            let mut scene = Scene::default();
            scene
                .quads
                .push(quad(0, bounds(0., 0., 14., 64.), 0xffffff80));
            if with_blur {
                let mut panel = blur(1, bounds(0., 0., 64., 64.), 4.);
                if clipped {
                    panel.clip = rounded_clip();
                }
                scene.backdrop_blurs.push(panel);
            }
            scene
                .quads
                .push(quad(2, bounds(30., 30., 4., 4.), 0xff0000ff));
            images.push(render(&mut renderer, &mut scene, 64, 64));
        }
        assert_eq!(images[2].get_pixel(10, 10), images[0].get_pixel(10, 10));
        let original = images[0].get_pixel(13, 13);
        let blurred = images[1].get_pixel(13, 13);
        let clipped = images[2].get_pixel(13, 13);
        assert!(blurred[3] + 10 < original[3], "blur must affect the edge");
        for channel in 0..4 {
            let expected = original[channel] as f32 * (1. - 0.479_184_72)
                + blurred[channel] as f32 * 0.479_184_72;
            assert!(
                (clipped[channel] as f32 - expected).abs() <= 3.,
                "channel={channel}: {clipped:?}"
            );
        }
        assert!(
            clipped[0].abs_diff(clipped[3]) <= 2,
            "premultiplied alpha: {clipped:?}"
        );
        assert_eq!(*images[2].get_pixel(30, 30), image::Rgba([255, 0, 0, 255]));
    }

    #[test]
    fn image_alpha_mask_matches_pixels_and_reuses_atlas_across_frames() {
        // CPU reference adapted from zui gpui_macos at
        // b564a34413bb7ef1da04121c4a287758f94993d9 (Apache-2.0).
        fn mask_alpha(mask: ImageAlphaMaskParams, x: u32, y: u32) -> f32 {
            if mask.feather <= 0. {
                return 1.;
            }
            let smooth = |value: f32| {
                let t = value.clamp(0., 1.);
                t * t * (3. - 2. * t)
            };
            let half_width = mask.bounds.size.width.0 * 0.5;
            let half_height = mask.bounds.size.height.0 * 0.5;
            let x = x as f32 + 0.5;
            let y = y as f32 + 0.5;
            let qx = (x - mask.bounds.origin.x.0 - half_width).abs() - half_width + mask.radius;
            let qy = (y - mask.bounds.origin.y.0 - half_height).abs() - half_height + mask.radius;
            let distance = qx.max(0.).hypot(qy.max(0.)) + qx.max(qy).min(0.) - mask.radius;
            let mut alpha = smooth((distance - mask.clearance) / mask.feather);
            if mask.bottom_feather > 0. {
                alpha = alpha.min(smooth((mask.bottom_y - y) / mask.bottom_feather));
            }
            alpha
        }

        let mut renderer = renderer();
        renderer.opaque = false;
        let tile_size = size(DevicePixels(64), DevicePixels(64));
        let key = RenderImageParams {
            image_id: ImageId(741),
            frame_index: 0,
        };
        let tile = renderer
            .sprite_atlas()
            .get_or_insert_with(key.clone().into(), &mut || {
                Ok(Some((
                    tile_size,
                    Cow::Owned([32u8, 96, 224, 160].repeat(64 * 64)),
                )))
            })
            .unwrap()
            .unwrap();
        let sprite = |alpha_mask, fade| PolychromeSprite {
            order: 1,
            pad: 0,
            tile,
            grayscale: false.into(),
            opacity: 0.75,
            bounds: bounds(0., 0., 64., 64.),
            content_mask: ContentMask {
                bounds: bounds(0., 0., 64., 64.),
            },
            corner_radii: Corners::default(),
            fade,
            alpha_mask,
        };
        let mut paint = |alpha_mask, fade| {
            let cached = renderer
                .sprite_atlas()
                .get_or_insert_with(key.clone().into(), &mut || {
                    panic!("changing mask geometry must not reupload the image")
                })
                .unwrap()
                .unwrap();
            assert_eq!(cached, tile);
            let mut scene = Scene::default();
            scene.polychrome_sprites.push(sprite(alpha_mask, fade));
            render(&mut renderer, &mut scene, 64, 64)
        };

        let plain = paint(Default::default(), Default::default());
        let source = plain.get_pixel(5, 5);
        assert!(
            (118..=122).contains(&source[3]),
            "source alpha and opacity: {source:?}"
        );
        assert!((102..=108).contains(&source[0]), "source red: {source:?}");
        assert!((42..=48).contains(&source[1]), "source green: {source:?}");
        assert!((12..=18).contains(&source[2]), "source blue: {source:?}");

        let mask = ImageAlphaMaskParams {
            bounds: bounds(18.25, 16.5, 23., 18.),
            radius: 6.,
            feather: 9.5,
            clearance: 2.,
            bottom_y: 55.,
            bottom_feather: 8.,
            pad: 0.,
        };
        let masks = [
            mask,
            ImageAlphaMaskParams {
                bounds: bounds(8.5, 6.25, 36., 29.),
                radius: 11.,
                feather: 6.,
                clearance: 0.,
                bottom_y: 46.25,
                bottom_feather: 13.,
                pad: 0.,
            },
            ImageAlphaMaskParams {
                bounds: bounds(34., 38., 14., 12.),
                radius: 3.,
                feather: 12.,
                clearance: 1.5,
                bottom_y: 0.,
                bottom_feather: 0.,
                pad: 0.,
            },
            ImageAlphaMaskParams {
                feather: 0.,
                bottom_y: 20.,
                bottom_feather: 8.,
                ..Default::default()
            },
            ImageAlphaMaskParams {
                feather: -1.,
                bottom_y: 20.,
                bottom_feather: 8.,
                ..Default::default()
            },
        ];
        for fade in [
            EdgeFadeParams::default(),
            EdgeFadeParams {
                left_x: 4.,
                band_left: 26.,
                clip: rounded_clip(),
                ..Default::default()
            },
        ] {
            let baseline = paint(Default::default(), fade);
            for mask in masks {
                let actual = paint(mask, fade);
                for y in 0..64 {
                    for x in 0..64 {
                        let alpha = mask_alpha(mask, x, y);
                        for channel in 0..4 {
                            let expected = baseline.get_pixel(x, y)[channel] as f32 * alpha;
                            let actual = actual.get_pixel(x, y)[channel] as f32;
                            assert!(
                                (actual - expected).abs() <= 2.,
                                "pixel ({x}, {y}) channel {channel}: {actual} vs {expected}"
                            );
                        }
                    }
                }
            }
            assert_eq!(paint(Default::default(), fade), baseline);
        }
        drop(paint);

        let mut shifted_images = Vec::new();
        for alpha_mask in [ImageAlphaMaskParams::default(), mask] {
            let mut shifted = sprite(alpha_mask, EdgeFadeParams::default());
            shifted.bounds = bounds(8., 6., 48., 50.);
            shifted.corner_radii = Corners::all(ScaledPixels(5.));
            let mut scene = Scene::default();
            scene.polychrome_sprites.push(shifted);
            shifted_images.push(render(&mut renderer, &mut scene, 64, 64));
        }
        for y in 6..56 {
            for x in 8..56 {
                let alpha = mask_alpha(mask, x, y);
                for channel in 0..4 {
                    let baseline = shifted_images[0].get_pixel(x, y)[channel] as f32;
                    let actual = shifted_images[1].get_pixel(x, y)[channel] as f32;
                    assert!(
                        (actual - baseline * alpha).abs() <= 2.,
                        "shifted pixel ({x}, {y}) channel {channel}"
                    );
                }
            }
        }

        let mut images = Vec::new();
        for alpha_mask in [ImageAlphaMaskParams::default(), mask] {
            let mut scene = Scene::default();
            scene
                .quads
                .push(quad(0, bounds(0., 0., 64., 64.), 0x0000ffff));
            scene
                .polychrome_sprites
                .push(sprite(alpha_mask, EdgeFadeParams::default()));
            scene
                .quads
                .push(quad(2, bounds(48., 20., 4., 4.), 0xff0000ff));
            images.push(render(&mut renderer, &mut scene, 64, 64));
        }
        assert_ne!(images[0].get_pixel(30, 30), &image::Rgba([0, 0, 255, 255]));
        assert_eq!(*images[1].get_pixel(30, 30), image::Rgba([0, 0, 255, 255]));
        assert_eq!(images[1].get_pixel(5, 30), images[0].get_pixel(5, 30));
        let visible = images[1].get_pixel(5, 30);
        assert!((100..=110).contains(&visible[0]) && (40..=50).contains(&visible[1]));
        assert!((140..=160).contains(&visible[2]) && visible[3] == 255);
        assert_eq!(*images[1].get_pixel(49, 21), image::Rgba([255, 0, 0, 255]));
    }
}
