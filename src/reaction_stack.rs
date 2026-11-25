use adw::prelude::*;
use gtk::subclass::prelude::ObjectSubclassIsExt;
use gtk::{gdk, glib, graphene};

use crate::thread::ReactionBadge;

const BADGE_SIZE: i32 = 26;
const OVERLAP: i32 = 12;
pub const HEADROOM: i32 = BADGE_SIZE - OVERLAP;
const STACK_STEP: f64 = 14.0;
const OPEN_STEP: f64 = 27.0;
const HIT_PAD_X: i32 = 10;
const HIT_PAD_TOP: i32 = 8;
const HIT_PAD_BOTTOM: i32 = 8;

mod emoji {
    use adw::prelude::*;
    use adw::subclass::prelude::*;
    use gtk::{gdk, glib, graphene, gsk};

    #[derive(Default)]
    pub struct Emoji;

    #[glib::object_subclass]
    impl ObjectSubclass for Emoji {
        const NAME: &'static str = "BlueBubblesReactionEmoji";
        type Type = super::Emoji;
        type ParentType = adw::Bin;
    }

    impl ObjectImpl for Emoji {}
    impl BinImpl for Emoji {}

    impl WidgetImpl for Emoji {
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let glyph = gtk::Snapshot::new();
            self.parent_snapshot(&glyph);
            let Some(glyph) = glyph.to_node() else { return };

            snapshot.save();
            snapshot.translate(&graphene::Point::new(0.0, 1.5));
            snapshot.push_blur(1.7);
            snapshot.push_mask(gsk::MaskMode::Alpha);
            snapshot.append_node(&glyph);
            snapshot.pop();
            snapshot.append_color(&gdk::RGBA::new(0.01, 0.02, 0.03, 0.72), &glyph.bounds());
            snapshot.pop();
            snapshot.pop();
            snapshot.restore();
            snapshot.append_node(&glyph);
        }
    }
}

glib::wrapper! {
    pub struct Emoji(ObjectSubclass<emoji::Emoji>) @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

mod imp {
    use std::cell::{Cell, RefCell};

    use gtk::{glib, subclass::prelude::*};

    #[derive(Clone, Copy, PartialEq)]
    pub(super) struct BadgeLayout {
        pub(super) rightward: bool,
        pub(super) step: f64,
        pub(super) gaps: usize,
        pub(super) pad_left: i32,
        pub(super) pad_top: i32,
    }

    #[derive(Default)]
    pub struct ReactionStack {
        pub(super) hovered: Cell<bool>,
        pub(super) pinned: Cell<bool>,
        pub(super) openness: Cell<f64>,
        pub(super) animation: RefCell<Option<adw::TimedAnimation>>,
        pub(super) offsets: Cell<Option<BadgeLayout>>,
        pub(super) outgoing: Cell<bool>,
        pub(super) guid: RefCell<String>,
        pub(super) count: Cell<usize>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ReactionStack {
        const NAME: &'static str = "BlueBubblesReactionStack";
        type Type = super::ReactionStack;
        type ParentType = gtk::Button;
    }

    impl ObjectImpl for ReactionStack {}
    impl WidgetImpl for ReactionStack {}
    impl ButtonImpl for ReactionStack {}
}

glib::wrapper! {
    pub struct ReactionStack(ObjectSubclass<imp::ReactionStack>)
        @extends gtk::Button, gtk::Widget,
        @implements gtk::Accessible, gtk::Actionable, gtk::Buildable, gtk::ConstraintTarget;
}

impl ReactionStack {
    pub fn new() -> Self {
        let stack: Self = glib::Object::new();
        stack.add_css_class("flat");
        stack.add_css_class("reaction-stack");
        stack.set_child(Some(&gtk::Fixed::new()));
        stack.set_cursor_from_name(Some("pointer"));

        let pointer = gtk::EventControllerMotion::new();
        let weak = stack.downgrade();
        pointer.connect_enter(move |_, _, _| {
            if let Some(stack) = weak.upgrade() {
                stack.imp().hovered.set(true);
                stack.sync_expanded();
            }
        });
        let weak = stack.downgrade();
        pointer.connect_leave(move |_| {
            if let Some(stack) = weak.upgrade() {
                stack.imp().hovered.set(false);
                stack.sync_expanded();
            }
        });
        stack.add_controller(pointer);

        stack.connect_clicked(|stack| {
            if stack.imp().count.get() < 2 {
                return;
            }
            let was_pinned = stack.imp().pinned.replace(!stack.imp().pinned.get());
            if was_pinned {
                stack.imp().hovered.set(false);
            }
            stack.sync_expanded();
        });

        let focus = gtk::EventControllerFocus::new();
        let weak = stack.downgrade();
        focus.connect_leave(move |_| {
            if let Some(stack) = weak.upgrade() {
                stack.imp().pinned.set(false);
                stack.sync_expanded();
            }
        });
        stack.add_controller(focus);

        let keys = gtk::EventControllerKey::new();
        let weak = stack.downgrade();
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::Escape
                && let Some(stack) = weak.upgrade()
                && (stack.imp().pinned.get() || stack.imp().hovered.get())
            {
                stack.imp().hovered.set(false);
                stack.imp().pinned.set(false);
                stack.sync_expanded();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        stack.add_controller(keys);
        stack
    }

    pub fn bind(&self, guid: &str, outgoing: bool, badges: &[ReactionBadge]) {
        let state = self.imp();
        if state.guid.borrow().as_str() != guid || badges.is_empty() {
            if let Some(animation) = state.animation.take() {
                animation.pause();
            }
            state.guid.replace(guid.into());
            state.hovered.set(false);
            state.pinned.set(false);
            state.openness.set(0.0);
        }
        state.offsets.set(None);
        state.outgoing.set(outgoing);
        state.count.set(badges.len());
        let fixed = self.fixed();
        while let Some(child) = fixed.first_child() {
            fixed.remove(&child);
        }
        let mut ordered: Vec<_> = badges.iter().collect();
        ordered.sort_by_key(|badge| badge.own_count == 0);
        for badge in ordered.iter().rev() {
            fixed.put(&reaction(badge), 0.0, 0.0);
        }
        let label = ordered
            .iter()
            .map(|badge| {
                format!(
                    "{} × {}{}",
                    badge.kind.label(),
                    badge.count,
                    if badge.own_count > 0 { " (yours)" } else { "" }
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        self.update_property(&[gtk::accessible::Property::Label(&format!(
            "Reactions. {label}"
        ))]);
        self.update_state(&[gtk::accessible::State::Expanded(
            (badges.len() > 1).then_some(state.hovered.get() || state.pinned.get()),
        )]);
        let interactive = badges.len() > 1;
        self.set_cursor_from_name(interactive.then_some("pointer"));
        self.set_focusable(interactive);
        self.set_can_target(interactive);
        self.set_visible(!badges.is_empty());
    }

    pub fn position(&self, anchor: &graphene::Rect, lane_width: i32) -> gdk::Rectangle {
        let state = self.imp();
        let gaps = state.count.get().saturating_sub(1);
        let max_left = (lane_width - BADGE_SIZE).max(0) as f64;
        let corner_inset = 2.0;
        let half = f64::from(BADGE_SIZE) * 0.5;
        let leading = if state.outgoing.get() {
            f64::from(anchor.x()) + corner_inset - half
        } else {
            f64::from(anchor.x() + anchor.width()) - corner_inset - half
        }
        .clamp(0.0, max_left);
        let room = [leading, max_left - leading];
        let needed = OPEN_STEP * gaps as f64;
        let preferred = usize::from(state.outgoing.get());
        let side = if room[preferred] >= needed || room[preferred] >= room[1 - preferred] {
            preferred
        } else {
            1 - preferred
        };
        let step_open = OPEN_STEP.min(room[side] / gaps.max(1) as f64);
        let step_closed = STACK_STEP.min(step_open);
        let step = step_closed + (step_open - step_closed) * state.openness.get();
        let span = step * gaps as f64;
        let rightward = side == 1;
        let x = if rightward { leading } else { leading - span };
        let visual_x = x.floor() as i32;
        let visual_y = (anchor.y().round() as i32 - HEADROOM).max(0);
        let visual_w = BADGE_SIZE + span.ceil() as i32;
        let (pad_left, pad_right, pad_top, pad_bottom) = if gaps > 0 {
            let left = visual_x.min(HIT_PAD_X);
            let right = (lane_width - (visual_x + visual_w)).clamp(0, HIT_PAD_X);
            let top = visual_y.min(HIT_PAD_TOP);
            (left, right, top, HIT_PAD_BOTTOM)
        } else {
            (0, 0, 0, 0)
        };
        let layout = imp::BadgeLayout {
            rightward,
            step,
            gaps,
            pad_left,
            pad_top,
        };
        if state.offsets.replace(Some(layout)) != Some(layout) {
            let fixed = self.fixed();
            let mut child = fixed.first_child();
            for index in 0..=gaps {
                let Some(badge) = child else { break };
                let slot = if rightward { gaps - index } else { index };
                fixed.move_(
                    &badge,
                    f64::from(pad_left) + slot as f64 * step,
                    f64::from(pad_top),
                );
                child = badge.next_sibling();
            }
        }
        gdk::Rectangle::new(
            visual_x - pad_left,
            visual_y - pad_top,
            visual_w + pad_left + pad_right,
            BADGE_SIZE + pad_top + pad_bottom,
        )
    }

    fn sync_expanded(&self) {
        let state = self.imp();
        if state.count.get() < 2 {
            return;
        }
        let expanded = state.pinned.get() || state.hovered.get();
        let to = f64::from(expanded);
        if state
            .animation
            .borrow()
            .as_ref()
            .is_some_and(|animation| animation.value_to() == to)
        {
            return;
        }
        if let Some(animation) = state.animation.take() {
            animation.pause();
        }
        self.update_state(&[gtk::accessible::State::Expanded(Some(expanded))]);
        let weak = self.downgrade();
        let target = adw::CallbackAnimationTarget::new(move |value| {
            if let Some(stack) = weak.upgrade() {
                stack.imp().openness.set(value);
                if let Some(overlay) = stack.parent() {
                    overlay.queue_allocate();
                }
            }
        });
        let animation = adw::TimedAnimation::new(
            self,
            state.openness.get(),
            to,
            if expanded { 210 } else { 145 },
            target,
        );
        animation.set_easing(adw::Easing::EaseOutCubic);
        animation.play();
        state.animation.replace(Some(animation));
    }

    fn fixed(&self) -> gtk::Fixed {
        self.child()
            .and_downcast::<gtk::Fixed>()
            .expect("Reaction badge container")
    }
}

fn reaction(badge: &ReactionBadge) -> gtk::Overlay {
    let reaction = gtk::Overlay::new();
    reaction.set_size_request(BADGE_SIZE, BADGE_SIZE);
    let glyph = gtk::Label::new(Some(badge.kind.emoji()));
    glyph.add_css_class("reaction-stack-emoji");
    let emoji: Emoji = glib::Object::new();
    emoji.set_child(Some(&glyph));
    emoji.set_halign(gtk::Align::Center);
    emoji.set_valign(gtk::Align::Center);
    emoji.set_margin_bottom(3);
    reaction.set_child(Some(&emoji));
    if badge.count > 1 {
        let count = gtk::Label::new(Some(&badge.count.to_string()));
        count.add_css_class("reaction-stack-count");
        count.set_halign(gtk::Align::End);
        count.set_valign(gtk::Align::End);
        reaction.add_overlay(&count);
    }
    reaction
}
