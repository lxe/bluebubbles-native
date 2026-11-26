use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use adw::prelude::*;
use gtk::{gdk, glib};

use crate::api::PAGE_SIZE;
use crate::cache::Cache;
use crate::model::{Conversation, Message, TapbackKind};
use crate::thread::{self, MessageRow};
use crate::timeline_view::TimelineView;
use crate::{rows, sample};

use super::{
    ACTIVE_SYNC_INTERVAL, Draft, MEMORY_THREAD_LIMIT, Mode, TIMELINE_VIEW_LIMIT, Workspace,
    background, boxed, composer::PendingSend,
};

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Fetch {
    Latest,
    CatchUp,
    Older,
}

pub(super) struct LoadedThread {
    pub(super) messages: Option<Vec<Message>>,
    pub(super) view_dirty: bool,
    pub(super) pending_live: Vec<Message>,
    pub(super) pending_reactions: HashMap<String, Option<TapbackKind>>,
    pub(super) loading_disk: bool,
    pub(super) fetching: bool,
    pub(super) loading_older: bool,
    pub(super) offset: Option<usize>,
    pub(super) has_older: bool,
    pub(super) pending_sends: Vec<PendingSend>,
    pub(super) confirmations: HashMap<String, Instant>,
    pub(super) older_requested: bool,
    pub(super) last_checked: Option<Instant>,
    pub(super) last_opened: Instant,
}

impl Default for LoadedThread {
    fn default() -> Self {
        Self {
            messages: None,
            view_dirty: true,
            pending_live: Vec::new(),
            pending_reactions: HashMap::new(),
            loading_disk: false,
            fetching: false,
            loading_older: false,
            offset: None,
            has_older: false,
            pending_sends: Vec::new(),
            confirmations: HashMap::new(),
            older_requested: false,
            last_checked: None,
            last_opened: Instant::now(),
        }
    }
}

impl LoadedThread {
    pub(super) fn reconcile_send(&mut self, confirmed: &Message) -> bool {
        if !confirmed.outgoing || confirmed.tapback.is_some() || self.pending_sends.is_empty() {
            return false;
        }
        let count = self.pending_sends.len();
        self.pending_sends.retain(|message| {
            let id = &message.job.temp_guid;
            id != &confirmed.guid && Some(id) != confirmed.temp_guid.as_ref()
        });
        let did_confirm = count != self.pending_sends.len();
        if did_confirm {
            self.confirmations
                .retain(|_, at| at.elapsed() < Duration::from_secs(1));
            self.confirmations
                .insert(confirmed.guid.clone(), Instant::now());
        }
        did_confirm
    }
}

impl Workspace {
    pub(super) fn activate_chat(self: &Rc<Self>, conversation: Conversation, remember: bool) {
        let _profile = crate::profile::span("thread-activate-ui");
        if remember
            && self.state.borrow().organization.last_open.as_deref() != Some(&conversation.guid)
        {
            let _profile = crate::profile::span("thread-save-selection-ui");
            let mut organization = self.state.borrow().organization.clone();
            organization.last_open = Some(conversation.guid.clone());
            if self.save_organization(&organization) {
                self.state.borrow_mut().organization = organization;
            }
        }
        {
            let mut state = self.state.borrow_mut();
            if let Some(previous) = state.active.as_ref() {
                if previous.guid == conversation.guid {
                    drop(state);
                    self.layout.show_thread();
                    self.update_active_highlight();
                    return;
                }
                let guid = previous.guid.clone();
                let draft = Draft {
                    text: self.editor_text(),
                    files: std::mem::take(&mut state.files),
                    reply: state.reply.take(),
                };
                state.drafts.insert(guid, draft);
            }
            state.active = Some(conversation.clone());
            let draft = state
                .drafts
                .get(&conversation.guid)
                .cloned()
                .unwrap_or_default();
            state.files = draft.files;
            state.reply = draft.reply;
            drop(state);
            self.editor.buffer().set_text(&draft.text);
        }
        let retained = self.select_timeline(&conversation.guid);
        self.layout.show_thread();
        self.update_active_highlight();
        {
            let _profile = crate::profile::span("thread-header-ui");
            self.update_thread_avatar(&conversation);
        }
        self.thread_avatar.set_visible(true);
        self.compose.set_visible(self.mode != Mode::Stress);
        self.render_files();
        if self.mode == Mode::Live {
            self.load_cached_thread(&conversation.guid, retained);
        } else if retained {
            self.thread_stack.set_visible_child_name("timeline");
            self.update_load_more();
        } else {
            let messages = sample::messages(&conversation);
            if self.mode == Mode::Stress {
                eprintln!(
                    "{}: {} message rows in model",
                    conversation.name,
                    messages.len()
                );
            }
            let view = self.view();
            let rows = boxed(thread::timeline(&messages, &conversation));
            view.store.extend_from_slice(&rows);
            if view.follow_latest.get() {
                view.scroll_latest();
            }
            self.thread_stack.set_visible_child_name("timeline");
            self.update_load_more();
        }
    }

    pub(super) fn load_cached_thread(self: &Rc<Self>, guid: &str, retained: bool) {
        let (server, session, warm, loading_disk, needs_sync, view_dirty) = {
            let mut state = self.state.borrow_mut();
            if state.threads.len() >= MEMORY_THREAD_LIMIT
                && !state.threads.contains_key(guid)
                && let Some(oldest) = state
                    .threads
                    .iter()
                    .filter(|(_, thread)| {
                        !thread.loading_disk
                            && !thread.fetching
                            && thread.pending_reactions.is_empty()
                            && thread.pending_sends.is_empty()
                    })
                    .min_by_key(|(_, thread)| thread.last_opened)
                    .map(|(guid, _)| guid.clone())
            {
                state.threads.remove(&oldest);
            }
            let server = state.server_address.clone();
            let session = state.session;
            let activity = state
                .active
                .as_ref()
                .map(|chat| chat.last_activity)
                .unwrap_or_default();
            let thread = state.threads.entry(guid.to_owned()).or_default();
            thread.last_opened = Instant::now();
            let warm = thread.messages.is_some();
            let loading_disk = std::mem::replace(&mut thread.loading_disk, !warm);
            let latest = thread.messages.as_ref().and_then(|rows| rows.last());
            let needs_sync = thread.last_checked.is_none()
                || latest.is_some_and(|message| activity > message.sent_at);
            (
                server,
                session,
                warm,
                loading_disk,
                needs_sync,
                thread.view_dirty,
            )
        };
        if warm {
            let view = self.view();
            if retained && !view_dirty && view.store.n_items() > 0 {
                self.cancel_loading_indicator();
                self.thread_stack.set_visible_child_name("timeline");
                self.update_load_more();
                self.schedule_previews();
            } else {
                self.render_thread(guid, view.follow_latest.get() && !retained);
            }
            if needs_sync {
                self.load_messages(Fetch::Latest);
            }
            return;
        }
        self.cancel_loading_indicator();
        self.view().store.remove_all();
        self.thread_stack.set_visible_child_name("waiting");
        self.wait_for_thread();
        if loading_disk {
            return;
        }
        let guid = guid.to_owned();
        background(
            {
                let guid = guid.clone();
                move || {
                    let _profile = crate::profile::span("thread-cache-read-worker");
                    Ok(Cache::new(&server).messages(&guid))
                }
            },
            {
                let weak = Rc::downgrade(self);
                move |result| {
                    let Some(app) = weak.upgrade() else { return };
                    {
                        let mut state = app.state.borrow_mut();
                        if state.session != session {
                            return;
                        }
                        let Some(thread) = state.threads.get_mut(&guid) else {
                            return;
                        };
                        thread.loading_disk = false;
                        let mut messages = result.unwrap_or_default();
                        crate::model::merge_messages(
                            &mut messages,
                            std::mem::take(&mut thread.pending_live),
                        );
                        thread.messages = Some(messages);
                    }
                    if app.active_guid().as_deref() == Some(&guid) {
                        app.render_thread(&guid, app.view().follow_latest.get());
                        app.load_messages(Fetch::Latest);
                    }
                }
            },
        );
    }

    pub(super) fn load_messages(self: &Rc<Self>, fetch: Fetch) {
        let (server, guid, session, offset) = {
            let mut state = self.state.borrow_mut();
            if state.connecting {
                return;
            }
            let (Some(server), Some(active)) = (state.server.clone(), state.active.as_ref()) else {
                return;
            };
            let guid = active.guid.clone();
            let session = state.session;
            let Some(thread) = state.threads.get_mut(&guid) else {
                return;
            };
            if thread.loading_disk || thread.fetching {
                thread.older_requested |= fetch == Fetch::Older && thread.has_older;
                return;
            }
            let offset = match fetch {
                Fetch::Older if thread.has_older => thread.offset.unwrap_or_default(),
                Fetch::Older => return,
                Fetch::Latest
                    if thread
                        .last_checked
                        .is_some_and(|last| last.elapsed() < ACTIVE_SYNC_INTERVAL) =>
                {
                    return;
                }
                Fetch::Latest | Fetch::CatchUp => 0,
            };
            thread.fetching = true;
            thread.loading_older = fetch == Fetch::Older;
            (server, guid, session, offset)
        };
        if self.view().store.n_items() == 0 {
            self.wait_for_thread();
        }
        self.update_load_more();
        background(
            {
                let guid = guid.clone();
                move || {
                    let _profile = crate::profile::span("thread-sync-worker");
                    server.messages(&guid, offset)
                }
            },
            {
                let weak = Rc::downgrade(self);
                move |result| {
                    let Some(app) = weak.upgrade() else { return };
                    let (visible, older_requested) = {
                        let mut state = app.state.borrow_mut();
                        if state.session != session {
                            return;
                        }
                        let visible = state.active.as_ref().is_some_and(|chat| chat.guid == guid);
                        let Some(thread) = state.threads.get_mut(&guid) else {
                            return;
                        };
                        thread.fetching = false;
                        thread.loading_older = false;
                        if fetch != Fetch::Older {
                            thread.last_checked = Some(Instant::now());
                        }
                        (visible, std::mem::take(&mut thread.older_requested))
                    };
                    match result {
                        Ok((rows, count)) => app.apply_messages(&guid, visible, fetch, rows, count),
                        Err(error) if visible => {
                            if app.view().store.n_items() == 0 {
                                app.status_page("Could not open conversation", Some(&error));
                            } else if fetch == Fetch::Older {
                                app.notify_error(&error);
                            }
                        }
                        Err(_) => {}
                    }
                    if visible {
                        app.update_load_more();
                    }
                    if visible && older_requested && fetch != Fetch::Older {
                        app.load_messages(Fetch::Older);
                    }
                }
            },
        );
    }

    pub(super) fn apply_messages(
        &self,
        guid: &str,
        visible: bool,
        fetch: Fetch,
        mut rows: Vec<Message>,
        count: usize,
    ) {
        let (changed, new_content) = {
            let mut state = self.state.borrow_mut();
            let thread = state.threads.get_mut(guid).expect("Requested conversation");
            let mut reconciled = false;
            for message in &mut rows {
                reconciled |= thread.reconcile_send(message);
                message.temp_guid = None;
            }
            let messages = thread.messages.get_or_insert_with(Vec::new);
            let previous = messages.len();
            let latest = messages
                .iter()
                .rev()
                .find(|message| message.tapback.is_none());
            let new_content = fetch != Fetch::Older
                && rows.iter().any(|row| {
                    row.tapback.is_none()
                        && latest.is_none_or(|message| row.sent_at >= message.sent_at)
                        && !messages.iter().any(|message| message.guid == row.guid)
                });
            let changed = crate::model::merge_messages(messages, rows);
            let added = messages.len() - previous;
            if fetch == Fetch::Older {
                thread.offset = Some(thread.offset.unwrap_or_default() + count);
                thread.has_older = count == PAGE_SIZE;
            } else if thread.offset.is_none() || added >= PAGE_SIZE {
                thread.offset = Some(count);
                thread.has_older = count == PAGE_SIZE;
            } else if let Some(offset) = thread.offset.as_mut() {
                *offset += added;
            }
            thread.view_dirty |= !visible && (changed || reconciled);
            (changed || reconciled, new_content)
        };
        if visible {
            let view = self.view();
            let empty = view.store.n_items() == 0;
            if changed || empty {
                self.render_thread(guid, view.follow_latest.get() && (empty || new_content));
            }
        }
    }

    pub(super) fn render_thread(&self, guid: &str, scroll_latest: bool) {
        let _profile = crate::profile::span("thread-update-ui");
        let view = self.view();
        let (rows, pending, online) = {
            let state = self.state.borrow();
            let thread = &state.threads[guid];
            (
                state
                    .active
                    .as_ref()
                    .map(|chat| {
                        let messages = thread.messages.as_deref().unwrap_or(&[]);
                        let _profile = crate::profile::span("thread-project-rows-ui");
                        let mut rows = thread::timeline(messages, chat);
                        if !thread.confirmations.is_empty() {
                            for row in &mut rows {
                                row.confirmed_at =
                                    thread.confirmations.get(&row.message.guid).copied();
                            }
                        }
                        let mut last_sent_at = rows
                            .last()
                            .map(|row| row.message.sent_at)
                            .filter(|&t| t > 0);
                        for pending in &thread.pending_sends {
                            let service = pending.message.service.unwrap_or_else(|| chat.service());
                            let show_time =
                                thread::should_show_time(pending.message.sent_at, last_sent_at);
                            if pending.message.sent_at > 0 {
                                last_sent_at = Some(pending.message.sent_at);
                            }
                            rows.push(MessageRow {
                                message: pending.message.clone(),
                                service,
                                group: chat.is_group(),
                                starts_group: show_time,
                                show_time,
                                sender: None,
                                quote: pending.job.reply.as_ref().map(|reply| reply.quote.clone()),
                                reactions: Vec::new(),
                                my_reaction: None,
                                send_state: Some(pending.status),
                                confirmed_at: None,
                            });
                        }
                        if !thread.pending_reactions.is_empty() {
                            for row in &mut rows {
                                if let Some(reaction) =
                                    thread.pending_reactions.get(&row.message.guid)
                                {
                                    row.set_my_reaction(*reaction);
                                }
                            }
                        }
                        rows
                    })
                    .unwrap_or_default(),
                thread.loading_disk
                    || thread.fetching
                    || (thread.last_checked.is_none()
                        && (state.server.is_some()
                            || state.connecting
                            || state.restoring_connection)),
                state.server.is_some(),
            )
        };
        if let Some(thread) = self.state.borrow_mut().threads.get_mut(guid) {
            thread.view_dirty = false;
        }
        if rows.is_empty() {
            view.store.remove_all();
            if pending {
                self.wait_for_thread();
            } else if online {
                self.status_page("No messages to show", None);
            } else {
                self.status_page(
                    "No saved messages",
                    Some("Connect to download this conversation."),
                );
            }
            return;
        }

        self.cancel_loading_indicator();
        {
            let _profile = crate::profile::span("thread-bind-rows-ui");
            rows::replace_changed(&view.store, rows);
        }
        self.thread_stack.set_visible_child_name("timeline");
        if scroll_latest || view.follow_latest.get() {
            view.scroll_latest();
        } else {
            view.queue_scroll_bounds();
        }
        self.update_load_more();
        self.update_composer();
    }

    pub(super) fn update_load_more(&self) {
        let Ok(state) = self.state.try_borrow() else {
            return;
        };
        let thread = state
            .active
            .as_ref()
            .and_then(|chat| state.threads.get(&chat.guid));
        let view = self.view();
        let adjustment = view.scroll.vadjustment();
        let timeline_visible =
            self.thread_stack.visible_child_name().as_deref() == Some("timeline");
        self.jump_latest
            .set_visible(timeline_visible && !view.follow_latest.get() && view.bottom_gap() > 48.0);
        let show = state.server.is_some()
            && !view.follow_latest.get()
            && thread.is_some_and(|thread| thread.has_older)
            && adjustment.value() - adjustment.lower() <= 48.0
            && timeline_visible;
        self.load_more.set_visible(show);
        if show {
            let loading =
                thread.is_some_and(|thread| thread.loading_older || thread.older_requested);
            self.load_more.set_sensitive(!loading);
            let label = if loading {
                "Loading More Messages…"
            } else {
                "Load More Messages"
            };
            if self.load_more.label().as_deref() != Some(label) {
                self.load_more.set_label(label);
            }
        }
    }

    pub(super) fn wait_for_thread(&self) {
        if self.loading_indicator.borrow().is_some()
            || self.thread_stack.visible_child_name().as_deref() == Some("loading")
        {
            return;
        }
        self.thread_stack.set_visible_child_name("waiting");
        let stack = self.thread_stack.downgrade();
        self.loading_indicator
            .replace(Some(glib::timeout_add_local_once(
                Duration::from_millis(250),
                move || {
                    if let Some(stack) = stack.upgrade()
                        && stack.visible_child_name().as_deref() == Some("waiting")
                    {
                        let _profile = crate::profile::span("thread-loading-indicator");
                        stack.set_visible_child_name("loading");
                    }
                },
            )));
    }

    pub(super) fn cancel_loading_indicator(&self) {
        if let Some(source) = self.loading_indicator.borrow_mut().take() {
            // A completed timeout has already switched the stack to the spinner.
            if self.thread_stack.visible_child_name().as_deref() == Some("waiting") {
                source.remove();
            }
        }
    }

    pub(super) fn update_empty_thread(&self) {
        if self.view().store.n_items() == 0
            && let Some(guid) = self.active_guid()
            && self
                .state
                .borrow()
                .threads
                .get(&guid)
                .is_some_and(|thread| thread.messages.is_some())
        {
            self.render_thread(&guid, false);
        }
    }

    pub(super) fn scroll_latest(&self) {
        self.view().scroll_latest();
    }

    pub(super) fn scroll_to_message(&self, position: u32) {
        self.view()
            .list
            .scroll_to(position, gtk::ListScrollFlags::NONE, None);
    }

    pub(super) fn view(&self) -> Rc<TimelineView> {
        self.timeline_view.borrow().clone()
    }

    pub(super) fn select_timeline(self: &Rc<Self>, guid: &str) -> bool {
        let _profile = crate::profile::span("thread-select-view-ui");
        let previous = self.view();
        let mut cache = self.timeline_cache.borrow_mut();
        let retained = cache.iter().position(|(id, _)| id == guid);
        let view = if let Some(index) = retained {
            cache.remove(index).expect("Retained timeline").1
        } else {
            let view = Rc::new(TimelineView::new(&self.counts, &self.attachment_handler));
            let saved = self.saved_scroll.borrow_mut().remove(guid);
            view.restore_offset.set(saved);
            view.follow_latest.set(saved.is_none());
            self.connect_timeline(&view);
            self.timeline_pages.add_child(&view.scroll);
            view
        };
        // The initial blank page has no conversation and can be discarded.
        if !cache.iter().any(|(_, entry)| Rc::ptr_eq(entry, &previous))
            && !Rc::ptr_eq(&previous, &view)
        {
            self.timeline_pages.remove(&previous.scroll);
        }
        cache.push_back((guid.to_owned(), view.clone()));
        self.timeline_view.replace(view.clone());
        self.timeline_pages.set_visible_child(&view.scroll);
        if view.restore_offset.get().is_some() {
            view.queue_scroll_bounds();
        }
        if cache.len() > TIMELINE_VIEW_LIMIT
            && let Some((id, old)) = cache.pop_front()
        {
            if let Some(offset) = old.saved_offset() {
                self.saved_scroll.borrow_mut().insert(id, offset);
            }
            self.timeline_pages.remove(&old.scroll);
        }
        retained.is_some()
    }

    pub(super) fn connect_timeline(self: &Rc<Self>, view: &Rc<TimelineView>) {
        let adjustment = view.scroll.vadjustment();
        adjustment.connect_value_changed({
            let app = Rc::downgrade(self);
            let view = Rc::downgrade(view);
            move |_| {
                if let (Some(app), Some(view)) = (app.upgrade(), view.upgrade())
                    && Rc::ptr_eq(&app.view(), &view)
                {
                    app.schedule_previews();
                    app.update_load_more();
                }
            }
        });
        adjustment.connect_changed({
            let app = Rc::downgrade(self);
            let view = Rc::downgrade(view);
            move |_| {
                if let (Some(app), Some(view)) = (app.upgrade(), view.upgrade())
                    && Rc::ptr_eq(&app.view(), &view)
                {
                    view.queue_scroll_bounds();
                    app.schedule_previews();
                    app.update_load_more();
                }
            }
        });
        let manual_scroll =
            gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        manual_scroll.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(view);
        manual_scroll.connect_scroll(move |_, _, dy| {
            if dy != 0.0
                && let Some(view) = weak.upgrade()
            {
                view.follow_latest.set(false);
                view.restore_offset.set(None);
            }
            glib::Propagation::Proceed
        });
        view.scroll.add_controller(manual_scroll);

        let scrollbar_press = gtk::GestureClick::new();
        scrollbar_press.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(view);
        scrollbar_press.connect_pressed(move |_, _, _, _| {
            if let Some(view) = weak.upgrade() {
                view.follow_latest.set(false);
                view.restore_offset.set(None);
            }
        });
        view.scroll.vscrollbar().add_controller(scrollbar_press);

        let scroll_keys = gtk::EventControllerKey::new();
        scroll_keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(view);
        scroll_keys.connect_key_pressed(move |_, key, _, _| {
            if matches!(key, gdk::Key::Up | gdk::Key::Page_Up | gdk::Key::Home)
                && let Some(view) = weak.upgrade()
            {
                view.follow_latest.set(false);
                view.restore_offset.set(None);
            }
            glib::Propagation::Proceed
        });
        view.scroll.add_controller(scroll_keys);
    }

    pub(super) fn message_position(&self, guid: &str) -> Option<u32> {
        (0..self.view().store.n_items()).find(|&index| {
            self.view()
                .store
                .item(index)
                .and_downcast::<glib::BoxedAnyObject>()
                .and_then(|row| {
                    row.try_borrow::<MessageRow>()
                        .ok()
                        .map(|row| row.message.guid.eq_ignore_ascii_case(guid))
                })
                .unwrap_or(false)
        })
    }

    pub(super) fn message_row(&self, guid: &str) -> Option<MessageRow> {
        self.message_row_at(self.message_position(guid)?)
    }

    pub(super) fn message_row_at(&self, index: u32) -> Option<MessageRow> {
        let row = self
            .view()
            .store
            .item(index)
            .and_downcast::<glib::BoxedAnyObject>()?;
        row.try_borrow::<MessageRow>().ok().map(|row| row.clone())
    }
}
