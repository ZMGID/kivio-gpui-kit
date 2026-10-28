use crate::{
    AnyElement, App, Bounds, Context, EdgeFade, EdgeFadeParams, Element, ElementId, Entity,
    FocusHandle, GlobalElementId, InspectorElementId, IntoElement, LayoutId, Pixels, Render,
    StyleRefinement, TestAppContext, Window, div, point, prelude::*, px, rgb, size,
};
use std::{cell::Cell, rc::Rc};

struct FadeElement {
    child: AnyElement,
    fade: Option<EdgeFade>,
    prepaint_scope: bool,
    inherit_parent: bool,
    focus_during_paint: Option<(FocusHandle, Rc<Cell<bool>>)>,
}

impl IntoElement for FadeElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for FadeElement {
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
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        if self.prepaint_scope {
            window.with_edge_fade(self.fade, |window| {
                self.child.prepaint(window, cx);
            });
        } else {
            self.child.prepaint(window, cx);
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some((focus, trigger)) = &self.focus_during_paint
            && trigger.get()
        {
            window.focus(focus, cx);
        }
        let fade = if self.inherit_parent {
            let mut fade = window.edge_fade.unwrap_or_else(inactive_fade);
            fade.right = true;
            fade.band_right = Some(px(6.));
            Some(fade)
        } else {
            self.fade
        };
        window.with_edge_fade(fade, |window| self.child.paint(window, cx));
    }
}

fn inactive_fade() -> EdgeFade {
    EdgeFade {
        bounds: Bounds::new(point(px(0.), px(0.)), size(px(100.), px(100.))),
        band: px(10.),
        band_top: None,
        band_bottom: None,
        band_left: None,
        band_right: None,
        top: false,
        bottom: false,
        left: false,
        right: false,
    }
}

struct CachedChild {
    renders: Rc<Cell<usize>>,
}

impl Render for CachedChild {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get() + 1);
        div().size_full().bg(rgb(0xff0000)).child(FadeElement {
            child: div().size(px(30.)).bg(rgb(0x0000ff)).into_any_element(),
            fade: None,
            prepaint_scope: false,
            inherit_parent: true,
            focus_during_paint: None,
        })
    }
}

struct Root {
    child: Entity<CachedChild>,
    fade: Rc<Cell<Option<EdgeFade>>>,
    prepaint_scope: bool,
}

impl Render for Root {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        FadeElement {
            child: self
                .child
                .clone()
                .cached(StyleRefinement::default().size(px(100.)))
                .into_any_element(),
            fade: self.fade.get(),
            prepaint_scope: self.prepaint_scope,
            inherit_parent: false,
            focus_during_paint: None,
        }
    }
}

#[gpui::test]
fn cached_child_repaints_when_paint_only_fade_changes(cx: &mut TestAppContext) {
    exercise_fade_cache(cx, false);
}

#[gpui::test]
fn cached_child_repaints_when_prepaint_and_paint_fade_changes(cx: &mut TestAppContext) {
    exercise_fade_cache(cx, true);
}

fn exercise_fade_cache(cx: &mut TestAppContext, prepaint_scope: bool) {
    let fade = Rc::new(Cell::new(None));
    let renders = Rc::new(Cell::new(0));
    let window = cx.add_window({
        let fade = fade.clone();
        let renders = renders.clone();
        move |_, cx| Root {
            child: cx.new(|_| CachedChild { renders }),
            fade,
            prepaint_scope,
        }
    });
    let draw = |cx: &mut TestAppContext| -> (EdgeFadeParams, EdgeFadeParams, f32) {
        cx.update_window(window.into(), |_, window, cx| {
            window.draw(cx).clear(cx);
            let scale = window.scale_factor();
            let quads = window.painted_quads();
            let outer = quads
                .iter()
                .find(|q| q.bounds.size.width.0 > 75. * scale)
                .unwrap()
                .fade;
            let inner = quads
                .iter()
                .find(|q| q.bounds.size.width.0 < 50. * scale)
                .unwrap()
                .fade;
            (outer, inner, scale)
        })
        .unwrap()
    };

    let (outer, inner, scale) = draw(cx);
    assert_eq!(outer, EdgeFadeParams::default());
    assert_eq!(inner.band_top, 0.);
    assert_eq!(inner.band_right, 6. * scale);
    let initial_renders = renders.get();
    draw(cx);
    assert_eq!(
        renders.get(),
        initial_renders,
        "unchanged cached child must be reused"
    );

    let mut active = inactive_fade();
    active.top = true;
    active.band_top = Some(px(20.));
    fade.set(Some(active));
    let (outer, inner, scale) = draw(cx);
    assert_eq!(outer.band_top, 20. * scale);
    assert_eq!(
        inner.band_top,
        20. * scale,
        "nested paint must inherit the new outer fade"
    );
    assert_eq!(inner.band_right, 6. * scale);
    assert_eq!(renders.get(), initial_renders + 1);
    draw(cx);
    assert_eq!(
        renders.get(),
        initial_renders + 1,
        "unchanged active fade must reuse cache"
    );

    active.top = false;
    active.bottom = true;
    active.band_bottom = Some(px(12.));
    fade.set(Some(active));
    let (outer, inner, scale) = draw(cx);
    assert_eq!(outer.band_top, 0.);
    assert_eq!(outer.band_bottom, 12. * scale);
    assert_eq!(inner.band_bottom, 12. * scale);
    assert_eq!(renders.get(), initial_renders + 2);

    fade.set(None);
    let (outer, inner, scale) = draw(cx);
    assert_eq!(outer, EdgeFadeParams::default());
    assert_eq!(inner.band_bottom, 0.);
    assert_eq!(inner.band_right, 6. * scale);
    assert_eq!(renders.get(), initial_renders + 3);
}

struct ClipElement {
    child: AnyElement,
    radii: Option<crate::Corners<Pixels>>,
    prepaint_scope: bool,
    outer: Option<Bounds<Pixels>>,
}

impl IntoElement for ClipElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for ClipElement {
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
        if let Some(radii) = self.radii.filter(|_| self.prepaint_scope) {
            window.with_rounded_clip(bounds, radii, |window| self.child.prepaint(window, cx));
        } else {
            self.child.prepaint(window, cx);
        }
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
        let radii = self.radii;
        let outer = self.outer;
        let mut paint = |window: &mut Window| {
            if let Some(radii) = radii {
                window.with_rounded_clip(bounds, radii, |window| self.child.paint(window, cx));
            } else {
                self.child.paint(window, cx);
            }
        };
        if let Some(outer) = outer {
            window.with_rounded_clip(outer, px(6.), paint);
        } else {
            paint(window);
        }
    }
}

struct ClipRoot {
    child: Entity<CachedChild>,
    radii: Rc<Cell<Option<crate::Corners<Pixels>>>>,
    prepaint_scope: bool,
}

impl Render for ClipRoot {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        ClipElement {
            child: self
                .child
                .clone()
                .cached(StyleRefinement::default().size(px(100.)))
                .into_any_element(),
            radii: self.radii.get(),
            prepaint_scope: self.prepaint_scope,
            outer: None,
        }
    }
}

#[gpui::test]
fn cached_child_updates_when_prepaint_or_paint_only_clip_changes(cx: &mut TestAppContext) {
    for prepaint_scope in [false, true] {
        let radii = Rc::new(Cell::new(None));
        let renders = Rc::new(Cell::new(0));
        let window = cx.add_window({
            let radii = radii.clone();
            let renders = renders.clone();
            move |_, cx| ClipRoot {
                child: cx.new(|_| CachedChild { renders }),
                radii,
                prepaint_scope,
            }
        });
        let draw = |cx: &mut TestAppContext| {
            cx.update_window(window.into(), |_, window, cx| {
                window.draw(cx).clear(cx);
                let scale = window.scale_factor();
                let clip = window
                    .painted_quads()
                    .iter()
                    .find(|quad| quad.bounds.size.width.0 > 75. * scale)
                    .unwrap()
                    .fade
                    .clip;
                (clip, scale)
            })
            .unwrap()
        };

        let (unclipped, _) = draw(cx);
        assert_eq!(unclipped, crate::RoundedClipParams::default());
        let initial = renders.get();
        draw(cx);
        assert_eq!(renders.get(), initial);

        radii.set(Some(crate::Corners {
            top_left: px(16.),
            top_right: px(8.),
            bottom_right: px(0.),
            bottom_left: px(4.),
        }));
        let (clip, scale) = draw(cx);
        assert_eq!(clip.top_left, 16. * scale);
        assert_eq!(clip.top_right, 8. * scale);
        assert_eq!(clip.bottom_right, 0.);
        assert_eq!(clip.bottom_left, 4. * scale);
        assert_eq!(renders.get(), initial + 1);
        draw(cx);
        assert_eq!(
            renders.get(),
            initial + 1,
            "unchanged clip should reuse the view"
        );

        radii.set(Some(crate::Corners::all(px(24.))));
        let (clip, scale) = draw(cx);
        assert_eq!(clip.top_left, 24. * scale);
        assert_eq!(clip.bottom_right, 24. * scale);
        assert_eq!(
            renders.get(),
            initial + 2,
            "same bounds but new radii must repaint"
        );

        radii.set(None);
        let (clip, _) = draw(cx);
        assert_eq!(clip, crate::RoundedClipParams::default());
        assert_eq!(renders.get(), initial + 3);
    }
}

struct NestedClipRoot {
    child: Entity<CachedChild>,
    outer: Rc<Cell<Bounds<Pixels>>>,
}

impl Render for NestedClipRoot {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        ClipElement {
            child: self
                .child
                .clone()
                .cached(StyleRefinement::default().size(px(100.)))
                .into_any_element(),
            radii: Some(crate::Corners::all(px(16.))),
            prepaint_scope: false,
            outer: Some(self.outer.get()),
        }
    }
}

#[gpui::test]
fn cached_child_updates_when_outer_paint_only_clip_changes(cx: &mut TestAppContext) {
    let full = Bounds::new(point(px(0.), px(0.)), size(px(100.), px(100.)));
    let narrow = Bounds::new(point(px(20.), px(0.)), size(px(40.), px(100.)));
    let empty = Bounds::new(point(px(10.), px(0.)), size(px(0.), px(100.)));
    let outer = Rc::new(Cell::new(full));
    let renders = Rc::new(Cell::new(0));
    let window = cx.add_window({
        let outer = outer.clone();
        let renders = renders.clone();
        move |_, cx| NestedClipRoot {
            child: cx.new(|_| CachedChild { renders }),
            outer,
        }
    });
    let draw = |cx: &mut TestAppContext| {
        cx.update_window(window.into(), |_, window, cx| {
            window.draw(cx).clear(cx);
            let scale = window.scale_factor();
            let quad = window
                .painted_quads()
                .into_iter()
                .find(|quad| quad.bounds.size.width.0 > 75. * scale);
            (quad, scale)
        })
        .unwrap()
    };

    let (quad, scale) = draw(cx);
    let first = quad.unwrap();
    assert_eq!(first.content_mask.bounds.size.width.0, 100. * scale);
    assert_eq!(first.fade.clip.top_left, 16. * scale);
    let initial = renders.get();
    draw(cx);
    assert_eq!(renders.get(), initial);

    outer.set(narrow);
    let (quad, scale) = draw(cx);
    let narrowed = quad.unwrap();
    assert_eq!(narrowed.content_mask.bounds.origin.x.0, 20. * scale);
    assert_eq!(narrowed.content_mask.bounds.size.width.0, 40. * scale);
    assert_eq!(narrowed.fade.clip, first.fade.clip);
    assert_eq!(renders.get(), initial + 1);
    draw(cx);
    assert_eq!(renders.get(), initial + 1);

    outer.set(empty);
    assert!(draw(cx).0.is_none(), "an empty outer clip must hide the cached child");
    assert_eq!(renders.get(), initial + 2);

    outer.set(full);
    let (quad, _) = draw(cx);
    assert_eq!(quad.unwrap().content_mask, first.content_mask);
    assert_eq!(renders.get(), initial + 3);
}

#[cfg(feature = "profiler")]
type PhaseSamples = Rc<std::cell::RefCell<[Vec<std::time::Duration>; 3]>>;

#[cfg(feature = "profiler")]
fn record_phase_work(samples: &PhaseSamples, phase: usize) {
    let start = scheduler::Instant::now();
    std::thread::sleep(std::time::Duration::from_millis(2));
    samples.borrow_mut()[phase].push(scheduler::Instant::now().duration_since(start));
}

#[cfg(feature = "profiler")]
struct TimedElement {
    child: AnyElement,
    samples: PhaseSamples,
    retry_invalidation: Rc<Cell<bool>>,
}

#[cfg(feature = "profiler")]
impl IntoElement for TimedElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

#[cfg(feature = "profiler")]
impl Element for TimedElement {
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
        record_phase_work(&self.samples, 0);
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        record_phase_work(&self.samples, 1);
        self.child.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        record_phase_work(&self.samples, 2);
        if self.retry_invalidation.replace(false) {
            window.invalidator.set_dirty(true);
        }
        self.child.paint(window, cx);
    }
}

#[cfg(feature = "profiler")]
struct ProfilingRoot {
    child: Entity<CachedChild>,
    fade: Rc<Cell<Option<EdgeFade>>>,
    samples: PhaseSamples,
    retry_invalidation: Rc<Cell<bool>>,
}

#[cfg(feature = "profiler")]
impl Render for ProfilingRoot {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        TimedElement {
            child: FadeElement {
                child: self
                    .child
                    .clone()
                    .cached(StyleRefinement::default().size(px(100.)))
                    .into_any_element(),
                fade: self.fade.get(),
                prepaint_scope: false,
                inherit_parent: false,
                focus_during_paint: None,
            }
            .into_any_element(),
            samples: self.samples.clone(),
            retry_invalidation: self.retry_invalidation.clone(),
        }
    }
}

#[cfg(feature = "profiler")]
#[gpui::test]
fn paint_only_fade_retry_is_one_profiled_draw_with_all_phase_work(cx: &mut TestAppContext) {
    use crate::profiler::{
        self,
        journal::{ForegroundEvent, ForegroundJournalEntry, IntervalBoundary},
    };
    let (journal, _journal_guard) = profiler::journal::install_test_foreground_journal(128, 16);
    let _trace = profiler::trace_scope();
    let fade = Rc::new(Cell::new(None));
    let samples: PhaseSamples =
        Rc::new(std::cell::RefCell::new(std::array::from_fn(|_| Vec::new())));
    let retry_invalidation = Rc::new(Cell::new(false));
    let window = cx.add_window({
        let fade = fade.clone();
        let samples = samples.clone();
        let retry_invalidation = retry_invalidation.clone();
        move |_, cx| ProfilingRoot {
            child: cx.new(|_| CachedChild {
                renders: Rc::new(Cell::new(0)),
            }),
            fade,
            samples,
            retry_invalidation,
        }
    });

    cx.update_window(window.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
        window.present_if_needed();
    })
    .unwrap();
    for phase in samples.borrow_mut().iter_mut() {
        phase.clear();
    }
    let mut collector = journal.collector();
    let mut active = inactive_fade();
    active.top = true;
    fade.set(Some(active));
    retry_invalidation.set(true);
    let (before, after) = cx
        .update_window(window.into(), |_, window, cx| {
            let before: u64 = window
                .frame_duration_snapshot()
                .by_refresh_interval
                .iter()
                .map(|stats| stats.frames_drawn)
                .sum();
            window.invalidator.set_dirty(true);
            window.draw(cx).clear(cx);
            window.present_if_needed();
            let after: u64 = window
                .frame_duration_snapshot()
                .by_refresh_interval
                .iter()
                .map(|stats| stats.frames_drawn)
                .sum();
            (before, after)
        })
        .unwrap();
    assert_eq!(after - before, 1);
    let events: Vec<_> = collector
        .collect_unseen()
        .entries
        .into_iter()
        .filter_map(|entry| match entry {
            ForegroundJournalEntry::Event(
                event @ (ForegroundEvent::Draw(_) | ForegroundEvent::Present(_)),
            ) => Some(event),
            ForegroundJournalEntry::Boundary(IntervalBoundary::Presented(presented)) => {
                Some(ForegroundEvent::Present(presented.presentation))
            }
            _ => None,
        })
        .collect();
    assert_eq!(events.len(), 2, "one draw and one present must be recorded");
    let [
        ForegroundEvent::Draw(timing),
        ForegroundEvent::Present(present),
    ] = events.as_slice()
    else {
        panic!("expected draw followed by present: {events:?}");
    };
    assert!(timing.draw_end <= present.present_start);
    assert!(
        timing.invalidations >= 2,
        "retry invalidations belong to the same draw"
    );
    let samples = samples.borrow();
    assert!(
        samples.iter().all(|phase| phase.len() == 2),
        "the fade change must retry every phase"
    );
    let work = |phase: usize| samples[phase].iter().copied().sum::<std::time::Duration>();
    assert!(timing.phases.request_layout >= work(0));
    assert!(timing.phases.prepaint >= work(1));
    assert!(timing.phases.paint >= work(2));
    assert!(
        timing.draw_duration()
            >= timing.phases.request_layout + timing.phases.prepaint + timing.phases.paint
    );
}

struct InputRoot {
    child: Entity<crate::input::tests::ConfigurationTestView>,
    fade: Rc<Cell<Option<EdgeFade>>>,
    focus: FocusHandle,
    focus_during_paint: Rc<Cell<bool>>,
    passes: Rc<Cell<usize>>,
}

impl Render for InputRoot {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.passes.set(self.passes.get() + 1);
        FadeElement {
            child: self
                .child
                .clone()
                .cached(StyleRefinement::default().size(px(100.)))
                .into_any_element(),
            fade: self.fade.get(),
            prepaint_scope: false,
            inherit_parent: false,
            focus_during_paint: Some((self.focus.clone(), self.focus_during_paint.clone())),
        }
    }
}

#[gpui::test]
fn paint_only_fade_retry_publishes_focus_after_the_final_input_handler(cx: &mut TestAppContext) {
    use crate::{TextInputAction, TextInputConfiguration, TextInputStateChange};
    let fade = Rc::new(Cell::new(None));
    let trigger_focus = Rc::new(Cell::new(false));
    let passes = Rc::new(Cell::new(0));
    let focus_observations = Rc::new(std::cell::RefCell::new(Vec::new()));
    let configuration = TextInputConfiguration {
        autocorrect: true,
        input_action: TextInputAction::Send,
        ..Default::default()
    };
    let window = cx.add_window({
        let fade = fade.clone();
        let trigger_focus = trigger_focus.clone();
        let passes = passes.clone();
        let focus_observations = focus_observations.clone();
        let configuration = configuration.clone();
        move |window, cx| {
            let focus = cx.focus_handle();
            let focus_id = focus.id;
            cx.on_focus(&focus, window, move |_: &mut InputRoot, window, _| {
                let handler = window.platform_window.take_input_handler();
                focus_observations.borrow_mut().push(
                    handler.as_ref().and_then(|handler| handler.focus_id()) == Some(focus_id),
                );
                if let Some(handler) = handler {
                    window.platform_window.set_input_handler(handler);
                }
            })
            .detach();
            let child = cx.new({
                let focus = focus.clone();
                move |_| crate::input::tests::ConfigurationTestView {
                    focus_handle: focus,
                    configuration,
                    accepts_text_input: true,
                }
            });
            InputRoot {
                child,
                fade,
                focus,
                focus_during_paint: trigger_focus,
                passes,
            }
        }
    });
    let test_window = cx.test_window(window.into());
    test_window.simulate_active_status_change(true);
    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
        .unwrap();
    assert!(focus_observations.borrow().is_empty());
    assert!(test_window.text_input_state_changes().is_empty());
    assert_eq!(
        test_window.text_input_configurations(),
        vec![TextInputConfiguration::default()]
    );

    let initial_passes = passes.get();
    let mut active = inactive_fade();
    active.top = true;
    fade.set(Some(active));
    trigger_focus.set(true);
    cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
        .unwrap();
    assert_eq!(
        passes.get(),
        initial_passes + 2,
        "the fade transition must retry"
    );
    assert_eq!(focus_observations.borrow().as_slice(), &[true]);
    assert_eq!(
        test_window.text_input_state_changes(),
        vec![TextInputStateChange::FocusGained]
    );
    assert_eq!(
        test_window.text_input_configurations(),
        vec![TextInputConfiguration::default(), configuration]
    );

    trigger_focus.set(false);
    fade.set(None);
    cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
        .unwrap();
    assert_eq!(focus_observations.borrow().as_slice(), &[true]);
    assert_eq!(
        test_window.text_input_state_changes(),
        vec![TextInputStateChange::FocusGained]
    );
    assert_eq!(test_window.text_input_configurations().len(), 2);
}
