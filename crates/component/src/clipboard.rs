use std::{rc::Rc, time::Duration};

use gpui::{
    App, ClipboardItem, ElementId, IntoElement, RenderOnce, SharedString, Window,
    prelude::FluentBuilder,
};
use rust_i18n::t;

use crate::{
    IconName, Sizable, Size,
    button::{Button, ButtonVariants as _},
};

/// An element that provides clipboard copy functionality.
#[derive(IntoElement)]
pub struct Clipboard {
    id: ElementId,
    value: SharedString,
    value_fn: Option<Rc<dyn Fn(&mut Window, &mut App) -> SharedString>>,
    on_copied: Option<Rc<dyn Fn(SharedString, &mut Window, &mut App)>>,
    tooltip_text: Option<SharedString>,
    tooltip_show_delay: Option<Duration>,
    accessibility_label: Option<SharedString>,
    size: Size,
}

impl Sizable for Clipboard {
    fn with_size(mut self, size: impl Into<Size>) -> Self {
        self.size = size.into();
        self
    }
}

impl Clipboard {
    /// Create a new Clipboard element with the given ID.
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            value: SharedString::default(),
            value_fn: None,
            on_copied: None,
            tooltip_text: None,
            tooltip_show_delay: None,
            accessibility_label: None,
            size: Size::XSmall,
        }
    }

    /// Set tooltip text for the clipboard button.
    pub fn tooltip(mut self, tooltip: impl Into<SharedString>) -> Self {
        self.tooltip_text = Some(tooltip.into());
        self
    }

    /// Overrides how long the pointer must rest before the tooltip shows.
    ///
    /// Defaults to [`gpui_base::TooltipDefaults::show_delay`].
    pub fn tooltip_show_delay(mut self, delay: Duration) -> Self {
        self.tooltip_show_delay = Some(delay);
        self
    }

    /// Set the name a screen reader announces. Defaults to the localized "Copy".
    ///
    /// The clipboard button shows only an icon, so it has no visible label to
    /// read, and its tooltip is a hint rather than a name.
    pub fn accessibility_label(mut self, label: impl Into<SharedString>) -> Self {
        self.accessibility_label = Some(label.into());
        self
    }

    /// Set the value for copying to the clipboard. Default is an empty string.
    pub fn value(mut self, value: impl Into<SharedString>) -> Self {
        self.value = value.into();
        self
    }

    /// Set the value of the clipboard to the result of the given function. Default is None.
    ///
    /// When used this, the copy value will use the result of the function.
    pub fn value_fn(
        mut self,
        value: impl Fn(&mut Window, &mut App) -> SharedString + 'static,
    ) -> Self {
        self.value_fn = Some(Rc::new(value));
        self
    }

    /// Set a callback to be invoked when the content is copied to the clipboard.
    pub fn on_copied<F>(mut self, handler: F) -> Self
    where
        F: Fn(SharedString, &mut Window, &mut App) + 'static,
    {
        self.on_copied = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for Clipboard {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = window.use_keyed_state(self.id.clone(), cx, |_, _| ClipboardState::default());

        let value = self.value.clone();
        let clipboard_id = self.id.clone();
        let copied = state.read(cx).copied;
        let value_fn = self.value_fn.clone();

        Button::new(clipboard_id)
            .icon(if copied {
                IconName::Check
            } else {
                IconName::Copy
            })
            .ghost()
            .with_size(self.size)
            .when_some(self.tooltip_text, |this, text| this.tooltip(text))
            .when_some(self.tooltip_show_delay, |this, delay| {
                this.tooltip_show_delay(delay)
            })
            .accessibility_label(
                self.accessibility_label
                    .unwrap_or_else(|| t!("Copy").into()),
            )
            .on_click({
                let state = state.clone();
                let on_copied = self.on_copied.clone();
                move |_, window, cx| {
                    cx.stop_propagation();
                    let value = value_fn
                        .as_ref()
                        .map(|f| f(window, cx))
                        .unwrap_or_else(|| value.clone());
                    cx.write_to_clipboard(ClipboardItem::new_string(value.to_string()));
                    let generation = state.update(cx, |state, cx| {
                        state.copied = true;
                        state.generation = state.generation.wrapping_add(1);
                        cx.notify();
                        state.generation
                    });

                    let state = state.clone();
                    cx.spawn(async move |cx| {
                        cx.background_executor().timer(Duration::from_secs(2)).await;
                        _ = state.update(cx, |state, cx| {
                            if state.generation == generation {
                                state.copied = false;
                                cx.notify();
                            }
                        });
                    })
                    .detach();

                    if let Some(on_copied) = &on_copied {
                        on_copied(value.clone(), window, cx);
                    }
                }
            })
    }
}

#[doc(hidden)]
#[derive(Default)]
struct ClipboardState {
    copied: bool,
    generation: u64,
}

#[cfg(all(test, feature = "test-support"))]
mod tests {
    use std::{cell::RefCell, rc::Rc};

    use gpui::{
        div, point, px, AppContext as _, Context, Modifiers, ParentElement as _,
        Render, Styled as _, TestAppContext, Window,
    };

    use super::Clipboard;

    struct CopyButtons(Rc<RefCell<Vec<String>>>);

    impl Render for CopyButtons {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
            let first = self.0.clone();
            let second = self.0.clone();
            div()
                .relative()
                .w(px(160.))
                .h(px(48.))
                .child(div().absolute().left(px(0.)).top(px(0.)).child(
                    Clipboard::new("copy-a").value("A").on_copied(move |value, _, _| {
                        first.borrow_mut().push(value.to_string());
                    }),
                ))
                .child(div().absolute().left(px(80.)).top(px(0.)).child(
                    Clipboard::new("copy-b").value("B").on_copied(move |value, _, _| {
                        second.borrow_mut().push(value.to_string());
                    }),
                ))
        }
    }

    #[gpui::test]
    fn copied_indicator_does_not_block_repeated_copy(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let copied = Rc::new(RefCell::new(Vec::new()));
        let (_, cx) = cx.add_window_view({
            let copied = copied.clone();
            move |_, _| CopyButtons(copied)
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        for (x, expected) in [(10., "A"), (90., "B"), (10., "A")] {
            cx.simulate_click(point(px(x), px(10.)), Modifiers::default());
            cx.update(|window, cx| {
                assert_eq!(cx.read_from_clipboard().and_then(|item| item.text()).as_deref(), Some(expected));
                window.draw(cx).clear(cx);
            });
        }
        assert_eq!(copied.borrow().as_slice(), &["A".to_owned(), "B".to_owned(), "A".to_owned()]);
    }
}
