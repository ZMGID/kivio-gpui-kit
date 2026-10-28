#![cfg(all(feature = "test-support", feature = "component"))]

use gpui_kit::{
    AppContext as _, Context, FocusHandle, MouseButton, Render, TestAppContext,
    TestSupportExt as _, Window,
    base::{SelectableText, TextSelection},
    component::text::TextView,
    div, point,
    prelude::*,
    px,
    test::TestWindowExt as _,
};

struct CopyFixture {
    plain: bool,
    text: &'static str,
    focus: FocusHandle,
}

impl Render for CopyFixture {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let focus = self.focus.clone();
        let text = if self.plain {
            SelectableText::new("plain-copy", self.text).into_any_element()
        } else {
            TextView::markdown("markdown-copy", self.text).into_any_element()
        };
        div().p_4().child(
            div()
                .id("copy-target")
                .test_support()
                .w(px(300.))
                .track_focus(&self.focus)
                .when(self.plain, |element| {
                    element.on_mouse_down(MouseButton::Left, move |_, window, cx| {
                        focus.focus(window, cx);
                    })
                })
                .child(text),
        )
    }
}

#[gpui_kit::test]
fn copy_keeps_selected_boundary_whitespace_in_root_and_text_view(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    for (plain, text) in [
        (true, "  alpha  \nbeta  "),
        (true, "  \n  "),
        (false, "alpha\n\nbeta"),
    ] {
        let (window, _) = cx.update(|cx| {
            gpui_kit::open_window(Default::default(), cx, |_, cx| {
                cx.new(|cx| CopyFixture {
                    plain,
                    text,
                    focus: cx.focus_handle(),
                })
            })
            .unwrap()
        });
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            let bounds = window.find("copy-target").bounds();
            window.drag(
                point(bounds.left() + px(0.5), bounds.top() + px(4.)),
                point(bounds.right() + px(20.), bounds.bottom() - px(4.)),
                cx,
            );
            let selected = TextSelection::selected_text(window, cx);
            if plain {
                assert_eq!(selected, text);
            } else {
                assert!(
                    selected.starts_with("alpha") && selected.contains("beta"),
                    "{selected:?}"
                );
                assert!(
                    selected.ends_with('\n'),
                    "the fixture must include its selected boundary: {selected:?}"
                );
            }
            window.press("secondary-c", cx);
            assert_eq!(
                cx.read_from_clipboard().and_then(|item| item.text()),
                Some(selected)
            );
        })
        .unwrap();
    }
}
