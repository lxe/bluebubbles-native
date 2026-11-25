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
    scroll_queued: Cell<bool>,
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
            scroll_queued: Cell::new(false),
        }
    }

    pub fn bottom_gap(&self) -> f64 {
        let adjustment = self.scroll.vadjustment();
        adjustment.upper() - adjustment.page_size() - adjustment.value()
    }

    fn use_scroll_bounds(&self) {
        let adjustment = self.scroll.vadjustment();
        if let Some(value) = self.restore_offset.get() {
            if self.store.n_items() != 0
                && adjustment.page_size() > 0.0
                && adjustment.upper() - adjustment.page_size() >= value.min(1.0)
            {
                self.restore_offset.set(None);
                adjustment.set_value(value);
            }
        } else if self.follow_latest.get() && self.bottom_gap() > 0.5 {
            adjustment.set_value(adjustment.upper() - adjustment.page_size());
        }
    }

    pub fn queue_scroll_bounds(self: &Rc<Self>) {
        if (self.restore_offset.get().is_none() && !self.follow_latest.get())
            || self.scroll_queued.replace(true)
        {
            return;
        }
        let weak = Rc::downgrade(self);
        glib::idle_add_local_once(move || {
            if let Some(view) = weak.upgrade() {
                view.scroll_queued.set(false);
                if view.scroll.is_mapped() {
                    view.use_scroll_bounds();
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
        self.list.scroll_to(last, gtk::ListScrollFlags::NONE, None);
        self.queue_scroll_bounds();

        let expected = self.store.item(last);
        let weak = Rc::downgrade(self);
        // GTK initially estimates the positions of rows that have yet to be measured.
        // Re-anchor after layout when a send, media update, or explicit jump changes the end.
        glib::timeout_add_local_once(std::time::Duration::from_millis(32), move || {
            if let Some(view) = weak.upgrade()
                && view.scroll.is_mapped()
                && view.follow_latest.get()
                && view.store.n_items() == last + 1
                && view.store.item(last) == expected
            {
                view.list.scroll_to(last, gtk::ListScrollFlags::NONE, None);
                view.use_scroll_bounds();
            }
        });
    }

    pub fn saved_offset(&self) -> Option<f64> {
        self.restore_offset.get().or_else(|| {
            (!self.follow_latest.get() && self.bottom_gap() > 1.0)
                .then(|| self.scroll.vadjustment().value())
        })
    }
}
