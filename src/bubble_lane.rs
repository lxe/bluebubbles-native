use std::time::{Duration, Instant};

use adw::prelude::*;
use adw::subclass::prelude::ObjectSubclassIsExt;
use gtk::{glib, graphene, gsk};

mod imp {
    use std::cell::Cell;
    use std::time::Instant;

    use adw::prelude::*;
    use adw::subclass::prelude::BinImpl;
    use gtk::glib;
    use gtk::subclass::prelude::*;

    #[derive(Default)]
    pub struct BubbleLane {
        pub(super) confirmation: Cell<Option<Instant>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for BubbleLane {
        const NAME: &'static str = "BlueBubblesMessageLane";
        type Type = super::BubbleLane;
        type ParentType = adw::Bin;
    }

    impl ObjectImpl for BubbleLane {}
    impl BinImpl for BubbleLane {}
    impl WidgetImpl for BubbleLane {}

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
        if at.is_some_and(|at| at.elapsed() <= Duration::from_millis(160))
            && crate::loading::animations_enabled()
        {
            let _profile = crate::profile::span("send-confirm-motion");
        }
    }
}

fn message_width(child: &gtk::Widget, lane_width: i32) -> i32 {
    let (_, natural, _, _) = child.measure(gtk::Orientation::Horizontal, -1);
    natural.min(lane_width * 3 / 4).max(0)
}
