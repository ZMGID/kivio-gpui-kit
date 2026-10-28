// Adapted from Zeron crates/ui/src/rounded_clip.rs at
// 3d2f1e64af3b40ec1354ca199fbe439c666f3681. Modified for Kit and per-corner radii.
// MIT License — Copyright (c) 2026 Wing
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

use gpui::{
    AnyElement, App, Bounds, Corners, Element, ElementId, GlobalElementId, InspectorElementId,
    IntoElement, LayoutId, Pixels, Window,
};

/// Clip content to its own rounded bounds without changing its layout.
///
/// Put outer shadows and outward focus rings outside this element. Hit testing
/// retains GPUI's rectangular content mask; nested roundings use the innermost
/// scope while all ancestor rectangular bounds remain intersected.
pub fn rounded_clip(corners: impl Into<Corners<Pixels>>, child: impl IntoElement) -> RoundedClip {
    RoundedClip {
        corners: corners.into(),
        child: child.into_any_element(),
    }
}

/// A layout-transparent rounded content boundary, available on native GPUI.
pub struct RoundedClip {
    corners: Corners<Pixels>,
    child: AnyElement,
}

impl Element for RoundedClip {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        window.with_rounded_clip(bounds, self.corners, |window| {
            self.child.prepaint(window, cx)
        });
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        window.with_rounded_clip(bounds, self.corners, |window| self.child.paint(window, cx));
    }
}

impl IntoElement for RoundedClip {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Theme, ThemeMode,
        attachment::{Attachment, AttachmentMedia},
    };
    use gpui::{
        Background, Context, ParentElement as _, Render, Styled as _, TestAppContext, div, px, rgb,
        size,
    };

    struct Media;

    impl Render for Media {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .p_4()
                .child(
                    Attachment::new()
                        .id("attachment")
                        .on_remove(|_, _, _| {})
                        .media(
                            AttachmentMedia::new()
                                .rounded_tl(px(4.))
                                .rounded_tr(px(8.))
                                .rounded_br(px(12.))
                                .rounded_bl(px(16.))
                                .child(div().size_full().bg(rgb(0x436789)))
                                .overlay(div().size_full().bg(rgb(0x798643)).opacity(0.5)),
                        ),
                )
                .child(div().size(px(20.)).bg(rgb(0x123456)))
        }
    }

    #[gpui::test]
    fn attachment_custom_content_and_overlays_share_only_the_media_clip(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let (_, cx) = cx.add_window_view(|_, _| Media);
        cx.simulate_resize(size(px(400.), px(240.)));
        for mode in [ThemeMode::Light, ThemeMode::Dark] {
            cx.update(|window, cx| {
                Theme::change(mode, None, cx);
                window.draw(cx).clear(cx);
                let quads = window.painted_quads();
                let content = quads
                    .iter()
                    .find(|quad| quad.background == Background::from(rgb(0x436789)))
                    .expect("custom media painted");
                let overlay = quads
                    .iter()
                    .find(|quad| quad.background == Background::from(rgb(0x798643)).opacity(0.5))
                    .expect("overlay painted");
                let clip = content.fade.clip;
                assert_eq!(clip, overlay.fade.clip);
                let scale = window.scale_factor();
                assert_eq!(clip.top_left, 4. * scale);
                assert_eq!(clip.top_right, 8. * scale);
                assert_eq!(clip.bottom_right, 12. * scale);
                assert_eq!(clip.bottom_left, 16. * scale);
                assert_eq!(clip.left, content.bounds.left().0);
                assert_eq!(clip.right, content.bounds.right().0);
                let outside = quads
                    .iter()
                    .find(|quad| quad.background == Background::from(rgb(0x123456)))
                    .expect("following sibling painted");
                assert_eq!(outside.fade.clip, Default::default());
                // The attachment card and its outboard controls stay outside the media boundary.
                assert!(quads.iter().any(|quad| {
                    quad.bounds.left() < content.bounds.left()
                        && quad.bounds.right() > content.bounds.right()
                        && quad.fade.clip == Default::default()
                }));
            });
        }
    }
}
