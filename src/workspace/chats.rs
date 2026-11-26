use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use adw::prelude::*;
use gtk::{gdk, gio, glib};

use crate::api::BlueBubbles;
use crate::avatar::Avatar;
use crate::model::{ContactEntry, Conversation, Message};
use crate::organization::{ChatAction, ChatOrganization};
use crate::{realtime, rows, settings};

use super::{
    GroupAvatar, Mode, RecoveredChats, Workspace, background, boxed, preview_background,
    timeline::Fetch,
};

impl Workspace {
    pub(super) fn refresh_chats(self: &Rc<Self>, full: bool) {
        let (server, session, recover_messages) = {
            let mut state = self.state.borrow_mut();
            if state.refreshing || state.connecting {
                return;
            }
            let Some(server) = state.server.clone() else {
                return;
            };
            state.refreshing = true;
            state.show_refresh = full || state.chats.is_empty();
            state.last_sidebar_check = Some(Instant::now());
            state.live_during_refresh.clear();
            (server, state.session, !state.live)
        };
        let notification_start = self.notification_start;
        self.update_connection_controls();
        background(
            move || {
                let cache = server.cache();
                let _profile = crate::profile::span("sidebar-sync-worker");
                let previous = cache.conversations();
                let baseline = if recover_messages {
                    previous
                        .conversations
                        .iter()
                        .map(|chat| (chat.guid.clone(), chat.last_activity))
                        .collect::<HashMap<_, _>>()
                } else {
                    HashMap::new()
                };
                let snapshot = server.sync_conversations(previous, full)?;
                cache.save_conversations(&snapshot);
                let mut missed = Vec::new();
                if recover_messages {
                    for chat in snapshot
                        .conversations
                        .iter()
                        .filter(|chat| {
                            chat.last_activity >= notification_start
                                && chat.last_activity
                                    > baseline.get(&chat.guid).copied().unwrap_or_default()
                        })
                        .take(12)
                    {
                        let since = baseline
                            .get(&chat.guid)
                            .copied()
                            .unwrap_or_default()
                            .max(notification_start);
                        missed.push((chat.clone(), since));
                    }
                }
                let contacts = cache.contacts();
                Ok((snapshot.conversations, contacts, missed))
            },
            {
                let weak = Rc::downgrade(self);
                move |result: Result<
                    (Vec<Conversation>, Vec<ContactEntry>, RecoveredChats),
                    String,
                >| {
                    let Some(app) = weak.upgrade() else {
                        return;
                    };
                    if app.state.borrow().session != session {
                        return;
                    }
                    {
                        let mut state = app.state.borrow_mut();
                        state.refreshing = false;
                        state.show_refresh = false;
                    }
                    app.update_connection_controls();
                    match result {
                        Ok((mut chats, contacts, missed)) => {
                            if !contacts.is_empty() {
                                app.state.borrow_mut().contacts = contacts;
                            }
                            app.preserve_live_chats(&mut chats);
                            app.replace_chats(chats);
                            if app.state.borrow().composing_new {
                                app.sync_new_message_directory();
                            } else if app.active_guid().is_none() {
                                app.restore_last_thread();
                            }
                            app.recover_notifications(session, missed);
                        }
                        Err(error) if full => app.notify_error(&error),
                        Err(_) => {}
                    }
                }
            },
        );
    }

    pub(super) fn start_realtime(self: &Rc<Self>, server: BlueBubbles) {
        self.realtime.replace(None);
        let session = {
            let mut state = self.state.borrow_mut();
            state.live = false;
            state.session
        };
        let Some((listener, events)) = realtime::Listener::start(server) else {
            return;
        };
        self.realtime.replace(Some(listener));
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            while let Ok(event) = events.recv().await {
                let Some(app) = weak.upgrade() else { break };
                if app.state.borrow().session != session || events.is_closed() {
                    break;
                }
                match event {
                    realtime::Event::Connection(connected) => {
                        if connected {
                            let _profile = crate::profile::span("realtime-connected");
                        }
                        if connected && app.state.borrow().was_live {
                            app.refresh_chats(false);
                            app.load_messages(Fetch::CatchUp);
                        }
                        let mut state = app.state.borrow_mut();
                        state.live = connected;
                        state.was_live |= connected;
                    }
                    realtime::Event::Message(delivery) => {
                        let realtime::Delivery {
                            chat,
                            message,
                            is_new,
                        } = *delivery;
                        app.receive_live_message(chat, message, is_new);
                    }
                }
            }
        });
    }

    fn preserve_live_chats(&self, chats: &mut Vec<Conversation>) {
        let state = self.state.borrow();
        let mut remaining: HashMap<_, _> = state
            .chats
            .iter()
            .filter(|chat| state.live_during_refresh.contains(&chat.guid))
            .map(|chat| (chat.guid.as_str(), chat))
            .collect();
        for chat in &mut *chats {
            if let Some(live) = remaining.remove(chat.guid.as_str())
                && live.last_activity >= chat.last_activity
            {
                chat.set_preview(live.last_activity, live.preview.clone());
            }
        }
        chats.extend(remaining.into_values().cloned());
        crate::model::sort_conversations(chats);
    }

    fn recover_notifications(self: &Rc<Self>, session: u64, chats: RecoveredChats) {
        if chats.is_empty() {
            return;
        }
        let Some(server) = self.state.borrow().server.clone() else {
            return;
        };
        background(
            move || {
                let mut recovered = Vec::new();
                for (chat, since) in chats {
                    let Ok((messages, _)) = server.messages(&chat.guid, 0) else {
                        continue;
                    };
                    recovered.extend(
                        messages
                            .into_iter()
                            .filter(|message| message.sent_at >= since)
                            .map(|message| (chat.clone(), message)),
                    );
                }
                Ok(recovered)
            },
            {
                let weak = Rc::downgrade(self);
                move |result| {
                    let (Some(app), Ok(recovered)) = (weak.upgrade(), result) else {
                        return;
                    };
                    if app.state.borrow().session == session {
                        for (chat, message) in recovered {
                            app.receive_live_message(chat, message, true);
                        }
                    }
                }
            },
        );
    }

    pub(super) fn receive_live_message(
        &self,
        mut chat: Conversation,
        mut message: Message,
        is_new: bool,
    ) {
        let guid = chat.guid.clone();
        let (sidebar_changed, thread_changed) = {
            let mut state = self.state.borrow_mut();
            let sidebar_changed =
                if let Some(known) = state.chats.iter_mut().find(|row| row.guid == guid) {
                    let changed = known.update_preview(&message);
                    chat.clone_from(known);
                    changed
                } else if message.tapback.is_none() {
                    chat.update_preview(&message);
                    state.chats.push(chat.clone());
                    true
                } else {
                    false
                };
            if sidebar_changed {
                if state.refreshing {
                    state.live_during_refresh.insert(guid.clone());
                }
                crate::model::sort_conversations(&mut state.chats);
                if let Some(active) = state.active.as_mut().filter(|active| active.guid == guid) {
                    active.clone_from(&chat);
                }
            }
            let active = state
                .active
                .as_ref()
                .is_some_and(|active| active.guid == guid);
            let mut changed = false;
            if let Some(thread) = state.threads.get_mut(&guid) {
                changed = thread.reconcile_send(&message);
                message.temp_guid = None;
                if let Some(rows) = thread.messages.as_mut() {
                    let is_newest = rows
                        .last()
                        .is_none_or(|last| message.sent_at >= last.sent_at);
                    let count = rows.len();
                    changed |= crate::model::merge_messages(rows, vec![message.clone()]);
                    if is_newest
                        && rows.len() > count
                        && let Some(offset) = &mut thread.offset
                    {
                        *offset += 1;
                    }
                } else {
                    thread.pending_live.push(message.clone());
                }
                thread.view_dirty |= changed && !active;
            }
            (sidebar_changed, active && changed)
        };
        if sidebar_changed {
            self.render_organized_chats();
        }
        if thread_changed {
            self.render_thread(&guid, is_new && message.tapback.is_none());
        }
        if is_new {
            self.notify_message(&chat, &message);
        }
    }

    fn notify_message(&self, chat: &Conversation, message: &Message) {
        if message.outgoing
            || message.tapback.is_some()
            || message.sent_at < self.notification_start
        {
            return;
        }
        let viewing = {
            let mut state = self.state.borrow_mut();
            if state.seen_notifications.contains(&message.guid) {
                return;
            }
            if state.seen_notifications.len() == 512 {
                state.seen_notifications.pop_front();
            }
            state.seen_notifications.push_back(message.guid.clone());
            if state.organization.is_archived(&chat.guid) || !self.notifications.get() {
                return;
            }
            state
                .active
                .as_ref()
                .is_some_and(|active| active.guid == chat.guid)
                && self.layout.shows_thread()
        };
        let Some(window) = self.window.upgrade() else {
            return;
        };
        if viewing && window.is_active() {
            return;
        }
        if let Some(app) = window.application() {
            let notification = gio::Notification::new(&chat.name);
            let preview: String = message.preview().chars().take(200).collect();
            let body = if chat.is_group()
                && let Some(address) = &message.sender
            {
                format!("{}: {preview}", chat.participant(address).name)
            } else {
                preview
            };
            notification.set_body(Some(&body));
            notification.set_icon(&gio::ThemedIcon::new("mail-message-new-symbolic"));
            notification.set_default_action_and_target_value(
                "app.open-conversation",
                Some(&chat.guid.to_variant()),
            );
            app.send_notification(Some(&chat.guid), &notification);
        }
    }

    pub(super) fn open_notification(self: &Rc<Self>, guid: &str) {
        let chat = self
            .state
            .borrow()
            .chats
            .iter()
            .find(|chat| chat.guid == guid)
            .cloned();
        if let Some(window) = self.window.upgrade() {
            crate::tray::present_window(&window, None);
            if let Some(app) = window.application() {
                app.withdraw_notification(guid);
            }
            if let Some(chat) = chat {
                self.activate_chat(chat, true);
            }
        }
    }

    pub(super) fn replace_chats(self: &Rc<Self>, chats: Vec<Conversation>) {
        let mut state = self.state.borrow_mut();
        if state.chats == chats {
            return;
        }
        let existing = state
            .chats
            .iter()
            .filter(|chat| state.threads.contains_key(&chat.guid))
            .map(|chat| (&chat.guid, chat))
            .collect::<HashMap<_, _>>();
        let revised = chats
            .iter()
            .filter_map(|updated| {
                let previous = existing.get(&updated.guid)?;
                (previous.participants != updated.participants
                    || previous.name != updated.name
                    || previous.is_group() != updated.is_group())
                .then_some(updated.guid.clone())
            })
            .collect::<Vec<_>>();
        for guid in revised {
            if let Some(thread) = state.threads.get_mut(&guid) {
                thread.view_dirty = true;
            }
        }
        let updated_thread = state.active.as_mut().and_then(|active| {
            let updated = chats.iter().find(|chat| chat.guid == active.guid)?;
            let participants_changed = active.participants != updated.participants
                || active.is_group() != updated.is_group();
            let avatar_changed = active.avatar != updated.avatar
                || active.group_photo != updated.group_photo
                || active.name != updated.name
                || active.is_group() != updated.is_group();
            *active = updated.clone();
            (participants_changed || avatar_changed).then(|| (active.clone(), participants_changed))
        });
        state.chats = chats;
        drop(state);
        self.render_organized_chats();
        if let Some((chat, participants_changed)) = updated_thread {
            self.update_thread_avatar(&chat);
            if participants_changed
                && self
                    .state
                    .borrow()
                    .threads
                    .get(&chat.guid)
                    .is_some_and(|thread| thread.messages.is_some())
            {
                self.render_thread(&chat.guid, false);
            }
        }
    }

    pub(super) fn render_organized_chats(&self) {
        let _profile = crate::profile::span("sidebar-update-ui");
        let state = self.state.borrow();
        let sidebar = state.organization.sidebar(&state.chats);
        let archive = state.organization.archive(&state.chats);
        drop(state);

        let revision = self.sidebar_revision.get().wrapping_add(1);
        self.sidebar_revision.set(revision);
        const FIRST_ROWS: usize = 64;
        const LATER_ROWS: usize = 96;
        if self.conversation_store.n_items() == 0 && sidebar.len() > LATER_ROWS {
            // Bind the first screen before adding the rest of the cached inventory.
            let mut remaining = sidebar.into_iter();
            self.conversation_store
                .extend_from_slice(&boxed(remaining.by_ref().take(FIRST_ROWS).collect()));
            let revision_cell = self.sidebar_revision.clone();
            let store = self.conversation_store.downgrade();
            glib::timeout_add_local(Duration::from_millis(8), move || {
                let Some(store) = store.upgrade() else {
                    return glib::ControlFlow::Break;
                };
                if revision_cell.get() != revision {
                    return glib::ControlFlow::Break;
                }
                let _profile = crate::profile::span("sidebar-batch-ui");
                store.extend_from_slice(&boxed(remaining.by_ref().take(LATER_ROWS).collect()));
                if remaining.len() == 0 {
                    glib::ControlFlow::Break
                } else {
                    glib::ControlFlow::Continue
                }
            });
        } else {
            rows::replace_changed(&self.conversation_store, sidebar);
        }
        if let Some(prefs) = self.prefs.borrow().as_ref() {
            prefs.set_archive(archive);
        }
    }

    pub(super) fn organize_chat(&self, action: ChatAction, guid: &str) {
        let changed = {
            let state = self.state.borrow();
            let mut updated = state.organization.clone();
            if !updated.apply(
                action,
                guid,
                state.chats.iter().find(|chat| chat.guid == guid),
            ) {
                return;
            }
            updated
        };
        if !self.save_organization(&changed) {
            return;
        }
        self.state.borrow_mut().organization = changed;
        if matches!(action, ChatAction::Archive) && self.active_guid().as_deref() == Some(guid) {
            self.close_thread();
            self.status_page("Select a conversation", None);
        }
        self.render_organized_chats();
    }

    pub(super) fn save_organization(&self, organization: &ChatOrganization) -> bool {
        if self.mode == Mode::Live
            && let Err(error) = settings::save_chat_organization(
                &self.state.borrow().server_address,
                organization.clone(),
            )
        {
            self.notify_error(&format!("Could not save conversation preferences. {error}"));
            false
        } else {
            true
        }
    }

    pub(super) fn restore_last_thread(self: &Rc<Self>) -> bool {
        let chat = {
            let state = self.state.borrow();
            state
                .organization
                .last_open
                .as_deref()
                .and_then(|guid| state.chats.iter().find(|chat| chat.guid == guid))
                .cloned()
        };
        if let Some(chat) = chat {
            self.activate_chat(chat, false);
            true
        } else {
            false
        }
    }

    pub(super) fn update_thread_avatar(self: &Rc<Self>, chat: &Conversation) {
        self.thread_title.set_title(&chat.name);
        rows::set_avatar(&self.thread_avatar, chat);
        if let Some(photo) = &chat.group_photo
            && chat.avatar.is_none()
        {
            self.request_group_avatar(
                chat.guid.clone(),
                photo.clone(),
                self.thread_avatar.downgrade(),
            );
        }
    }

    pub(super) fn request_group_avatar(
        self: &Rc<Self>,
        chat: String,
        photo: String,
        target: glib::WeakRef<Avatar>,
    ) {
        let mut state = self.state.borrow_mut();
        match state.group_avatars.get_mut(&photo) {
            Some(GroupAvatar::Ready(texture)) => set_group_photo(&target, &photo, texture),
            Some(GroupAvatar::Waiting(_, targets) | GroupAvatar::Loading(targets)) => {
                targets.push(target);
            }
            Some(GroupAvatar::RetryAfter(when)) if *when > Instant::now() => {}
            _ => {
                state
                    .group_avatars
                    .insert(photo, GroupAvatar::Waiting(chat, vec![target]));
            }
        }
        drop(state);
        self.load_group_avatars();
    }

    pub(super) fn load_group_avatars(self: &Rc<Self>) {
        let jobs = {
            let mut state = self.state.borrow_mut();
            let Some(server) = state.server.clone() else {
                return;
            };
            let session = state.session;
            state
                .group_avatars
                .iter_mut()
                .filter_map(|(photo, pending)| {
                    let GroupAvatar::Waiting(chat, targets) = pending else {
                        return None;
                    };
                    let job = (server.clone(), session, chat.clone(), photo.clone());
                    *pending = GroupAvatar::Loading(std::mem::take(targets));
                    Some(job)
                })
                .collect::<Vec<_>>()
        };
        for (server, session, chat, photo) in jobs {
            let id = photo.clone();
            preview_background(move || server.group_avatar(&chat, &id), {
                let weak = Rc::downgrade(self);
                move |result| {
                    let Some(app) = weak.upgrade() else { return };
                    let mut state = app.state.borrow_mut();
                    if state.session != session {
                        return;
                    }
                    let Some(GroupAvatar::Loading(targets)) = state.group_avatars.remove(&photo)
                    else {
                        return;
                    };
                    let image = result
                        .ok()
                        .and_then(|path| gtk::gdk_pixbuf::Pixbuf::from_file(path).ok())
                        .map(|bitmap| gdk::Texture::for_pixbuf(&bitmap));
                    let value = if let Some(image) = image {
                        for target in &targets {
                            set_group_photo(target, &photo, &image);
                        }
                        GroupAvatar::Ready(image)
                    } else {
                        GroupAvatar::RetryAfter(Instant::now() + Duration::from_secs(60))
                    };
                    state.group_avatars.insert(photo, value);
                }
            });
        }
    }
}

fn set_group_photo(target: &glib::WeakRef<Avatar>, photo: &str, texture: &gdk::Texture) {
    if let Some(target) = target.upgrade()
        && target.widget_name() == photo
    {
        target.set_photo(texture);
    }
}
