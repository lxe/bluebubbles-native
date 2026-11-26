use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};

use crate::thread::MessageRow;
use crate::{media, rows};

pub struct TimelineView {
    pub store: gio::ListStore,
    pub list: gtk::ListView,
    pub scroll: gtk::ScrolledWindow,
    pub follow_latest: Cell<bool>,
    pub restore_offset: Cell<Option<f64>>,
    updater: rows::MessageUpdater,
    applying_bounds: Cell<bool>,
    scroll_queued: Cell<bool>,
    anchor_generation: Cell<u64>,
}

impl TimelineView {
    pub fn new(counts: &std::rc::Rc<rows::Counts>, handler: &media::Handler) -> Self {
        let store = gio::ListStore::new::<glib::BoxedAnyObject>();
        let (factory, updater) = rows::messages(counts, handler);
        let list = gtk::ListView::new(
            Some(gtk::NoSelection::new(Some(store.clone()))),
            Some(factory),
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
            updater,
            applying_bounds: Cell::new(false),
            scroll_queued: Cell::new(false),
            anchor_generation: Cell::new(0),
        }
    }

    pub fn replace_rows(&self, rows: Vec<MessageRow>) -> bool {
        replace_in_store(&self.store, rows, |position, row| {
            self.updater.rebind(position, row);
        })
    }

    pub fn update_row(&self, position: u32, row: MessageRow) {
        if let Some(existing) = self
            .store
            .item(position)
            .and_downcast::<glib::BoxedAnyObject>()
        {
            *existing.borrow_mut::<MessageRow>() = row;
            let borrowed = existing.borrow::<MessageRow>();
            self.updater.rebind(position, &borrowed);
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
        // Always defer set_value() out of adjustment::changed: GtkListBase emits changed while its
        // own value_changed handler is blocked, so a synchronous set_value() leaves its internal
        // scroll anchor stale and snaps the viewport back on the next allocation.
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

fn replace_in_store<T: PartialEq + 'static>(
    store: &gio::ListStore,
    rows: Vec<T>,
    mut on_rebind: impl FnMut(u32, &T),
) -> bool {
    let old_len = store.n_items() as usize;
    let matches = |index: usize, item: &T| {
        store
            .item(index as u32)
            .and_downcast::<glib::BoxedAnyObject>()
            .is_some_and(|row| *row.borrow::<T>() == *item)
    };
    let prefix = rows
        .iter()
        .take(old_len)
        .enumerate()
        .take_while(|(index, row)| matches(*index, row))
        .count();
    let suffix = rows[prefix..]
        .iter()
        .rev()
        .take(old_len - prefix)
        .enumerate()
        .take_while(|(index, row)| matches(old_len - index - 1, row))
        .count();
    let remove = old_len - prefix - suffix;
    let insert_end = rows.len() - suffix;
    let add = insert_end - prefix;
    if remove == 0 && add == 0 {
        return false;
    }
    if remove == add {
        for (offset, row) in rows.into_iter().skip(prefix).take(add).enumerate() {
            let position = (prefix + offset) as u32;
            if let Some(existing) = store.item(position).and_downcast::<glib::BoxedAnyObject>() {
                *existing.borrow_mut::<T>() = row;
                let borrowed = existing.borrow::<T>();
                on_rebind(position, &borrowed);
            }
        }
        return false;
    }
    let changed: Vec<_> = rows
        .into_iter()
        .skip(prefix)
        .take(add)
        .map(glib::BoxedAnyObject::new)
        .collect();
    store.splice(prefix as u32, remove as u32, &changed);
    true
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;
    use std::time::Instant;

    use super::*;
    use crate::model::{Message, Service};
    use crate::thread::SendState;

    fn row(
        guid: &str,
        text: &str,
        send_state: Option<SendState>,
        confirmed: Option<Instant>,
    ) -> MessageRow {
        MessageRow {
            message: Message {
                guid: guid.into(),
                text: text.into(),
                outgoing: true,
                ..Message::default()
            },
            service: Service::IMessage,
            group: false,
            starts_group: false,
            show_time: false,
            sender: None,
            quote: None,
            reactions: Vec::new(),
            my_reaction: None,
            send_state,
            confirmed_at: confirmed,
        }
    }

    #[test]
    fn in_place_confirmation_updates_without_splicing_store() {
        let store = gio::ListStore::new::<glib::BoxedAnyObject>();
        let splices = Rc::new(Cell::new(0u32));
        store.connect_items_changed({
            let splices = splices.clone();
            move |_, _, _, _| splices.set(splices.get() + 1)
        });

        let first = row("m1", "Earlier", None, None);
        let pending = row("temp-1", "Hello", Some(SendState::Sending), None);
        let mut rebound = Vec::new();
        assert!(replace_in_store(
            &store,
            vec![first.clone(), pending],
            |pos, _| {
                rebound.push(pos);
            }
        ));
        assert_eq!(splices.get(), 1);
        assert!(rebound.is_empty());

        let pending_obj = store.item(1).unwrap();
        let confirmed = row("server-1", "Hello", None, Some(Instant::now()));
        assert!(!replace_in_store(
            &store,
            vec![first.clone(), confirmed.clone()],
            |pos, _| {
                rebound.push(pos);
            }
        ));
        // Confirming a pending message must not emit items-changed or swap the ListStore object.
        assert_eq!(splices.get(), 1);
        assert_eq!(rebound, vec![1]);
        assert_eq!(store.item(1).unwrap(), pending_obj);
        assert_eq!(
            store
                .item(1)
                .unwrap()
                .downcast::<glib::BoxedAnyObject>()
                .unwrap()
                .borrow::<MessageRow>()
                .message
                .guid,
            "server-1"
        );

        // Identical follow-up render performs neither a splice nor a rebind.
        rebound.clear();
        assert!(!replace_in_store(
            &store,
            vec![first, confirmed],
            |pos, _| {
                rebound.push(pos);
            }
        ));
        assert_eq!(splices.get(), 1);
        assert!(rebound.is_empty());
    }
}
