use gpui::{
    AnyElement, App, Background, Bounds, Corners, Element, ElementId, Global, GlobalElementId,
    InspectorElementId, IntoElement, LayoutId, ParentElement, Pixels, StyleRefinement, Styled,
    Window,
};

/// Optional backdrop treatment for the actual dialog, menu and notification surfaces.
///
/// Install this as an app global to enable it. Without one, Kit retains its
/// ordinary opaque surfaces. The tint uses each surface's current theme color.
/// The optional `gpui-fast` backend has no backdrop API and keeps the defaults.
#[derive(Clone, Copy)]
pub struct OverlayBackdrop {
    blur_radius: Pixels,
    tint_opacity: f32,
}

impl OverlayBackdrop {
    /// Set the Gaussian sigma in logical pixels and the theme tint's opacity.
    pub fn new(blur_radius: Pixels, tint_opacity: f32) -> Self {
        Self {
            blur_radius,
            tint_opacity: tint_opacity.clamp(0., 1.),
        }
    }

    pub(crate) fn active(cx: &App) -> Option<Self> {
        #[cfg(not(feature = "gpui-fast"))]
        {
            cx.try_global::<Self>().copied()
        }
        #[cfg(feature = "gpui-fast")]
        {
            let _ = cx;
            None
        }
    }

    pub(crate) fn tint(self, color: impl Into<Background>) -> Background {
        color.into().opacity(self.tint_opacity)
    }
}

impl Global for OverlayBackdrop {}

/// Inserts the backdrop immediately before the unchanged surface paints.
pub(crate) struct BackdropSurface<E> {
    child: Option<E>,
    backdrop: Option<OverlayBackdrop>,
}

impl<E> BackdropSurface<E> {
    pub(crate) fn new(child: E, backdrop: Option<OverlayBackdrop>) -> Self {
        Self {
            child: Some(child),
            backdrop,
        }
    }
}

impl<E: Styled> Styled for BackdropSurface<E> {
    fn style(&mut self) -> &mut StyleRefinement {
        self.child.as_mut().unwrap().style()
    }
}

impl<E: ParentElement> ParentElement for BackdropSurface<E> {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.child.as_mut().unwrap().extend(elements);
    }
}

impl<E: Styled + IntoElement + 'static> IntoElement for BackdropSurface<E> {
    type Element = AnyElement;

    fn into_element(mut self) -> Self::Element {
        if self.backdrop.is_some() {
            Element::into_any(self)
        } else {
            self.child.take().unwrap().into_any_element()
        }
    }

    fn into_any_element(self) -> AnyElement {
        self.into_element()
    }
}

impl<E: Styled + IntoElement + 'static> Element for BackdropSurface<E> {
    type RequestLayoutState = (AnyElement, Corners<Pixels>, f32);
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
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut child = self.child.take().unwrap();
        let (corners, opacity) = if self.backdrop.is_some() {
            let style = child.style();
            (
                crate::styled::corner_radii(style, window.rem_size()),
                style.opacity.unwrap_or(1.).clamp(0., 1.),
            )
        } else {
            (Corners::default(), 1.)
        };
        let mut child = child.into_any_element();
        let layout = child.request_layout(window, cx);
        (layout, (child, corners, opacity))
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        (child, _, _): &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) {
        child.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        (child, corners, opacity): &mut Self::RequestLayoutState,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        #[cfg(not(feature = "gpui-fast"))]
        if let Some(backdrop) = self.backdrop {
            // shortcut: fades taper sigma; add compositing alpha if exact opacity is needed.
            let opacity = *opacity * window.element_opacity();
            if opacity > 0. {
                window.paint_layer(bounds, |window| {
                    window.paint_backdrop_blur(
                        bounds,
                        corners.clamp_radii_for_quad_size(bounds.size),
                        backdrop.blur_radius * opacity,
                    );
                    child.paint(window, cx);
                });
                return;
            }
        }
        #[cfg(feature = "gpui-fast")]
        let _ = (bounds, corners, opacity);
        child.paint(window, cx);
    }
}

#[cfg(all(test, not(feature = "gpui-fast")))]
mod tests {
    use super::*;
    use crate::{
        ActiveTheme as _, Theme, ThemeMode,
        menu::{PopupMenu, PopupMenuItem},
        notification::Notification,
    };
    use gpui::{AppContext as _, Context, Entity, Render, TestAppContext, div, px, rgb, size};

    struct Surfaces {
        menu: Entity<PopupMenu>,
        notification: Entity<Notification>,
        opacity: f32,
    }

    impl Render for Surfaces {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .bg(rgb(0x102030))
                .child(
                    div()
                        .absolute()
                        .left(px(35.))
                        .top(px(45.))
                        .opacity(self.opacity)
                        .child(self.menu.clone()),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(300.))
                        .top(px(220.))
                        .w(px(230.))
                        .opacity(self.opacity)
                        .child(self.notification.clone()),
                )
        }
    }

    #[gpui::test]
    fn menu_and_notification_backdrops_precede_their_tint_and_content(cx: &mut TestAppContext) {
        cx.update(|cx| {
            crate::init(cx);
            cx.set_reduce_motion(true);
        });
        let (view, cx) = cx.add_window_view(|_, cx| Surfaces {
            menu: cx.new(|cx| {
                PopupMenu::new(cx).item(PopupMenuItem::element(|_, _| {
                    div().w(px(20.)).h(px(12.)).bg(rgb(0xfa2184))
                }))
            }),
            notification: cx.new(|_| {
                Notification::new().message("notice").content(|_, _, _| {
                    div()
                        .w(px(20.))
                        .h(px(12.))
                        .bg(rgb(0xfa2184))
                        .into_any_element()
                })
            }),
            opacity: 1.,
        });
        cx.simulate_resize(size(px(800.), px(600.)));
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            assert!(window.painted_backdrop_blurs().is_empty());
            let opaque = Background::from(cx.theme().popover);
            assert!(
                window
                    .painted_quads()
                    .iter()
                    .filter(|quad| quad.background == opaque)
                    .count()
                    >= 2
            );
            cx.set_global(OverlayBackdrop::new(px(12.), 0.9));
        });

        for mode in [ThemeMode::Light, ThemeMode::Dark] {
            cx.update(|window, cx| {
                Theme::change(mode, None, cx);
                window.refresh();
                window.draw(cx).clear(cx);

                let blurs = window.painted_backdrop_blurs();
                let quads = window.painted_quads();
                assert_eq!(blurs.len(), 2);
                let parent = quads
                    .iter()
                    .find(|quad| quad.background == Background::from(rgb(0x102030)))
                    .expect("the window content was painted");
                let menu_tint = Background::from(cx.theme().popover).opacity(0.9);
                let notification_tint = Background::from(cx.theme().tokens.popover).opacity(0.9);
                for blur in blurs {
                    let tint = quads
                        .iter()
                        .find(|quad| {
                            quad.bounds == blur.bounds
                                && (quad.background == menu_tint
                                    || quad.background == notification_tint)
                        })
                        .expect("a theme tint matches each real surface");
                    let foreground = quads
                        .iter()
                        .find(|quad| {
                            quad.background == Background::from(rgb(0xfa2184))
                                && !quad.bounds.intersect(&blur.bounds).is_empty()
                        })
                        .expect("the surface's foreground was painted");
                    assert_eq!(blur.corner_radii, tint.corner_radii);
                    assert!(parent.order < blur.order);
                    assert!(blur.order < tint.order);
                    assert!(tint.order < foreground.order);
                    assert!(blur.bounds.size.width < parent.bounds.size.width);
                    assert!(blur.bounds.size.height < parent.bounds.size.height);
                }
            });
        }

        for opacity in [0., 0.5] {
            view.update_in(cx, |view, _, cx| {
                view.opacity = opacity;
                cx.notify();
            });
            cx.update(|window, cx| {
                window.draw(cx).clear(cx);
                let blurs = window.painted_backdrop_blurs();
                if opacity == 0. {
                    assert!(blurs.is_empty());
                } else {
                    assert_eq!(blurs.len(), 2);
                    for blur in blurs {
                        assert_eq!(blur.blur_radius, px(6.).scale(window.scale_factor()));
                    }
                }
            });
        }
    }
}
