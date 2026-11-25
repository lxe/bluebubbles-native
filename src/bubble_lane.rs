use std::time::{Duration, Instant};

use adw::prelude::*;
use adw::subclass::prelude::ObjectSubclassIsExt;
use gtk::{glib, graphene, gsk};

mod imp {
    use std::cell::{Cell, RefCell};
    use std::time::Instant;

    use adw::prelude::*;
    use adw::subclass::prelude::BinImpl;
    use gtk::glib;
    use gtk::subclass::prelude::*;

    #[derive(Default)]
    pub struct BubbleLane {
        pub(super) confirmation: Cell<Option<Instant>>,
        pub(super) animation: RefCell<Option<adw::TimedAnimation>>,
        pub(super) progress: Cell<f32>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for BubbleLane {
        const NAME: &'static str = "BlueBubblesMessageLane";
        type Type = super::BubbleLane;
        type ParentType = adw::Bin;
    }

    impl ObjectImpl for BubbleLane {}
    impl BinImpl for BubbleLane {}

    impl WidgetImpl for BubbleLane {
        fn map(&self) {
            self.parent_map();
            self.obj().play_confirmation();
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let progress = self.progress.get();
            if self.animation.borrow().is_none() || progress >= 1.0 {
                self.parent_snapshot(snapshot);
                return;
            }
            let widget = self.obj();
            let Some(child) = widget.child() else { return };
            let Some(bounds) = child.compute_bounds(&*widget) else {
                return;
            };
            let origin =
                gtk::graphene::Point::new(bounds.x() + bounds.width() * 0.5, bounds.height() * 0.5);
            // Small overshoot followed by a cubic return. Layout and scroll position stay fixed.
            let scale = if progress < 0.62 {
                let phase = progress / 0.62;
                0.968 + 0.054 * (1.0 - (1.0 - phase).powi(3))
            } else {
                1.0 + 0.022 * ((1.0 - progress) / 0.38).powi(2)
            };
            snapshot.save();
            snapshot.translate(&origin);
            snapshot.scale(scale, scale);
            snapshot.translate(&gtk::graphene::Point::new(-origin.x(), -origin.y()));
            self.parent_snapshot(snapshot);
            snapshot.restore();
        }
    }

    #[derive(Default)]
    pub struct BubbleLayout;

    #[glib::object_subclass]
    impl ObjectSubclass for BubbleLayout {
        const NAME: &'static str = "BlueBubblesMessageLayout";
        type Type = super::BubbleLayout;
        type ParentType = gtk::LayoutManager;
    }

    impl ObjectImpl for BubbleLayout {}

    impl LayoutManagerImpl for BubbleLayout {
        fn request_mode(&self, _widget: &gtk::Widget) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::HeightForWidth
        }

        fn measure(
            &self,
            widget: &gtk::Widget,
            orientation: gtk::Orientation,
            for_size: i32,
        ) -> (i32, i32, i32, i32) {
            let Some(child) = widget.first_child() else {
                return (0, 0, -1, -1);
            };
            let for_size = if orientation == gtk::Orientation::Vertical && for_size >= 0 {
                super::message_width(&child, for_size)
            } else {
                for_size
            };
            child.measure(orientation, for_size)
        }

        fn allocate(&self, widget: &gtk::Widget, width: i32, height: i32, baseline: i32) {
            let Some(child) = widget.first_child() else {
                return;
            };
            let child_width = super::message_width(&child, width);
            let x = if widget.has_css_class("outgoing") {
                width - child_width
            } else {
                0
            };
            let transform =
                super::gsk::Transform::new().translate(&super::graphene::Point::new(x as f32, 0.0));
            child.allocate(child_width, height, baseline, Some(transform));
        }
    }
}

glib::wrapper! {
    pub struct BubbleLayout(ObjectSubclass<imp::BubbleLayout>) @extends gtk::LayoutManager;
}

glib::wrapper! {
    pub struct BubbleLane(ObjectSubclass<imp::BubbleLane>) @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

pub fn new(child: &impl IsA<gtk::Widget>) -> BubbleLane {
    let lane: BubbleLane = glib::Object::new();
    lane.set_layout_manager(Some(glib::Object::new::<BubbleLayout>()));
    lane.set_child(Some(child));
    lane
}

impl BubbleLane {
    pub fn confirm(&self, at: Option<Instant>) {
        let state = self.imp();
        if state.confirmation.replace(at) == at {
            return;
        }
        if let Some(animation) = state.animation.take() {
            animation.pause();
        }
        state.progress.set(1.0);
        if self.is_mapped() {
            self.play_confirmation();
        }
    }

    fn play_confirmation(&self) {
        let state = self.imp();
        if state.animation.borrow().is_some()
            || state
                .confirmation
                .get()
                .is_none_or(|at| at.elapsed() > Duration::from_millis(160))
            || !crate::loading::animations_enabled()
        {
            return;
        }
        let _profile = crate::profile::span("send-confirm-motion");
        state.progress.set(0.0);
        let lane = self.downgrade();
        let target = adw::CallbackAnimationTarget::new(move |value| {
            if let Some(lane) = lane.upgrade() {
                lane.imp().progress.set(value as f32);
                lane.queue_draw();
            }
        });
        let animation = adw::TimedAnimation::new(self, 0.0, 1.0, 230, target);
        animation.set_easing(adw::Easing::Linear);
        state.animation.replace(Some(animation.clone()));
        animation.play();
    }
}

fn message_width(child: &gtk::Widget, lane_width: i32) -> i32 {
    let (_, natural, _, _) = child.measure(gtk::Orientation::Horizontal, -1);
    natural.min(lane_width * 3 / 4).max(0)
}
