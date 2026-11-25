use std::path::Path;

use adw::prelude::*;
use gtk::subclass::prelude::ObjectSubclassIsExt;
use gtk::{gdk, glib};

mod imp {
    use std::cell::{Cell, RefCell};

    use adw::prelude::*;
    use gtk::subclass::prelude::*;
    use gtk::{gdk, glib, graphene, gsk, pango};

    use crate::icons::Icon;

    #[derive(Default)]
    pub struct Avatar {
        pub(super) diameter: Cell<i32>,
        pub(super) initials: RefCell<String>,
        pub(super) palette_index: Cell<usize>,
        pub(super) group: Cell<bool>,
        pub(super) photo: RefCell<Option<gdk::Texture>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Avatar {
        const NAME: &'static str = "BlueBubblesAvatar";
        type Type = super::Avatar;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for Avatar {}

    impl WidgetImpl for Avatar {
        fn measure(&self, _orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            let diameter = self.diameter.get();
            (diameter, diameter, -1, -1)
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let avatar = self.obj();
            let width = avatar.width() as f32;
            let height = avatar.height() as f32;
            let diameter = width.min(height);
            if diameter <= 0.0 {
                return;
            }
            let x = (width - diameter) / 2.0;
            let y = (height - diameter) / 2.0;
            let bounds = graphene::Rect::new(x, y, diameter, diameter);
            snapshot.push_rounded_clip(&gsk::RoundedRect::from_rect(bounds, diameter / 2.0));

            if let Some(photo) = self.photo.borrow().as_ref() {
                let scale = diameter / photo.width().min(photo.height()) as f32;
                let w = photo.width() as f32 * scale;
                let h = photo.height() as f32 * scale;
                snapshot.append_texture(
                    photo,
                    &graphene::Rect::new((width - w) / 2.0, (height - h) / 2.0, w, h),
                );
            } else {
                let (r, g, b) = super::BACKGROUNDS[self.palette_index.get()];
                let background = gdk::RGBA::new(r / 255.0, g / 255.0, b / 255.0, 1.0);
                let foreground = gdk::RGBA::WHITE;
                snapshot.append_color(&background, &bounds);
                let initials = self.initials.borrow();
                if self.group.get() || initials.is_empty() {
                    let icon = if self.group.get() {
                        Icon::Group
                    } else {
                        Icon::Person
                    };
                    let size = (diameter * 0.64).round();
                    let rect = graphene::Rect::new(
                        (width - size) / 2.0,
                        (height - size) / 2.0,
                        size,
                        size,
                    );
                    let cr = snapshot.append_cairo(&rect);
                    cr.translate(f64::from(rect.x()), f64::from(rect.y()));
                    icon.paint(&cr, f64::from(size), foreground);
                } else {
                    let layout = avatar.create_pango_layout(Some(&initials));
                    let mut font = pango::FontDescription::new();
                    font.set_weight(pango::Weight::Semibold);
                    font.set_absolute_size(f64::from(diameter * 0.39 * pango::SCALE as f32));
                    layout.set_font_description(Some(&font));

                    // Center the visible glyphs rather than the font's line box.
                    let (ink, _) = layout.pixel_extents();
                    let text_x = ((width - ink.width() as f32) / 2.0).round() - ink.x() as f32;
                    let text_y = ((height - ink.height() as f32) / 2.0).round() - ink.y() as f32;
                    snapshot.save();
                    snapshot.translate(&graphene::Point::new(text_x, text_y));
                    snapshot.append_layout(&layout, &foreground);
                    snapshot.restore();
                }
            }
            snapshot.pop();
        }
    }
}

glib::wrapper! {
    pub struct Avatar(ObjectSubclass<imp::Avatar>) @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

const BACKGROUNDS: [(f32, f32, f32); 10] = [
    (52.0, 103.0, 148.0),
    (91.0, 74.0, 142.0),
    (132.0, 65.0, 117.0),
    (150.0, 64.0, 92.0),
    (150.0, 79.0, 50.0),
    (123.0, 96.0, 46.0),
    (43.0, 108.0, 81.0),
    (39.0, 107.0, 117.0),
    (61.0, 89.0, 124.0),
    (107.0, 78.0, 105.0),
];

impl Avatar {
    pub fn new(diameter: i32) -> Self {
        let avatar: Self = glib::Object::new();
        avatar.imp().diameter.set(diameter);
        avatar.set_halign(gtk::Align::Center);
        avatar.set_valign(gtk::Align::Center);
        avatar.set_can_target(false);
        avatar
    }

    pub fn bind(&self, name: &str, group: bool, path: Option<&Path>) {
        let state = self.imp();
        let mut letters = name
            .split_whitespace()
            .filter_map(|word| word.chars().find(|character| character.is_alphabetic()));
        let initials: String = letters
            .next()
            .into_iter()
            .chain(letters.next_back())
            .flat_map(|letter| letter.to_uppercase().take(1))
            .collect();
        state.initials.replace(initials);
        let hash = name.bytes().fold(5381_usize, |hash, byte| {
            hash.wrapping_mul(33).wrapping_add(usize::from(byte))
        });
        state.palette_index.set(hash % BACKGROUNDS.len());
        state.group.set(group);
        // Cached contact and group photos are already normalized to small PNGs.
        let photo = path.and_then(|path| gdk::Texture::from_filename(path).ok());
        state.photo.replace(photo);
        self.queue_draw();
    }

    pub fn set_photo(&self, texture: &gdk::Texture) {
        self.imp().photo.replace(Some(texture.clone()));
        self.queue_draw();
    }
}
