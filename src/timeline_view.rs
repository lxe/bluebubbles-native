use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};

use crate::{media, rows};

pub struct TimelineView {
    pub store: gio::ListStore,
    pub list: gtk::ListView,
    pub scroll: gtk::ScrolledWindow,
    pub follow_latest: Cell<bool>,
    pub restore_offset: Cell<Option<f64>>,
    applying_bounds: Cell<bool>,
    scroll_queued: Cell<bool>,
    anchor_generation: Cell<u64>,
}

impl TimelineView {
    pub fn new(counts: &std::rc::Rc<rows::Counts>, handler: &media::Handler) -> Self {
        let store = gio::ListStore::new::<glib::BoxedAnyObject>();
        let list = gtk::ListView::new(
            Some(gtk::NoSelection::new(Some(store.clone()))),
            Some(rows::messages(counts, handler)),
        );
        list.add_css_class("timeline");
        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .overlay_scrolling(true)
            .child(&list)
            .build();
        scroll.add_css_class("timeline-scroll");
        Self {
            store,
            list,
            scroll,
            follow_latest: Cell::new(true),
            restore_offset: Cell::new(None),
            applying_bounds: Cell::new(false),
            scroll_queued: Cell::new(false),
            anchor_generation: Cell::new(0),
        }
    }

    pub fn bottom_gap(&self) -> f64 {
        let adjustment = self.scroll.vadjustment();
        (adjustment.upper() - adjustment.page_size() - adjustment.value()).max(0.0)
    }

    pub fn apply_scroll_bounds(&self) {
        if self.applying_bounds.replace(true) {
            return;
        }
        let adjustment = self.scroll.vadjustment();
        if let Some(value) = self.restore_offset.get() {
            if self.store.n_items() != 0
                && adjustment.page_size() > 0.0
                && adjustment.upper() - adjustment.page_size() >= value.min(1.0)
            {
                self.restore_offset.set(None);
                let max_value = (adjustment.upper() - adjustment.page_size()).max(0.0);
                adjustment.set_value(value.min(max_value));
            }
        } else if self.follow_latest.get()
            && adjustment.page_size() > 0.0
            && self.bottom_gap() > 0.5
        {
            adjustment.set_value((adjustment.upper() - adjustment.page_size()).max(0.0));
        }
        self.applying_bounds.set(false);
    }

    pub fn queue_scroll_bounds(self: &Rc<Self>) {
        if self.restore_offset.get().is_none() && !self.follow_latest.get() {
            return;
        }
        if self.scroll.is_mapped() {
            self.apply_scroll_bounds();
        }
        if self.scroll_queued.replace(true) {
            return;
        }
        let weak = Rc::downgrade(self);
        glib::idle_add_local_once(move || {
            if let Some(view) = weak.upgrade() {
                view.scroll_queued.set(false);
                if view.scroll.is_mapped() {
                    view.apply_scroll_bounds();
                }
            }
        });
    }

    pub fn scroll_latest(self: &Rc<Self>) {
        self.restore_offset.set(None);
        self.follow_latest.set(true);
        let Some(last) = self.store.n_items().checked_sub(1) else {
            return;
        };
        // Do not call adjustment.set_value() immediately after scroll_to(): in GTK 4,
        // mutating the adjustment cancels GtkListView's pending scroll_to target before
        // layout measures newly added rows.
        self.list.scroll_to(last, gtk::ListScrollFlags::NONE, None);

        let generation = self.anchor_generation.get().wrapping_add(1);
        self.anchor_generation.set(generation);
        let expected = self.store.item(last);
        let weak = Rc::downgrade(self);
        let remaining = Cell::new(4u8);
        self.scroll.add_tick_callback(move |scroll, _| {
            let Some(view) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if view.anchor_generation.get() != generation
                || !view.follow_latest.get()
                || view.store.n_items() != last + 1
                || view.store.item(last) != expected
            {
                return glib::ControlFlow::Break;
            }
            if scroll.is_mapped() {
                if view.bottom_gap() > 0.5 {
                    view.apply_scroll_bounds();
                }
                let left = remaining.get().saturating_sub(1);
                remaining.set(left);
                if left == 0 {
                    return glib::ControlFlow::Break;
                }
            }
            glib::ControlFlow::Continue
        });
    }

    pub fn saved_offset(&self) -> Option<f64> {
        self.restore_offset.get().or_else(|| {
            (!self.follow_latest.get() && self.bottom_gap() > 1.0)
                .then(|| self.scroll.vadjustment().value())
        })
    }
}
