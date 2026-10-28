use gpui::{
    AnyElement, App, ClickEvent, IntoElement, ParentElement, Pixels, RenderOnce, SharedString,
    StyleRefinement, Styled, Window, prelude::FluentBuilder as _,
};

use crate::{
    StyledExt as _, WindowExt as _,
    button::ButtonVariant,
    dialog::{
        Dialog, DialogButtonProps, DialogDescription, DialogEntrance, DialogFooter, DialogHeader,
        DialogTitle,
    },
    h_flex, v_flex,
};

/// AlertDialog is a modal dialog that interrupts the user with important content
/// and expects a response.
///
/// It is built on top of the Dialog component with opinionated defaults:
/// - Footer buttons are center-aligned (vs right-aligned in Dialog)
/// - Icon is optional (disabled by default, enable with `.show_icon(true)`)
/// - Simplified API for common alert scenarios
/// - Uses declarative DialogHeader, DialogTitle, DialogDescription, and DialogFooter components
/// - Supports both imperative and declarative API styles
///
/// # Examples
///
/// ## Imperative API (using WindowExt)
///
/// ```ignore
/// use gpui_kit::component::{AlertDialog, alert::AlertVariant};
///
/// // Using WindowExt trait
/// window.open_alert_dialog(cx, |alert, _, _| {
///     alert
///         .title("Unsaved Changes")
///         .description("You have unsaved changes. Are you sure you want to leave?")
///         .show_cancel(true)
/// });
/// ```
///
/// ## Declarative API (using trigger and content)
///
/// ```ignore
/// use gpui_kit::component::{AlertDialog, DialogHeader, DialogTitle, DialogDescription, DialogFooter};
///
/// AlertDialog::new(cx)
///     .trigger(Button::new("delete").label("Delete"))
///     .content(|content, _, cx| {
///         content
///             .child(
///                 DialogHeader::new()
///                     .items_center()
///                     .child(DialogTitle::new().child("Delete File"))
///                     .child(DialogDescription::new().child("Are you sure?"))
///             )
///             .child(
///                 DialogFooter::new()
///                     .justify_center()
///                     .child(Button::new("cancel").label("Cancel"))
///                     .child(Button::new("confirm").label("Delete"))
///             )
///     })
/// ```
#[derive(IntoElement)]
pub struct AlertDialog {
    base: Dialog,
    trigger: Option<AnyElement>,
    icon: Option<AnyElement>,
    title: Option<AnyElement>,
    description: Option<AnyElement>,
    children: Vec<AnyElement>,
}

impl AlertDialog {
    /// Create a new AlertDialog.
    ///
    /// By default, the dialog is not overlay closable with a OK button.
    ///
    pub fn new(cx: &mut App) -> Self {
        Self {
            base: Dialog::new(cx)
                .with_base_alert_dialog(gpui_base::AlertDialog::new(cx))
                .close_button(false),
            trigger: None,
            icon: None,
            title: None,
            description: None,
            children: Vec::new(),
        }
    }

    /// Set to use confirm dialog, with OK and Cancel buttons.
    ///
    /// The default of [`AlertDialog`] has OK button.
    pub fn confirm(mut self) -> Self {
        self.base.button_props.show_cancel = Some(true);
        self
    }

    /// Sets the trigger element for the alert dialog.
    ///
    /// When a trigger is set, the dialog will render as a trigger element that opens the dialog when clicked.
    ///
    /// **Note**: When using `.trigger()`, you should also use `.content()` to define the dialog content
    /// declaratively instead of using `.title()`, `.description()`, etc.
    ///
    /// The `title`, `description`, `icon`, and `button_props` will be ignored when used together with `.trigger()`.
    pub fn trigger(mut self, trigger: impl IntoElement) -> Self {
        self.trigger = Some(trigger.into_any_element());
        self
    }

    /// Sets the content builder for declarative API.
    ///
    /// When using this method, you define the dialog content using declarative components like
    /// `DialogHeader`, `DialogTitle`, `DialogDescription`, and `DialogFooter`.
    ///
    /// This method is typically used together with `.trigger()` for a fully declarative API.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// AlertDialog::new(cx)
    ///     .trigger(Button::new("delete").label("Delete"))
    ///     .content(|content, _, cx| {
    ///         content
    ///             .child(DialogHeader::new().child(DialogTitle::new().child("Confirm")))
    ///             .child(DialogFooter::new().child(Button::new("ok").label("OK")))
    ///     })
    /// ```
    pub fn content<F>(mut self, builder: F) -> Self
    where
        F: Fn(crate::dialog::DialogContent, &mut Window, &mut App) -> crate::dialog::DialogContent
            + 'static,
    {
        self.base = self.base.content(builder);
        self
    }

    /// Sets the footer builder for declarative API.
    ///
    /// This is used to define the footer content using declarative components like `DialogFooter`.
    ///
    /// If not set, a default footer with OK and optional Cancel button will be used.
    pub fn footer(mut self, footer: impl IntoElement) -> Self {
        self.base = self.base.footer(footer);
        self
    }

    #[track_caller]
    fn debug_assert_no_trigger(&self) {
        debug_assert!(
            self.trigger.is_none() && self.base.content_builder.is_none(),
            "Cannot set this property when trigger is used. Use content() to define dialog content instead."
        );
    }

    /// Sets the icon of the alert dialog, default is None.
    #[track_caller]
    pub fn icon(mut self, icon: impl IntoElement) -> Self {
        self.debug_assert_no_trigger();
        self.icon = Some(icon.into_any_element());
        self
    }

    /// Sets the title of the alert dialog.
    #[track_caller]
    pub fn title(mut self, title: impl IntoElement) -> Self {
        self.debug_assert_no_trigger();
        self.title = Some(title.into_any_element());
        self
    }

    /// Sets the description of the alert dialog.
    #[track_caller]
    pub fn description(mut self, description: impl IntoElement) -> Self {
        self.debug_assert_no_trigger();
        self.description = Some(description.into_any_element());
        self
    }

    /// Set the button props of the alert dialog.
    ///
    /// Use this to configure button text, variants, and visibility in one
    /// value. It overrides only the fields `button_props` sets, so the Cancel
    /// button [`Self::confirm`] asked for and callbacks set earlier survive,
    /// whatever the call order. For a single property, prefer the direct
    /// builders — [`Self::ok_text`], [`Self::ok_variant`],
    /// [`Self::cancel_text`], [`Self::cancel_variant`].
    ///
    /// # Examples
    ///
    /// ```ignore
    /// alert.confirm().button_props(
    ///     DialogButtonProps::default()
    ///         .ok_text("Delete")
    ///         .ok_variant(ButtonVariant::Danger)
    ///         .cancel_text("Keep")
    /// )
    /// ```
    #[track_caller]
    pub fn button_props(mut self, button_props: DialogButtonProps) -> Self {
        self.debug_assert_no_trigger();
        self.base = self.base.button_props(button_props);
        self
    }

    /// Sets the text of the OK button. Default is `OK`.
    pub fn ok_text(mut self, ok_text: impl Into<SharedString>) -> Self {
        self.base.button_props.ok_text = Some(ok_text.into());
        self
    }

    /// Sets the variant of the OK button. Default is `ButtonVariant::Primary`.
    pub fn ok_variant(mut self, ok_variant: ButtonVariant) -> Self {
        self.base.button_props.ok_variant = Some(ok_variant);
        self
    }

    /// Sets the text of the Cancel button. Default is `Cancel`.
    pub fn cancel_text(mut self, cancel_text: impl Into<SharedString>) -> Self {
        self.base.button_props.cancel_text = Some(cancel_text.into());
        self
    }

    /// Sets the variant of the Cancel button. Default is `ButtonVariant::default()`.
    pub fn cancel_variant(mut self, cancel_variant: ButtonVariant) -> Self {
        self.base.button_props.cancel_variant = Some(cancel_variant);
        self
    }

    /// Sets the width of the alert dialog, defaults to 420px.
    pub fn width(mut self, width: impl Into<Pixels>) -> Self {
        self.base = self.base.width(width);
        self
    }

    /// Show cancel button. Default is false.
    pub fn show_cancel(mut self, show_cancel: bool) -> Self {
        self.base.button_props.show_cancel = Some(show_cancel);
        self
    }

    /// Alert dialogs never close from a backdrop press.
    #[deprecated(note = "AlertDialog backdrop dismissal is disabled by design")]
    pub fn overlay_closable(self, _: bool) -> Self {
        self
    }

    /// Set the close button of the alert dialog, defaults to `false`.
    pub fn close_button(mut self, close_button: bool) -> Self {
        self.base = self.base.close_button(close_button);
        self
    }

    /// Set whether to support keyboard esc to close the dialog, defaults to `true`.
    pub fn keyboard(mut self, keyboard: bool) -> Self {
        self.base = self.base.keyboard(keyboard);
        self
    }

    /// Sets the top offset of the alert dialog.
    ///
    /// Without this, the surface keeps the [`Dialog`] default: a tenth of the
    /// viewport height below the top edge, which pins short alerts near the
    /// top of a tall window. Pass a larger offset to seat an alert further
    /// down, e.g. in the vertical middle of the window.
    pub fn margin_top(mut self, margin_top: impl Into<Pixels>) -> Self {
        self.base = self.base.margin_top(margin_top);
        self
    }

    /// Centers the alert dialog vertically in the window, whatever its
    /// height, in place of the [`Self::margin_top`] offset.
    pub fn centered(mut self) -> Self {
        self.base = self.base.centered();
        self
    }

    /// Sets how the alert dialog enters, defaulting to [`DialogEntrance::SlideDown`].
    ///
    /// Shares the surface, backdrop, and reduced-motion policy of [`Dialog::entrance`].
    pub fn entrance(mut self, entrance: DialogEntrance) -> Self {
        self.base = self.base.entrance(entrance);
        self
    }

    /// Sets the callback for when the alert dialog is closed.
    ///
    /// Called after [`Self::on_action`] or [`Self::on_cancel`] callback.
    pub fn on_close(
        mut self,
        on_close: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.base = self.base.on_close(on_close);
        self
    }

    /// Sets the callback for when the OK/action button is clicked.
    ///
    /// The callback should return `true` to close the dialog, if return `false` the dialog will not be closed.
    pub fn on_ok(
        mut self,
        on_ok: impl Fn(&ClickEvent, &mut Window, &mut App) -> bool + 'static,
    ) -> Self {
        self.base = self.base.on_ok(on_ok);
        self
    }

    /// Sets the callback for when the alert dialog has been canceled.
    ///
    /// The callback should return `true` to close the dialog, if return `false` the dialog will not be closed.
    pub fn on_cancel(
        mut self,
        on_cancel: impl Fn(&ClickEvent, &mut Window, &mut App) -> bool + 'static,
    ) -> Self {
        self.base = self.base.on_cancel(on_cancel);
        self
    }

    /// Build the styled dialog surface around the Base alert-dialog host.
    pub(crate) fn build_surface(self, window: &mut Window, cx: &mut App) -> Dialog {
        let button_props = self.base.button_props.clone();
        let has_title = self.icon.is_some() || self.title.is_some();
        let has_header = has_title || self.description.is_some();
        let has_footer = self.base.footer.is_some();

        self.base
            .when(has_header, |this| {
                this.header(
                    DialogHeader::new().child(
                        h_flex()
                            .gap_2()
                            .items_start()
                            .when_some(self.icon, |row, icon| row.child(icon))
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap_1()
                                    .when_some(self.title, |this, title| {
                                        this.child(DialogTitle::new().child(title))
                                    })
                                    .when_some(self.description, |this, desc| {
                                        this.child(DialogDescription::new().child(desc))
                                    }),
                            ),
                    ),
                )
            })
            .children(self.children)
            .when(!has_footer, |this| {
                // Default footer for AlertDialog if user doesn't provide one, with OK and optional Cancel button
                this.footer(
                    DialogFooter::new()
                        .when(button_props.is_cancel_shown(), |this| {
                            this.child(button_props.render_cancel(window, cx))
                        })
                        .child(button_props.render_ok(window, cx)),
                )
            })
    }
}

impl Styled for AlertDialog {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.base.style
    }
}

impl ParentElement for AlertDialog {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl AlertDialog {
    fn render_trigger(self, trigger: AnyElement, _: &mut Window, _: &mut App) -> AnyElement {
        let content_builder = self.base.content_builder.clone();
        let style = self.base.style.clone();
        let props = self.base.props.clone();
        let button_props = self.base.button_props.clone();

        gpui_base::AlertDialogTrigger::new(trigger)
            .on_open(move |window, cx| {
                let content_builder = content_builder.clone();
                let style = style.clone();
                let props = props.clone();
                let button_props = button_props.clone();
                window.open_dialog(cx, move |dialog, _, cx| {
                    dialog
                        .with_base_alert_dialog(gpui_base::AlertDialog::new(cx))
                        .refine_style(&style)
                        .button_props(button_props.clone())
                        .with_props(props.clone())
                        .when_some(content_builder.clone(), |this, content_builder| {
                            this.content(move |content, window, cx| {
                                content_builder(content, window, cx)
                            })
                        })
                });
            })
            .into_any_element()
    }
}

impl RenderOnce for AlertDialog {
    fn render(mut self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        if let Some(trigger) = self.trigger.take() {
            // If a trigger is provided, render the trigger element that opens the dialog
            self.render_trigger(trigger, window, cx)
        } else {
            // Otherwise, render the dialog content directly
            self.build_surface(window, cx).into_any_element()
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, rc::Rc};

    use gpui::{ClickEvent, TestAppContext, px, size};

    use super::*;
    use crate::dialog::dialog::tests::window;

    /// `button_props` overrides only the fields it sets, so the Cancel button
    /// `confirm` asked for survives a later props value that never mentions it.
    #[gpui::test]
    fn button_props_after_confirm_keeps_the_cancel_button(cx: &mut TestAppContext) {
        let cx = window(cx, size(px(800.), px(600.)));
        cx.update(|_, cx| {
            let alert = AlertDialog::new(cx)
                .confirm()
                .button_props(DialogButtonProps::default().ok_text("Delete"));

            assert!(alert.base.button_props.is_cancel_shown());
            assert_eq!(alert.base.button_props.ok_text.as_deref(), Some("Delete"));
        });
    }

    /// The call order does not matter either way round.
    #[gpui::test]
    fn confirm_after_button_props_keeps_the_ok_text(cx: &mut TestAppContext) {
        let cx = window(cx, size(px(800.), px(600.)));
        cx.update(|_, cx| {
            let alert = AlertDialog::new(cx)
                .button_props(DialogButtonProps::default().ok_text("Delete"))
                .confirm();

            assert!(alert.base.button_props.is_cancel_shown());
            assert_eq!(alert.base.button_props.ok_text.as_deref(), Some("Delete"));
        });
    }

    /// A callback installed with `on_ok` survives a later props value that
    /// carries no callback of its own.
    #[gpui::test]
    fn button_props_after_on_ok_keeps_the_callback(cx: &mut TestAppContext) {
        let cx = window(cx, size(px(800.), px(600.)));
        let confirmed = Rc::new(Cell::new(0));
        let counter = confirmed.clone();
        cx.update(|window, cx| {
            let alert = AlertDialog::new(cx)
                .on_ok(move |_, _, _| {
                    counter.set(counter.get() + 1);
                    true
                })
                .button_props(DialogButtonProps::default().ok_text("Delete"));

            assert!(alert.base.button_props.ok_handler()(
                &ClickEvent::default(),
                window,
                cx
            ));
        });

        assert_eq!(confirmed.get(), 1);
    }

    /// The direct builders resolve to the same buttons as a props value.
    #[gpui::test]
    fn the_direct_builders_match_button_props(cx: &mut TestAppContext) {
        let cx = window(cx, size(px(800.), px(600.)));
        cx.update(|_, cx| {
            let direct = AlertDialog::new(cx)
                .confirm()
                .ok_text("Delete")
                .ok_variant(ButtonVariant::Danger)
                .cancel_text("Keep")
                .cancel_variant(ButtonVariant::Ghost);
            let bundled = AlertDialog::new(cx).button_props(
                DialogButtonProps::default()
                    .show_cancel(true)
                    .ok_text("Delete")
                    .ok_variant(ButtonVariant::Danger)
                    .cancel_text("Keep")
                    .cancel_variant(ButtonVariant::Ghost),
            );

            for props in [&direct.base.button_props, &bundled.base.button_props] {
                assert!(props.is_cancel_shown());
                assert_eq!(props.ok_text.as_deref(), Some("Delete"));
                assert_eq!(props.ok_variant, Some(ButtonVariant::Danger));
                assert_eq!(props.cancel_text.as_deref(), Some("Keep"));
                assert_eq!(props.cancel_variant, Some(ButtonVariant::Ghost));
            }
        });
    }

    /// `margin_top` overrides the tenth-of-the-viewport default offset, which
    /// seats a short alert near the top of a tall window.
    #[gpui::test]
    fn margin_top_moves_the_alert_surface_off_the_default_offset(cx: &mut TestAppContext) {
        let cx = window(cx, size(px(800.), px(600.)));
        cx.update(|window, cx| {
            window.open_alert_dialog(cx, |alert, _, _| {
                alert
                    .title("Unsaved changes")
                    .description("Discard the draft?")
                    .confirm()
                    .margin_top(px(100.))
            })
        });
        cx.run_until_parked();
        // One frame mounts the layer, the next paints it at rest.
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));

        let bounds = cx
            .debug_bounds("dialog-0")
            .expect("the alert dialog surface was not painted");
        // The alert chrome carries fractional text metrics (this surface is
        // 128.5px tall), so the painted origin lands within a subpixel of the
        // requested offset. The default offset would be a tenth of the
        // viewport: 60px.
        assert!(
            (bounds.origin.y - px(100.)).abs() <= px(0.5),
            "expected the surface within 0.5px of y=100, got {bounds:?}"
        );
    }

    #[cfg(not(feature = "gpui-fast"))]
    #[gpui::test]
    fn backdrop_blur_tracks_the_alert_surface_and_escape_still_cancels(cx: &mut TestAppContext) {
        use crate::{ActiveTheme as _, OverlayBackdrop, Theme, ThemeMode};
        use gpui::{Background, div, rgb};

        let cx = window(cx, size(px(800.), px(600.)));
        let cancelled = Rc::new(Cell::new(false));
        cx.update(|window, cx| {
            cx.set_global(OverlayBackdrop::new(px(12.), 0.9));
            let cancelled = cancelled.clone();
            window.open_alert_dialog(cx, move |alert, _, _| {
                let cancelled = cancelled.clone();
                alert
                    .title("Unsaved changes")
                    .description("Discard the draft?")
                    .confirm()
                    .child(div().w(px(30.)).h(px(12.)).bg(rgb(0xfa2184)))
                    .on_cancel(move |_, _, _| {
                        cancelled.set(true);
                        true
                    })
            });
        });
        cx.run_until_parked();

        for (mode, dimensions) in [
            (ThemeMode::Light, size(px(800.), px(600.))),
            (ThemeMode::Dark, size(px(560.), px(400.))),
        ] {
            cx.simulate_resize(dimensions);
            cx.update(|window, cx| {
                Theme::change(mode, None, cx);
                window.draw(cx).clear(cx);
                window.draw(cx).clear(cx);

                let blurs = window.painted_backdrop_blurs();
                assert_eq!(blurs.len(), 1);
                let blur = &blurs[0];
                let quads = window.painted_quads();
                let tint = Background::from(cx.theme().tokens.background).opacity(0.9);
                let surface = quads
                    .iter()
                    .find(|quad| quad.bounds == blur.bounds && quad.background == tint)
                    .expect("the blur follows the dialog panel, not its modal root");
                let foreground = quads
                    .iter()
                    .find(|quad| quad.background == Background::from(rgb(0xfa2184)))
                    .expect("the dialog content was painted");
                assert_eq!(blur.corner_radii, surface.corner_radii);
                assert!(blur.order < surface.order);
                assert!(surface.order < foreground.order);
                assert!(blur.bounds.size.width < dimensions.width.scale(window.scale_factor()));
                assert!(blur.bounds.size.height < dimensions.height.scale(window.scale_factor()));
            });
        }
        cx.simulate_keystrokes("escape");
        assert!(cancelled.get());
        cx.update(|window, cx| assert!(!window.has_active_dialog(cx)));
    }

    #[cfg(not(feature = "gpui-fast"))]
    #[gpui::test]
    fn stacked_backdrops_keep_paint_order_and_escape_on_the_top_dialog(cx: &mut TestAppContext) {
        use crate::{ActiveTheme as _, OverlayBackdrop, root::WindowState};
        use gpui::Background;

        let cx = window(cx, size(px(800.), px(600.)));
        let cancelled = Rc::new(Cell::new(0));
        cx.update(|window, cx| {
            cx.set_global(OverlayBackdrop::new(px(12.), 0.9));
            for layer in 1..=2 {
                let cancelled = cancelled.clone();
                window.open_alert_dialog(cx, move |alert, _, _| {
                    let cancelled = cancelled.clone();
                    alert
                        .title(format!("Dialog {layer}"))
                        .confirm()
                        .on_cancel(move |_, _, _| {
                            cancelled.set(layer);
                            true
                        })
                });
            }
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            window.draw(cx).clear(cx);
            let blurs = window.painted_backdrop_blurs();
            assert_eq!(blurs.len(), 2);
            let tint = Background::from(cx.theme().tokens.background).opacity(0.9);
            let quads = window.painted_quads();
            let first = quads
                .iter()
                .find(|quad| quad.bounds == blurs[0].bounds && quad.background == tint)
                .unwrap();
            let second = quads
                .iter()
                .find(|quad| quad.bounds == blurs[1].bounds && quad.background == tint)
                .unwrap();
            assert!(blurs[0].order < first.order);
            assert!(first.order < blurs[1].order);
            assert!(blurs[1].order < second.order);
            assert!(
                WindowState::read(window, cx).active_dialogs[1]
                    .focus_handle()
                    .is_focused(window)
            );
        });
        cx.simulate_keystrokes("escape");
        assert_eq!(cancelled.get(), 2);
        cx.update(|window, cx| {
            let state = WindowState::read(window, cx);
            assert_eq!(state.active_dialogs.len(), 1);
            assert!(state.active_dialogs[0].focus_handle().is_focused(window));
        });
        cx.simulate_keystrokes("escape");
        assert_eq!(cancelled.get(), 1);
        cx.update(|window, cx| assert!(!window.has_active_dialog(cx)));
    }

    /// `centered` seats the surface in the vertical middle of the window,
    /// whatever its height.
    #[gpui::test]
    fn centered_seats_the_alert_surface_in_the_middle(cx: &mut TestAppContext) {
        let cx = window(cx, size(px(800.), px(600.)));
        cx.update(|window, cx| {
            window.open_alert_dialog(cx, |alert, _, _| {
                alert
                    .title("Unsaved changes")
                    .description("Discard the draft?")
                    .confirm()
                    .centered()
            })
        });
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));

        let bounds = cx
            .debug_bounds("dialog-0")
            .expect("the alert dialog surface was not painted");
        assert!(
            (bounds.center().y - px(300.)).abs() <= px(0.5),
            "expected the surface centered on y=300, got {bounds:?}"
        );
    }
}
