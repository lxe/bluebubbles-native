use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, glib, pango};

use crate::avatar::Avatar;
use crate::icons::Icon;
use crate::layout::thread_clamp;
use crate::model::{
    ContactEntry, Conversation, Service, address_key, find_existing_conversation,
    looks_like_address,
};

const MAX_SUGGESTIONS: usize = 60;

#[derive(Default)]
pub struct Callbacks {
    pub on_changed: Option<Box<dyn Fn()>>,
    pub on_open_existing: Option<Box<dyn Fn(String)>>,
    pub on_focus_composer: Option<Box<dyn Fn()>>,
    pub on_cancel: Option<Box<dyn Fn()>>,
}

struct PickerState {
    directory: Vec<ContactEntry>,
    conversations: Vec<Conversation>,
    recipients: Vec<ContactEntry>,
    visible_entries: Vec<ContactEntry>,
    existing_guid: Option<String>,
    loading_contacts: bool,
    service: Service,
}

pub struct NewMessageView {
    pub root: gtk::Box,
    chips: gtk::FlowBox,
    entry: gtk::Entry,
    service_group: gtk::Box,
    imessage_button: gtk::ToggleButton,
    sms_button: gtk::ToggleButton,
    existing_banner: gtk::Box,
    existing_avatar: Avatar,
    existing_label: gtk::Label,
    section_label: gtk::Label,
    list: gtk::ListBox,
    empty_hint: gtk::Label,
    state: RefCell<PickerState>,
    callbacks: RefCell<Callbacks>,
    updating_ui: Cell<bool>,
}

impl NewMessageView {
    pub fn new() -> Rc<Self> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.set_vexpand(true);
        root.add_css_class("new-message-view");

        let to_label = gtk::Label::new(Some("To:"));
        to_label.add_css_class("dim-label");
        to_label.add_css_class("new-message-to");
        to_label.set_valign(gtk::Align::Start);

        let chips = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .homogeneous(false)
            .column_spacing(6)
            .row_spacing(6)
            .min_children_per_line(1)
            .max_children_per_line(12)
            .halign(gtk::Align::Start)
            .visible(false)
            .build();
        chips.add_css_class("recipient-chips");

        let entry = gtk::Entry::builder()
            .placeholder_text("Name, phone number, or email")
            .hexpand(true)
            .has_frame(false)
            .build();
        entry.add_css_class("new-message-entry");
        entry.update_property(&[gtk::accessible::Property::Label("To recipient")]);

        let recipients_column = gtk::Box::new(gtk::Orientation::Vertical, 6);
        recipients_column.set_hexpand(true);
        recipients_column.append(&chips);
        recipients_column.append(&entry);

        let imessage_button = gtk::ToggleButton::with_label("iMessage");
        imessage_button.set_active(true);
        imessage_button.add_css_class("service-pill-item");
        let sms_button = gtk::ToggleButton::with_label("SMS");
        sms_button.set_group(Some(&imessage_button));
        sms_button.add_css_class("service-pill-item");
        let service_group = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        service_group.add_css_class("linked");
        service_group.add_css_class("new-message-service");
        service_group.set_valign(gtk::Align::Start);
        service_group.append(&imessage_button);
        service_group.append(&sms_button);

        let bar_inner = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        bar_inner.add_css_class("new-message-bar");
        bar_inner.add_css_class("thread-content-inset");
        bar_inner.append(&to_label);
        bar_inner.append(&recipients_column);
        bar_inner.append(&service_group);

        let bar_wrap = gtk::Box::new(gtk::Orientation::Vertical, 0);
        bar_wrap.add_css_class("new-message-bar-wrap");
        bar_wrap.append(&thread_clamp(&bar_inner));
        let bar_divider = gtk::Separator::new(gtk::Orientation::Horizontal);
        bar_divider.add_css_class("new-message-divider");
        bar_wrap.append(&bar_divider);
        root.append(&bar_wrap);

        let existing_avatar = Avatar::new(26);
        let existing_label = gtk::Label::builder()
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(pango::EllipsizeMode::End)
            .build();
        existing_label.add_css_class("existing-chat-text");
        let open_existing = gtk::Button::with_label("Open");
        open_existing.add_css_class("existing-chat-open");
        open_existing.set_valign(gtk::Align::Center);
        let existing_banner = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        existing_banner.add_css_class("existing-chat-banner");
        existing_banner.set_visible(false);
        existing_banner.append(&existing_avatar);
        existing_banner.append(&existing_label);
        existing_banner.append(&open_existing);

        let section_label = gtk::Label::builder().label("Contacts").xalign(0.0).build();
        section_label.add_css_class("dim-label");
        section_label.add_css_class("new-message-section");

        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::Single)
            .activate_on_single_click(true)
            .build();
        list.add_css_class("contact-picker-list");

        let empty_hint = gtk::Label::builder()
            .wrap(true)
            .xalign(0.0)
            .visible(false)
            .build();
        empty_hint.add_css_class("dim-label");
        empty_hint.add_css_class("contact-picker-empty");

        let body_inner = gtk::Box::new(gtk::Orientation::Vertical, 10);
        body_inner.add_css_class("new-message-body");
        body_inner.add_css_class("thread-content-inset");
        body_inner.append(&existing_banner);
        body_inner.append(&section_label);
        body_inner.append(&list);
        body_inner.append(&empty_hint);

        let scroll = gtk::ScrolledWindow::builder()
            .vexpand(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .overlay_scrolling(true)
            .child(&thread_clamp(&body_inner))
            .build();
        scroll.add_css_class("new-message-scroll");
        root.append(&scroll);

        let view = Rc::new(Self {
            root,
            chips,
            entry,
            service_group,
            imessage_button,
            sms_button,
            existing_banner,
            existing_avatar,
            existing_label,
            section_label,
            list,
            empty_hint,
            state: RefCell::new(PickerState {
                directory: Vec::new(),
                conversations: Vec::new(),
                recipients: Vec::new(),
                visible_entries: Vec::new(),
                existing_guid: None,
                loading_contacts: false,
                service: Service::IMessage,
            }),
            callbacks: RefCell::new(Callbacks::default()),
            updating_ui: Cell::new(false),
        });

        view.wire(&open_existing);
        view
    }

    pub fn set_callbacks(&self, callbacks: Callbacks) {
        *self.callbacks.borrow_mut() = callbacks;
    }

    pub fn reset(
        self: &Rc<Self>,
        directory: Vec<ContactEntry>,
        conversations: Vec<Conversation>,
        loading_contacts: bool,
    ) {
        {
            let mut state = self.state.borrow_mut();
            state.directory = directory;
            state.conversations = conversations;
            state.recipients.clear();
            state.existing_guid = None;
            state.loading_contacts = loading_contacts;
            state.service = Service::IMessage;
        }
        self.updating_ui.set(true);
        self.entry.set_text("");
        self.imessage_button.set_active(true);
        self.updating_ui.set(false);
        self.rebuild_chips();
        self.refresh_picker();
        self.update_existing_match();
        self.notify_changed();
    }

    pub fn update_directory(
        self: &Rc<Self>,
        directory: Vec<ContactEntry>,
        conversations: Vec<Conversation>,
        loading_contacts: bool,
    ) {
        {
            let mut state = self.state.borrow_mut();
            state.directory = directory;
            state.conversations = conversations;
            state.loading_contacts = loading_contacts;
        }
        self.refresh_picker();
        self.update_existing_match();
        self.notify_changed();
    }

    pub fn focus_input(&self) {
        self.entry.grab_focus();
    }

    pub fn has_recipients_or_valid_input(&self) -> bool {
        !self.state.borrow().recipients.is_empty() || looks_like_address(&self.entry.text())
    }

    pub fn commit_pending_input(self: &Rc<Self>) -> bool {
        let text = self.entry.text().trim().to_owned();
        if !looks_like_address(&text) {
            return false;
        }
        self.add_recipient(ContactEntry {
            name: text.clone(),
            address: text,
            avatar: None,
        });
        true
    }

    pub fn recipients(&self) -> Vec<ContactEntry> {
        self.state.borrow().recipients.clone()
    }

    pub fn existing_conversation_guid(&self) -> Option<String> {
        self.state.borrow().existing_guid.clone()
    }

    pub fn service(&self) -> Service {
        let state = self.state.borrow();
        if let Some(guid) = &state.existing_guid
            && let Some(chat) = state.conversations.iter().find(|chat| &chat.guid == guid)
        {
            return chat.service();
        }
        state.service
    }

    fn wire(self: &Rc<Self>, open_existing: &gtk::Button) {
        self.entry.connect_changed({
            let weak = Rc::downgrade(self);
            move |_| {
                if let Some(view) = weak.upgrade()
                    && !view.updating_ui.get()
                {
                    view.refresh_picker();
                    view.notify_changed();
                }
            }
        });

        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, modifiers| {
            let Some(view) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            if modifiers.intersects(gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::ALT_MASK) {
                return glib::Propagation::Proceed;
            }
            match key {
                gdk::Key::Down => {
                    view.move_selection(1);
                    glib::Propagation::Stop
                }
                gdk::Key::Up => {
                    view.move_selection(-1);
                    glib::Propagation::Stop
                }
                gdk::Key::Return | gdk::Key::KP_Enter => {
                    view.handle_enter();
                    glib::Propagation::Stop
                }
                gdk::Key::comma => {
                    if view.commit_pending_input() {
                        glib::Propagation::Stop
                    } else {
                        glib::Propagation::Proceed
                    }
                }
                gdk::Key::BackSpace => {
                    if view.entry.text().is_empty() && view.remove_last_recipient() {
                        glib::Propagation::Stop
                    } else {
                        glib::Propagation::Proceed
                    }
                }
                gdk::Key::Escape => {
                    if !view.entry.text().is_empty() {
                        view.entry.set_text("");
                    } else if let Some(cancel) = view.callbacks.borrow().on_cancel.as_ref() {
                        cancel();
                    }
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        });
        self.entry.add_controller(keys);

        self.list.connect_row_activated({
            let weak = Rc::downgrade(self);
            move |_, row| {
                let Some(view) = weak.upgrade() else {
                    return;
                };
                let index = row.index();
                if index < 0 {
                    return;
                }
                let chosen = view
                    .state
                    .borrow()
                    .visible_entries
                    .get(index as usize)
                    .cloned();
                if let Some(entry) = chosen {
                    view.add_recipient(entry);
                    view.entry.grab_focus();
                }
            }
        });

        open_existing.connect_clicked({
            let weak = Rc::downgrade(self);
            move |_| {
                let Some(view) = weak.upgrade() else {
                    return;
                };
                let guid = view.state.borrow().existing_guid.clone();
                if let Some(guid) = guid
                    && let Some(callback) = view.callbacks.borrow().on_open_existing.as_ref()
                {
                    callback(guid);
                }
            }
        });

        let wire_service = |button: &gtk::ToggleButton, service: Service| {
            let weak = Rc::downgrade(self);
            button.connect_toggled(move |button| {
                if !button.is_active() {
                    return;
                }
                let Some(view) = weak.upgrade() else {
                    return;
                };
                if view.updating_ui.get() {
                    return;
                }
                view.state.borrow_mut().service = service;
                view.notify_changed();
            });
        };
        wire_service(&self.imessage_button, Service::IMessage);
        wire_service(&self.sms_button, Service::Sms);
    }

    fn handle_enter(self: &Rc<Self>) {
        let query = self.entry.text().trim().to_owned();
        if query.is_empty() {
            if !self.state.borrow().recipients.is_empty()
                && let Some(focus) = self.callbacks.borrow().on_focus_composer.as_ref()
            {
                focus();
            }
            return;
        }
        if let Some(selected) = self.list.selected_row() {
            let index = selected.index();
            if index >= 0 {
                let chosen = self
                    .state
                    .borrow()
                    .visible_entries
                    .get(index as usize)
                    .cloned();
                if let Some(entry) = chosen {
                    self.add_recipient(entry);
                    self.entry.grab_focus();
                    return;
                }
            }
        }
        if !self.commit_pending_input() {
            let first = self.state.borrow().visible_entries.first().cloned();
            if let Some(entry) = first {
                self.add_recipient(entry);
                self.entry.grab_focus();
            }
        }
    }

    fn move_selection(&self, delta: i32) {
        let count = self.state.borrow().visible_entries.len() as i32;
        if count == 0 {
            return;
        }
        let current = self
            .list
            .selected_row()
            .map(|row| row.index())
            .unwrap_or(-1);
        let next = if current < 0 {
            if delta > 0 { 0 } else { count - 1 }
        } else {
            (current + delta).clamp(0, count - 1)
        };
        if let Some(row) = self.list.row_at_index(next) {
            self.list.select_row(Some(&row));
        }
    }

    fn add_recipient(self: &Rc<Self>, entry: ContactEntry) {
        let key = address_key(&entry.address);
        if key.is_empty() {
            return;
        }
        let added = {
            let mut state = self.state.borrow_mut();
            if state
                .recipients
                .iter()
                .any(|existing| address_key(&existing.address) == key)
            {
                false
            } else {
                state.recipients.push(entry);
                true
            }
        };
        self.updating_ui.set(true);
        self.entry.set_text("");
        self.updating_ui.set(false);
        if added {
            self.rebuild_chips();
            self.update_existing_match();
        }
        self.refresh_picker();
        self.notify_changed();
    }

    fn remove_recipient(self: &Rc<Self>, key: &str) {
        let removed = {
            let mut state = self.state.borrow_mut();
            let before = state.recipients.len();
            state
                .recipients
                .retain(|existing| address_key(&existing.address) != key);
            state.recipients.len() != before
        };
        if removed {
            self.rebuild_chips();
            self.update_existing_match();
            self.refresh_picker();
            self.notify_changed();
        }
    }

    fn remove_last_recipient(self: &Rc<Self>) -> bool {
        let removed = self.state.borrow_mut().recipients.pop().is_some();
        if removed {
            self.rebuild_chips();
            self.update_existing_match();
            self.refresh_picker();
            self.notify_changed();
        }
        removed
    }

    fn rebuild_chips(self: &Rc<Self>) {
        while let Some(child) = self.chips.first_child() {
            self.chips.remove(&child);
        }
        let recipients = self.state.borrow().recipients.clone();
        self.chips.set_visible(!recipients.is_empty());
        self.entry
            .set_placeholder_text(Some(if recipients.is_empty() {
                "Name, phone number, or email"
            } else {
                "Add another recipient…"
            }));
        for recipient in recipients {
            let chip = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            chip.add_css_class("recipient-chip");
            chip.set_halign(gtk::Align::Start);
            let avatar = Avatar::new(20);
            avatar.bind(&recipient.name, false, recipient.avatar.as_deref());
            let name = gtk::Label::builder()
                .label(&recipient.name)
                .ellipsize(pango::EllipsizeMode::End)
                .max_width_chars(24)
                .build();
            name.add_css_class("recipient-chip-label");
            let remove = gtk::Button::builder()
                .child(&Icon::Remove.sized_widget(11))
                .tooltip_text(format!("Remove {}", recipient.name))
                .valign(gtk::Align::Center)
                .build();
            remove.add_css_class("flat");
            remove.add_css_class("circular");
            remove.add_css_class("recipient-chip-remove");
            let key = address_key(&recipient.address);
            let weak = Rc::downgrade(self);
            remove.connect_clicked(move |_| {
                if let Some(view) = weak.upgrade() {
                    view.remove_recipient(&key);
                    view.entry.grab_focus();
                }
            });
            chip.append(&avatar);
            chip.append(&name);
            chip.append(&remove);
            let item = gtk::FlowBoxChild::new();
            item.set_focusable(false);
            item.set_halign(gtk::Align::Start);
            item.set_child(Some(&chip));
            self.chips.insert(&item, -1);
        }
    }

    fn update_existing_match(&self) {
        let matched = {
            let mut state = self.state.borrow_mut();
            let addresses: Vec<String> = state
                .recipients
                .iter()
                .map(|recipient| recipient.address.clone())
                .collect();
            let found = find_existing_conversation(&state.conversations, &addresses).cloned();
            state.existing_guid = found.as_ref().map(|chat| chat.guid.clone());
            found
        };
        if let Some(chat) = matched {
            self.existing_avatar
                .bind(&chat.name, chat.is_group(), chat.avatar.as_deref());
            self.existing_label
                .set_label(&format!("Existing conversation with {}", chat.name));
            self.existing_banner.set_visible(true);
            self.service_group.set_visible(false);
        } else {
            self.existing_banner.set_visible(false);
            self.service_group.set_visible(true);
        }
    }

    fn refresh_picker(&self) {
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        let query = self.entry.text().trim().to_owned();
        let (entries, is_direct_first, loading) = {
            let mut state = self.state.borrow_mut();
            let selected_keys: Vec<String> = state
                .recipients
                .iter()
                .map(|recipient| address_key(&recipient.address))
                .collect();
            let mut filtered: Vec<ContactEntry> = state
                .directory
                .iter()
                .filter(|contact| {
                    let key = address_key(&contact.address);
                    !selected_keys.contains(&key) && contact.matches_query(&query)
                })
                .take(MAX_SUGGESTIONS)
                .cloned()
                .collect();

            let mut direct_added = false;
            if looks_like_address(&query) {
                let query_key = address_key(&query);
                let already_selected = selected_keys.contains(&query_key);
                let exact_in_results = filtered
                    .iter()
                    .any(|contact| address_key(&contact.address) == query_key);
                if !already_selected && !exact_in_results {
                    filtered.insert(
                        0,
                        ContactEntry {
                            name: query.clone(),
                            address: query.clone(),
                            avatar: None,
                        },
                    );
                    direct_added = true;
                }
            }
            state.visible_entries = filtered.clone();
            (filtered, direct_added, state.loading_contacts)
        };

        self.section_label
            .set_label(if query.is_empty() || entries.is_empty() {
                "Contacts"
            } else {
                "Suggestions"
            });

        for (index, entry) in entries.iter().enumerate() {
            let row = gtk::ListBoxRow::new();
            row.add_css_class("contact-picker-row");
            let content = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            content.add_css_class("contact-picker-row-content");

            let is_direct = is_direct_first && index == 0;
            if is_direct {
                let badge = gtk::Box::new(gtk::Orientation::Horizontal, 0);
                badge.add_css_class("contact-add-badge");
                badge.set_halign(gtk::Align::Center);
                badge.set_valign(gtk::Align::Center);
                badge.append(&Icon::Plus.sized_widget(16));
                content.append(&badge);
            } else {
                let avatar = Avatar::new(34);
                avatar.bind(&entry.name, false, entry.avatar.as_deref());
                content.append(&avatar);
            }

            let labels = gtk::Box::new(gtk::Orientation::Vertical, 2);
            labels.set_hexpand(true);
            labels.set_valign(gtk::Align::Center);
            let title = gtk::Label::builder()
                .label(if is_direct {
                    format!("Message {}", entry.address)
                } else {
                    entry.name.clone()
                })
                .xalign(0.0)
                .ellipsize(pango::EllipsizeMode::End)
                .build();
            title.add_css_class("heading");
            labels.append(&title);

            let subtitle_text = if is_direct {
                Some("Add phone number or email")
            } else if entry.name != entry.address {
                Some(entry.address.as_str())
            } else {
                None
            };
            if let Some(sub) = subtitle_text {
                let subtitle = gtk::Label::builder()
                    .label(sub)
                    .xalign(0.0)
                    .ellipsize(pango::EllipsizeMode::End)
                    .build();
                subtitle.add_css_class("dim-label");
                subtitle.add_css_class("contact-picker-address");
                labels.append(&subtitle);
            }
            content.append(&labels);
            row.set_child(Some(&content));
            self.list.append(&row);
        }

        if entries.is_empty() {
            self.list.set_visible(false);
            self.empty_hint.set_visible(true);
            self.empty_hint.set_label(if loading {
                "Loading contacts…"
            } else if query.is_empty() {
                "Type a phone number or email address above to start a new conversation."
            } else {
                "No matching contacts. Enter a full phone number or email address to message anyone."
            });
        } else {
            self.list.set_visible(true);
            self.empty_hint.set_visible(false);
            if !query.is_empty()
                && let Some(first) = self.list.row_at_index(0)
            {
                self.list.select_row(Some(&first));
            }
        }
    }

    fn notify_changed(&self) {
        if let Some(callback) = self.callbacks.borrow().on_changed.as_ref() {
            callback();
        }
    }
}
