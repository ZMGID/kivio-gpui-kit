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
    use gpui::{EdgeFadeParams, MonochromeSprite, PolychromeSprite, Quad, SubpixelSprite};
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
        assert_eq!(align_of::<EdgeFadeParams>(), align_of::<f32>());
        assert_eq!(size_of::<EdgeFadeParams>(), 32);
        assert_eq!(offset_of!(EdgeFadeParams, top_y), 0);
        assert_eq!(offset_of!(EdgeFadeParams, bottom_y), 4);
        assert_eq!(offset_of!(EdgeFadeParams, band_top), 8);
        assert_eq!(offset_of!(EdgeFadeParams, band_bottom), 12);
        assert_eq!(offset_of!(EdgeFadeParams, left_x), 16);
        assert_eq!(offset_of!(EdgeFadeParams, right_x), 20);
        assert_eq!(offset_of!(EdgeFadeParams, band_left), 24);
        assert_eq!(offset_of!(EdgeFadeParams, band_right), 28);
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
            ],
        );
        assert_eq!(offset_of!(Quad, fade), 160);
        assert_eq!(size_of::<Quad>(), 192);
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
        assert_eq!(size_of::<MonochromeSprite>(), 144);
        assert_eq!(size_of::<SubpixelSprite>(), 144);
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
        assert_eq!(offset_of!(PolychromeSprite, fade), 96);
        assert_eq!(size_of::<PolychromeSprite>(), 128);
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
            ],
        );
    }
}
