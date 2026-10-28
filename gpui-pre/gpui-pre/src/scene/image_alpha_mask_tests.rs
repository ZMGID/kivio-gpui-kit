// Adapted from zui crates/gpui/src/scene.rs at
// b564a34413bb7ef1da04121c4a287758f94993d9 (Apache-2.0).
use super::*;
use crate::{px, size};
use std::mem::{offset_of, size_of};

#[test]
fn mask_scales_window_geometry_and_clamps_radii() {
    let mut mask = ImageAlphaMask::new(
        Bounds::new(point(px(40.25), px(300.5)), size(px(500.), px(100.))),
        px(80.),
        px(220.),
    );
    assert_eq!(mask.clearance, Pixels::ZERO);
    assert_eq!(mask.bottom_fade, None);
    mask.clearance = px(8.);
    mask.bottom_fade = Some((px(480.), px(96.)));
    for factor in [1., 1.25, 1.5, 2., 3.] {
        let scaled = mask.scale(factor);
        assert_eq!(scaled.bounds, mask.bounds.scale(factor));
        assert_eq!(scaled.radius, 50. * factor);
        assert_eq!(scaled.feather, 220. * factor);
        assert_eq!(scaled.clearance, 8. * factor);
        assert_eq!(scaled.bottom_y, 480. * factor);
        assert_eq!(scaled.bottom_feather, 96. * factor);
    }
    mask.radius = px(-5.);
    mask.clearance = px(-8.);
    mask.bottom_fade = Some((px(480.), px(-4.)));
    let scaled = mask.scale(2.);
    assert_eq!(scaled.radius, 0.);
    assert_eq!(scaled.clearance, 0.);
    assert_eq!(scaled.bottom_feather, 0.);
}

#[test]
fn nonpositive_feather_disables_mask_even_with_bottom_fade() {
    let mut mask = ImageAlphaMask::new(
        Bounds::new(point(px(10.), px(20.)), size(px(30.), px(40.))),
        px(4.),
        px(0.),
    );
    mask.bottom_fade = Some((px(50.), px(8.)));
    assert_eq!(mask.scale(2.).feather, 0.);
    mask.feather = px(-1.);
    assert_eq!(mask.scale(2.).feather, 0.);
    assert_eq!(ImageAlphaMaskParams::default().feather, 0.);
}

#[test]
fn image_mask_gpu_layout_matches_shader_abi() {
    assert_eq!(size_of::<ImageAlphaMaskParams>(), 40);
    assert_eq!(offset_of!(ImageAlphaMaskParams, bounds), 0);
    assert_eq!(offset_of!(ImageAlphaMaskParams, radius), 16);
    assert_eq!(offset_of!(ImageAlphaMaskParams, feather), 20);
    assert_eq!(offset_of!(ImageAlphaMaskParams, clearance), 24);
    assert_eq!(offset_of!(ImageAlphaMaskParams, bottom_y), 28);
    assert_eq!(offset_of!(ImageAlphaMaskParams, bottom_feather), 32);
    assert_eq!(offset_of!(ImageAlphaMaskParams, pad), 36);
    assert_eq!(offset_of!(PolychromeSprite, alpha_mask), 160);
    assert_eq!(size_of::<PolychromeSprite>(), 200);
}
