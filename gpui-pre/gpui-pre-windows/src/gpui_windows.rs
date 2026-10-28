#![cfg(target_os = "windows")]

mod clipboard;
mod destination_list;
mod dialog;
mod direct_manipulation;
mod direct_write;
mod directx_atlas;
mod directx_devices;
mod directx_renderer;
mod dispatcher;
mod display;
mod events;
mod keyboard;
mod platform;
mod system_notifications;
mod system_settings;
mod util;
mod vsync;
mod window;
mod wrapper;

pub(crate) use clipboard::*;
pub(crate) use destination_list::*;
pub(crate) use direct_write::*;
pub(crate) use directx_atlas::*;
pub(crate) use directx_devices::*;
pub(crate) use directx_renderer::*;
pub(crate) use dispatcher::*;
pub(crate) use display::*;
pub(crate) use events::*;
pub(crate) use keyboard::*;
pub(crate) use platform::*;
pub(crate) use system_notifications::*;
pub(crate) use system_settings::*;
pub(crate) use util::*;
pub(crate) use vsync::*;
pub(crate) use window::*;
pub(crate) use wrapper::*;

pub use platform::WindowsPlatform;

pub(crate) use windows::Win32::Foundation::HWND;

#[cfg(test)]
mod tests {
    use gpui::{
        AtlasTile, BackdropBlur, EdgeFadeParams, ImageAlphaMaskParams, MonochromeSprite,
        PolychromeSprite, Quad, RoundedClipParams, Shadow, SubpixelSprite, Underline,
    };
    use std::mem::{align_of, offset_of, size_of};

    const SHADERS: &str = include_str!("shaders.hlsl");

    fn hlsl_struct(name: &str) -> &str {
        SHADERS
            .split_once(&format!("struct {name} {{"))
            .unwrap_or_else(|| panic!("missing HLSL {name} declaration"))
            .1
            .split_once("};")
            .unwrap_or_else(|| panic!("unterminated HLSL {name} declaration"))
            .0
    }

    fn assert_fields_in_order(body: &str, fields: &[&str]) {
        let mut rest = body;
        for field in fields {
            rest = rest
                .split_once(field)
                .unwrap_or_else(|| panic!("missing or out-of-order field `{field}` in {body}"))
                .1;
        }
    }

    #[test]
    fn edge_fade_hlsl_layout_matches_scene_primitives() {
        assert_eq!(align_of::<RoundedClipParams>(), align_of::<f32>());
        assert_eq!(size_of::<RoundedClipParams>(), 32);
        assert_eq!(offset_of!(RoundedClipParams, left), 0);
        assert_eq!(offset_of!(RoundedClipParams, top), 4);
        assert_eq!(offset_of!(RoundedClipParams, right), 8);
        assert_eq!(offset_of!(RoundedClipParams, bottom), 12);
        assert_eq!(offset_of!(RoundedClipParams, top_left), 16);
        assert_eq!(offset_of!(RoundedClipParams, top_right), 20);
        assert_eq!(offset_of!(RoundedClipParams, bottom_right), 24);
        assert_eq!(offset_of!(RoundedClipParams, bottom_left), 28);
        assert_fields_in_order(
            hlsl_struct("RoundedClipParams"),
            &[
                "float left;",
                "float top;",
                "float right;",
                "float bottom;",
                "float top_left;",
                "float top_right;",
                "float bottom_right;",
                "float bottom_left;",
            ],
        );
        assert_eq!(align_of::<EdgeFadeParams>(), align_of::<f32>());
        assert_eq!(size_of::<EdgeFadeParams>(), 64);
        assert_eq!(offset_of!(EdgeFadeParams, top_y), 0);
        assert_eq!(offset_of!(EdgeFadeParams, bottom_y), 4);
        assert_eq!(offset_of!(EdgeFadeParams, band_top), 8);
        assert_eq!(offset_of!(EdgeFadeParams, band_bottom), 12);
        assert_eq!(offset_of!(EdgeFadeParams, left_x), 16);
        assert_eq!(offset_of!(EdgeFadeParams, right_x), 20);
        assert_eq!(offset_of!(EdgeFadeParams, band_left), 24);
        assert_eq!(offset_of!(EdgeFadeParams, band_right), 28);
        assert_eq!(offset_of!(EdgeFadeParams, clip), 32);
        assert_fields_in_order(
            hlsl_struct("EdgeFadeParams"),
            &[
                "float top_y;",
                "float bottom_y;",
                "float band_top;",
                "float band_bottom;",
                "float left_x;",
                "float right_x;",
                "float band_left;",
                "float band_right;",
                "RoundedClipParams clip;",
            ],
        );
        assert_eq!(offset_of!(Quad, fade), 160);
        assert_eq!(size_of::<Quad>(), 224);
        assert_fields_in_order(
            hlsl_struct("Quad"),
            &[
                "uint order;",
                "uint border_style;",
                "Bounds bounds;",
                "Bounds content_mask;",
                "Background background;",
                "Hsla border_color;",
                "Corners corner_radii;",
                "Edges border_widths;",
                "EdgeFadeParams fade;",
            ],
        );
        assert_eq!(offset_of!(MonochromeSprite, fade), 112);
        assert_eq!(offset_of!(SubpixelSprite, fade), 112);
        assert_eq!(size_of::<MonochromeSprite>(), 176);
        assert_eq!(size_of::<SubpixelSprite>(), 176);
        assert_fields_in_order(
            hlsl_struct("MonochromeSprite"),
            &[
                "uint order;",
                "uint pad;",
                "AtlasTile tile;",
                "Bounds bounds;",
                "Bounds content_mask;",
                "Hsla color;",
                "TransformationMatrix transformation;",
                "EdgeFadeParams fade;",
            ],
        );
        assert_eq!(size_of::<AtlasTile>(), 32);
        assert_eq!(offset_of!(PolychromeSprite, order), 0);
        assert_eq!(offset_of!(PolychromeSprite, pad), 4);
        assert_eq!(offset_of!(PolychromeSprite, tile), 8);
        assert_eq!(offset_of!(PolychromeSprite, grayscale), 40);
        assert_eq!(offset_of!(PolychromeSprite, opacity), 44);
        assert_eq!(offset_of!(PolychromeSprite, bounds), 48);
        assert_eq!(offset_of!(PolychromeSprite, content_mask), 64);
        assert_eq!(offset_of!(PolychromeSprite, corner_radii), 80);
        assert_eq!(offset_of!(PolychromeSprite, fade), 96);
        assert_eq!(offset_of!(PolychromeSprite, alpha_mask), 160);
        assert_eq!(size_of::<PolychromeSprite>(), 200);
        assert_fields_in_order(
            hlsl_struct("PolychromeSprite"),
            &[
                "uint order;",
                "uint pad;",
                "AtlasTile tile;",
                "uint grayscale;",
                "float opacity;",
                "Bounds bounds;",
                "Bounds content_mask;",
                "Corners corner_radii;",
                "EdgeFadeParams fade;",
                "ImageAlphaMaskParams alpha_mask;",
            ],
        );
        assert_eq!(offset_of!(Shadow, clip), 112);
        assert_eq!(size_of::<Shadow>(), 144);
        assert_fields_in_order(
            hlsl_struct("Shadow"),
            &[
                "uint order;",
                "float blur_radius;",
                "Bounds bounds;",
                "Corners corner_radii;",
                "Bounds content_mask;",
                "Hsla color;",
                "Bounds element_bounds;",
                "Corners element_corner_radii;",
                "uint inset;",
                "uint pad;",
                "RoundedClipParams clip;",
            ],
        );
        assert_eq!(offset_of!(Underline, clip), 64);
        assert_eq!(size_of::<Underline>(), 96);
        assert_fields_in_order(
            hlsl_struct("Underline"),
            &[
                "uint order;",
                "uint pad;",
                "Bounds bounds;",
                "Bounds content_mask;",
                "Hsla color;",
                "float thickness;",
                "uint wavy;",
                "RoundedClipParams clip;",
            ],
        );
        assert_fields_in_order(
            hlsl_struct("PathRasterizationSprite"),
            &[
                "float2 xy_position;",
                "float2 st_position;",
                "Background color;",
                "Bounds bounds;",
            ],
        );
        assert_fields_in_order(
            hlsl_struct("PathSprite"),
            &["Bounds bounds;", "RoundedClipParams clip;"],
        );
        assert_eq!(offset_of!(BackdropBlur, clip), 56);
        assert_eq!(size_of::<BackdropBlur>(), 88);
        let backdrop = SHADERS
            .split_once("cbuffer BackdropParams: register(b2) {")
            .expect("missing HLSL BackdropParams")
            .1
            .split_once("};")
            .expect("unterminated HLSL BackdropParams")
            .0;
        assert_fields_in_order(
            backdrop,
            &[
                "Bounds backdrop_bounds;",
                "Corners backdrop_corners;",
                "Bounds backdrop_clip;",
                "RoundedClipParams backdrop_rounded_clip;",
                "float4 backdrop_source;",
                "float4 backdrop_kernel;",
                "float4 backdrop_weights[33];",
            ],
        );
    }

    #[test]
    fn image_alpha_mask_hlsl_layout_matches_host() {
        assert_eq!(align_of::<ImageAlphaMaskParams>(), align_of::<f32>());
        assert_eq!(size_of::<ImageAlphaMaskParams>(), 40);
        assert_eq!(offset_of!(ImageAlphaMaskParams, bounds), 0);
        assert_eq!(offset_of!(ImageAlphaMaskParams, radius), 16);
        assert_eq!(offset_of!(ImageAlphaMaskParams, feather), 20);
        assert_eq!(offset_of!(ImageAlphaMaskParams, clearance), 24);
        assert_eq!(offset_of!(ImageAlphaMaskParams, bottom_y), 28);
        assert_eq!(offset_of!(ImageAlphaMaskParams, bottom_feather), 32);
        assert_eq!(offset_of!(ImageAlphaMaskParams, pad), 36);
        assert_fields_in_order(
            hlsl_struct("ImageAlphaMaskParams"),
            &[
                "Bounds bounds;",
                "float radius;",
                "float feather;",
                "float clearance;",
                "float bottom_y;",
                "float bottom_feather;",
                "float pad;",
            ],
        );
    }
}
