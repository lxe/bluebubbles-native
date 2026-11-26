use adw::prelude::*;
use adw::subclass::prelude::ObjectSubclassIsExt;
use gtk::{glib, gsk};

mod imp {
    use std::cell::{Cell, RefCell};

    use adw::prelude::*;
    use adw::subclass::prelude::*;
    use gtk::{gdk, glib, gsk};

    #[derive(Default)]
    pub struct Squircle {
        path: RefCell<Option<(i32, i32, gsk::Path)>>,
        pub(super) shimmer: RefCell<Option<adw::TimedAnimation>>,
        pub(super) shimmer_fade: RefCell<Option<adw::TimedAnimation>>,
        pub(super) settle_timer: RefCell<Option<glib::SourceId>>,
        pub(super) progress: Cell<f32>,
        pub(super) shimmer_alpha: Cell<f32>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Squircle {
        const NAME: &'static str = "BlueBubblesSquircle";
        type Type = super::Squircle;
        type ParentType = adw::Bin;

        fn class_init(class: &mut Self::Class) {
            class.set_css_name("bb-squircle");
        }
    }

    impl ObjectImpl for Squircle {}
    impl BinImpl for Squircle {}

    impl WidgetImpl for Squircle {
        fn map(&self) {
            self.parent_map();
            self.obj().start_shimmer();
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let widget = self.obj();
            let (width, height) = (widget.width(), widget.height());
            let color = widget.color();
            let clip = widget.has_css_class("media-contour");
            if width > 0 && height > 0 && (color.alpha() > 0.0 || clip) {
                let mut cache = self.path.borrow_mut();
                if cache
                    .as_ref()
                    .is_none_or(|(w, h, _)| *w != width || *h != height)
                {
                    *cache = Some((width, height, super::outline(width as f32, height as f32)));
                }
                let path = &cache.as_ref().unwrap().2;
                let shadow = clip || widget.has_css_class("thread-bubble");
                if shadow {
                    snapshot.push_shadow(&[gsk::Shadow::new(
                        gdk::RGBA::new(0.01, 0.025, 0.04, 0.23),
                        0.0,
                        2.0,
                        3.0,
                    )]);
                }
                if color.alpha() > 0.0 {
                    snapshot.append_fill(path, gsk::FillRule::Winding, &color);
                }
                if shadow {
                    snapshot.pop();
                }
                let progress = self.progress.get();
                let shimmer_alpha = self.shimmer_alpha.get();
                if shimmer_alpha > 0.001 && 0.0 < progress && progress < 1.0 {
                    let width = width as f32;
                    let height = height as f32;
                    let band = (width * 0.35).clamp(32.0, 92.0);
                    let x = (width + band * 2.0) * progress - band;
                    let clear = gdk::RGBA::new(0.70, 0.86, 1.0, 0.0);
                    let highlight = gdk::RGBA::new(0.70, 0.86, 1.0, 0.14 * shimmer_alpha);
                    snapshot.push_fill(path, gsk::FillRule::Winding);
                    snapshot.append_linear_gradient(
                        &gtk::graphene::Rect::new(0.0, 0.0, width, height),
                        &gtk::graphene::Point::new(x - band, 0.0),
                        &gtk::graphene::Point::new(x + band, height * 0.6),
                        &[
                            gsk::ColorStop::new(0.0, clear),
                            gsk::ColorStop::new(0.5, highlight),
                            gsk::ColorStop::new(1.0, clear),
                        ],
                    );
                    snapshot.pop();
                }
                if clip {
                    snapshot.push_fill(path, gsk::FillRule::Winding);
                    self.parent_snapshot(snapshot);
                    snapshot.pop();
                    return;
                }
            }
            self.parent_snapshot(snapshot);
        }
    }
}

glib::wrapper! {
    pub struct Squircle(ObjectSubclass<imp::Squircle>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Squircle {
    pub fn new(child: &impl IsA<gtk::Widget>, class: &str) -> Self {
        let widget: Self = glib::Object::new();
        widget.add_css_class(class);
        widget.set_child(Some(child));
        widget
    }

    pub fn set_pending(&self, pending: bool) {
        let was_pending = self.has_css_class("pending-bubble");
        if pending {
            self.cancel_settle();
            self.add_css_class("pending-bubble");
            self.imp().shimmer_alpha.set(1.0);
            if self.is_mapped() {
                self.start_shimmer();
            }
            return;
        }
        if was_pending && self.is_mapped() && crate::loading::animations_enabled() {
            self.add_css_class("settling-bubble");
            self.remove_css_class("pending-bubble");
            self.fade_out_shimmer();
            if let Some(source) = self.imp().settle_timer.borrow_mut().take() {
                source.remove();
            }
            let weak = self.downgrade();
            let source =
                glib::timeout_add_local_once(std::time::Duration::from_millis(260), move || {
                    if let Some(widget) = weak.upgrade() {
                        widget.imp().settle_timer.borrow_mut().take();
                        widget.remove_css_class("settling-bubble");
                    }
                });
            self.imp().settle_timer.replace(Some(source));
        } else {
            self.remove_css_class("pending-bubble");
            self.stop_shimmer_immediately();
        }
    }

    pub fn reset_transitions(&self) {
        self.cancel_settle();
        self.stop_shimmer_immediately();
    }

    fn cancel_settle(&self) {
        if let Some(source) = self.imp().settle_timer.borrow_mut().take() {
            source.remove();
        }
        if let Some(fade) = self.imp().shimmer_fade.take() {
            fade.pause();
        }
        self.remove_css_class("settling-bubble");
    }

    fn stop_shimmer_immediately(&self) {
        if let Some(shimmer) = self.imp().shimmer.take() {
            shimmer.pause();
        }
        if let Some(fade) = self.imp().shimmer_fade.take() {
            fade.pause();
        }
        self.imp().progress.set(0.0);
        self.imp().shimmer_alpha.set(0.0);
    }

    fn fade_out_shimmer(&self) {
        if let Some(fade) = self.imp().shimmer_fade.take() {
            fade.pause();
        }
        let progress = self.imp().progress.get();
        let alpha = self.imp().shimmer_alpha.get();
        if alpha <= 0.001 || !(0.0 < progress && progress < 1.0) {
            self.stop_shimmer_immediately();
            return;
        }
        let widget = self.downgrade();
        let target = adw::CallbackAnimationTarget::new(move |value| {
            if let Some(widget) = widget.upgrade() {
                widget.imp().shimmer_alpha.set(value as f32);
                widget.queue_draw();
            }
        });
        let fade = adw::TimedAnimation::new(self, f64::from(alpha), 0.0, 180, target);
        fade.set_easing(adw::Easing::EaseOutCubic);
        let weak = self.downgrade();
        fade.connect_done(move |_| {
            if let Some(widget) = weak.upgrade()
                && !widget.has_css_class("pending-bubble")
            {
                widget.stop_shimmer_immediately();
                widget.queue_draw();
            }
        });
        self.imp().shimmer_fade.replace(Some(fade.clone()));
        fade.play();
    }

    fn start_shimmer(&self) {
        if !self.has_css_class("pending-bubble")
            || self.imp().shimmer.borrow().is_some()
            || !crate::loading::animations_enabled()
        {
            return;
        }
        self.imp().shimmer_alpha.set(1.0);
        let widget = self.downgrade();
        let target = adw::CallbackAnimationTarget::new(move |value| {
            if let Some(widget) = widget.upgrade() {
                widget.imp().progress.set(value as f32);
                widget.queue_draw();
            }
        });
        let shimmer = adw::TimedAnimation::new(self, 0.0, 1.0, 1550, target);
        shimmer.set_easing(adw::Easing::Linear);
        shimmer.set_repeat_count(2);
        self.imp().shimmer.replace(Some(shimmer.clone()));
        shimmer.play();
    }
}

fn outline(width: f32, height: f32) -> gsk::Path {
    let r = 19.0_f32.min(width * 0.48).min(height * 0.48);
    let builder = gsk::PathBuilder::new();
    builder.move_to(r, 0.0);
    for (origin, tangent, normal) in [
        ((width - r, 0.0), (r, 0.0), (0.0, r)),
        ((width, height - r), (0.0, r), (-r, 0.0)),
        ((r, height), (-r, 0.0), (0.0, -r)),
        ((0.0, r), (0.0, -r), (r, 0.0)),
    ] {
        builder.line_to(origin.0, origin.1);
        // A pair of mirrored cubics keeps the tangent and curvature continuous
        // where the corner meets each straight edge.
        let point = |x: f32, y: f32| {
            (
                origin.0 + tangent.0 * x + normal.0 * y,
                origin.1 + tangent.1 * x + normal.1 * y,
            )
        };
        let (a, b, middle) = (point(0.38, 0.0), point(0.63, 0.0), point(0.815, 0.185));
        builder.cubic_to(a.0, a.1, b.0, b.1, middle.0, middle.1);
        let (a, b, end) = (point(1.0, 0.37), point(1.0, 0.62), point(1.0, 1.0));
        builder.cubic_to(a.0, a.1, b.0, b.1, end.0, end.1);
    }
    builder.close();
    builder.to_path()
}
