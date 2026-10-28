// Adapted from zui crates/gpui/src/scene.rs at
// b564a34413bb7ef1da04121c4a287758f94993d9 (Apache-2.0).
use super::*;
use std::mem::{align_of, offset_of, size_of};

fn bounds() -> Bounds<ScaledPixels> {
    Bounds::new(
        point(ScaledPixels(0.), ScaledPixels(0.)),
        crate::size(ScaledPixels(100.), ScaledPixels(100.)),
    )
}

fn rect(x: f32, y: f32, width: f32, height: f32) -> Bounds<ScaledPixels> {
    Bounds::new(
        point(ScaledPixels(x), ScaledPixels(y)),
        crate::size(ScaledPixels(width), ScaledPixels(height)),
    )
}

fn quad(bounds: Bounds<ScaledPixels>, color: Hsla) -> Quad {
    Quad {
        bounds,
        content_mask: ContentMask { bounds },
        background: color.into(),
        ..Quad::default()
    }
}

fn blur_at(bounds: Bounds<ScaledPixels>, sigma: f32) -> BackdropBlur {
    BackdropBlur {
        order: 0,
        blur_radius: ScaledPixels(sigma),
        bounds,
        content_mask: ContentMask { bounds },
        corner_radii: Corners::default(),
    }
}

fn shadow(color: Hsla) -> Shadow {
    Shadow {
        order: 0,
        blur_radius: ScaledPixels(0.),
        bounds: bounds(),
        corner_radii: Corners::default(),
        content_mask: ContentMask { bounds: bounds() },
        color,
        element_bounds: bounds(),
        element_corner_radii: Corners::default(),
        inset: 0,
        pad: 0,
    }
}

fn blur() -> BackdropBlur {
    BackdropBlur {
        order: 0,
        blur_radius: ScaledPixels(10.),
        bounds: bounds(),
        content_mask: ContentMask { bounds: bounds() },
        corner_radii: Corners::default(),
    }
}

fn paint_blur(scene: &mut Scene) {
    scene.insert_primitive(shadow(crate::transparent_black()));
    scene.insert_backdrop_blur(blur());
}

#[test]
fn batches_split_shadows_at_nested_blurs() {
    let mut scene = Scene::default();
    scene.insert_primitive(shadow(crate::black()));
    scene.push_layer(bounds());
    paint_blur(&mut scene);
    scene.insert_primitive(shadow(crate::black()));
    scene.push_layer(bounds());
    paint_blur(&mut scene);
    scene.insert_primitive(shadow(crate::black()));
    scene.pop_layer();
    scene.pop_layer();
    scene.finish();

    let orders: Vec<_> = scene.shadows.iter().map(|shadow| shadow.order).collect();
    let first_blur = scene.backdrop_blurs[0].order;
    let second_blur = scene.backdrop_blurs[1].order;
    assert!(orders[0] < orders[1] && orders[1] < first_blur);
    assert!(first_blur < orders[2] && orders[2] < orders[3]);
    assert!(orders[3] < second_blur && second_blur < orders[4]);
    let batches: Vec<_> = scene.batches().collect();
    assert_eq!(
        batches
            .iter()
            .map(|batch| batch.first_order(&scene))
            .collect::<Vec<_>>(),
        [orders[0], orders[2], orders[4]]
    );
    let ranges: Vec<_> = batches
        .into_iter()
        .map(|batch| match batch {
            PrimitiveBatch::Shadows(range) => range,
            other => panic!("unexpected batch: {other:?}"),
        })
        .collect();
    assert_eq!(ranges, [0..2, 2..4, 4..5]);
}

#[test]
fn separate_layer_blur_waits_for_stripe_in_sampling_padding() {
    let mut scene = Scene::default();
    let background = rect(0., 0., 200., 80.);
    let stripe = rect(88., 20., 10., 40.);
    let panel = rect(100., 20., 20., 40.);
    scene.insert_primitive(quad(background, crate::black()));
    scene.insert_primitive(quad(stripe, crate::white()));
    let background_order = scene.quads[0].order;
    let stripe_order = scene.quads[1].order;

    scene.push_layer(panel);
    let mut splitter = shadow(crate::transparent_black());
    splitter.bounds = panel;
    splitter.element_bounds = panel;
    splitter.content_mask.bounds = panel;
    scene.insert_primitive(splitter);
    scene.insert_backdrop_blur(blur_at(panel, 8.));
    scene.insert_primitive(quad(rect(104., 25., 5., 20.), crate::white()));
    scene.pop_layer();
    let splitter_order = scene.shadows[0].order;
    let blur_order = scene.backdrop_blurs[0].order;
    let foreground_order = scene.quads[2].order;

    assert!(background_order < stripe_order);
    assert!(stripe_order < blur_order && splitter_order < blur_order);
    assert!(blur_order < foreground_order);
    scene.finish();
    let batches: Vec<_> = scene.batches().collect();
    assert!(matches!(
        &batches[..],
        [
            PrimitiveBatch::Quads(background),
            PrimitiveBatch::Shadows(splitter),
            PrimitiveBatch::Quads(stripe),
            PrimitiveBatch::Quads(foreground),
        ] if *background == (0..1) && *splitter == (0..1)
            && *stripe == (1..2) && *foreground == (2..3)
    ));
}

#[test]
fn same_layer_blur_separates_foreground_without_splitting_its_batch() {
    let mut scene = Scene::default();
    let background = rect(0., 0., 200., 80.);
    let panel = rect(100., 20., 20., 40.);
    scene.insert_primitive(quad(background, crate::black()));
    scene.push_layer(background);
    scene.insert_primitive(quad(rect(88., 20., 10., 40.), crate::white()));
    scene.insert_backdrop_blur(blur_at(panel, 8.));
    scene.insert_primitive(quad(rect(101., 25., 4., 20.), crate::white()));
    let shadow_bounds = rect(106., 25., 3., 20.);
    let mut foreground_shadow = shadow(crate::white());
    foreground_shadow.bounds = shadow_bounds;
    foreground_shadow.element_bounds = shadow_bounds;
    foreground_shadow.content_mask.bounds = shadow_bounds;
    scene.insert_primitive(foreground_shadow);
    scene.insert_primitive(quad(rect(110., 25., 4., 20.), crate::white()));
    scene.pop_layer();

    let blur_order = scene.backdrop_blurs[0].order;
    assert!(scene.quads[1].order < blur_order);
    assert!(blur_order < scene.quads[2].order);
    assert_eq!(scene.quads[2].order, scene.quads[3].order);
    assert_eq!(scene.shadows[0].order, scene.quads[2].order);
    scene.finish();
    let batches: Vec<_> = scene.batches().collect();
    assert!(matches!(
        &batches[..],
        [PrimitiveBatch::Quads(before), PrimitiveBatch::Shadows(shadows), PrimitiveBatch::Quads(after)]
            if *before == (0..2) && *shadows == (0..1) && *after == (2..4)
    ));
}

#[test]
fn contained_layer_paint_keeps_its_existing_batching_without_blur() {
    let mut scene = Scene::default();
    scene.push_layer(bounds());
    scene.insert_primitive(quad(rect(0., 0., 10., 10.), crate::white()));
    let mut middle = shadow(crate::white());
    middle.bounds = rect(20., 0., 10., 10.);
    middle.element_bounds = middle.bounds;
    scene.insert_primitive(middle);
    scene.insert_primitive(quad(rect(40., 0., 10., 10.), crate::white()));
    scene.pop_layer();

    assert_eq!(scene.quads[0].order, scene.shadows[0].order);
    assert_eq!(scene.quads[0].order, scene.quads[1].order);
    scene.finish();
    let batches: Vec<_> = scene.batches().collect();
    assert!(matches!(
        &batches[..],
        [PrimitiveBatch::Shadows(shadows), PrimitiveBatch::Quads(quads)]
            if *shadows == (0..1) && *quads == (0..2)
    ));
}

#[test]
fn blur_waits_for_drop_and_inset_shadow_raster_bounds() {
    let background = rect(0., 0., 200., 80.);
    let panel = rect(100., 20., 20., 40.);
    for inset in [false, true] {
        let mut scene = Scene::default();
        scene.insert_primitive(quad(background, crate::black()));
        let mut previous = shadow(crate::white());
        previous.bounds = rect(60., 20., 10., 40.);
        previous.element_bounds = if inset {
            rect(88., 20., 10., 40.)
        } else {
            previous.bounds
        };
        previous.content_mask.bounds = background;
        previous.blur_radius = ScaledPixels(10.);
        previous.inset = inset as u32;
        scene.insert_primitive(previous);
        scene.insert_primitive(previous);
        let shadow_order = scene.shadows[1].order;

        scene.push_layer(panel);
        scene.insert_backdrop_blur(blur_at(panel, 8.));
        scene.insert_primitive(quad(rect(104., 25., 5., 20.), crate::white()));
        scene.pop_layer();
        let blur_order = scene.backdrop_blurs[0].order;
        assert!(shadow_order < blur_order);
        assert!(blur_order < scene.quads[1].order);
        scene.finish();
        let batches: Vec<_> = scene.batches().collect();
        assert!(matches!(
            &batches[..],
            [PrimitiveBatch::Quads(before), PrimitiveBatch::Shadows(shadows), PrimitiveBatch::Quads(after)]
                if *before == (0..1) && *shadows == (0..2) && *after == (1..2)
        ));
    }
}

#[test]
fn clipped_shadow_tail_is_recorded_even_when_nominal_bounds_are_outside_mask() {
    let background = rect(0., 0., 200., 80.);
    let panel = rect(100., 20., 20., 40.);
    let mut scene = Scene::default();
    scene.insert_primitive(quad(background, crate::black()));
    let mut previous = shadow(crate::white());
    previous.bounds = rect(60., 20., 10., 40.);
    previous.element_bounds = previous.bounds;
    previous.content_mask.bounds = rect(75., 25., 4., 20.);
    previous.blur_radius = ScaledPixels(10.);
    scene.insert_primitive(previous);
    assert_eq!(scene.shadows.len(), 1);

    scene.push_layer(panel);
    scene.insert_backdrop_blur(blur_at(panel, 8.));
    scene.pop_layer();
    assert!(scene.shadows[0].order < scene.backdrop_blurs[0].order);
}

#[test]
fn nested_layers_index_paint_outside_their_declared_bounds_and_replay_it() {
    let background = rect(0., 0., 200., 80.);
    let layer = rect(60., 20., 10., 40.);
    let panel = rect(100., 20., 20., 40.);
    let mut previous_shadow = shadow(crate::white());
    previous_shadow.bounds = layer;
    previous_shadow.element_bounds = layer;
    previous_shadow.content_mask.bounds = background;
    previous_shadow.blur_radius = ScaledPixels(10.);
    let paints = [
        Primitive::Quad(quad(rect(88., 25., 10., 20.), crate::white())),
        Primitive::Shadow(previous_shadow),
    ];
    for paint in paints {
        let is_shadow = matches!(&paint, Primitive::Shadow(_));
        let mut source = Scene::default();
        source.insert_primitive(quad(background, crate::black()));
        source.push_layer(background);
        let start = source.len();
        source.push_layer(layer);
        source.push_layer(layer);
        source.insert_primitive(paint);
        let previous_order = if is_shadow {
            source.shadows[0].order
        } else {
            source.quads[1].order
        };
        source.push_layer(panel);
        source.insert_backdrop_blur(blur_at(panel, 8.));
        source.insert_primitive(quad(rect(104., 25., 5., 20.), crate::white()));
        let foreground_order = source.quads.last().unwrap().order;
        source.pop_layer();
        source.pop_layer();
        source.pop_layer();
        source.insert_primitive(quad(rect(105., 26., 3., 12.), crate::white()));
        let later_order = source.quads.last().unwrap().order;
        let end = source.len();
        source.pop_layer();
        let blur_order = source.backdrop_blurs[0].order;
        assert!(previous_order < blur_order);
        assert!(blur_order < foreground_order && foreground_order < later_order);
        source.finish();

        let mut replay = Scene::default();
        replay.insert_primitive(quad(background, crate::black()));
        replay.insert_primitive(quad(background, crate::black()));
        replay.push_layer(background);
        replay.replay(start..end, &source);
        replay.pop_layer();
        let replay_previous_order = if is_shadow {
            replay.shadows[0].order
        } else {
            replay.quads[2].order
        };
        assert!(replay_previous_order < replay.backdrop_blurs[0].order);
        assert!(replay.backdrop_blurs[0].order > blur_order);
        assert!(replay.backdrop_blurs[0].order < replay.quads.last().unwrap().order);
    }
}

#[test]
fn cached_range_recomputes_barriers_through_nested_layer_returns() {
    let outer = rect(0., 0., 200., 80.);
    let middle = rect(70., 0., 80., 80.);
    let panel = rect(100., 20., 20., 40.);
    let mut source = Scene::default();
    source.push_layer(outer);
    let start = source.len();
    source.push_layer(middle);
    source.insert_primitive(quad(rect(88., 25., 10., 20.), crate::white()));
    source.push_layer(panel);
    source.insert_backdrop_blur(blur_at(panel, 8.));
    source.insert_primitive(quad(rect(104., 25., 5., 20.), crate::white()));
    source.pop_layer();
    source.insert_primitive(quad(rect(105., 26., 3., 12.), crate::white()));
    source.pop_layer();
    source.insert_primitive(quad(rect(105.5, 27., 2., 8.), crate::white()));
    source.insert_backdrop_blur(blur_at(panel, 8.));
    let end = source.len();
    source.pop_layer();
    source.finish();

    let mut replay = Scene::default();
    replay.insert_primitive(quad(outer, crate::black()));
    replay.insert_primitive(quad(outer, crate::black()));
    replay.push_layer(outer);
    replay.replay(start..end, &source);
    replay.insert_primitive(quad(rect(106., 28., 1., 4.), crate::white()));
    replay.pop_layer();

    let first_blur = replay.backdrop_blurs[0].order;
    let second_blur = replay.backdrop_blurs[1].order;
    assert!(first_blur > source.backdrop_blurs[0].order);
    assert!(replay.quads[2].order < first_blur);
    assert!(first_blur < replay.quads[3].order);
    assert!(replay.quads[3].order < replay.quads[4].order);
    assert!(replay.quads[4].order < replay.quads[5].order);
    assert!(replay.quads[5].order < second_blur);
    assert!(second_blur < replay.quads[6].order);
    replay.finish();
    let batches: Vec<_> = replay.batches().collect();
    assert!(matches!(
        &batches[..],
        [PrimitiveBatch::Quads(before), PrimitiveBatch::Quads(between), PrimitiveBatch::Quads(after)]
            if *before == (0..3) && *between == (3..6) && *after == (6..7)
    ));
}

#[test]
fn partial_mask_sampling_uses_rounded_padding_without_global_barriers() {
    let visible = rect(112.25, 30.25, 4., 16.);
    let markers = [
        rect(85.05, 36., 0.15, 1.),
        rect(143.3, 36., 0.2, 1.),
        rect(113., 3.05, 1., 0.15),
        rect(113., 73.3, 1., 0.2),
    ];
    for marker in markers {
        let mut scene = Scene::default();
        for _ in 0..3 {
            scene.insert_primitive(quad(marker, crate::white()));
        }
        for _ in 0..6 {
            scene.insert_primitive(quad(rect(145., 36., 1., 1.), crate::white()));
            scene.insert_primitive(quad(rect(400., 36., 1., 1.), crate::white()));
        }
        let marker_order = scene.quads[2].order;
        let outside_order = scene.quads[scene.quads.len() - 2].order;
        let distant_order = scene.quads.last().unwrap().order;
        scene.push_layer(visible);
        let mut panel = blur_at(rect(100., 20., 20., 40.), 8.2);
        panel.content_mask.bounds = visible;
        scene.insert_backdrop_blur(panel);
        scene.pop_layer();

        let blur_order = scene.backdrop_blurs[0].order;
        assert_eq!(blur_order, marker_order + 1);
        assert!(blur_order < outside_order && blur_order < distant_order);
    }
}

#[test]
fn sampling_padding_uses_clamped_sigma() {
    let visible = rect(220.25, 220.25, 4., 4.);
    for (sigma, clamped, padding) in [(0.1, 1., 5.), (1_000., 64., 194.)] {
        let marker = rect(visible.left().0 - padding - 0.2, 221., 0.15, 1.);
        let mut scene = Scene::default();
        scene.insert_primitive(quad(marker, crate::white()));
        scene.insert_primitive(quad(marker, crate::white()));
        for _ in 0..6 {
            scene.insert_primitive(quad(rect(600., 221., 1., 1.), crate::white()));
        }
        scene.push_layer(visible);
        let mut panel = blur_at(rect(210., 210., 30., 30.), sigma);
        panel.content_mask.bounds = visible;
        scene.insert_backdrop_blur(panel);
        scene.pop_layer();

        assert_eq!(scene.backdrop_blurs[0].blur_radius, ScaledPixels(clamped));
        assert_eq!(scene.backdrop_blurs[0].order, scene.quads[1].order + 1);
        assert!(scene.backdrop_blurs[0].order < scene.quads.last().unwrap().order);
    }
}

#[test]
fn direct_blur_splits_same_kind_batches_and_preserves_coalescing_without_blur() {
    let quad = Quad {
        bounds: bounds(),
        content_mask: ContentMask { bounds: bounds() },
        ..Quad::default()
    };
    let mut scene = Scene::default();
    scene.insert_primitive(quad);
    scene.insert_primitive(quad);
    scene.finish();
    assert!(matches!(scene.batches().collect::<Vec<_>>().as_slice(),
        [PrimitiveBatch::Quads(range)] if *range == (0..2)));

    scene.clear();
    scene.insert_primitive(quad);
    scene.insert_backdrop_blur(blur());
    scene.insert_primitive(quad);
    scene.finish();
    assert!(scene.quads[0].order < scene.backdrop_blurs[0].order);
    assert!(scene.quads[1].order > scene.backdrop_blurs[0].order);
    let batches: Vec<_> = scene.batches().collect();
    assert!(
        matches!(&batches[..], [PrimitiveBatch::Quads(a), PrimitiveBatch::Quads(b)]
        if *a == (0..1) && *b == (1..2))
    );
    assert_eq!(batches[1].first_order(&scene), scene.quads[1].order);
}

#[test]
fn replay_clear_and_clipping_keep_blur_operations_consistent() {
    let quad = Quad {
        bounds: bounds(),
        content_mask: ContentMask { bounds: bounds() },
        ..Quad::default()
    };
    let mut source = Scene::default();
    source.insert_primitive(quad);
    source.push_layer(bounds());
    source.insert_backdrop_blur(blur());
    source.insert_primitive(quad);
    source.pop_layer();
    source.finish();

    let mut replay = Scene::default();
    replay.replay(0..source.len(), &source);
    replay.finish();
    assert_eq!(replay.len(), source.len());
    assert_eq!(replay.backdrop_blurs.len(), 1);
    assert_eq!(
        replay.backdrop_blurs[0].order,
        source.backdrop_blurs[0].order
    );
    assert_eq!(
        replay.quads.iter().map(|q| q.order).collect::<Vec<_>>(),
        source.quads.iter().map(|q| q.order).collect::<Vec<_>>()
    );

    replay.clear();
    assert_eq!(replay.len(), 0);
    assert!(replay.backdrop_blurs.is_empty());
    let mut clipped = blur();
    clipped.content_mask.bounds.origin = point(ScaledPixels(200.), ScaledPixels(200.));
    replay.insert_backdrop_blur(clipped);
    assert_eq!(replay.len(), 0);
    assert!(replay.backdrop_blurs.is_empty());
    replay.insert_backdrop_blur(blur());
    assert_eq!(replay.batches().count(), 0);
    assert_eq!(replay.backdrop_blurs.len(), 1);
}

#[test]
fn invalid_geometry_is_rejected_and_sigma_is_bounded() {
    let mut scene = Scene::default();
    for radius in [0., -1., f32::NAN, f32::INFINITY] {
        let mut invalid = blur();
        invalid.blur_radius = ScaledPixels(radius);
        scene.insert_backdrop_blur(invalid);
    }
    let mut invalid = blur();
    invalid.bounds.origin.x = ScaledPixels(f32::NAN);
    scene.insert_backdrop_blur(invalid);
    invalid = blur();
    invalid.corner_radii.top_left = ScaledPixels(-1.);
    scene.insert_backdrop_blur(invalid);
    assert_eq!(scene.len(), 0);
    assert!(scene.backdrop_blurs.is_empty());

    let mut small = blur();
    small.bounds.origin.x = ScaledPixels(-5.);
    small.blur_radius = ScaledPixels(0.1);
    scene.insert_backdrop_blur(small);
    let mut large = blur();
    large.blur_radius = ScaledPixels(1_000.);
    scene.insert_backdrop_blur(large);
    assert_eq!(scene.backdrop_blurs[0].blur_radius, ScaledPixels(1.));
    assert_eq!(scene.backdrop_blurs[1].blur_radius, ScaledPixels(64.));
}

#[test]
fn backdrop_instance_layout_matches_metal_bindings() {
    assert_eq!(align_of::<BackdropBlur>(), align_of::<f32>());
    assert_eq!(size_of::<BackdropBlur>(), 56);
    assert_eq!(offset_of!(BackdropBlur, order), 0);
    assert_eq!(offset_of!(BackdropBlur, blur_radius), 4);
    assert_eq!(offset_of!(BackdropBlur, bounds), 8);
    assert_eq!(offset_of!(BackdropBlur, content_mask), 24);
    assert_eq!(offset_of!(BackdropBlur, corner_radii), 40);
}
