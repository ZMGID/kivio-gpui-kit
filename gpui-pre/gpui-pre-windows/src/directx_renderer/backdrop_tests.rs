//! Pixel regressions through the production draw loop, using software D3D11.
use super::*;
use windows::{
    Win32::{Foundation::HMODULE, UI::WindowsAndMessaging::*},
    core::w,
};

struct TestWindow(HWND);

impl Drop for TestWindow {
    fn drop(&mut self) {
        unsafe { DestroyWindow(self.0).unwrap() };
    }
}

fn warp_devices() -> Result<DirectXDevices> {
    unsafe {
        let mut device = None;
        let mut context = None;
        let debug_result = D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_WARP,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_DEBUG,
            Some(&[D3D_FEATURE_LEVEL_11_0]),
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        );
        if let Err(error) = debug_result {
            if error.code() != DXGI_ERROR_SDK_COMPONENT_MISSING {
                return Err(error.into());
            }
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_WARP,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&[D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )?;
        }
        let device = device.unwrap();
        let dxgi: IDXGIDevice = device.cast()?;
        Ok(DirectXDevices {
            adapter: dxgi.GetAdapter()?.cast()?,
            dxgi_factory: CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0))?,
            device,
            device_context: context.unwrap(),
        })
    }
}

fn warp_renderer() -> Result<(DirectXRenderer, TestWindow)> {
    unsafe {
        let devices = warp_devices()?;
        // A hidden built-in window class avoids a message loop and custom WndProc.
        let window = TestWindow(CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("STATIC"),
            w!("GPUI WARP regression"),
            WS_OVERLAPPED,
            0,
            0,
            64,
            64,
            None,
            None,
            None,
            None,
        )?);
        let mut renderer = DirectXRenderer::new(window.0, &devices, true)?;
        renderer.resize(size(DevicePixels(64), DevicePixels(64)))?;
        Ok((renderer, window))
    }
}

fn bounds(x: f32, y: f32, width: f32, height: f32) -> Bounds<ScaledPixels> {
    Bounds::new(
        point(ScaledPixels(x), ScaledPixels(y)),
        size(ScaledPixels(width), ScaledPixels(height)),
    )
}

fn quad(order: u32, rect: Bounds<ScaledPixels>, color: Hsla) -> Quad {
    Quad {
        order,
        bounds: rect,
        content_mask: ContentMask {
            bounds: bounds(0., 0., 64., 64.),
        },
        background: color.into(),
        ..Default::default()
    }
}

fn blur(order: u32) -> BackdropBlur {
    BackdropBlur {
        order,
        blur_radius: ScaledPixels(3.),
        bounds: bounds(8., 8., 48., 48.),
        content_mask: ContentMask {
            bounds: bounds(0., 0., 64., 64.),
        },
        corner_radii: Corners::all(ScaledPixels(8.)),
        clip: RoundedClipParams::default(),
    }
}

fn pixels(renderer: &DirectXRenderer) -> Result<Vec<[u8; 4]>> {
    let devices = renderer.devices.as_ref().unwrap();
    let target = renderer
        .resources
        .as_ref()
        .unwrap()
        .render_target
        .as_ref()
        .unwrap();
    unsafe {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        target.GetDesc(&mut desc);
        desc.Usage = D3D11_USAGE_STAGING;
        desc.BindFlags = 0;
        desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
        let mut staging = None;
        devices
            .device
            .CreateTexture2D(&desc, None, Some(&mut staging))?;
        let staging = staging.unwrap();
        devices.device_context.OMSetRenderTargets(None, None);
        devices.device_context.CopyResource(&staging, target);
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        devices
            .device_context
            .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
        let mut result = Vec::new();
        for y in 0..desc.Height {
            let row = slice::from_raw_parts(
                (mapped.pData as *const u8).add((y * mapped.RowPitch) as usize) as *const [u8; 4],
                desc.Width as usize,
            );
            result.extend_from_slice(row);
        }
        devices.device_context.Unmap(&staging, 0);
        assert_no_gpu_errors(&devices.device)?;
        Ok(result)
    }
}

fn scratch_textures(renderer: &DirectXRenderer, downsample: u32) -> [ID3D11Texture2D; 3] {
    renderer
        .resources
        .as_ref()
        .unwrap()
        .backdrop
        .as_ref()
        .unwrap()
        .scratch_textures(downsample)
        .unwrap()
        .map(|texture| (*texture).clone())
}

fn texture_sizes(textures: &[ID3D11Texture2D; 3]) -> [[u32; 2]; 3] {
    std::array::from_fn(|index| {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe { textures[index].GetDesc(&mut desc) };
        [desc.Width, desc.Height]
    })
}

fn assert_no_gpu_errors(device: &ID3D11Device) -> Result<()> {
    let Ok(queue) = device.cast::<ID3D11InfoQueue>() else {
        return Ok(());
    };
    unsafe {
        for index in 0..queue.GetNumStoredMessagesAllowedByRetrievalFilter() {
            let mut length = 0;
            queue.GetMessage(index, None, &mut length)?;
            // usize storage guarantees the alignment required by D3D11_MESSAGE.
            let mut buffer = vec![0usize; length.div_ceil(std::mem::size_of::<usize>())];
            let message = buffer.as_mut_ptr().cast::<D3D11_MESSAGE>();
            queue.GetMessage(index, Some(message), &mut length)?;
            let message = &*message;
            assert!(
                !matches!(
                    message.Severity,
                    D3D11_MESSAGE_SEVERITY_CORRUPTION
                        | D3D11_MESSAGE_SEVERITY_ERROR
                        | D3D11_MESSAGE_SEVERITY_WARNING
                ),
                "D3D11 validation: {}",
                std::ffi::CStr::from_ptr(message.pDescription.cast()).to_string_lossy()
            );
        }
        queue.ClearStoredMessages();
    }
    Ok(())
}

#[::core::prelude::v1::test]
fn warp_backdrop_blurs_pixels() -> Result<()> {
    let (mut renderer, _window) = warp_renderer()?;
    let mut scene = Scene::default();
    scene
        .quads
        .push(quad(0, bounds(0., 0., 32., 64.), hsla(0., 0., 0., 1.)));
    scene
        .quads
        .push(quad(0, bounds(32., 0., 32., 64.), hsla(0., 0., 1., 1.)));
    renderer.render(&scene, WindowBackgroundAppearance::Opaque)?;
    let sharp = pixels(&renderer)?;
    scene.backdrop_blurs.push(blur(1));
    renderer.render(&scene, WindowBackgroundAppearance::Opaque)?;
    let blurred = pixels(&renderer)?;
    assert_eq!(sharp[32 * 64 + 31][0], 0);
    assert!(
        blurred[32 * 64 + 31][0] > 50,
        "backdrop should soften black/white boundary: {:?}",
        blurred[32 * 64 + 31]
    );
    assert!(blurred[32 * 64 + 32][0] < 205);
    assert_eq!(
        blurred[2 * 64 + 31],
        sharp[2 * 64 + 31],
        "outside blur remains untouched"
    );
    Ok(())
}

#[::core::prelude::v1::test]
fn warp_headless_readback_contains_backdrop_blur() -> Result<()> {
    let (mut renderer, _window) = warp_renderer()?;
    let mut scene = Scene::default();
    scene
        .quads
        .push(quad(0, bounds(0., 0., 32., 64.), hsla(0., 0., 0., 1.)));
    scene
        .quads
        .push(quad(0, bounds(32., 0., 32., 64.), hsla(0., 0., 1., 1.)));
    scene.backdrop_blurs.push(blur(1));
    let image = renderer.render_to_image(&scene, WindowBackgroundAppearance::Transparent)?;
    assert!(image.get_pixel(31, 32).0[0] > 50);
    for (rgba, bgra) in image.pixels().zip(pixels(&renderer)?) {
        assert_eq!(rgba.0, [bgra[2], bgra[1], bgra[0], bgra[3]]);
    }
    Ok(())
}

// CPU oracle deliberately uses individual Gaussian taps (not the GPU's paired
// taps) and quantizes each BGRA8 intermediate. Inputs here fill the cached copy.
fn sample(image: &[[u8; 4]], width: usize, height: usize, x: f32, y: f32) -> [f32; 4] {
    let x = x.clamp(0., (width - 1) as f32);
    let y = y.clamp(0., (height - 1) as f32);
    let x0 = x.floor() as usize;
    let y0 = y.floor() as usize;
    let x1 = (x0 + 1).min(width - 1);
    let y1 = (y0 + 1).min(height - 1);
    let tx = x - x0 as f32;
    let ty = y - y0 as f32;
    std::array::from_fn(|c| {
        let top =
            image[y0 * width + x0][c] as f32 * (1. - tx) + image[y0 * width + x1][c] as f32 * tx;
        let bottom =
            image[y1 * width + x0][c] as f32 * (1. - tx) + image[y1 * width + x1][c] as f32 * tx;
        top * (1. - ty) + bottom * ty
    })
}

fn cpu_blur(image: &[[u8; 4]], width: usize, height: usize, blur: BackdropBlur) -> Vec<[u8; 4]> {
    let sigma = blur.blur_radius.0.clamp(1., 64.);
    let stride = ((sigma / 8.) as usize).clamp(1, 4);
    let bw = width.div_ceil(stride);
    let bh = height.div_ceil(stride);
    let radius = (sigma * 3.).ceil() as i32;
    let weights: Vec<f32> = (-radius..=radius)
        .map(|k| (-(k * k) as f32 / (2. * sigma * sigma)).exp())
        .collect();
    let total: f32 = weights.iter().sum();
    let mut horizontal = Vec::with_capacity(bw * bh);
    let mut vertical = Vec::with_capacity(bw * bh);
    for y in 0..height {
        for x in 0..bw {
            let mut sum = [0.; 4];
            for (k, weight) in (-radius..=radius).zip(&weights) {
                let value = sample(
                    image,
                    width,
                    height,
                    (x as f32 + 0.5) * width as f32 / bw as f32 - 0.5 + k as f32,
                    y as f32,
                );
                for c in 0..4 {
                    sum[c] += value[c] * weight / total;
                }
            }
            horizontal.push(sum.map(|v| v.round() as u8));
        }
    }
    for y in 0..bh {
        for x in 0..bw {
            let mut sum = [0.; 4];
            for (k, weight) in (-radius..=radius).zip(&weights) {
                let value = sample(
                    &horizontal,
                    bw,
                    height,
                    x as f32,
                    (y as f32 + 0.5) * height as f32 / bh as f32 - 0.5 + k as f32,
                );
                for c in 0..4 {
                    sum[c] += value[c] * weight / total;
                }
            }
            vertical.push(sum.map(|v| v.round() as u8));
        }
    }
    let mut result = image.to_vec();
    let rect = blur.bounds;
    let clip = rect.intersect(&blur.content_mask.bounds);
    for y in 0..height {
        for x in 0..width {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            if px < clip.origin.x.0
                || px > clip.right().0
                || py < clip.origin.y.0
                || py > clip.bottom().0
            {
                continue;
            }
            let left = px < rect.center().x.0;
            let top = py < rect.center().y.0;
            let radius = match (left, top) {
                (true, true) => blur.corner_radii.top_left.0,
                (false, true) => blur.corner_radii.top_right.0,
                (false, false) => blur.corner_radii.bottom_right.0,
                (true, false) => blur.corner_radii.bottom_left.0,
            };
            let cx = if left {
                rect.left().0 + radius
            } else {
                rect.right().0 - radius
            };
            let cy = if top {
                rect.top().0 + radius
            } else {
                rect.bottom().0 - radius
            };
            if (left && px < cx || !left && px > cx)
                && (top && py < cy || !top && py > cy)
                && (px - cx).hypot(py - cy) > radius
            {
                continue;
            }
            result[y * width + x] = sample(
                &vertical,
                bw,
                bh,
                px * bw as f32 / width as f32 - 0.5,
                py * bh as f32 / height as f32 - 0.5,
            )
            .map(|v| v.round() as u8);
        }
    }
    result
}

fn assert_pixels(actual: &[[u8; 4]], expected: &[[u8; 4]], tolerance: u8) {
    assert_eq!(actual.len(), expected.len());
    for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
        for c in 0..4 {
            assert!(
                actual[c].abs_diff(expected[c]) <= tolerance,
                "pixel {index} channel {c}: actual {actual:?}, expected {expected:?}"
            );
        }
    }
}

fn checkerboard() -> Scene {
    let mut scene = Scene::default();
    for y in 0..16 {
        for x in 0..16 {
            scene.quads.push(quad(
                0,
                bounds((x * 4) as f32, (y * 4) as f32, 4., 4.),
                hsla(0., 0., ((x + y) % 2) as f32, 1.),
            ));
        }
    }
    scene
}

#[::core::prelude::v1::test]
fn warp_nested_backdrops_match_sequential_reference() -> Result<()> {
    let (mut renderer, _window) = warp_renderer()?;
    let mut scene = checkerboard();
    renderer.render(&scene, WindowBackgroundAppearance::Opaque)?;
    let original = pixels(&renderer)?;
    let parent = blur(1);
    let child = BackdropBlur {
        order: 3,
        bounds: bounds(16., 16., 32., 32.),
        content_mask: ContentMask {
            bounds: bounds(20., 18., 24., 28.),
        },
        corner_radii: Corners::all(ScaledPixels(9.)),
        ..blur(3)
    };
    let mut expected = cpu_blur(&original, 64, 64, parent);
    // Intervening content must be in the child's fresh snapshot.
    for y in 0..64 {
        for x in 30..34 {
            expected[y * 64 + x] = [0, 0, 255, 255];
        }
    }
    expected = cpu_blur(&expected, 64, 64, child);
    // Foreground at the child's exact order must remain sharp.
    for y in 28..36 {
        for x in 28..36 {
            expected[y * 64 + x] = [0, 255, 0, 255];
        }
    }
    scene
        .quads
        .push(quad(2, bounds(30., 0., 4., 64.), hsla(0., 1., 0.5, 1.)));
    scene.quads.push(quad(
        3,
        bounds(28., 28., 8., 8.),
        hsla(1. / 3., 1., 0.5, 1.),
    ));
    scene.backdrop_blurs = vec![parent, child];
    renderer.render(&scene, WindowBackgroundAppearance::Opaque)?;
    assert_pixels(&pixels(&renderer)?, &expected, 2);
    Ok(())
}

#[::core::prelude::v1::test]
fn warp_layered_backdrop_samples_earlier_stripe_and_preserves_foreground() -> Result<()> {
    let (mut renderer, _window) = warp_renderer()?;
    renderer.resize(size(DevicePixels(160), DevicePixels(64)))?;
    let viewport = bounds(0., 0., 160., 64.);
    let mut scene = Scene::default();
    let mut background = quad(0, viewport, hsla(0., 0., 0., 1.));
    background.content_mask.bounds = viewport;
    scene.insert_primitive(background);

    let stripe_bounds = bounds(88., 0., 10., 64.);
    let mut stripe = quad(0, stripe_bounds, hsla(0., 0., 1., 1.));
    stripe.content_mask.bounds = viewport;
    scene.push_layer(stripe_bounds);
    scene.insert_primitive(stripe);
    scene.pop_layer();

    let panel = bounds(100., 8., 20., 48.);
    scene.push_layer(panel);
    scene.insert_backdrop_blur(BackdropBlur {
        blur_radius: ScaledPixels(8.),
        bounds: panel,
        content_mask: ContentMask { bounds: viewport },
        corner_radii: Corners::default(),
        ..blur(0)
    });
    let mut foreground = quad(0, bounds(108., 28., 4., 8.), hsla(0., 1., 0.5, 1.));
    foreground.content_mask.bounds = viewport;
    scene.insert_primitive(foreground);
    scene.pop_layer();
    scene.finish();

    renderer.render(&scene, WindowBackgroundAppearance::Opaque)?;
    let image = pixels(&renderer)?;
    assert_eq!(image[32 * 160 + 92], [255; 4]);
    assert_eq!(image[32 * 160 + 99], [0, 0, 0, 255]);
    assert!(
        image[32 * 160 + 101][0] > 20,
        "the blur must sample the earlier stripe outside its output bounds"
    );
    assert_eq!(image[32 * 160 + 108], [0, 0, 255, 255]);
    assert_eq!(image[32 * 160 + 111], [0, 0, 255, 255]);
    for x in [107, 112] {
        let pixel = image[32 * 160 + x];
        assert_eq!(pixel[0], pixel[2], "foreground edges must remain sharp");
    }
    Ok(())
}

#[::core::prelude::v1::test]
fn warp_backdrop_preserves_premultiplied_alpha() -> Result<()> {
    let (mut renderer, _window) = warp_renderer()?;
    let mut scene = Scene::default();
    scene
        .quads
        .push(quad(0, bounds(0., 0., 32., 64.), hsla(0., 1., 0.5, 0.5)));
    renderer.render(&scene, WindowBackgroundAppearance::Transparent)?;
    let original = pixels(&renderer)?;
    let expected = cpu_blur(&original, 64, 64, blur(1));
    scene.backdrop_blurs = vec![blur(1)];
    renderer.render(&scene, WindowBackgroundAppearance::Transparent)?;
    let actual = pixels(&renderer)?;
    assert_pixels(&actual, &expected, 2);
    assert_eq!(
        actual[32 * 64 + 16][3],
        original[32 * 64 + 16][3],
        "replacement must not accumulate alpha"
    );
    for pixel in actual {
        assert!(
            pixel[2] <= pixel[3] + 1,
            "premultiplied red cannot exceed alpha"
        );
    }
    Ok(())
}

#[::core::prelude::v1::test]
fn warp_backdrop_downsampling_and_large_kernel() -> Result<()> {
    let (mut renderer, _window) = warp_renderer()?;
    renderer.resize(size(DevicePixels(65), DevicePixels(67)))?;
    // Upload a known gradient plus a sharp edge directly, before any ordinary
    // primitive has bound globals. This also covers first-operation blurs.
    let original: Vec<[u8; 4]> = (0..67)
        .flat_map(|y| {
            (0..65).map(move |x| {
                [
                    (x * 3) as u8,
                    (y * 3) as u8,
                    if x < 32 { 0 } else { 255 },
                    255,
                ]
            })
        })
        .collect();
    for sigma in [0.1, 2.5, 16., 24., 32., 64., 180., f32::MAX] {
        renderer.pre_draw(&[0.; 4])?;
        let context = &renderer.devices.as_ref().unwrap().device_context;
        unsafe {
            context.VSSetConstantBuffers(0, Some(&[None]));
            context.PSSetConstantBuffers(0, Some(&[None]));
            context.OMSetRenderTargets(None, None);
            context.UpdateSubresource(
                renderer
                    .resources
                    .as_ref()
                    .unwrap()
                    .render_target
                    .as_ref()
                    .unwrap(),
                0,
                None,
                original.as_ptr().cast(),
                65 * 4,
                0,
            );
        }
        let blur = BackdropBlur {
            blur_radius: ScaledPixels(sigma),
            bounds: bounds(0., 0., 65., 67.),
            content_mask: ContentMask {
                bounds: bounds(0., 0., 65., 67.),
            },
            corner_radii: Corners::default(),
            ..blur(0)
        };
        renderer.draw_backdrop_blur(&blur)?;
        assert_pixels(&pixels(&renderer)?, &cpu_blur(&original, 65, 67, blur), 2);
    }
    Ok(())
}

#[::core::prelude::v1::test]
fn warp_backdrop_cache_lifecycle() -> Result<()> {
    let (mut renderer, _window) = warp_renderer()?;
    let empty = Scene::default();
    renderer.render(&empty, WindowBackgroundAppearance::Opaque)?;
    assert!(renderer.resources.as_ref().unwrap().backdrop.is_none());
    let mut scene = checkerboard();
    scene.backdrop_blurs = vec![blur(1)];
    renderer.render(&scene, WindowBackgroundAppearance::Opaque)?;
    let expected = pixels(&renderer)?;
    let scratch = scratch_textures(&renderer, 1);
    renderer.render(&scene, WindowBackgroundAppearance::Opaque)?;
    assert_eq!(
        scratch,
        scratch_textures(&renderer, 1),
        "stable blur reuses all GPU scratch textures"
    );
    assert_pixels(&pixels(&renderer)?, &expected, 0);
    drop(scratch);
    for _ in 0..119 {
        renderer.render(&empty, WindowBackgroundAppearance::Opaque)?;
    }
    assert!(renderer.resources.as_ref().unwrap().backdrop.is_some());
    renderer.render(&empty, WindowBackgroundAppearance::Opaque)?;
    assert!(
        renderer.resources.as_ref().unwrap().backdrop.is_none(),
        "idle cache must be released"
    );
    renderer.render(&scene, WindowBackgroundAppearance::Opaque)?;
    assert_pixels(&pixels(&renderer)?, &expected, 0);
    renderer.resize(size(DevicePixels(81), DevicePixels(73)))?;
    assert!(
        renderer.resources.as_ref().unwrap().backdrop.is_none(),
        "resize releases scratch"
    );
    renderer.resize(size(DevicePixels(64), DevicePixels(64)))?;
    renderer.render(&scene, WindowBackgroundAppearance::Opaque)?;
    assert_pixels(&pixels(&renderer)?, &expected, 0);
    let new_devices = warp_devices()?;
    renderer.handle_device_lost(&new_devices)?;
    assert!(
        renderer.resources.as_ref().unwrap().backdrop.is_none(),
        "recovery releases all old-device blur state"
    );
    // The recovery frame must still be skipped, then the platform makes the
    // renderer drawable once atlas/scene resources have been regenerated.
    renderer.draw(&scene, WindowBackgroundAppearance::Opaque)?;
    assert!(renderer.resources.as_ref().unwrap().backdrop.is_none());
    renderer.mark_drawable();
    renderer.render(&scene, WindowBackgroundAppearance::Opaque)?;
    assert_pixels(&pixels(&renderer)?, &expected, 0);
    Ok(())
}

#[::core::prelude::v1::test]
fn warp_backdrop_reuses_scratch_for_alternating_downsample_factors() -> Result<()> {
    let (mut renderer, _window) = warp_renderer()?;
    let (mut fresh, _fresh_window) = warp_renderer()?;
    for renderer in [&mut renderer, &mut fresh] {
        renderer.resize(size(DevicePixels(257), DevicePixels(193)))?;
    }

    let viewport = bounds(0., 0., 257., 193.);
    let mut scene = Scene::default();
    for (x, width, lightness, alpha) in [
        (0., 80., 0., 0.45),
        (80., 20., 1., 0.8),
        (100., 157., 0., 0.45),
    ] {
        let mut stripe = quad(
            0,
            bounds(x, 0., width, 193.),
            hsla(0., 0., lightness, alpha),
        );
        stripe.content_mask.bounds = viewport;
        scene.quads.push(stripe);
    }
    renderer.render(&scene, WindowBackgroundAppearance::Transparent)?;
    let original = pixels(&renderer)?;
    let parent = BackdropBlur {
        order: 1,
        blur_radius: ScaledPixels(17.5),
        bounds: bounds(48., 40., 100., 80.),
        content_mask: ContentMask {
            bounds: bounds(60., 45., 65., 50.),
        },
        ..blur(1)
    };
    let child = BackdropBlur {
        order: 2,
        blur_radius: ScaledPixels(12.5),
        bounds: bounds(70., 55., 40., 30.),
        content_mask: ContentMask {
            bounds: bounds(75., 60., 25., 20.),
        },
        ..blur(2)
    };
    scene.backdrop_blurs = vec![parent, child];
    renderer.render(&scene, WindowBackgroundAppearance::Transparent)?;
    // Keep COM references to make replacement distinguishable from pointer reuse.
    let factor_one = scratch_textures(&renderer, 1);
    let factor_two = scratch_textures(&renderer, 2);
    assert_eq!(texture_sizes(&factor_one), [[128, 128]; 3]);
    assert_eq!(
        texture_sizes(&factor_two),
        [[192, 192], [96, 192], [96, 96]]
    );
    let backdrop = renderer
        .resources
        .as_ref()
        .unwrap()
        .backdrop
        .as_ref()
        .unwrap();
    assert!(backdrop.scratch_textures(3).is_none());
    assert!(backdrop.scratch_textures(4).is_none());

    for frame in 0..6 {
        let offset = if frame % 3 == 0 { 100. } else { 0. };
        let moving_child = BackdropBlur {
            bounds: bounds(70. + offset, 55., 40., 30.),
            content_mask: ContentMask {
                bounds: bounds(75. + offset, 60., 25., 20.),
            },
            ..child
        };
        scene.backdrop_blurs = if frame % 2 == 0 {
            vec![parent, moving_child]
        } else {
            vec![
                BackdropBlur {
                    order: 1,
                    ..moving_child
                },
                BackdropBlur { order: 2, ..parent },
            ]
        };
        renderer.render(&scene, WindowBackgroundAppearance::Transparent)?;
        assert_eq!(scratch_textures(&renderer, 1), factor_one);
        assert_eq!(scratch_textures(&renderer, 2), factor_two);
        let actual = pixels(&renderer)?;
        fresh.resources.as_mut().unwrap().backdrop = None;
        fresh.render(&scene, WindowBackgroundAppearance::Transparent)?;
        assert_pixels(&actual, &pixels(&fresh)?, 2);
        assert_eq!(actual[70 * 257 + 55], original[70 * 257 + 55]);
        assert_ne!(actual[70 * 257 + 82], original[70 * 257 + 82]);
        assert!(actual[70 * 257 + 55][3] > 0 && actual[70 * 257 + 55][3] < 255);
        assert!(actual.iter().all(|pixel| {
            pixel[..3]
                .iter()
                .all(|channel| *channel <= pixel[3].saturating_add(1))
        }));
    }

    // Growing one factor must not recreate or enlarge the other factor's set.
    let grown_child = BackdropBlur {
        bounds: viewport,
        content_mask: ContentMask { bounds: viewport },
        corner_radii: Corners::default(),
        ..child
    };
    scene.backdrop_blurs = vec![parent, grown_child];
    renderer.render(&scene, WindowBackgroundAppearance::Transparent)?;
    let grown_one = scratch_textures(&renderer, 1);
    assert_eq!(texture_sizes(&grown_one), [[257, 193]; 3]);
    for (old, new) in factor_one.iter().zip(&grown_one) {
        assert_ne!(old, new);
    }
    assert_eq!(scratch_textures(&renderer, 2), factor_two);

    // All four factor slots stay bounded and independent when populated.
    scene.backdrop_blurs.extend([
        BackdropBlur {
            order: 3,
            blur_radius: ScaledPixels(24.),
            ..child
        },
        BackdropBlur {
            order: 4,
            blur_radius: ScaledPixels(32.),
            ..child
        },
    ]);
    renderer.render(&scene, WindowBackgroundAppearance::Transparent)?;
    let factor_three = scratch_textures(&renderer, 3);
    let factor_four = scratch_textures(&renderer, 4);
    let before_resize = [grown_one, factor_two, factor_three, factor_four];
    renderer.render(&scene, WindowBackgroundAppearance::Transparent)?;
    for factor in 1..=4 {
        assert_eq!(
            scratch_textures(&renderer, factor),
            before_resize[(factor - 1) as usize]
        );
    }
    fresh.resources.as_mut().unwrap().backdrop = None;
    fresh.render(&scene, WindowBackgroundAppearance::Transparent)?;
    assert_pixels(&pixels(&renderer)?, &pixels(&fresh)?, 2);

    renderer.resize(size(DevicePixels(197), DevicePixels(157)))?;
    assert!(renderer.resources.as_ref().unwrap().backdrop.is_none());
    fresh.resize(size(DevicePixels(197), DevicePixels(157)))?;
    renderer.render(&scene, WindowBackgroundAppearance::Transparent)?;
    let after_resize: [[ID3D11Texture2D; 3]; 4] =
        std::array::from_fn(|index| scratch_textures(&renderer, index as u32 + 1));
    for (old_set, new_set) in before_resize.iter().zip(&after_resize) {
        for (old, new) in old_set.iter().zip(new_set) {
            assert_ne!(old, new, "resize must recreate each scratch texture");
        }
    }
    assert_eq!(texture_sizes(&after_resize[0]), [[197, 157]; 3]);
    fresh.render(&scene, WindowBackgroundAppearance::Transparent)?;
    let expected = pixels(&fresh)?;
    assert_pixels(&pixels(&renderer)?, &expected, 2);

    let empty = Scene::default();
    for _ in 0..119 {
        renderer.render(&empty, WindowBackgroundAppearance::Transparent)?;
    }
    assert!(renderer.resources.as_ref().unwrap().backdrop.is_some());
    for factor in 1..=4 {
        assert_eq!(
            scratch_textures(&renderer, factor),
            after_resize[(factor - 1) as usize]
        );
    }
    renderer.render(&empty, WindowBackgroundAppearance::Transparent)?;
    assert!(renderer.resources.as_ref().unwrap().backdrop.is_none());
    renderer.render(&scene, WindowBackgroundAppearance::Transparent)?;
    let after_idle: [[ID3D11Texture2D; 3]; 4] =
        std::array::from_fn(|index| scratch_textures(&renderer, index as u32 + 1));
    for (old_set, new_set) in after_resize.iter().zip(&after_idle) {
        for (old, new) in old_set.iter().zip(new_set) {
            assert_ne!(old, new, "idle expiry must recreate each scratch texture");
        }
    }
    assert_pixels(&pixels(&renderer)?, &expected, 2);

    renderer.handle_device_lost(&warp_devices()?)?;
    assert!(renderer.resources.as_ref().unwrap().backdrop.is_none());
    renderer.draw(&scene, WindowBackgroundAppearance::Transparent)?;
    assert!(renderer.resources.as_ref().unwrap().backdrop.is_none());
    renderer.mark_drawable();
    renderer.render(&scene, WindowBackgroundAppearance::Transparent)?;
    for factor in 1..=4 {
        for (old, new) in after_idle[(factor - 1) as usize]
            .iter()
            .zip(&scratch_textures(&renderer, factor))
        {
            assert_ne!(
                old, new,
                "device recovery must recreate each scratch texture"
            );
        }
    }
    assert_pixels(&pixels(&renderer)?, &expected, 2);
    Ok(())
}

#[::core::prelude::v1::test]
fn warp_backdrop_clipped_and_blur_only_frames() -> Result<()> {
    let (mut renderer, _window) = warp_renderer()?;
    let mut scene = Scene::default();
    scene.backdrop_blurs = vec![BackdropBlur {
        bounds: bounds(-100., -100., 8., 8.),
        ..blur(0)
    }];
    renderer.render(&scene, WindowBackgroundAppearance::Opaque)?;
    assert!(
        renderer.resources.as_ref().unwrap().backdrop.is_none(),
        "offscreen blur allocates nothing"
    );
    for sigma in [0., -1., f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        scene.backdrop_blurs = vec![BackdropBlur {
            blur_radius: ScaledPixels(sigma),
            ..blur(0)
        }];
        renderer.render(&scene, WindowBackgroundAppearance::Opaque)?;
        assert!(renderer.resources.as_ref().unwrap().backdrop.is_none());
    }
    // More than 32 blur-only operations also guards against silently adopting
    // another backend's per-frame uniform-slot cap.
    scene.backdrop_blurs = (0..40).map(blur).collect();
    renderer.render(&scene, WindowBackgroundAppearance::Opaque)?;
    assert!(
        renderer.resources.as_ref().unwrap().backdrop.is_some(),
        "a visible blur-only scene must execute the GPU passes"
    );
    assert!(pixels(&renderer)?.iter().all(|pixel| *pixel == [255; 4]));
    Ok(())
}

#[::core::prelude::v1::test]
fn warp_backdrop_reuses_cropped_snapshot_at_window_edges() -> Result<()> {
    let (mut renderer, _window) = warp_renderer()?;
    renderer.resize(size(DevicePixels(193), DevicePixels(131)))?;
    let original: Vec<[u8; 4]> = (0..131)
        .flat_map(|y| {
            (0..193).map(move |x| {
                [
                    (x % 256) as u8,
                    (y % 256) as u8,
                    if (x / 4 + y / 4) % 2 == 0 { 0 } else { 255 },
                    255,
                ]
            })
        })
        .collect();
    renderer.pre_draw(&[0.; 4])?;
    unsafe {
        let context = &renderer.devices.as_ref().unwrap().device_context;
        context.OMSetRenderTargets(None, None);
        context.UpdateSubresource(
            renderer
                .resources
                .as_ref()
                .unwrap()
                .render_target
                .as_ref()
                .unwrap(),
            0,
            None,
            original.as_ptr().cast(),
            193 * 4,
            0,
        );
    }
    let mut expected = original;
    for rect in [
        bounds(166., 108., 24., 20.),
        bounds(0., 0., 21., 18.),
        bounds(80., 60., 8., 9.),
    ] {
        let region = BackdropBlur {
            bounds: rect,
            content_mask: ContentMask {
                bounds: bounds(0., 0., 193., 131.),
            },
            corner_radii: Corners::all(ScaledPixels(4.)),
            ..blur(0)
        };
        renderer.draw_backdrop_blur(&region)?;
        // Full-frame CPU filtering is independent of the GPU's smaller cache
        // and moving copy origin; only the composited region should differ.
        expected = cpu_blur(&expected, 193, 131, region);
        assert_pixels(&pixels(&renderer)?, &expected, 2);
    }
    Ok(())
}

#[::core::prelude::v1::test]
fn warp_composer_tint_preserves_translucent_backdrop() -> Result<()> {
    let (mut renderer, _window) = warp_renderer()?;
    for shell_alpha in [0.8, 1.0] {
        for with_blur in [true, false] {
            let mut scene = Scene::default();
            scene.quads.push(quad(
                0,
                bounds(0., 0., 64., 64.),
                hsla(0., 0., 0.2, shell_alpha),
            ));
            if with_blur {
                scene.backdrop_blurs = vec![blur(1)];
            }
            scene
                .quads
                .push(quad(1, bounds(8., 8., 48., 48.), hsla(0., 0., 0.1, 0.15)));
            renderer.render(&scene, WindowBackgroundAppearance::Transparent)?;
            let actual = pixels(&renderer)?;
            let pixel = actual[32 * 64 + 32];
            let expected_alpha = 0.15 + shell_alpha * 0.85;
            assert!(
                (pixel[3] as f32 / 255. - expected_alpha).abs() < 0.01,
                "shell={shell_alpha}, blur={with_blur}: alpha {:?}, expected {expected_alpha}",
                pixel
            );
            // Composite the premultiplied result over a white desktop.
            let expected_rgb = 0.1 * 0.15 + 0.2 * shell_alpha * 0.85 + 1. - expected_alpha;
            for channel in &pixel[..3] {
                let over_white = *channel as f32 / 255. + 1. - pixel[3] as f32 / 255.;
                assert!(
                    (over_white - expected_rgb).abs() < 0.02,
                    "composer must retain desktop contribution: {over_white} vs {expected_rgb}"
                );
            }
            assert!((actual[0][3] as f32 / 255. - shell_alpha).abs() < 0.01);
            if with_blur {
                // A second frosted surface must retain the parent's transparency.
                let mut child = blur(2);
                child.bounds = bounds(20., 20., 24., 24.);
                scene.backdrop_blurs.push(child);
                scene
                    .quads
                    .push(quad(2, child.bounds, hsla(0., 0., 0.1, 0.15)));
                renderer.render(&scene, WindowBackgroundAppearance::Transparent)?;
                let nested = pixels(&renderer)?[32 * 64 + 32];
                let nested_alpha = 0.15 + expected_alpha * 0.85;
                assert!(
                    (nested[3] as f32 / 255. - nested_alpha).abs() < 0.01,
                    "nested tint must retain backdrop alpha: {nested:?}, expected {nested_alpha}"
                );
            }
        }
    }
    Ok(())
}

#[::core::prelude::v1::test]
fn warp_path_tint_preserves_translucent_backdrop() -> Result<()> {
    let (mut renderer, _window) = warp_renderer()?;
    let mut scene = Scene::default();
    scene
        .quads
        .push(quad(0, bounds(0., 0., 64., 64.), hsla(0., 0., 0.2, 0.8)));
    scene.backdrop_blurs = vec![blur(1)];
    let mut path = gpui::Path::new(point(gpui::px(8.), gpui::px(8.)));
    path.line_to(point(gpui::px(56.), gpui::px(8.)));
    path.line_to(point(gpui::px(56.), gpui::px(56.)));
    let mut path = path.scale(1.);
    path.order = 1;
    path.color = hsla(0., 0., 0.1, 0.15).into();
    path.content_mask.bounds = bounds(0., 0., 64., 64.);
    for vertex in &mut path.vertices {
        vertex.content_mask = path.content_mask;
    }
    scene.paths.push(path);
    renderer.render(&scene, WindowBackgroundAppearance::Transparent)?;
    let actual = pixels(&renderer)?;
    let pixel = actual[24 * 64 + 40];
    assert!(
        (pixel[3] as f32 / 255. - 0.83).abs() < 0.01,
        "path tint must retain backdrop alpha: {pixel:?}"
    );
    Ok(())
}

#[::core::prelude::v1::test]
fn warp_fractional_blur_clip_matches_primitive_clip() -> Result<()> {
    let (mut renderer, _window) = warp_renderer()?;
    for scale in [1.0, 1.25, 1.5, 2.0] {
        let mut scene = Scene::default();
        for x in 0..64 {
            scene.quads.push(quad(
                0,
                bounds(x as f32, 0., 1., 64.),
                hsla(0., 0., if x % 2 == 0 { 0. } else { 1. }, 0.8),
            ));
        }
        renderer.render(&scene, WindowBackgroundAppearance::Transparent)?;
        let original = pixels(&renderer)?;
        let mut region = blur(1);
        region.corner_radii = Corners::default();
        scene.backdrop_blurs = vec![region];
        renderer.render(&scene, WindowBackgroundAppearance::Transparent)?;
        let filtered = pixels(&renderer)?;

        // Use the real quad rasterizer as the clipping oracle, rather than
        // duplicating the blur shader's comparisons in the expected result.
        let clip = bounds(10.5 * scale, 11.5 * scale, 12. * scale, 13. * scale);
        let mut mask = quad(0, bounds(0., 0., 64., 64.), hsla(0., 0., 1., 1.));
        mask.content_mask.bounds = clip;
        let mut mask_scene = Scene::default();
        mask_scene.quads.push(mask);
        renderer.render(&mask_scene, WindowBackgroundAppearance::Transparent)?;
        let mask_pixels = pixels(&renderer)?;
        region.content_mask.bounds = clip;
        scene.backdrop_blurs = vec![region];
        renderer.render(&scene, WindowBackgroundAppearance::Transparent)?;
        let actual = pixels(&renderer)?;
        let expected: Vec<_> = original
            .iter()
            .zip(&filtered)
            .zip(&mask_pixels)
            .map(|((before, after), mask)| if mask[3] > 0 { *after } else { *before })
            .collect();
        assert_pixels(&actual, &expected, 2);
    }
    Ok(())
}

#[::core::prelude::v1::test]
fn warp_translucent_sprites_keep_backdrop_at_scaled_edges() -> Result<()> {
    let (mut renderer, _window) = warp_renderer()?;
    let tile_size = size(DevicePixels(8), DevicePixels(8));
    for monochrome in [true, false] {
        let key = if monochrome {
            AtlasKey::Svg(RenderSvgParams {
                path: "coverage-fixture".into(),
                size: tile_size,
            })
        } else {
            AtlasKey::Image(RenderImageParams {
                image_id: ImageId(1001),
                frame_index: 0,
            })
        };
        let coverage = [0, 16, 64, 128, 192, 255, 128, 0];
        let bytes: Vec<u8> = (0..64)
            .flat_map(|i| {
                if monochrome {
                    vec![coverage[i % 8]]
                } else {
                    vec![40, 90, 180, coverage[i % 8]]
                }
            })
            .collect();
        let tile = renderer
            .atlas
            .get_or_insert_with(key, &mut || {
                Ok(Some((tile_size, std::borrow::Cow::Borrowed(&bytes))))
            })?
            .unwrap();
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let sprite_bounds = bounds(16.25, 16.5, 8. * scale, 8. * scale);
            let content_mask = ContentMask {
                bounds: bounds(0., 0., 64., 64.),
            };
            let mut scene = Scene::default();
            if monochrome {
                scene.monochrome_sprites.push(MonochromeSprite {
                    order: 1,
                    pad: 0,
                    bounds: sprite_bounds,
                    content_mask,
                    color: hsla(0.1, 0.6, 0.8, 0.6),
                    fade: EdgeFadeParams::default(),
                    tile,
                    transformation: TransformationMatrix::default(),
                });
            } else {
                scene.polychrome_sprites.push(PolychromeSprite {
                    order: 1,
                    pad: 0,
                    grayscale: false.into(),
                    opacity: 0.6,
                    bounds: sprite_bounds,
                    content_mask,
                    corner_radii: Corners::all(ScaledPixels(2. * scale)),
                    fade: EdgeFadeParams::default(),
                    tile,
                });
            }
            renderer.render(&scene, WindowBackgroundAppearance::Transparent)?;
            let foreground = pixels(&renderer)?;
            assert!(
                foreground
                    .iter()
                    .any(|pixel| pixel[3] > 0 && pixel[3] < 150),
                "fixture must exercise partial glyph/image coverage"
            );
            scene
                .quads
                .push(quad(0, bounds(0., 0., 64., 64.), hsla(0., 0., 0.3, 0.8)));
            renderer.render(&scene, WindowBackgroundAppearance::Transparent)?;
            let actual = pixels(&renderer)?;
            for (src, actual) in foreground.iter().zip(actual) {
                let a = src[3] as f32 / 255.;
                let expected = [
                    src[0] as f32 + 255. * 0.3 * 0.8 * (1. - a),
                    src[1] as f32 + 255. * 0.3 * 0.8 * (1. - a),
                    src[2] as f32 + 255. * 0.3 * 0.8 * (1. - a),
                    255. * (a + 0.8 * (1. - a)),
                ];
                for channel in 0..4 {
                    assert!(
                        (actual[channel] as f32 - expected[channel]).abs() <= 2.,
                        "mono={monochrome} scale={scale} channel={channel}: {actual:?} vs {expected:?}"
                    );
                }
            }
        }
    }
    Ok(())
}

fn assert_fade_ramp(
    renderer: &mut DirectXRenderer,
    scene: &mut Scene,
    apply: impl FnOnce(&mut Scene, EdgeFadeParams),
) -> Result<()> {
    renderer.render(scene, WindowBackgroundAppearance::Transparent)?;
    let reference = pixels(renderer)?;
    apply(
        scene,
        EdgeFadeParams {
            top_y: 16.,
            band_top: 16.,
            ..Default::default()
        },
    );
    renderer.render(scene, WindowBackgroundAppearance::Transparent)?;
    let faded = pixels(renderer)?;
    for y in [8, 24, 40] {
        let ramp = ((y as f32 + 0.5 - 16.) / 16.).clamp(0., 1.);
        let opacity = ramp * ramp;
        for x in [3, 32] {
            let index = y * 64 + x;
            assert!(reference[index][3] > 0, "fade fixture must be visible");
            for channel in 0..4 {
                let expected = reference[index][channel] as f32 * opacity;
                assert!(
                    (faded[index][channel] as f32 - expected).abs() <= 2.,
                    "fade at ({x},{y}), channel {channel}: {:?} versus {:?} * {opacity}",
                    faded[index],
                    reference[index]
                );
            }
        }
    }
    Ok(())
}

#[::core::prelude::v1::test]
fn warp_edge_fade_applies_once_after_border_composition_and_to_sprites() -> Result<()> {
    let (mut renderer, _window) = warp_renderer()?;
    let mut scene = Scene::default();
    let mut bordered = quad(0, bounds(0., 0., 64., 64.), hsla(2. / 3., 1., 0.5, 0.4));
    bordered.border_widths = Edges::all(ScaledPixels(10.));
    bordered.border_color = hsla(0., 1., 0.5, 0.5);
    scene.quads.push(bordered);
    assert_fade_ramp(&mut renderer, &mut scene, |scene, fade| {
        scene.quads[0].fade = fade;
    })?;

    for monochrome in [true, false] {
        let tile_size = size(DevicePixels(8), DevicePixels(8));
        let key = if monochrome {
            AtlasKey::Svg(RenderSvgParams {
                path: "edge-fade-fixture".into(),
                size: tile_size,
            })
        } else {
            AtlasKey::Image(RenderImageParams {
                image_id: ImageId(1002),
                frame_index: 0,
            })
        };
        let bytes = if monochrome {
            vec![255; 64]
        } else {
            [40, 90, 180, 192].repeat(64)
        };
        let tile = renderer
            .atlas
            .get_or_insert_with(key, &mut || {
                Ok(Some((tile_size, std::borrow::Cow::Borrowed(&bytes))))
            })?
            .unwrap();
        let mut scene = Scene::default();
        let sprite_bounds = bounds(0., 0., 64., 64.);
        let content_mask = ContentMask {
            bounds: sprite_bounds,
        };
        if monochrome {
            scene.monochrome_sprites.push(MonochromeSprite {
                order: 0,
                pad: 0,
                tile,
                bounds: sprite_bounds,
                content_mask,
                color: hsla(0.1, 0.6, 0.8, 0.6),
                transformation: TransformationMatrix::default(),
                fade: EdgeFadeParams::default(),
            });
        } else {
            scene.polychrome_sprites.push(PolychromeSprite {
                order: 0,
                pad: 0,
                tile,
                grayscale: false.into(),
                opacity: 0.6,
                bounds: sprite_bounds,
                content_mask,
                corner_radii: Corners::default(),
                fade: EdgeFadeParams::default(),
            });
        }
        assert_fade_ramp(&mut renderer, &mut scene, |scene, fade| {
            if monochrome {
                scene.monochrome_sprites[0].fade = fade;
            } else {
                scene.polychrome_sprites[0].fade = fade;
            }
        })?;
    }
    Ok(())
}

#[::core::prelude::v1::test]
fn warp_animated_nested_blur_matches_fresh_resources() -> Result<()> {
    let (mut cached, _cached_window) = warp_renderer()?;
    let (mut fresh, _fresh_window) = warp_renderer()?;
    for (width, height) in [(193, 131), (137, 99), (211, 143)] {
        cached.resize(size(DevicePixels(width), DevicePixels(height)))?;
        fresh.resize(size(DevicePixels(width), DevicePixels(height)))?;
        for frame in 0..12 {
            let viewport = bounds(0., 0., width as f32, height as f32);
            let mut scene = Scene::default();
            for x in 0..width {
                let mut stripe = quad(
                    0,
                    bounds(x as f32, 0., 1., height as f32),
                    hsla(0.1, 0.5, if (x + frame) % 7 < 3 { 0.2 } else { 0.8 }, 0.7),
                );
                stripe.content_mask.bounds = viewport;
                scene.quads.push(stripe);
            }
            let offset = frame as f32 * 7.25;
            let parent = BackdropBlur {
                bounds: bounds(offset - 12., 4.25, 100., 80.),
                content_mask: ContentMask { bounds: viewport },
                blur_radius: ScaledPixels(if frame % 3 == 0 { 16. } else { 3. }),
                ..blur(1)
            };
            let mut tint = quad(2, parent.bounds, hsla(0., 0., 0.1, 0.15));
            tint.content_mask.bounds = viewport;
            scene.quads.push(tint);
            scene.backdrop_blurs = vec![
                parent,
                BackdropBlur {
                    bounds: bounds(offset + 9.5, 21.25, 41.5, 39.5),
                    content_mask: ContentMask {
                        bounds: parent.bounds.intersect(&viewport),
                    },
                    ..blur(3)
                },
            ];
            cached.render(&scene, WindowBackgroundAppearance::Transparent)?;
            let actual = pixels(&cached)?;
            // Discard all blur resources in the reference renderer each frame.
            // Pixel output must not depend on previously drawn sizes or content.
            fresh.resources.as_mut().unwrap().backdrop = None;
            fresh.render(&scene, WindowBackgroundAppearance::Transparent)?;
            assert_pixels(&actual, &pixels(&fresh)?, 2);
        }
    }
    Ok(())
}

#[::core::prelude::v1::test]
fn warp_large_blur_removes_fine_stripes() -> Result<()> {
    let (mut renderer, _window) = warp_renderer()?;
    renderer.resize(size(DevicePixels(256), DevicePixels(256)))?;
    for phase in 0..4 {
        for horizontal in [false, true] {
            let mut scene = Scene::default();
            for n in 0..256 {
                let rect = if horizontal {
                    bounds(0., n as f32, 256., 1.)
                } else {
                    bounds(n as f32, 0., 1., 256.)
                };
                let mut stripe = quad(
                    0,
                    rect,
                    hsla(0., 0., if (n + phase) % 4 < 2 { 0. } else { 1. }, 1.),
                );
                stripe.content_mask.bounds = bounds(0., 0., 256., 256.);
                scene.quads.push(stripe);
            }
            scene.backdrop_blurs.push(BackdropBlur {
                bounds: bounds(0., 0., 256., 256.),
                content_mask: ContentMask {
                    bounds: bounds(0., 0., 256., 256.),
                },
                blur_radius: ScaledPixels(32.),
                corner_radii: Corners::default(),
                ..blur(1)
            });
            renderer.render(&scene, WindowBackgroundAppearance::Transparent)?;
            let image = pixels(&renderer)?;

            assert!(
                (image[128 * 256 + 128][0] as i32 - 128).abs() <= 3,
                "fine stripes should average to gray: orientation={horizontal}, center={:?}",
                image[128 * 256 + 128]
            );
        }
    }
    Ok(())
}

#[::core::prelude::v1::test]
fn warp_invalid_geometry_allocates_nothing_and_idle_cache_expires() -> Result<()> {
    let (mut renderer, _window) = warp_renderer()?;
    let mut scene = Scene::default();
    for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.] {
        for field in 0..4 {
            let mut region = blur(0);
            match field {
                0 => region.bounds.size.width = ScaledPixels(value),
                1 => region.content_mask.bounds.size.height = ScaledPixels(value),
                2 => region.corner_radii.top_left = ScaledPixels(value),
                _ => {
                    region.bounds.origin.x =
                        ScaledPixels(if value == -1. { f32::INFINITY } else { value })
                }
            }
            scene.backdrop_blurs = vec![region];
            renderer.render(&scene, WindowBackgroundAppearance::Opaque)?;
            assert!(renderer.resources.as_ref().unwrap().backdrop.is_none());
            assert!(pixels(&renderer)?.iter().all(|pixel| *pixel == [255; 4]));
        }
    }
    scene.backdrop_blurs = vec![BackdropBlur {
        bounds: bounds(f32::MAX, 0., f32::MAX, 10.),
        ..blur(0)
    }];
    renderer.render(&scene, WindowBackgroundAppearance::Opaque)?;
    assert!(renderer.resources.as_ref().unwrap().backdrop.is_none());
    scene.backdrop_blurs = vec![blur(0)];
    renderer.render(&scene, WindowBackgroundAppearance::Opaque)?;
    assert!(renderer.resources.as_ref().unwrap().backdrop.is_some());
    scene.backdrop_blurs[0].bounds = bounds(-100., -100., 8., 8.);
    for _ in 0..120 {
        renderer.render(&scene, WindowBackgroundAppearance::Opaque)?;
    }
    assert!(renderer.resources.as_ref().unwrap().backdrop.is_none());
    Ok(())
}

fn rounded_clip_fixture(scale: f32) -> RoundedClipParams {
    RoundedClipParams {
        left: 8.25 * scale,
        top: 8.25 * scale,
        right: 55.75 * scale,
        bottom: 55.75 * scale,
        top_left: 18. * scale,
        top_right: 0.,
        bottom_right: 12. * scale,
        bottom_left: 5. * scale,
    }
}

fn rounded_clip_coverage(clip: RoundedClipParams, x: usize, y: usize) -> f32 {
    if clip.right <= clip.left {
        return 1.;
    }
    let half_x = (clip.right - clip.left) * 0.5;
    let half_y = (clip.bottom - clip.top) * 0.5;
    let dx = x as f32 + 0.5 - (clip.left + half_x);
    let dy = y as f32 + 0.5 - (clip.top + half_y);
    let radius = match (dx < 0., dy < 0.) {
        (true, true) => clip.top_left,
        (false, true) => clip.top_right,
        (false, false) => clip.bottom_right,
        (true, false) => clip.bottom_left,
    };
    let qx = dx.abs() - half_x + radius;
    let qy = dy.abs() - half_y + radius;
    let distance = qx.max(0.).hypot(qy.max(0.)) + qx.max(qy).min(0.) - radius;
    (0.5 - distance).clamp(0., 1.)
}

fn assert_rounded_clip(
    renderer: &mut DirectXRenderer,
    scene: &mut Scene,
    appearance: WindowBackgroundAppearance,
    clip: RoundedClipParams,
    apply: impl FnOnce(&mut Scene, RoundedClipParams),
) -> Result<Vec<[u8; 4]>> {
    renderer.render(scene, appearance)?;
    let reference = pixels(renderer)?;
    apply(scene, clip);
    renderer.render(scene, appearance)?;
    let actual = pixels(renderer)?;
    let clear = match appearance {
        WindowBackgroundAppearance::Opaque => 255.,
        _ => 0.,
    };
    let mut samples = [false; 3];
    for y in 0..64 {
        for x in 0..64 {
            let coverage = rounded_clip_coverage(clip, x, y);
            let index = y * 64 + x;
            let visible = reference[index]
                .iter()
                .any(|channel| (*channel as f32 - clear).abs() > 16.);
            if visible {
                if coverage == 0. {
                    samples[0] = true;
                } else if coverage < 0.9 {
                    samples[1] = true;
                } else if coverage == 1. {
                    samples[2] = true;
                }
            }
            for channel in 0..4 {
                let expected = clear + (reference[index][channel] as f32 - clear) * coverage;
                assert!(
                    (actual[index][channel] as f32 - expected).abs() <= 4.,
                    "({x},{y}) channel {channel}, coverage {coverage}: {:?} versus {:?}",
                    actual[index],
                    reference[index]
                );
            }
        }
    }
    assert_eq!(
        samples, [true; 3],
        "fixture must cover outside, antialiased and inside pixels"
    );
    Ok(actual)
}

#[::core::prelude::v1::test]
fn warp_rounded_clip_covers_distinct_corners_and_faded_borders() -> Result<()> {
    let (mut renderer, _window) = warp_renderer()?;
    for (scale, bordered) in [(1., false), (1., true), (1.1, true)] {
        let mut scene = Scene::default();
        let mut item = quad(0, bounds(0., 0., 64., 64.), hsla(2. / 3., 1., 0.5, 0.4));
        if bordered {
            item.border_widths = Edges::all(ScaledPixels(10.));
            item.border_color = hsla(0., 1., 0.5, 0.5);
        }
        item.fade.top_y = -20.;
        item.fade.band_top = 40.;
        scene.quads.push(item);
        let actual = assert_rounded_clip(
            &mut renderer,
            &mut scene,
            WindowBackgroundAppearance::Transparent,
            rounded_clip_fixture(scale),
            |scene, clip| scene.quads[0].fade.clip = clip,
        )?;
        if scale == 1. {
            assert_eq!(actual[9 * 64 + 9][3], 0);
            assert!(actual[9 * 64 + 54][3] > 40);
            assert_eq!(actual[54 * 64 + 54][3], 0);
            assert!((10..80).contains(&actual[54 * 64 + 9][3]));
        }
    }
    let mut scene = Scene::default();
    let mut item = quad(0, bounds(0., 0., 64., 64.), hsla(0., 0., 1., 0.7));
    item.content_mask.bounds = bounds(16., 0., 32., 64.);
    item.fade.clip = rounded_clip_fixture(1.);
    scene.quads.push(item);
    renderer.render(&scene, WindowBackgroundAppearance::Transparent)?;
    let masked = pixels(&renderer)?;
    assert_eq!(masked[32 * 64 + 15][3], 0);
    assert!(masked[32 * 64 + 16][3] > 150);
    Ok(())
}

#[::core::prelude::v1::test]
fn warp_rounded_clip_applies_to_glyphs_subpixel_and_images() -> Result<()> {
    let (mut renderer, _window) = warp_renderer()?;
    let tile_size = size(DevicePixels(8), DevicePixels(8));
    for kind in 0..3 {
        let (key, bytes) = match kind {
            0 => (
                AtlasKey::Svg(RenderSvgParams {
                    path: "rounded-clip-glyph".into(),
                    size: tile_size,
                }),
                vec![255; 64],
            ),
            1 => (
                AtlasKey::Glyph(RenderGlyphParams {
                    font_id: FontId(100),
                    glyph_id: GlyphId(1),
                    font_size: px(12.),
                    subpixel_variant: point(0, 0),
                    scale_factor: 1.,
                    is_emoji: false,
                    subpixel_rendering: true,
                    dilation: 0,
                }),
                [255, 210, 150, 255].repeat(64),
            ),
            _ => (
                AtlasKey::Image(RenderImageParams {
                    image_id: ImageId(1003),
                    frame_index: 0,
                }),
                [40, 90, 180, 192].repeat(64),
            ),
        };
        let tile = renderer
            .atlas
            .get_or_insert_with(key, &mut || {
                Ok(Some((tile_size, std::borrow::Cow::Borrowed(&bytes))))
            })?
            .unwrap();
        let mut scene = Scene::default();
        let content_mask = ContentMask {
            bounds: bounds(0., 0., 64., 64.),
        };
        let fade = EdgeFadeParams {
            top_y: -20.,
            band_top: 40.,
            ..Default::default()
        };
        let transformation = TransformationMatrix {
            translation: [3., 2.],
            ..Default::default()
        };
        match kind {
            0 => scene.monochrome_sprites.push(MonochromeSprite {
                order: 0,
                pad: 0,
                tile,
                bounds: bounds(-3., -2., 64., 64.),
                content_mask,
                color: hsla(0., 0., 0., 0.6),
                transformation,
                fade,
            }),
            1 => scene.subpixel_sprites.push(SubpixelSprite {
                order: 0,
                pad: 0,
                tile,
                bounds: bounds(-3., -2., 64., 64.),
                content_mask,
                color: hsla(0., 0., 0., 0.6),
                transformation,
                fade,
            }),
            _ => scene.polychrome_sprites.push(PolychromeSprite {
                order: 0,
                pad: 0,
                tile,
                grayscale: false.into(),
                opacity: 0.6,
                bounds: bounds(0., 0., 64., 64.),
                content_mask,
                corner_radii: Corners::default(),
                fade,
            }),
        }
        let appearance = if kind == 1 {
            WindowBackgroundAppearance::Opaque
        } else {
            WindowBackgroundAppearance::Transparent
        };
        assert_rounded_clip(
            &mut renderer,
            &mut scene,
            appearance,
            rounded_clip_fixture(1.),
            |scene, clip| match kind {
                0 => scene.monochrome_sprites[0].fade.clip = clip,
                1 => scene.subpixel_sprites[0].fade.clip = clip,
                _ => scene.polychrome_sprites[0].fade.clip = clip,
            },
        )?;
    }
    Ok(())
}

fn test_path(color: Hsla, order: u32) -> Path<ScaledPixels> {
    let mut path = Path::new(point(px(0.), px(0.)));
    path.line_to(point(px(64.), px(0.)));
    path.line_to(point(px(64.), px(64.)));
    path.line_to(point(px(0.), px(64.)));
    let mut path = path.scale(1.);
    path.order = order;
    path.color = color.into();
    path.content_mask.bounds = bounds(0., 0., 64., 64.);
    path
}

#[::core::prelude::v1::test]
fn warp_rounded_clip_applies_to_shadows_underlines_and_paths() -> Result<()> {
    let (mut renderer, _window) = warp_renderer()?;
    let viewport = bounds(0., 0., 64., 64.);
    for inset in [false, true] {
        let mut scene = Scene::default();
        scene.shadows.push(Shadow {
            order: 0,
            blur_radius: ScaledPixels(3.),
            bounds: if inset {
                bounds(20., 20., 24., 24.)
            } else {
                bounds(-16., -16., 96., 96.)
            },
            corner_radii: Corners::default(),
            content_mask: ContentMask { bounds: viewport },
            color: hsla(0., 0., 1., 0.65),
            element_bounds: viewport,
            element_corner_radii: Corners::default(),
            inset: u32::from(inset),
            pad: 0,
            clip: RoundedClipParams::default(),
        });
        assert_rounded_clip(
            &mut renderer,
            &mut scene,
            WindowBackgroundAppearance::Transparent,
            rounded_clip_fixture(1.),
            |scene, clip| scene.shadows[0].clip = clip,
        )?;
    }

    for wavy in [false, true] {
        let mut scene = Scene::default();
        scene.underlines.push(Underline {
            order: 0,
            pad: 0,
            bounds: viewport,
            content_mask: ContentMask { bounds: viewport },
            color: hsla(0., 0., 1., 0.65),
            thickness: ScaledPixels(12.),
            wavy: wavy.into(),
            clip: RoundedClipParams::default(),
        });
        assert_rounded_clip(
            &mut renderer,
            &mut scene,
            WindowBackgroundAppearance::Transparent,
            rounded_clip_fixture(1.),
            |scene, clip| scene.underlines[0].clip = clip,
        )?;
    }

    for duplicate_triangles in [false, true] {
        let mut scene = Scene::default();
        let mut path = test_path(hsla(0., 0., 1., 0.65), 0);
        if duplicate_triangles {
            path.vertices.extend_from_within(..);
        }
        scene.paths.push(path);
        assert_rounded_clip(
            &mut renderer,
            &mut scene,
            WindowBackgroundAppearance::Transparent,
            rounded_clip_fixture(1.),
            |scene, clip| scene.paths[0].clip = clip,
        )?;
    }
    Ok(())
}

#[::core::prelude::v1::test]
fn warp_fractional_rounded_path_clip_keeps_conservative_mask_coverage() -> Result<()> {
    let (mut renderer, _window) = warp_renderer()?;
    // Window::with_rounded_clip snaps the rectangular content mask outwards,
    // but leaves the rounded scope at its original device-pixel coordinates.
    let clip = RoundedClipParams {
        left: 8.75,
        top: 6.75,
        right: 38.75,
        bottom: 36.75,
        top_left: 8.,
        top_right: 8.,
        bottom_right: 8.,
        bottom_left: 8.,
    };
    assert_eq!(rounded_clip_coverage(clip, 8, 16), 0.25);
    assert_eq!(rounded_clip_coverage(clip, 24, 6), 0.25);
    let corner = rounded_clip_coverage(clip, 10, 9);
    assert!(corner > 0.1 && corner < 0.9);

    for duplicate_triangles in [false, true] {
        let mut path = test_path(hsla(0., 0., 1., 0.6), 0);
        path.content_mask.bounds = bounds(8., 6., 31., 31.);
        if duplicate_triangles {
            path.vertices.extend_from_within(..);
        }
        let mut scene = Scene::default();
        scene.paths.push(path);
        renderer.render(&scene, WindowBackgroundAppearance::Transparent)?;
        let reference = pixels(&renderer)?;
        scene.paths[0].clip = clip;
        renderer.render(&scene, WindowBackgroundAppearance::Transparent)?;
        let actual = pixels(&renderer)?;
        for (x, y) in [(8, 16), (24, 6), (10, 9), (8, 8), (38, 20), (24, 36), (24, 20)] {
            let index = y * 64 + x;
            let coverage = rounded_clip_coverage(clip, x, y);
            assert!(
                reference[index][3] > 150,
                "conservative mask must retain the whole MSAA pixel at ({x},{y})"
            );
            for channel in 0..4 {
                let expected = reference[index][channel] as f32 * coverage;
                assert!(
                    (actual[index][channel] as f32 - expected).abs() <= 2.,
                    "duplicate={duplicate_triangles}, ({x},{y}), coverage {coverage}: {:?} vs {:?}",
                    actual[index],
                    reference[index]
                );
            }
        }
    }
    Ok(())
}

#[::core::prelude::v1::test]
fn warp_rounded_clips_apply_before_overlapping_paths_are_composited() -> Result<()> {
    let (mut renderer, _window) = warp_renderer()?;
    let mut first = test_path(hsla(2. / 3., 1., 0.5, 0.65), 0);
    first.clip = rounded_clip_fixture(1.);
    let middle = test_path(hsla(1. / 3., 1., 0.5, 0.4), 1);
    let mut last = test_path(hsla(0., 1., 0.5, 0.65), 2);
    last.clip = RoundedClipParams {
        left: 16.25,
        top: 4.25,
        right: 60.75,
        bottom: 51.75,
        top_left: 3.,
        top_right: 14.,
        bottom_right: 0.,
        bottom_left: 18.,
    };
    let mut scene = Scene::default();
    scene.paths.push(first.clone());
    renderer.render(&scene, WindowBackgroundAppearance::Transparent)?;
    let behind = pixels(&renderer)?;
    scene.paths[0] = middle.clone();
    renderer.render(&scene, WindowBackgroundAppearance::Transparent)?;
    let between = pixels(&renderer)?;
    scene.paths[0] = last.clone();
    renderer.render(&scene, WindowBackgroundAppearance::Transparent)?;
    let front = pixels(&renderer)?;
    scene.paths = vec![first, middle, last];
    renderer.render(&scene, WindowBackgroundAppearance::Transparent)?;
    let combined = pixels(&renderer)?;
    for y in 6..58 {
        for x in 6..58 {
            let index = y * 64 + x;
            let remaining = 1. - front[index][3] as f32 / 255.;
            let middle_remaining = 1. - between[index][3] as f32 / 255.;
            for channel in 0..4 {
                let expected = front[index][channel] as f32
                    + (between[index][channel] as f32
                        + behind[index][channel] as f32 * middle_remaining)
                        * remaining;
                assert!(
                    (combined[index][channel] as f32 - expected).abs() <= 4.,
                    "path order at ({x},{y}): {:?} over {:?} over {:?} yielded {:?}",
                    front[index],
                    between[index],
                    behind[index],
                    combined[index]
                );
            }
        }
    }
    assert!(combined[32 * 64 + 32][2] > combined[32 * 64 + 32][0]);
    Ok(())
}

#[::core::prelude::v1::test]
fn warp_rounded_backdrop_mixes_snapshot_and_keeps_later_content() -> Result<()> {
    let (mut renderer, _window) = warp_renderer()?;
    let mut scene = Scene::default();
    scene
        .quads
        .push(quad(0, bounds(0., 0., 32., 64.), hsla(0., 1., 0.5, 0.35)));
    scene.quads.push(quad(
        0,
        bounds(32., 0., 32., 64.),
        hsla(2. / 3., 1., 0.5, 0.8),
    ));
    renderer.render(&scene, WindowBackgroundAppearance::Transparent)?;
    let snapshot = pixels(&renderer)?;
    let mut region = blur(1);
    region.bounds = bounds(0., 0., 64., 64.);
    region.corner_radii = Corners::default();
    scene.backdrop_blurs.push(region);
    renderer.render(&scene, WindowBackgroundAppearance::Transparent)?;
    let blurred = pixels(&renderer)?;
    let clip = rounded_clip_fixture(1.);
    scene.backdrop_blurs[0].clip = clip;
    renderer.render(&scene, WindowBackgroundAppearance::Transparent)?;
    let clipped = pixels(&renderer)?;
    let mut partial_effect = false;
    for y in 0..64 {
        for x in 0..64 {
            let index = y * 64 + x;
            let coverage = rounded_clip_coverage(clip, x, y);
            for channel in 0..4 {
                let before = snapshot[index][channel] as f32;
                let after = blurred[index][channel] as f32;
                let expected = before + (after - before) * coverage;
                if coverage > 0.1 && coverage < 0.9 && (before - after).abs() > 16. {
                    partial_effect = true;
                }
                assert!(
                    (clipped[index][channel] as f32 - expected).abs() <= 2.,
                    "backdrop at ({x},{y}) channel {channel}: {:?}, {:?}, {:?}",
                    snapshot[index],
                    blurred[index],
                    clipped[index]
                );
            }
        }
    }
    assert!(
        partial_effect,
        "fixture must exercise partial blur coverage"
    );
    scene.quads.push(quad(
        1,
        bounds(28., 28., 8., 8.),
        hsla(1. / 3., 1., 0.5, 1.),
    ));
    renderer.render(&scene, WindowBackgroundAppearance::Transparent)?;
    assert_eq!(pixels(&renderer)?[32 * 64 + 32], [0, 255, 0, 255]);
    Ok(())
}

#[::core::prelude::v1::test]
fn path_sprite_clip_layout_matches_hlsl() {
    assert_eq!(std::mem::offset_of!(PathRasterizationSprite, bounds), 88);
    assert_eq!(std::mem::size_of::<PathRasterizationSprite>(), 104);
    assert_eq!(std::mem::offset_of!(PathSprite, clip), 16);
    assert_eq!(std::mem::size_of::<PathSprite>(), 48);
}
