use super::*;
use gpui::{
    ContentMask, Corners, EdgeFadeParams, Edges, Hsla, PlatformAtlas, PlatformHeadlessRenderer,
    Quad,
};

fn bounds(x: f32, y: f32, width: f32, height: f32) -> Bounds<ScaledPixels> {
    Bounds {
        origin: Point {
            x: x.into(),
            y: y.into(),
        },
        size: Size {
            width: width.into(),
            height: height.into(),
        },
    }
}

fn device_size(width: i32, height: i32) -> Size<DevicePixels> {
    Size {
        width: DevicePixels(width),
        height: DevicePixels(height),
    }
}

fn quad(order: u32, rect: Bounds<ScaledPixels>, color: Hsla) -> Quad {
    Quad {
        order,
        bounds: rect,
        content_mask: ContentMask { bounds: rect },
        background: color.into(),
        ..Default::default()
    }
}

fn blur(order: u32, rect: Bounds<ScaledPixels>, sigma: f32) -> BackdropBlur {
    BackdropBlur {
        order,
        blur_radius: sigma.into(),
        bounds: rect,
        content_mask: ContentMask { bounds: rect },
        corner_radii: Corners::default(),
        clip: Default::default(),
    }
}

fn edge_scene() -> Scene {
    let mut scene = Scene::default();
    scene
        .quads
        .push(quad(0, bounds(0.0, 0.0, 32.0, 64.0), gpui::white()));
    scene
}

fn striped_scene(width: u32, height: u32, horizontal: bool, color: Hsla) -> Scene {
    let mut scene = Scene::default();
    let extent = if horizontal { height } else { width };
    for offset in (0..extent).step_by(3) {
        let rect = if horizontal {
            bounds(0.0, offset as f32, width as f32, 1.0)
        } else {
            bounds(offset as f32, 0.0, 1.0, height as f32)
        };
        scene.quads.push(quad(0, rect, color));
    }
    scene
        .backdrop_blurs
        .push(blur(1, bounds(0.0, 0.0, width as f32, height as f32), 24.0));
    scene.finish();
    scene
}

#[test]
fn backdrop_pixels_clip_corners_mask_and_preserve_foreground() -> anyhow::Result<()> {
    let mut renderer = WgpuHeadlessRenderer::new()?;
    let mut scene = edge_scene();
    let mut region = blur(1, bounds(24.0, 8.0, 32.0, 48.0), 8.0);
    region.corner_radii = Corners::all(12.0.into());
    scene.backdrop_blurs.push(region);
    // Equal-order foreground must be drawn after the blur.
    scene
        .quads
        .push(quad(1, bounds(30.0, 29.0, 4.0, 6.0), gpui::blue()));
    scene.finish();
    let image = renderer.render_scene_to_image(&scene, device_size(64, 64))?;
    assert_eq!(image.get_pixel(24, 8).0, [255, 255, 255, 255]);
    let inside = image.get_pixel(24, 32).0;
    assert!(
        inside[0] < 250 && inside[0] > 100,
        "inside rounded region: {inside:?}"
    );
    let edge = image.get_pixel(31, 20).0;
    assert!(edge[0] > 30 && edge[0] < 230, "blurred edge: {edge:?}");
    assert_eq!(image.get_pixel(31, 32).0, [0, 0, 255, 255]);

    scene.backdrop_blurs[0].content_mask.bounds = bounds(28.0, 8.0, 28.0, 48.0);
    let clipped = renderer.render_scene_to_image(&scene, device_size(64, 64))?;
    assert_eq!(clipped.get_pixel(26, 32).0, [255, 255, 255, 255]);
    let edge = clipped.get_pixel(31, 20).0;
    assert!(edge[0] > 30 && edge[0] < 230, "masked edge: {edge:?}");
    Ok(())
}

#[test]
fn scene_order_samples_stripe_outside_blur_layer_before_foreground() -> anyhow::Result<()> {
    let mut renderer = WgpuHeadlessRenderer::new()?;
    let mut scene = Scene::default();
    scene.insert_primitive(quad(0, bounds(0.0, 0.0, 160.0, 80.0), gpui::black()));
    scene.insert_primitive(quad(0, bounds(88.0, 0.0, 10.0, 80.0), gpui::white()));
    let panel = bounds(100.0, 0.0, 20.0, 80.0);
    scene.push_layer(panel);
    scene.insert_backdrop_blur(blur(0, panel, 8.0));
    scene.insert_primitive(quad(0, bounds(108.0, 30.0, 4.0, 10.0), gpui::blue()));
    scene.pop_layer();
    scene.finish();

    let image = renderer.render_scene_to_image(&scene, device_size(160, 80))?;
    let sampled = image.get_pixel(101, 20).0;
    assert!(
        sampled[0] > 25 && sampled[0] < 160,
        "stripe missing from snapshot: {sampled:?}"
    );
    assert!(sampled[0].abs_diff(sampled[1]) <= 2 && sampled[0].abs_diff(sampled[2]) <= 2);
    assert_eq!(image.get_pixel(109, 35).0, [0, 0, 255, 255]);
    let beside = image.get_pixel(107, 35).0;
    assert!(
        beside[0].abs_diff(beside[2]) <= 2,
        "foreground bled outside its bounds: {beside:?}"
    );
    Ok(())
}

#[test]
fn nested_blur_snapshots_previous_blur_and_intervening_content() -> anyhow::Result<()> {
    let mut renderer = WgpuHeadlessRenderer::new()?;
    let mut scene = edge_scene();
    let parent = blur(1, bounds(8.0, 8.0, 48.0, 48.0), 4.0);
    scene.backdrop_blurs.push(parent);
    // This overlay is between the parent and child, so it must enter the child snapshot.
    scene
        .quads
        .push(quad(2, bounds(0.0, 0.0, 32.0, 64.0), gpui::white()));
    scene.finish();
    let before_child = renderer.render_scene_to_image(&scene, device_size(64, 64))?;
    assert_eq!(before_child.get_pixel(31, 32).0, [255, 255, 255, 255]);
    scene
        .backdrop_blurs
        .push(blur(3, bounds(16.0, 16.0, 32.0, 32.0), 4.0));
    scene.finish();
    let nested = renderer.render_scene_to_image(&scene, device_size(64, 64))?;
    let sample = nested.get_pixel(31, 32).0;
    assert!(
        sample[0] > 50 && sample[0] < 230,
        "child must blur the overlay: {sample:?}"
    );
    assert_eq!(nested.get_pixel(4, 32).0, before_child.get_pixel(4, 32).0);
    Ok(())
}

#[test]
fn alternating_factor_blurs_composite_their_own_snapshots() -> anyhow::Result<()> {
    let mut renderer = WgpuHeadlessRenderer::new()?;
    let mut scene = edge_scene();
    let rect = bounds(0.0, 0.0, 64.0, 64.0);
    scene.backdrop_blurs.push(blur(1, rect, 4.0));
    scene.finish();
    let parent = renderer.render_scene_to_image(&scene, device_size(64, 64))?;
    let parent_pixel = parent.get_pixel(22, 32).0[0];
    assert!(parent_pixel > 240, "parent: {parent_pixel}");

    scene.backdrop_blurs.push(blur(2, rect, 16.0));
    scene.backdrop_blurs.push(blur(3, rect, 4.0));
    scene.finish();
    let nested = renderer.render_scene_to_image(&scene, device_size(64, 64))?;
    let sample = nested.get_pixel(22, 32).0[0];
    assert!(
        sample > 130 && sample + 30 < parent_pixel,
        "nested: {sample}"
    );
    assert_eq!(
        renderer.render_scene_to_image(&scene, device_size(64, 64))?,
        nested
    );
    Ok(())
}

#[test]
fn child_blur_samples_a_path_drawn_after_parent() -> anyhow::Result<()> {
    let mut renderer = WgpuHeadlessRenderer::new()?;
    let mut scene = edge_scene();
    scene
        .backdrop_blurs
        .push(blur(1, bounds(8.0, 8.0, 48.0, 48.0), 4.0));
    let mut path = gpui::Path::new(gpui::point(gpui::px(24.0), gpui::px(16.0)));
    path.line_to(gpui::point(gpui::px(56.0), gpui::px(16.0)));
    path.line_to(gpui::point(gpui::px(56.0), gpui::px(48.0)));
    let mut path = path.scale(1.0);
    path.order = 2;
    path.color = gpui::blue().into();
    path.content_mask.bounds = bounds(0.0, 0.0, 64.0, 64.0);
    scene.paths.push(path);
    scene.finish();
    let before_child = renderer.render_scene_to_image(&scene, device_size(64, 64))?;
    assert!(before_child.get_pixel(50, 20).0[2] > 200);
    assert_eq!(before_child.get_pixel(57, 20).0, [0, 0, 0, 255]);
    scene
        .backdrop_blurs
        .push(blur(3, bounds(16.0, 8.0, 48.0, 48.0), 3.0));
    scene.finish();
    let nested = renderer.render_scene_to_image(&scene, device_size(64, 64))?;
    let outside_path = nested.get_pixel(57, 20).0;
    assert!(
        outside_path[2] > 10,
        "child snapshot omitted path: {outside_path:?}"
    );
    Ok(())
}

#[test]
fn transparent_backdrop_replaces_premultiplied_pixels() -> anyhow::Result<()> {
    let mut renderer = WgpuHeadlessRenderer::new()?;
    let core = &mut renderer.core;
    core.resources.pipelines = WgpuRendererCore::create_pipelines(
        &core.resources.device,
        &core.resources.bind_group_layouts,
        core.target_format,
        wgpu::CompositeAlphaMode::PreMultiplied,
        core.rendering_params.path_sample_count,
        core.dual_source_blending,
        core.uses_webgl_instance_data,
    );
    let mut color = gpui::red();
    color.a = 0.5;
    let mut scene = Scene::default();
    scene
        .quads
        .push(quad(0, bounds(0.0, 0.0, 32.0, 64.0), color));
    scene
        .backdrop_blurs
        .push(blur(1, bounds(8.0, 8.0, 48.0, 48.0), 4.0));
    scene.finish();
    let size = device_size(64, 64);
    renderer.ensure_render_target(size)?;
    let view = renderer.render_target.as_ref().unwrap().view.clone();
    renderer
        .core
        .render_frame(&scene, &view, size, true, wgpu::Color::TRANSPARENT)?;
    let image = renderer.read_image()?;
    let sample = image.get_pixel(31, 32).0;
    assert!(
        sample[3] > 20 && sample[3] < 115,
        "blur must replace alpha: {sample:?}"
    );
    assert!(
        sample[0].abs_diff(sample[3]) <= 3,
        "red must remain premultiplied: {sample:?}"
    );
    assert_eq!(image.get_pixel(60, 32).0, [0, 0, 0, 0]);
    Ok(())
}

#[test]
fn large_blur_filters_native_horizontal_and_vertical_stripes() -> anyhow::Result<()> {
    let mut renderer = WgpuHeadlessRenderer::new()?;
    for (width, height) in [(192, 192), (193, 195)] {
        for horizontal in [true, false] {
            let scene = striped_scene(width, height, horizontal, gpui::white());
            let image =
                renderer.render_scene_to_image(&scene, device_size(width as i32, height as i32))?;
            for (x, y) in [(80, 95), (95, 96), (110, 110)] {
                let pixel = image.get_pixel(x, y).0;
                assert!(
                    (75..=95).contains(&pixel[0]),
                    "{width}x{height} horizontal={horizontal}: {pixel:?}"
                );
                assert!(pixel[0].abs_diff(pixel[1]) <= 2 && pixel[0].abs_diff(pixel[2]) <= 2);
                assert_eq!(pixel[3], 255);
            }
        }
    }
    Ok(())
}

#[test]
fn downsampled_blur_preserves_premultiplied_stripe_alpha() -> anyhow::Result<()> {
    let mut renderer = WgpuHeadlessRenderer::new()?;
    let core = &mut renderer.core;
    core.resources.pipelines = WgpuRendererCore::create_pipelines(
        &core.resources.device,
        &core.resources.bind_group_layouts,
        core.target_format,
        wgpu::CompositeAlphaMode::PreMultiplied,
        core.rendering_params.path_sample_count,
        core.dual_source_blending,
        core.uses_webgl_instance_data,
    );
    let mut color = gpui::red();
    color.a = 0.5;
    let size = device_size(193, 195);
    for horizontal in [true, false] {
        let scene = striped_scene(193, 195, horizontal, color);
        renderer.ensure_render_target(size)?;
        let view = renderer.render_target.as_ref().unwrap().view.clone();
        renderer
            .core
            .render_frame(&scene, &view, size, true, wgpu::Color::TRANSPARENT)?;
        let image = renderer.read_image()?;
        for (x, y) in [(80, 95), (95, 96), (110, 110)] {
            let pixel = image.get_pixel(x, y).0;
            assert!(
                (35..=51).contains(&pixel[3]),
                "horizontal={horizontal}: {pixel:?}"
            );
            assert!(pixel[0].abs_diff(pixel[3]) <= 3, "premultiplied: {pixel:?}");
            assert!(pixel[1] <= 2 && pixel[2] <= 2);
        }
    }
    Ok(())
}

#[test]
fn native_filter_clamps_edges_and_fills_scissored_halos() -> anyhow::Result<()> {
    let mut renderer = WgpuHeadlessRenderer::new()?;
    let mut scene = Scene::default();
    scene
        .quads
        .push(quad(0, bounds(0.0, 0.0, 193.0, 195.0), gpui::white()));
    scene
        .backdrop_blurs
        .push(blur(1, bounds(0.0, 0.0, 193.0, 195.0), 24.0));
    scene.finish();
    for sigma in [24.0, 64.0] {
        scene.backdrop_blurs[0].blur_radius = sigma.into();
        let image = renderer.render_scene_to_image(&scene, device_size(193, 195))?;
        for (x, y) in [(0, 0), (192, 0), (0, 194), (192, 194), (96, 97)] {
            let pixel = image.get_pixel(x, y).0;
            assert!(
                pixel.iter().all(|channel| *channel >= 250),
                "sigma={sigma}, ({x}, {y}): {pixel:?}"
            );
        }
    }

    scene.quads[0] = quad(0, bounds(0.0, 0.0, 320.0, 320.0), gpui::white());
    scene.backdrop_blurs[0] = blur(1, bounds(120.0, 120.0, 60.0, 60.0), 24.0);
    let image = renderer.render_scene_to_image(&scene, device_size(320, 320))?;
    for (x, y) in [(120, 120), (179, 120), (120, 179), (179, 179), (150, 150)] {
        let pixel = image.get_pixel(x, y).0;
        assert!(
            pixel.iter().all(|channel| *channel >= 250),
            "scissor ({x}, {y}): {pixel:?}"
        );
    }
    Ok(())
}

#[test]
fn large_blur_downsamples_odd_targets_and_clamps_sigma() -> anyhow::Result<()> {
    let mut renderer = WgpuHeadlessRenderer::new()?;
    let mut scene = edge_scene();
    scene
        .backdrop_blurs
        .push(blur(1, bounds(0.0, 0.0, 65.0, 67.0), 24.0));
    scene.finish();
    let image = renderer.render_scene_to_image(&scene, device_size(65, 67))?;
    assert!(image.get_pixel(0, 32).0[0] > 180);
    assert!(image.get_pixel(64, 32).0[0] < 75);
    let edge = image.get_pixel(31, 32).0[0];
    assert!(edge > 70 && edge < 200, "downsampled edge: {edge}");
    assert!(renderer.core.resources.backdrop_scratch[2].is_some());

    scene.backdrop_blurs[0].blur_radius = 64.0.into();
    let max_radius = renderer.render_scene_to_image(&scene, device_size(65, 67))?;
    scene.backdrop_blurs[0].blur_radius = 10_000.0.into();
    let clamped = renderer.render_scene_to_image(&scene, device_size(65, 67))?;
    assert_eq!(clamped, max_radius);

    scene.backdrop_blurs[0].blur_radius = 1.0.into();
    let min_radius = renderer.render_scene_to_image(&scene, device_size(65, 67))?;
    scene.backdrop_blurs[0].blur_radius = 0.01.into();
    let clamped = renderer.render_scene_to_image(&scene, device_size(65, 67))?;
    assert_eq!(clamped, min_radius);
    Ok(())
}

#[test]
fn trailing_blurs_have_unlimited_slots_and_empty_scene_is_processed() -> anyhow::Result<()> {
    let mut renderer = WgpuHeadlessRenderer::new()?;
    let mut scene = edge_scene();
    for order in 1..=32 {
        scene
            .backdrop_blurs
            .push(blur(order, bounds(-50.0, -50.0, 8.0, 8.0), 4.0));
    }
    scene
        .backdrop_blurs
        .push(blur(33, bounds(8.0, 8.0, 48.0, 48.0), 4.0));
    scene.finish();
    let image = renderer.render_scene_to_image(&scene, device_size(64, 64))?;
    let edge = image.get_pixel(31, 32).0;
    assert!(
        edge[0] > 30 && edge[0] < 230,
        "33rd trailing blur was skipped: {edge:?}"
    );
    assert!(
        renderer
            .core
            .resources
            .backdrop_params
            .as_ref()
            .unwrap()
            .size()
            >= 33 * 3 * renderer.core.backdrop_slot_stride
    );

    let mut empty = Scene::default();
    empty
        .backdrop_blurs
        .push(blur(0, bounds(0.0, 0.0, 32.0, 32.0), 4.0));
    renderer.core.resources.backdrop_scratch = std::array::from_fn(|_| None);
    let image = renderer.render_scene_to_image(&empty, device_size(64, 64))?;
    assert!(
        renderer.core.resources.backdrop_scratch[0].is_some(),
        "blur without batches was skipped"
    );
    assert!(image.pixels().all(|pixel| pixel.0 == [0, 0, 0, 255]));
    Ok(())
}

#[test]
fn backdrop_resources_reuse_alternating_factors_resize_and_expire() -> anyhow::Result<()> {
    let mut renderer = WgpuHeadlessRenderer::new()?;
    let mut scene = Scene::default();
    for (order, sigma) in [4.0, 16.0, 4.0, 24.0, 32.0, 16.0, 24.0]
        .into_iter()
        .enumerate()
    {
        scene
            .backdrop_blurs
            .push(blur(order as u32, bounds(0.0, 0.0, 16.0, 16.0), sigma));
    }
    scene.finish();
    let textures = |renderer: &WgpuHeadlessRenderer| -> [[wgpu::Texture; 3]; 4] {
        std::array::from_fn(|index| {
            let scratch = renderer.core.resources.backdrop_scratch[index]
                .as_ref()
                .unwrap();
            [
                scratch.snapshot.clone(),
                scratch.blur_a_view.texture().clone(),
                scratch.blur_b_view.texture().clone(),
            ]
        })
    };
    renderer.render_scene(&scene, device_size(193, 195))?;
    let first = textures(&renderer);
    let first_frame = renderer
        .core
        .resources
        .backdrop_frame
        .as_ref()
        .unwrap()
        .texture
        .clone();
    let scratch = renderer.core.resources.backdrop_scratch[2]
        .as_ref()
        .unwrap();
    assert_eq!(
        (
            scratch.blur_a_view.texture().width(),
            scratch.blur_a_view.texture().height()
        ),
        (65, 195)
    );
    assert_eq!(
        (
            scratch.blur_b_view.texture().width(),
            scratch.blur_b_view.texture().height()
        ),
        (65, 65)
    );
    for _ in 0..3 {
        renderer.render_scene(&scene, device_size(193, 195))?;
        assert_eq!(textures(&renderer), first);
        assert_eq!(
            renderer
                .core
                .resources
                .backdrop_frame
                .as_ref()
                .unwrap()
                .texture,
            first_frame
        );
    }

    renderer.render_scene(&Scene::default(), device_size(17, 19))?;
    assert!(
        renderer
            .core
            .resources
            .backdrop_scratch
            .iter()
            .all(Option::is_none)
    );
    assert!(renderer.core.resources.backdrop_frame.is_none());
    renderer.render_scene(&scene, device_size(17, 19))?;
    let resized = textures(&renderer);
    for (old, new) in first.iter().zip(&resized) {
        for (old, new) in old.iter().zip(new) {
            assert_ne!(old, new);
        }
    }
    let frame = renderer.core.resources.backdrop_frame.as_ref().unwrap();
    assert_ne!(frame.texture, first_frame);
    assert_eq!((frame.texture.width(), frame.texture.height()), (17, 19));
    for scratch in renderer.core.resources.backdrop_scratch.iter().flatten() {
        assert!(scratch.width <= 17 && scratch.height <= 19);
    }

    renderer.core.resources.invalidate_intermediate_textures();
    assert!(
        renderer
            .core
            .resources
            .backdrop_scratch
            .iter()
            .all(Option::is_none)
    );
    assert!(renderer.core.resources.backdrop_frame.is_none());
    renderer.render_scene(&scene, device_size(17, 19))?;
    for (old, new) in resized.iter().zip(textures(&renderer)) {
        for (old, new) in old.iter().zip(new) {
            assert_ne!(old, &new);
        }
    }
    for _ in 1..SCRATCH_RELEASE_AFTER_FRAMES {
        renderer.render_scene(&Scene::default(), device_size(17, 19))?;
    }
    assert!(
        renderer
            .core
            .resources
            .backdrop_scratch
            .iter()
            .all(Option::is_some)
    );
    renderer.render_scene(&Scene::default(), device_size(17, 19))?;
    assert!(renderer.core.resources.backdrop_frame.is_none());
    assert!(
        renderer
            .core
            .resources
            .backdrop_scratch
            .iter()
            .all(Option::is_none)
    );
    assert!(renderer.core.resources.backdrop_params.is_none());
    Ok(())
}

#[test]
fn scratch_growth_does_not_shrink_the_other_axis() -> anyhow::Result<()> {
    let mut renderer = WgpuHeadlessRenderer::new()?;
    let mut scene = Scene::default();
    scene
        .backdrop_blurs
        .push(blur(0, bounds(20.0, 20.0, 300.0, 10.0), 4.0));
    scene
        .backdrop_blurs
        .push(blur(1, bounds(20.0, 20.0, 10.0, 300.0), 4.0));
    scene.finish();
    renderer.render_scene(&scene, device_size(530, 530))?;
    let scratch = renderer.core.resources.backdrop_scratch[0]
        .as_ref()
        .unwrap();
    assert_eq!((scratch.width, scratch.height), (512, 512));
    let snapshot = scratch.snapshot.clone();
    for _ in 0..2 {
        renderer.render_scene(&scene, device_size(530, 530))?;
        assert_eq!(
            renderer.core.resources.backdrop_scratch[0]
                .as_ref()
                .unwrap()
                .snapshot,
            snapshot
        );
    }
    Ok(())
}

#[test]
fn invalid_direct_blur_entries_leave_pixels_untouched() -> anyhow::Result<()> {
    let mut renderer = WgpuHeadlessRenderer::new()?;
    let mut scene = edge_scene();
    let valid = blur(1, bounds(8.0, 8.0, 48.0, 48.0), 4.0);
    for value in [f32::NAN, f32::INFINITY, 0.0, -1.0] {
        let mut invalid = valid;
        invalid.blur_radius = value.into();
        scene.backdrop_blurs.push(invalid);
    }
    let mut negative_corner = valid;
    negative_corner.corner_radii.top_left = (-1.0).into();
    scene.backdrop_blurs.push(negative_corner);
    let mut invalid_bounds = valid;
    invalid_bounds.bounds.origin.x = f32::NAN.into();
    scene.backdrop_blurs.push(invalid_bounds);
    let mut invalid_mask = valid;
    invalid_mask.content_mask.bounds.size.width = (-1.0).into();
    scene.backdrop_blurs.push(invalid_mask);
    scene.finish();
    let image = renderer.render_scene_to_image(&scene, device_size(64, 64))?;
    assert_eq!(image.get_pixel(31, 32).0, [255, 255, 255, 255]);
    assert_eq!(image.get_pixel(32, 32).0, [0, 0, 0, 255]);
    assert!(
        renderer
            .core
            .resources
            .backdrop_scratch
            .iter()
            .all(Option::is_none)
    );
    Ok(())
}

#[test]
fn fade_quad_pixels_cover_zero_partial_and_full() -> anyhow::Result<()> {
    let mut renderer = WgpuHeadlessRenderer::new()?;
    let mut background = gpui::white();
    background.a = 0.5;
    let mut border = gpui::red();
    border.a = 0.5;
    let mut faded = quad(0, bounds(4.0, 4.0, 56.0, 56.0), background);
    faded.border_color = border;
    faded.border_widths = Edges::all(4.0.into());
    faded.fade = EdgeFadeParams {
        left_x: 12.0,
        band_left: 16.0,
        ..Default::default()
    };
    let mut scene = Scene::default();
    scene.quads.push(faded);
    scene.finish();
    let image = renderer.render_scene_to_image(&scene, device_size(64, 64))?;
    for y in [5, 32] {
        let zero = image.get_pixel(10, y).0;
        let partial = image.get_pixel(20, y).0;
        let full = image.get_pixel(40, y).0;
        assert_eq!(zero, [0, 0, 0, 255], "fade zero at y={y}");
        assert!(
            partial[0] > 3 && partial[0] + 5 < full[0],
            "partial={partial:?}, full={full:?}, y={y}"
        );
        assert!(full[0] > 100, "full fade at y={y}: {full:?}");
    }
    Ok(())
}

fn asymmetric_clip() -> RoundedClipParams {
    RoundedClipParams {
        left: 8.0,
        top: 8.0,
        right: 56.0,
        bottom: 56.0,
        top_left: 16.0,
        top_right: 0.0,
        bottom_right: 12.0,
        bottom_left: 0.0,
    }
}

// Exercise WebGL's texture record transport on the same headless device.
fn use_texture_instances(renderer: &mut WgpuHeadlessRenderer) {
    let core = &mut renderer.core;
    let device = &core.resources.device;
    core.resources.bind_group_layouts.instances =
        WgpuRendererCore::create_bind_group_layouts(device, true).instances;
    core.resources.pipelines = WgpuRendererCore::create_pipelines(
        device,
        &core.resources.bind_group_layouts,
        core.target_format,
        wgpu::CompositeAlphaMode::Opaque,
        core.rendering_params.path_sample_count,
        false,
        true,
    );
    let max_dimension = device.limits().max_texture_dimension_2d;
    core.max_instance_data_size = (u64::from(max_dimension).pow(2) * INSTANCE_TEXTURE_TEXEL_SIZE)
        .min(MAX_INSTANCE_BUFFER_SIZE);
    let (instances, capacity) = WgpuRendererCore::create_instance_texture(
        device,
        (2 * 1024 * 1024).min(core.max_instance_data_size),
        max_dimension,
    );
    core.resources.instance_data = instances;
    core.instance_data_capacity = capacity;
    core.instance_data_alignment = INSTANCE_TEXTURE_TEXEL_SIZE;
    core.uses_webgl_instance_data = true;
    core.dual_source_blending = false;
}

fn enable_transparent_target(renderer: &mut WgpuHeadlessRenderer) {
    let core = &mut renderer.core;
    core.resources.pipelines = WgpuRendererCore::create_pipelines(
        &core.resources.device,
        &core.resources.bind_group_layouts,
        core.target_format,
        wgpu::CompositeAlphaMode::PreMultiplied,
        core.rendering_params.path_sample_count,
        core.dual_source_blending,
        core.uses_webgl_instance_data,
    );
}

fn render_transparent(
    renderer: &mut WgpuHeadlessRenderer,
    scene: &Scene,
) -> anyhow::Result<image::RgbaImage> {
    let size = device_size(64, 64);
    renderer.ensure_render_target(size)?;
    let view = renderer.render_target.as_ref().unwrap().view.clone();
    renderer
        .core
        .render_frame(scene, &view, size, true, wgpu::Color::TRANSPARENT)?;
    renderer.read_image()
}

#[test]
fn rounded_quad_corners_fade_and_premultiplied_alpha() -> anyhow::Result<()> {
    for texture_instances in [false, true] {
        let mut renderer = WgpuHeadlessRenderer::new()?;
        if texture_instances {
            use_texture_instances(&mut renderer);
        }
        enable_transparent_target(&mut renderer);
        let mut color = gpui::red();
        color.a = 0.5;
        let mut clipped = quad(0, bounds(8.0, 8.0, 48.0, 48.0), color);
        clipped.fade = EdgeFadeParams {
            left_x: 4.5,
            band_left: 16.0,
            clip: asymmetric_clip(),
            ..Default::default()
        };
        let mut scene = Scene::default();
        scene.quads.push(clipped);
        scene.finish();
        let image = render_transparent(&mut renderer, &scene)?;
        assert_eq!(image.get_pixel(8, 8).0, [0, 0, 0, 0]);
        assert_eq!(image.get_pixel(55, 55).0, [0, 0, 0, 0]);
        for (x, y) in [(55, 8), (32, 32)] {
            let pixel = image.get_pixel(x, y).0;
            assert!(pixel[3].abs_diff(128) <= 2, "pixel ({x}, {y}): {pixel:?}");
            assert!(pixel[0].abs_diff(pixel[3]) <= 2, "premultiplied: {pixel:?}");
        }
        let edge = image.get_pixel(12, 12).0;
        let distance = (11.5_f32.hypot(11.5) - 16.0).max(-0.5);
        let expected = ((0.5 - distance).clamp(0.0, 1.0) * 0.25 * 0.5 * 255.0).round() as u8;
        assert!(
            edge[3].abs_diff(expected) <= 2,
            "edge={edge:?}, expected alpha={expected}"
        );
        assert!(
            edge[0].abs_diff(edge[3]) <= 2,
            "premultiplied edge: {edge:?}"
        );
        scene.quads[0].content_mask.bounds = bounds(16.0, 8.0, 40.0, 48.0);
        let rectangular_mask = render_transparent(&mut renderer, &scene)?;
        assert_eq!(rectangular_mask.get_pixel(12, 32).0, [0, 0, 0, 0]);
        assert!(rectangular_mask.get_pixel(32, 32).0[3] > 120);
    }
    Ok(())
}

fn sprite_tile(renderer: &WgpuHeadlessRenderer, kind: u8) -> anyhow::Result<gpui::AtlasTile> {
    let key = if kind == 2 {
        gpui::AtlasKey::Image(gpui::RenderImageParams {
            image_id: gpui::ImageId(1),
            frame_index: 0,
        })
    } else {
        gpui::AtlasKey::Glyph(gpui::RenderGlyphParams {
            font_id: gpui::FontId(0),
            glyph_id: gpui::GlyphId(kind as u32),
            font_size: gpui::px(16.0),
            subpixel_variant: gpui::point(0, 0),
            scale_factor: 1.0,
            is_emoji: false,
            subpixel_rendering: kind == 1,
            dilation: 0,
        })
    };
    let channels = if kind == 0 { 1 } else { 4 };
    renderer
        .core
        .atlas
        .get_or_insert_with(key, &mut || {
            Ok(Some((
                device_size(16, 16),
                std::borrow::Cow::Owned(vec![255; 16 * 16 * channels]),
            )))
        })?
        .ok_or_else(|| anyhow::anyhow!("atlas did not create a sprite tile"))
}

#[test]
fn rounded_sprite_clipping_preserves_fade_opacity_and_order() -> anyhow::Result<()> {
    for texture_instances in [false, true] {
        for kind in 0..3 {
            let mut renderer = WgpuHeadlessRenderer::new()?;
            if texture_instances {
                use_texture_instances(&mut renderer);
            }
            let tile = sprite_tile(&renderer, kind)?;
            let mask = ContentMask {
                bounds: bounds(0.0, 0.0, 64.0, 64.0),
            };
            let fade = EdgeFadeParams {
                left_x: 4.5,
                band_left: 16.0,
                clip: asymmetric_clip(),
                ..Default::default()
            };
            let mut white = gpui::white();
            white.a = 0.5;
            let mut transform = gpui::TransformationMatrix::unit();
            transform.translation = [8.0, 8.0];
            let mut scene = Scene::default();
            scene.quads.push(quad(0, mask.bounds, gpui::blue()));
            match kind {
                0 => scene.monochrome_sprites.push(gpui::MonochromeSprite {
                    order: 1,
                    pad: 0,
                    tile,
                    bounds: bounds(0.0, 0.0, 48.0, 48.0),
                    content_mask: mask,
                    color: white,
                    transformation: transform,
                    fade,
                }),
                1 => scene.subpixel_sprites.push(gpui::SubpixelSprite {
                    order: 1,
                    pad: 0,
                    tile,
                    bounds: bounds(0.0, 0.0, 48.0, 48.0),
                    content_mask: mask,
                    color: white,
                    transformation: transform,
                    fade,
                }),
                _ => scene.polychrome_sprites.push(gpui::PolychromeSprite {
                    order: 1,
                    pad: 0,
                    tile,
                    grayscale: false.into(),
                    opacity: 0.5,
                    bounds: bounds(8.0, 8.0, 48.0, 48.0),
                    content_mask: mask,
                    corner_radii: Corners::default(),
                    fade,
                    alpha_mask: Default::default(),
                }),
            }
            scene
                .quads
                .push(quad(2, bounds(30.0, 30.0, 4.0, 4.0), gpui::red()));
            scene.finish();
            let image = renderer.render_scene_to_image(&scene, device_size(64, 64))?;
            assert_eq!(image.get_pixel(8, 8).0, [0, 0, 255, 255], "kind {kind}");
            assert_eq!(image.get_pixel(55, 55).0, [0, 0, 255, 255], "kind {kind}");
            assert_eq!(image.get_pixel(31, 31).0, [255, 0, 0, 255], "kind {kind}");
            let top_right = image.get_pixel(54, 9).0;
            assert!(
                top_right[0] > 110 && top_right[0] < 145,
                "kind {kind}: {top_right:?}"
            );
            let edge = image.get_pixel(12, 12).0;
            assert!((4..=12).contains(&edge[0]), "kind {kind}, edge: {edge:?}");
        }
    }
    Ok(())
}

#[test]
fn rounded_shadow_path_and_underline_clip_once() -> anyhow::Result<()> {
    for texture_instances in [false, true] {
        for kind in 0..3 {
            let mut renderer = WgpuHeadlessRenderer::new()?;
            if texture_instances {
                use_texture_instances(&mut renderer);
            }
            let rect = bounds(8.0, 8.0, 48.0, 48.0);
            let mask = ContentMask {
                bounds: bounds(0.0, 0.0, 64.0, 64.0),
            };
            let mut color = gpui::white();
            color.a = 0.5;
            let mut scene = Scene::default();
            scene.quads.push(quad(0, mask.bounds, gpui::blue()));
            match kind {
                0 => scene.shadows.push(gpui::Shadow {
                    order: 1,
                    blur_radius: 0.0.into(),
                    bounds: rect,
                    corner_radii: Corners::default(),
                    content_mask: mask,
                    color,
                    element_bounds: rect,
                    element_corner_radii: Corners::default(),
                    inset: 0,
                    pad: 0,
                    clip: asymmetric_clip(),
                }),
                1 => {
                    let mut path = gpui::Path::new(gpui::point(gpui::px(8.0), gpui::px(8.0)));
                    path.line_to(gpui::point(gpui::px(56.0), gpui::px(8.0)));
                    path.line_to(gpui::point(gpui::px(56.0), gpui::px(56.0)));
                    path.line_to(gpui::point(gpui::px(8.0), gpui::px(56.0)));
                    let mut path = path.scale(1.0);
                    path.order = 1;
                    path.content_mask = mask;
                    path.color = color.into();
                    path.clip = asymmetric_clip();
                    scene.paths.push(path);
                }
                _ => scene.underlines.push(gpui::Underline {
                    order: 1,
                    pad: 0,
                    bounds: rect,
                    content_mask: mask,
                    color,
                    thickness: 48.0.into(),
                    wavy: false.into(),
                    clip: asymmetric_clip(),
                }),
            }
            scene.finish();
            let image = renderer.render_scene_to_image(&scene, device_size(64, 64))?;
            assert_eq!(image.get_pixel(8, 8).0, [0, 0, 255, 255], "kind {kind}");
            assert_eq!(image.get_pixel(55, 55).0, [0, 0, 255, 255], "kind {kind}");
            for (x, y) in [(55, 8), (32, 24)] {
                let pixel = image.get_pixel(x, y).0;
                assert!(
                    pixel[0] > 110 && pixel[0] < 145,
                    "kind {kind}, {x},{y}: {pixel:?}"
                );
            }
        }
    }
    Ok(())
}

#[test]
fn rounded_backdrop_blends_with_snapshot_and_keeps_later_foreground() -> anyhow::Result<()> {
    for texture_instances in [false, true] {
        let mut renderer = WgpuHeadlessRenderer::new()?;
        if texture_instances {
            use_texture_instances(&mut renderer);
        }
        enable_transparent_target(&mut renderer);
        let mut red = gpui::red();
        red.a = 0.5;
        let mut scene = Scene::default();
        scene.quads.push(quad(0, bounds(0.0, 0.0, 13.0, 64.0), red));
        scene
            .quads
            .push(quad(2, bounds(30.0, 30.0, 4.0, 4.0), gpui::blue()));
        scene.finish();
        let original = render_transparent(&mut renderer, &scene)?;
        scene
            .backdrop_blurs
            .push(blur(1, bounds(8.0, 8.0, 48.0, 48.0), 8.0));
        scene.finish();
        let unmasked = render_transparent(&mut renderer, &scene)?;
        scene.backdrop_blurs[0].clip = asymmetric_clip();
        let clipped = render_transparent(&mut renderer, &scene)?;
        assert_eq!(clipped.get_pixel(8, 8), original.get_pixel(8, 8));
        assert_eq!(clipped.get_pixel(31, 31).0, [0, 0, 255, 255]);
        let before = original.get_pixel(12, 12).0;
        let blurred = unmasked.get_pixel(12, 12).0;
        let after = clipped.get_pixel(12, 12).0;
        let coverage = (0.5 - (11.5_f32.hypot(11.5) - 16.0)).clamp(0.0, 1.0);
        for channel in [0, 3] {
            let expected = (before[channel] as f32 * (1.0 - coverage)
                + blurred[channel] as f32 * coverage)
                .round() as u8;
            assert!(
                after[channel].abs_diff(expected) <= 3,
                "channel {channel}: original={before:?} blurred={blurred:?} clipped={after:?}"
            );
        }
        assert!(
            blurred[3] + 10 < before[3],
            "blur must change snapshot: {blurred:?}"
        );
        assert!(after[0].abs_diff(after[3]) <= 3, "premultiplied: {after:?}");
    }
    Ok(())
}

fn rectangular_path(color: Hsla, clip: RoundedClipParams, order: u32) -> Path<ScaledPixels> {
    let mut path = gpui::Path::new(gpui::point(gpui::px(8.0), gpui::px(8.0)));
    path.line_to(gpui::point(gpui::px(56.0), gpui::px(8.0)));
    path.line_to(gpui::point(gpui::px(56.0), gpui::px(56.0)));
    path.line_to(gpui::point(gpui::px(8.0), gpui::px(56.0)));
    let mut path = path.scale(1.0);
    path.order = order;
    path.content_mask.bounds = bounds(0.0, 0.0, 64.0, 64.0);
    path.color = color.into();
    path.clip = clip;
    path
}

fn triangle_path(
    vertices: [(f32, f32); 3],
    color: Hsla,
    clip: RoundedClipParams,
    order: u32,
) -> Path<ScaledPixels> {
    let position = |(x, y): (f32, f32)| gpui::point(gpui::px(x), gpui::px(y));
    let mut path = gpui::Path::new(position(vertices[0]));
    path.push_triangle(
        (
            position(vertices[0]),
            position(vertices[1]),
            position(vertices[2]),
        ),
        (
            gpui::point(0., 1.),
            gpui::point(0., 1.),
            gpui::point(0., 1.),
        ),
    );
    let mut path = path.scale(1.);
    path.order = order;
    path.color = color.into();
    path.clip = clip;
    path.content_mask.bounds = bounds(0., 0., 64., 64.);
    path
}

#[test]
fn clipped_path_batches_join_shared_edges_but_isolate_order_and_clip_changes() {
    let clip = asymmetric_clip();
    let first = triangle_path([(0., 0.), (64., 0.), (0., 64.)], gpui::white(), clip, 0);
    let second = triangle_path([(64., 64.), (64., 0.), (0., 64.)], gpui::white(), clip, 0);
    let mut paths = vec![first.clone(), second.clone()];
    let (batch, rest) = split_path_batch(&paths);
    assert_eq!(batch.len(), 2);
    assert!(rest.is_empty());
    assert_eq!(path_batch_copy_bounds(batch), Some(first.clipped_bounds()));

    paths[1].order = 1;
    assert_eq!(split_path_batch(&paths).0.len(), 1);
    paths[1].order = 0;
    paths[1].clip.top_left = 0.;
    assert_eq!(split_path_batch(&paths).0.len(), 1);
    paths[0].clip.top_left = f32::NAN;
    assert_eq!(split_path_batch(&paths).0.len(), 1);

    let distant = triangle_path([(48., 48.), (64., 48.), (48., 64.)], gpui::white(), clip, 0);
    let near = triangle_path([(0., 0.), (16., 0.), (0., 16.)], gpui::white(), clip, 0);
    assert_eq!(path_batch_copy_bounds(&[near, distant]), None);
}

#[test]
fn same_clip_complementary_paths_share_msaa_and_copy_alpha_once() -> anyhow::Result<()> {
    let clip = RoundedClipParams {
        left: 0.,
        top: 0.,
        right: 64.,
        bottom: 64.,
        top_left: 8.,
        top_right: 8.,
        bottom_right: 8.,
        bottom_left: 8.,
    };
    for texture_instances in [false, true] {
        let mut renderer = WgpuHeadlessRenderer::new()?;
        if texture_instances {
            use_texture_instances(&mut renderer);
        }
        let formats = renderer
            .context
            .adapter
            .get_texture_format_features(renderer.core.target_format);
        for sample_count in [1, 4] {
            if !formats.flags.sample_count_supported(sample_count) {
                continue;
            }
            renderer.core.rendering_params.path_sample_count = sample_count;
            renderer.core.resources.invalidate_intermediate_textures();
            enable_transparent_target(&mut renderer);
            for opacity in [1., 0.5] {
                let mut white = gpui::white();
                white.a = opacity;
                let mut scene = Scene::default();
                scene.push_layer(bounds(0., 0., 64., 64.));
                for vertices in [
                    [(0., 0.), (64., 0.), (0., 64.)],
                    [(64., 64.), (64., 0.), (0., 64.)],
                ] {
                    scene.insert_primitive(triangle_path(
                        vertices,
                        white,
                        RoundedClipParams::default(),
                        0,
                    ));
                }
                scene.pop_layer();
                scene.finish();
                assert_eq!(scene.paths[0].order, scene.paths[1].order);
                let unmasked = render_transparent(&mut renderer, &scene)?;
                for path in &mut scene.paths {
                    path.clip = clip;
                }
                let masked = render_transparent(&mut renderer, &scene)?;
                let expected = (255. * opacity).round() as u8;
                for (x, y) in [(10, 10), (31, 32), (40, 40)] {
                    for image in [&unmasked, &masked] {
                        let pixel = image.get_pixel(x, y).0;
                        assert!(
                            pixel.iter().all(|channel| channel.abs_diff(expected) <= 2),
                            "texture={texture_instances}, samples={sample_count}, opacity={opacity}, ({x},{y}): {pixel:?}"
                        );
                    }
                }
                let coverage = (0.5 - (5.5_f32.hypot(5.5) - 8.)).clamp(0., 1.);
                let expected_edge = (f32::from(expected) * coverage).round() as u8;
                let edge = masked.get_pixel(2, 2).0;
                assert!(
                    edge.iter()
                        .all(|channel| channel.abs_diff(expected_edge) <= 3),
                    "texture={texture_instances}, samples={sample_count}, opacity={opacity}: {edge:?}"
                );
                assert_eq!(masked.get_pixel(0, 0).0, [0, 0, 0, 0]);
            }
        }
    }
    Ok(())
}

#[test]
fn overlapping_clipped_paths_are_composited_separately() -> anyhow::Result<()> {
    for texture_instances in [false, true] {
        let mut renderer = WgpuHeadlessRenderer::new()?;
        if texture_instances {
            use_texture_instances(&mut renderer);
        }
        enable_transparent_target(&mut renderer);
        let mut red = gpui::red();
        red.a = 0.5;
        let mut blue = gpui::blue();
        blue.a = 0.5;
        let first = rectangular_path(red, asymmetric_clip(), 0);
        for (order, different_clip) in [(1, false), (0, true)] {
            let mut clip = asymmetric_clip();
            if different_clip {
                clip.top_left = 12.;
            }
            let second = rectangular_path(blue, clip, order);
            let mut scene = Scene::default();
            scene.paths.push(first.clone());
            let behind = render_transparent(&mut renderer, &scene)?;
            scene.paths[0] = second.clone();
            let front = render_transparent(&mut renderer, &scene)?;
            scene.paths = vec![first.clone(), second];
            scene.finish();
            let combined = render_transparent(&mut renderer, &scene)?;
            for (x, y) in [(11, 13), (32, 24)] {
                let back = behind.get_pixel(x, y).0;
                let front = front.get_pixel(x, y).0;
                let actual = combined.get_pixel(x, y).0;
                for channel in 0..4 {
                    let expected = front[channel] as f32
                        + back[channel] as f32 * (1. - front[3] as f32 / 255.);
                    assert!(
                        (actual[channel] as f32 - expected).abs() <= 3.,
                        "texture={texture_instances}, order={order}, different_clip={different_clip}, ({x},{y}): {actual:?}"
                    );
                }
            }
        }
    }
    Ok(())
}

#[test]
fn path_batches_keep_unclipped_runs_together() {
    let mut paths: Vec<_> = (0..303)
        .map(|order| rectangular_path(gpui::white(), RoundedClipParams::default(), order))
        .collect();
    for index in [100, 201, 202] {
        paths[index].clip = asymmetric_clip();
    }
    let mut remaining = paths.as_slice();
    for expected in [100, 1, 100, 1, 1, 100] {
        let (batch, rest) = split_path_batch(remaining);
        assert_eq!(batch.len(), expected);
        if expected == 1 {
            assert!(batch[0].clip.right > batch[0].clip.left);
        } else {
            assert!(batch.iter().all(|path| path.clip.right <= path.clip.left));
        }
        remaining = rest;
    }
    assert!(remaining.is_empty());
    let (empty, rest) = split_path_batch(remaining);
    assert!(empty.is_empty() && rest.is_empty());
}

#[test]
fn mixed_path_batches_preserve_alpha_and_order() -> anyhow::Result<()> {
    for texture_instances in [false, true] {
        let mut renderer = WgpuHeadlessRenderer::new()?;
        if texture_instances {
            use_texture_instances(&mut renderer);
        }
        enable_transparent_target(&mut renderer);
        let mut red = gpui::red();
        red.a = 0.5;
        let mut blue = gpui::blue();
        blue.a = 0.5;
        let green = gpui::hsla(1. / 3., 1., 0.5, 0.5);
        let mut scene = Scene::default();
        scene
            .paths
            .push(rectangular_path(red, RoundedClipParams::default(), 0));
        scene
            .paths
            .push(rectangular_path(blue, RoundedClipParams::default(), 1));
        scene
            .paths
            .push(rectangular_path(green, asymmetric_clip(), 2));
        scene
            .paths
            .push(rectangular_path(red, RoundedClipParams::default(), 3));
        scene
            .paths
            .push(rectangular_path(blue, RoundedClipParams::default(), 4));
        scene.finish();
        let image = render_transparent(&mut renderer, &scene)?;
        for (x, y, expected) in [
            (8, 8, [80_u8, 0, 159, 239]),
            (32, 24, [72_u8, 32, 143, 247]),
        ] {
            let actual = image.get_pixel(x, y).0;
            assert!(
                actual.iter().zip(expected).all(|(a, b)| a.abs_diff(b) <= 4),
                "pixel ({x}, {y})={actual:?}, expected {expected:?}"
            );
        }
    }
    Ok(())
}

#[test]
fn fractional_rounded_path_edge_is_clipped_after_msaa_resolve() -> anyhow::Result<()> {
    for texture_instances in [false, true] {
        let mut renderer = WgpuHeadlessRenderer::new()?;
        if texture_instances {
            use_texture_instances(&mut renderer);
        }
        let formats = renderer
            .context
            .adapter
            .get_texture_format_features(renderer.core.target_format);
        for sample_count in [1, 4] {
            if !formats.flags.sample_count_supported(sample_count) {
                continue;
            }
            renderer.core.rendering_params.path_sample_count = sample_count;
            renderer.core.resources.invalidate_intermediate_textures();
            enable_transparent_target(&mut renderer);
            let mut red = gpui::red();
            red.a = 0.5;
            let clip = RoundedClipParams {
                left: 8.75,
                top: 8.75,
                right: 55.25,
                bottom: 55.25,
                top_left: 8.0,
                top_right: 8.0,
                bottom_right: 8.0,
                bottom_left: 8.0,
            };
            let mut path = rectangular_path(red, clip, 0);
            // A rounded scope supplies the outward-covered rectangular content mask.
            path.content_mask.bounds = bounds(8.0, 8.0, 48.0, 48.0);
            path.vertices.extend(path.vertices.clone());
            let mut scene = Scene::default();
            scene.paths.push(path);
            scene.finish();
            let image = render_transparent(&mut renderer, &scene)?;
            for (x, y, expected) in [(8, 32, 48_u8), (32, 8, 48), (9, 32, 191)] {
                let pixel = image.get_pixel(x, y).0;
                assert!(
                    pixel[3].abs_diff(expected) <= 3 && pixel[0].abs_diff(pixel[3]) <= 2,
                    "samples={sample_count}, pixel ({x}, {y})={pixel:?}, expected alpha {expected}"
                );
            }
            assert_eq!(image.get_pixel(7, 32).0, [0, 0, 0, 0]);
        }
    }
    Ok(())
}

#[test]
fn rounded_paths_clip_resolved_overdraw_once_and_preserve_order() -> anyhow::Result<()> {
    for texture_instances in [false, true] {
        let mut renderer = WgpuHeadlessRenderer::new()?;
        if texture_instances {
            use_texture_instances(&mut renderer);
        }
        enable_transparent_target(&mut renderer);
        let mut red = gpui::red();
        red.a = 0.5;
        let mut path = rectangular_path(red, RoundedClipParams::default(), 0);
        path.vertices.extend(path.vertices.clone());
        let mut scene = Scene::default();
        scene.paths.push(path);
        scene.finish();
        let unmasked = render_transparent(&mut renderer, &scene)?;
        scene.paths[0].clip = asymmetric_clip();
        let masked = render_transparent(&mut renderer, &scene)?;
        let original = unmasked.get_pixel(11, 13).0;
        let actual = masked.get_pixel(11, 13).0;
        let coverage = (0.5 - (12.5_f32.hypot(10.5) - 16.0)).clamp(0.0, 1.0);
        let expected = (original[3] as f32 * coverage).round() as u8;
        assert!(original[3] > 180, "triangles did not overlap: {original:?}");
        assert!(
            actual[3].abs_diff(expected) <= 3,
            "clipped={actual:?}, expected={expected}"
        );
        assert!(
            actual[0].abs_diff(actual[3]) <= 2,
            "premultiplied: {actual:?}"
        );

        let mut blue = gpui::blue();
        blue.a = 0.5;
        let mut red_clip = asymmetric_clip();
        red_clip.bottom_right = 0.0;
        let mut blue_clip = asymmetric_clip();
        blue_clip.top_left = 0.0;
        let mut ordered = Scene::default();
        ordered.paths.push(rectangular_path(red, red_clip, 0));
        ordered.paths.push(rectangular_path(blue, blue_clip, 1));
        ordered.finish();
        let image = render_transparent(&mut renderer, &ordered)?;
        for (x, y, expected) in [
            (8, 8, [0_u8, 0, 128, 128]),
            (55, 55, [128, 0, 0, 128]),
            (32, 24, [64, 0, 128, 191]),
        ] {
            let actual = image.get_pixel(x, y).0;
            assert!(
                actual.iter().zip(expected).all(|(a, b)| a.abs_diff(b) <= 3),
                "pixel ({x}, {y}): {actual:?}, expected {expected:?}"
            );
        }
    }
    Ok(())
}

#[test]
fn rounded_clip_scales_wavy_underline_and_inset_shadow_coverage() -> anyhow::Result<()> {
    for texture_instances in [false, true] {
        for inset in [false, true] {
            let mut renderer = WgpuHeadlessRenderer::new()?;
            if texture_instances {
                use_texture_instances(&mut renderer);
            }
            let mask = ContentMask {
                bounds: bounds(0.0, 0.0, 64.0, 64.0),
            };
            let mut white = gpui::white();
            white.a = 0.5;
            let mut scene = Scene::default();
            scene.quads.push(quad(0, mask.bounds, gpui::blue()));
            if inset {
                scene.shadows.push(gpui::Shadow {
                    order: 1,
                    blur_radius: 0.0.into(),
                    bounds: bounds(100.0, 100.0, 8.0, 8.0),
                    corner_radii: Corners::default(),
                    content_mask: mask,
                    color: white,
                    element_bounds: bounds(8.0, 8.0, 48.0, 48.0),
                    element_corner_radii: Corners::default(),
                    inset: 1,
                    pad: 0,
                    clip: RoundedClipParams::default(),
                });
            } else {
                scene.underlines.push(gpui::Underline {
                    order: 1,
                    pad: 0,
                    bounds: bounds(8.0, 10.0, 48.0, 5.0),
                    content_mask: mask,
                    color: white,
                    thickness: 2.0.into(),
                    wavy: true.into(),
                    clip: RoundedClipParams::default(),
                });
            }
            scene.finish();
            let unmasked = renderer.render_scene_to_image(&scene, device_size(64, 64))?;
            if inset {
                scene.shadows[0].clip = asymmetric_clip();
            } else {
                scene.underlines[0].clip = asymmetric_clip();
            }
            let clipped = renderer.render_scene_to_image(&scene, device_size(64, 64))?;
            let before = unmasked.get_pixel(12, 12).0;
            let after = clipped.get_pixel(12, 12).0;
            let coverage = (0.5 - (11.5_f32.hypot(11.5) - 16.0)).clamp(0.0, 1.0);
            let expected = (before[0] as f32 * coverage).round() as u8;
            assert!(before[0] > 20, "missing unmasked primitive: {before:?}");
            assert!(
                after[0].abs_diff(expected) <= 3,
                "inset={inset}, unmasked={before:?}, clipped={after:?}, expected={expected}"
            );
        }
    }
    Ok(())
}

fn translucent_image_tile(renderer: &WgpuHeadlessRenderer) -> anyhow::Result<gpui::AtlasTile> {
    let key = gpui::AtlasKey::Image(gpui::RenderImageParams {
        image_id: gpui::ImageId(771),
        frame_index: 0,
    });
    renderer
        .core
        .atlas
        .get_or_insert_with(key, &mut || {
            let mut bytes = vec![255; 16 * 16 * 4];
            for pixel in bytes.chunks_exact_mut(4) {
                pixel[3] = 128;
            }
            Ok(Some((device_size(16, 16), std::borrow::Cow::Owned(bytes))))
        })?
        .ok_or_else(|| anyhow::anyhow!("atlas did not create an image tile"))
}

fn masked_image_sprite(tile: gpui::AtlasTile) -> gpui::PolychromeSprite {
    gpui::PolychromeSprite {
        order: 1,
        pad: 0,
        tile,
        grayscale: false.into(),
        opacity: 0.5,
        bounds: bounds(8.0, 8.0, 48.0, 48.0),
        content_mask: ContentMask {
            bounds: bounds(0.0, 0.0, 64.0, 64.0),
        },
        corner_radii: Corners::default(),
        fade: EdgeFadeParams::default(),
        alpha_mask: gpui::ImageAlphaMaskParams::default(),
    }
}

fn mask_smoothstep(value: f32) -> f32 {
    let value = value.clamp(0.0, 1.0);
    value * value * (3.0 - 2.0 * value)
}

fn assert_white_premultiplied(image: &image::RgbaImage, x: u32, y: u32, alpha: f32) {
    let expected = alpha.round() as u8;
    let pixel = image.get_pixel(x, y).0;
    assert!(
        pixel.iter().all(|channel| channel.abs_diff(expected) <= 2),
        "pixel ({x}, {y})={pixel:?}, expected premultiplied white alpha {expected}"
    );
}

#[test]
fn image_mask_pixels_feather_bottom_alpha_and_atlas_reuse() -> anyhow::Result<()> {
    for texture_instances in [false, true] {
        let mut renderer = WgpuHeadlessRenderer::new()?;
        if texture_instances {
            use_texture_instances(&mut renderer);
        }
        enable_transparent_target(&mut renderer);
        let atlas = renderer.core.atlas.clone();
        let tile = translucent_image_tile(&renderer)?;
        assert_eq!(atlas.upload_count(), 0);

        let mut scene = Scene::default();
        scene.polychrome_sprites.push(masked_image_sprite(tile));
        scene.finish();
        let baseline = render_transparent(&mut renderer, &scene)?;
        assert_eq!(atlas.upload_count(), 1);
        assert_white_premultiplied(&baseline, 31, 27, 64.0);
        assert_white_premultiplied(&baseline, 15, 46, 64.0);

        let mask = gpui::ImageAlphaMaskParams {
            bounds: bounds(24.0, 20.0, 16.0, 16.0),
            radius: 4.0,
            feather: 4.0,
            clearance: 2.0,
            bottom_y: 45.0,
            bottom_feather: 8.0,
            pad: 0.0,
        };
        scene.polychrome_sprites[0].alpha_mask = gpui::ImageAlphaMaskParams {
            feather: 0.0,
            ..mask
        };
        assert_eq!(render_transparent(&mut renderer, &scene)?, baseline);
        scene.polychrome_sprites[0].alpha_mask.feather = -1.0;
        assert_eq!(render_transparent(&mut renderer, &scene)?, baseline);

        scene.polychrome_sprites[0].alpha_mask = mask;
        let clipped = render_transparent(&mut renderer, &scene)?;
        assert_white_premultiplied(&clipped, 31, 27, 0.0);
        assert_white_premultiplied(&clipped, 15, 27, 64.0);
        assert_white_premultiplied(&clipped, 44, 27, 64.0 * mask_smoothstep(2.5 / 4.0));
        assert_white_premultiplied(&clipped, 20, 27, 64.0 * mask_smoothstep(1.5 / 4.0));
        let corner_distance = 6.5_f32.hypot(6.5) - 4.0;
        assert_white_premultiplied(
            &clipped,
            21,
            17,
            64.0 * mask_smoothstep((corner_distance - 2.0) / 4.0),
        );
        assert_white_premultiplied(&clipped, 21, 24, 64.0 * mask_smoothstep(0.5 / 4.0));
        assert_white_premultiplied(&clipped, 15, 41, 64.0 * mask_smoothstep(3.5 / 8.0));
        assert_white_premultiplied(&clipped, 15, 46, 0.0);
        // At this pixel both ramps are partial; the reference combines them with min.
        assert_white_premultiplied(&clipped, 31, 40, 64.0 * mask_smoothstep(4.5 / 8.0));

        scene.polychrome_sprites[0].alpha_mask.bottom_feather = 0.0;
        let no_bottom = render_transparent(&mut renderer, &scene)?;
        assert_white_premultiplied(&no_bottom, 15, 46, 64.0);
        assert_white_premultiplied(&no_bottom, 31, 27, 0.0);
        scene.polychrome_sprites[0].alpha_mask.bottom_feather = -1.0;
        assert_eq!(render_transparent(&mut renderer, &scene)?, no_bottom);

        scene.polychrome_sprites[0].alpha_mask = mask;
        scene.polychrome_sprites[0].alpha_mask.bounds = bounds(40.0, 20.0, 12.0, 16.0);
        let moved = render_transparent(&mut renderer, &scene)?;
        assert_white_premultiplied(&moved, 31, 27, 64.0);
        assert_eq!(translucent_image_tile(&renderer)?, tile);
        assert_eq!(
            atlas.upload_count(),
            1,
            "mask changes must not upload the atlas tile"
        );

        scene.polychrome_sprites[0].alpha_mask = mask;
        scene
            .quads
            .push(quad(0, bounds(0.0, 0.0, 64.0, 64.0), gpui::blue()));
        scene
            .quads
            .push(quad(2, bounds(30.0, 30.0, 4.0, 4.0), gpui::red()));
        scene.finish();
        // The transparent pipeline remains active, so read back its premultiplied result.
        let ordered = render_transparent(&mut renderer, &scene)?;
        assert_eq!(ordered.get_pixel(31, 27).0, [0, 0, 255, 255]);
        let image_pixel = ordered.get_pixel(15, 27).0;
        assert!(
            image_pixel[0].abs_diff(64) <= 2
                && image_pixel[1].abs_diff(64) <= 2
                && image_pixel[2] == 255
                && image_pixel[3] == 255,
            "{image_pixel:?}"
        );
        assert_eq!(ordered.get_pixel(31, 31).0, [255, 0, 0, 255]);
        assert_eq!(atlas.upload_count(), 1);
    }
    Ok(())
}

#[test]
fn image_mask_multiplies_source_alpha_fade_and_rounded_clip_once() -> anyhow::Result<()> {
    for texture_instances in [false, true] {
        let mut renderer = WgpuHeadlessRenderer::new()?;
        if texture_instances {
            use_texture_instances(&mut renderer);
        }
        enable_transparent_target(&mut renderer);
        let tile = translucent_image_tile(&renderer)?;
        let mut first = masked_image_sprite(tile);
        first.order = 0;
        first.bounds = bounds(0.0, 0.0, 4.0, 4.0);
        let mut sprite = masked_image_sprite(tile);
        sprite.fade = EdgeFadeParams {
            left_x: 0.5,
            band_left: 16.0,
            clip: asymmetric_clip(),
            ..Default::default()
        };
        sprite.alpha_mask = gpui::ImageAlphaMaskParams {
            bounds: bounds(8.0, 12.0, 2.0, 2.0),
            radius: 0.0,
            feather: 5.0,
            clearance: 0.0,
            bottom_y: 0.0,
            bottom_feather: 0.0,
            pad: 0.0,
        };
        let mut scene = Scene::default();
        // The second instance starts halfway through a WebGL texel at 50 words per record.
        scene.polychrome_sprites.extend([first, sprite]);
        scene
            .quads
            .push(quad(2, bounds(30.0, 30.0, 4.0, 4.0), gpui::red()));
        scene.finish();
        let image = render_transparent(&mut renderer, &scene)?;
        let clip = (0.5 - (11.5_f32.hypot(10.5) - 16.0)).clamp(0.0, 1.0);
        let expected = 128.0 * 0.5 * 0.75_f32.powi(2) * 0.5 * clip;
        assert_white_premultiplied(&image, 12, 13, expected);
        assert_eq!(image.get_pixel(8, 8).0, [0, 0, 0, 0]);
        assert_eq!(image.get_pixel(31, 31).0, [255, 0, 0, 255]);
    }
    Ok(())
}
