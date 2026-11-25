use std::collections::VecDeque;
use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, gio, glib};

use crate::api::BlueBubbles;
use crate::icons::Icon;
use crate::media as media_ui;
use crate::model::{Conversation, Message, MessagePointer, Service, TapbackKind};
use crate::thread::{self, MessageRow, SendState};

use super::{
    Mode, ReplyDraft, Workspace, background, boxed, preview_background, send_background,
    timeline::Fetch,
};

#[derive(Clone)]
pub(super) enum SendPart {
    Text(String),
    File(PathBuf),
}

impl SendPart {
    pub(super) fn preview(&self) -> String {
        match self {
            Self::Text(text) => text.clone(),
            Self::File(path) => path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
        }
    }
}

#[derive(Clone)]
pub(super) struct SendJob {
    pub(super) temp_guid: String,
    pub(super) part: SendPart,
    pub(super) reply: Option<ReplyDraft>,
}

pub(super) struct PendingSend {
    pub(super) job: SendJob,
    pub(super) message: Message,
    pub(super) status: SendState,
}

impl Workspace {
    pub(super) fn editor_text(&self) -> String {
        let buffer = self.editor.buffer();
        buffer
            .text(&buffer.start_iter(), &buffer.end_iter(), false)
            .to_string()
    }

    pub(super) fn resize_editor(&self) {
        let _profile = crate::profile::span("composer-layout-ui");
        let width = self.composer_grid.width();
        if width < 120 {
            return;
        }

        let text = self.editor_text();
        let layout =
            self.editor
                .create_pango_layout(Some(if text.is_empty() { " " } else { &text }));
        layout.set_wrap(gtk::pango::WrapMode::WordChar);
        layout.set_width((width - 76).max(80) * gtk::pango::SCALE);
        let expanded = layout.line_count() > 1;
        if expanded != self.composer_grid.has_css_class("expanded") {
            let manager = self.composer_grid.layout_manager().unwrap();
            let attach = manager
                .layout_child(&self.attach)
                .downcast::<gtk::GridLayoutChild>()
                .unwrap();
            let send = manager
                .layout_child(&self.send)
                .downcast::<gtk::GridLayoutChild>()
                .unwrap();
            let input = manager
                .layout_child(&self.editor_entry)
                .downcast::<gtk::GridLayoutChild>()
                .unwrap();
            attach.set_row(i32::from(expanded));
            send.set_row(i32::from(expanded));
            let controls_alignment = if expanded {
                gtk::Align::End
            } else {
                gtk::Align::Center
            };
            self.attach.set_valign(controls_alignment);
            self.send.set_valign(controls_alignment);
            input.set_column(if expanded { 0 } else { 1 });
            input.set_column_span(if expanded { 3 } else { 1 });
            if expanded {
                self.composer_grid.add_css_class("expanded");
            } else {
                self.composer_grid.remove_css_class("expanded");
            }
        }
        let height = if expanded {
            layout.set_width((width - 12).max(80) * gtk::pango::SCALE);
            (layout.pixel_size().1 + 13).clamp(28, 118)
        } else {
            (layout.pixel_size().1 + self.editor.top_margin() + self.editor.bottom_margin()).max(28)
        };
        if self.editor_scroll.height_request() != height {
            self.editor_scroll.set_height_request(height);
        }
        if height < 118 {
            let adjustment = self.editor_scroll.vadjustment();
            glib::idle_add_local_once(move || adjustment.set_value(adjustment.lower()));
        }
    }

    pub(super) fn composer_ready_guid(&self) -> Option<String> {
        let state = self.state.borrow();
        if (state.server.is_some() || self.mode == Mode::Patterns) && !state.connecting {
            state.active.as_ref().map(|active| active.guid.clone())
        } else {
            None
        }
    }

    pub(super) fn active_service(&self) -> Service {
        let state = self.state.borrow();
        let Some(active) = state.active.as_ref() else {
            return Service::IMessage;
        };
        if let Some(service) = Service::from_chat_guid(&active.guid) {
            return service;
        }
        state
            .threads
            .get(&active.guid)
            .and_then(|thread| thread.messages.as_deref())
            .and_then(|messages| {
                messages
                    .iter()
                    .rev()
                    .find(|message| message.tapback.is_none())
                    .and_then(|message| message.service)
            })
            .unwrap_or_else(|| active.service())
    }

    pub(super) fn update_composer(&self) {
        let ready = self.composer_ready_guid().is_some();
        let has_files = !self.state.borrow().files.is_empty();
        let service = self.active_service();
        if service.is_green() {
            self.compose.add_css_class("sms");
        } else {
            self.compose.remove_css_class("sms");
        }
        self.editor_placeholder
            .set_label(service.composer_placeholder());
        self.editor_placeholder
            .set_visible(self.editor_text().is_empty());
        self.editor.set_editable(ready);
        self.attach.set_sensitive(ready);
        self.pending.set_sensitive(ready);
        self.send
            .set_sensitive(ready && (!self.editor_text().trim().is_empty() || has_files));
    }

    pub(super) fn paste_clipboard_media(self: &Rc<Self>, fallback_text: bool) -> bool {
        let Some(guid) = self.composer_ready_guid() else {
            return false;
        };
        let clipboard = self.editor.clipboard();
        if !media_ui::clipboard_has_media(&clipboard.formats()) {
            return false;
        }
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = media_ui::read_clipboard_media(&clipboard).await;
            let Some(app) = weak.upgrade() else {
                return;
            };
            if app.composer_ready_guid().as_deref() != Some(guid.as_str()) {
                return;
            }
            match result {
                Ok(paths) if !paths.is_empty() => {
                    app.add_files(&guid, paths.into_iter());
                    app.editor.grab_focus();
                }
                Ok(_) if fallback_text => {
                    app.editor.buffer().paste_clipboard(&clipboard, None, true);
                }
                Ok(_) => {}
                Err(error) => app.notify_error(&error),
            }
        });
        true
    }

    pub(super) fn pick_files(self: &Rc<Self>) {
        let (Some(window), Some(guid)) = (self.window.upgrade(), self.active_guid()) else {
            return;
        };
        let dialog = gtk::FileDialog::builder().title("Attach Files").build();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            if let Ok(files) = dialog.open_multiple_future(Some(&window)).await
                && let Some(app) = weak.upgrade()
            {
                app.add_files(
                    &guid,
                    files
                        .iter::<gio::File>()
                        .filter_map(Result::ok)
                        .filter_map(|file| file.path()),
                );
            }
        });
    }

    pub(super) fn add_files(self: &Rc<Self>, guid: &str, files: impl Iterator<Item = PathBuf>) {
        let mut state = self.state.borrow_mut();
        if state.connecting
            || state
                .active
                .as_ref()
                .is_none_or(|active| active.guid != guid)
        {
            return;
        }
        for path in files {
            if path.is_file() && !state.files.contains(&path) {
                state.files.push(path);
            }
        }
        drop(state);
        self.render_files();
    }

    pub(super) fn render_files(self: &Rc<Self>) {
        while let Some(child) = self.pending.first_child() {
            self.pending.remove(&child);
        }
        let (files, reply) = {
            let state = self.state.borrow();
            (state.files.clone(), state.reply.clone())
        };
        self.pending
            .set_visible(!files.is_empty() || reply.is_some());
        if let Some(reply) = reply {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            row.add_css_class("thread-quote");
            let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
            text.set_hexpand(true);
            let title = gtk::Label::new(Some(&format!("Replying to {}", reply.quote.author)));
            title.set_xalign(0.0);
            title.add_css_class("heading");
            let excerpt = gtk::Label::new(Some(&reply.quote.text));
            excerpt.set_ellipsize(gtk::pango::EllipsizeMode::End);
            excerpt.set_xalign(0.0);
            text.append(&title);
            text.append(&excerpt);
            row.append(&text);
            let remove = Icon::Remove.button("Cancel reply");
            remove.add_css_class("flat");
            remove.connect_clicked({
                let weak = Rc::downgrade(self);
                move |_| {
                    if let Some(app) = weak.upgrade() {
                        app.state.borrow_mut().reply = None;
                        app.render_files();
                    }
                }
            });
            row.append(&remove);
            self.pending.append(&row);
        }
        for (index, path) in files.iter().enumerate() {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            row.add_css_class("pending-attachment");
            let kind = media_ui::Kind::of_path(path);
            if kind == media_ui::Kind::Image {
                let preview = gtk::Stack::new();
                preview.add_css_class("pending-thumb");
                preview.set_size_request(28, 28);
                preview.set_valign(gtk::Align::Center);
                preview.set_overflow(gtk::Overflow::Hidden);
                preview.add_named(&kind.icon().widget(), Some("icon"));
                let picture = gtk::Picture::builder()
                    .can_shrink(true)
                    .content_fit(gtk::ContentFit::Cover)
                    .width_request(28)
                    .height_request(28)
                    .build();
                preview.add_named(&picture, Some("thumb"));
                preview.set_visible_child_name("icon");
                row.append(&preview);
                let path = path.clone();
                let weak_preview = preview.downgrade();
                let weak_picture = picture.downgrade();
                preview_background(
                    move || media_ui::image_texture(&path, Some(64)).map_err(|e| e.to_string()),
                    move |result: Result<gdk::Texture, String>| {
                        if let (Ok(texture), Some(preview), Some(picture)) =
                            (result, weak_preview.upgrade(), weak_picture.upgrade())
                        {
                            picture.set_paintable(Some(&texture));
                            preview.set_visible_child_name("thumb");
                        }
                    },
                );
            } else {
                row.append(&kind.icon().widget());
            }
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            let label = gtk::Label::builder()
                .label(name.as_ref())
                .halign(gtk::Align::Start)
                .hexpand(true)
                .ellipsize(gtk::pango::EllipsizeMode::Middle)
                .build();
            let remove = Icon::Remove.button(&format!("Remove {name}"));
            remove.add_css_class("flat");
            row.append(&label);
            row.append(&remove);
            self.pending.append(&row);
            remove.connect_clicked({
                let weak = Rc::downgrade(self);
                move |_| {
                    if let Some(app) = weak.upgrade() {
                        app.state.borrow_mut().files.remove(index);
                        app.render_files();
                    }
                }
            });
        }
        self.update_composer();
    }

    pub(super) fn send_draft(self: &Rc<Self>) {
        if !self.send.is_sensitive() {
            return;
        }
        let Some(guid) = self.active_guid() else {
            return;
        };
        let body = self.editor_text();
        let (files, reply) = {
            let state = self.state.borrow();
            (state.files.clone(), state.reply.clone())
        };
        let active_service = self.active_service();
        if self.mode == Mode::Patterns {
            let text = body.trim();
            self.view()
                .store
                .append(&glib::BoxedAnyObject::new(MessageRow {
                    message: Message {
                        guid: uuid::Uuid::new_v4().to_string(),
                        text: if text.is_empty() {
                            "Sample attachment".into()
                        } else {
                            text.into()
                        },
                        outgoing: true,
                        reply_to: reply.as_ref().map(|reply| reply.target.clone()),
                        service: Some(active_service),
                        ..Message::default()
                    },
                    service: active_service,
                    group: self
                        .state
                        .borrow()
                        .active
                        .as_ref()
                        .is_some_and(Conversation::is_group),
                    starts_group: true,
                    show_time: false,
                    sender: None,
                    quote: reply.map(|reply| reply.quote),
                    reactions: Vec::new(),
                    my_reaction: None,
                    send_state: None,
                    confirmed_at: None,
                }));
            self.state.borrow_mut().files.clear();
            self.state.borrow_mut().reply = None;
            self.editor.buffer().set_text("");
            self.render_files();
            self.scroll_latest();
            return;
        }
        let Some(server) = self.state.borrow().server.clone() else {
            return;
        };
        if reply.is_some() && !server.supports_message_actions() {
            self.notify_error("Sending replies requires the BlueBubbles Private API helper.");
            return;
        }
        let _profile = crate::profile::span("send-optimistic-ui");
        let parts = (!body.trim().is_empty())
            .then(|| SendPart::Text(body.trim().to_owned()))
            .into_iter()
            .chain(files.into_iter().map(SendPart::File));
        let timestamp = glib::real_time() / 1_000;
        let mut state = self.state.borrow_mut();
        let mut pending = Vec::new();
        let mut queue = VecDeque::new();
        for part in parts {
            let job = SendJob {
                temp_guid: format!("temp-{}", uuid::Uuid::new_v4()),
                part,
                reply: reply.clone(),
            };
            let message = Message {
                guid: job.temp_guid.clone(),
                text: job.part.preview(),
                outgoing: true,
                sent_at: timestamp,
                reply_to: reply.as_ref().map(|reply| reply.target.clone()),
                service: Some(active_service),
                ..Message::default()
            };
            queue.push_back(job.clone());
            pending.push(PendingSend {
                job,
                message,
                status: SendState::Sending,
            });
        }
        let Some(batch_id) = queue.front().map(|job| job.temp_guid.clone()) else {
            return;
        };
        state.outbox.insert(batch_id.clone(), queue);
        state
            .threads
            .entry(guid.clone())
            .or_default()
            .pending_sends
            .extend(pending);
        state.files.clear();
        state.reply = None;
        drop(state);
        self.editor.buffer().set_text("");
        self.render_files();
        self.render_thread(&guid, true);
        self.update_connection_controls();
        self.dispatch_send(guid, batch_id);
    }

    fn dispatch_send(self: &Rc<Self>, chat: String, batch_id: String) {
        let (job, server, session) = {
            let state = self.state.borrow();
            let Some(job) = state.outbox.get(&batch_id).and_then(VecDeque::front) else {
                return;
            };
            let Some(server) = state.server.clone() else {
                return;
            };
            (job.clone(), server, state.session)
        };
        let target = chat.clone();
        let temp_guid = job.temp_guid.clone();
        let weak = Rc::downgrade(self);
        send_background(
            move || {
                let _profile = crate::profile::span("send-http-worker");
                let reply = job.reply.as_ref().map(|draft| &draft.target);
                let result = match job.part {
                    SendPart::Text(text) => server.send_text(&target, &text, reply, &job.temp_guid),
                    SendPart::File(path) => {
                        server.send_attachment(&target, &path, reply, &job.temp_guid)
                    }
                }?;
                if let Some(message) = result.as_ref() {
                    server
                        .cache()
                        .merge_messages(&target, std::slice::from_ref(message));
                }
                Ok(result)
            },
            move |result| {
                if let Some(app) = weak.upgrade() {
                    let current = app.state.borrow().session;
                    if current == session {
                        app.finish_send(chat, batch_id, &temp_guid, result);
                    }
                }
            },
        );
    }

    fn finish_send(
        self: &Rc<Self>,
        chat: String,
        batch_id: String,
        temp_guid: &str,
        result: Result<Option<Message>, String>,
    ) {
        let visible = self.active_guid().as_deref() == Some(&chat);
        let (conversation, sync, report_error, more) = {
            let mut state = self.state.borrow_mut();
            let Some(queue) = state.outbox.get_mut(&batch_id) else {
                return;
            };
            queue.pop_front();
            let more = !queue.is_empty();
            if !more {
                state.outbox.remove(&batch_id);
            }
            let thread = &mut state.threads.get_mut(&chat).expect("Queued conversation");
            let pending = thread
                .pending_sends
                .iter_mut()
                .find(|pending| pending.job.temp_guid == temp_guid);
            let sync = pending.is_some() && matches!(result, Ok(None));
            let report_error = pending.is_some() && result.is_err();
            if let Some(pending) = pending {
                thread.view_dirty |= !visible;
                pending.status = if report_error {
                    SendState::Failed
                } else {
                    SendState::Unconfirmed
                };
            }
            let conversation = state
                .chats
                .iter()
                .find(|known| known.guid == chat)
                .cloned()
                .or_else(|| {
                    state
                        .active
                        .as_ref()
                        .filter(|active| active.guid == chat)
                        .cloned()
                });
            (conversation, sync, report_error, more)
        };
        if more {
            self.dispatch_send(chat.clone(), batch_id);
        }
        self.update_connection_controls();
        match result {
            Ok(Some(mut message)) => {
                message.temp_guid = Some(temp_guid.to_owned());
                if let Some(conversation) = conversation {
                    self.receive_live_message(conversation, message, false);
                }
            }
            result => {
                if visible {
                    self.render_thread(&chat, false);
                    if sync {
                        self.load_messages(Fetch::CatchUp);
                    }
                }
                if report_error && let Err(error) = result {
                    self.notify_error(&error);
                }
            }
        }
    }

    pub(super) fn restore_unsent(self: &Rc<Self>, guid: &str) {
        let Some(chat) = self.active_guid() else {
            return;
        };
        let job = {
            let mut state = self.state.borrow_mut();
            let Some(thread) = state.threads.get_mut(&chat) else {
                return;
            };
            let Some(index) = thread.pending_sends.iter().position(|pending| {
                pending.job.temp_guid == guid && pending.status == SendState::Failed
            }) else {
                return;
            };
            thread.pending_sends.remove(index).job
        };
        let draft = self.editor_text();
        let mut state = self.state.borrow_mut();
        if state.reply.is_none() && draft.is_empty() && state.files.is_empty() {
            state.reply = job.reply;
        }
        match job.part {
            SendPart::Text(text) => {
                drop(state);
                let text = if draft.is_empty() {
                    text
                } else {
                    format!("{draft}\n{text}")
                };
                self.editor.buffer().set_text(&text);
            }
            SendPart::File(path) => {
                if !state.files.contains(&path) {
                    state.files.push(path);
                }
            }
        }
        self.render_files();
        self.render_thread(&chat, false);
        self.editor.grab_focus();
    }

    pub(super) fn reply_to(self: &Rc<Self>, guid: &str) {
        let Some(selected) = self.message_row(guid) else {
            return;
        };
        let state = self.state.borrow();
        let Some(chat) = state.active.as_ref() else {
            return;
        };
        if self.mode == Mode::Live
            && !state
                .server
                .as_ref()
                .is_some_and(BlueBubbles::supports_message_actions)
        {
            drop(state);
            self.notify_error("Sending replies requires the BlueBubbles Private API helper.");
            return;
        }
        let target = selected
            .message
            .reply_to
            .clone()
            .unwrap_or_else(|| MessagePointer {
                guid: selected.message.guid.clone(),
                part: 0,
            });
        let quote = selected
            .quote
            .unwrap_or_else(|| thread::quote(&selected.message, chat));
        drop(state);
        self.state.borrow_mut().reply = Some(ReplyDraft { target, quote });
        self.render_files();
        self.editor.grab_focus();
    }

    pub(super) fn react_to(self: &Rc<Self>, guid: &str, kind: TapbackKind) {
        let Some(selected) = self.message_row(guid) else {
            return;
        };
        let Some(chat) = self.active_guid() else {
            return;
        };
        let Some(server) = self.state.borrow().server.clone() else {
            self.notify_error("Connect to a server to send a reaction.");
            return;
        };
        if !server.supports_message_actions() {
            self.notify_error("Sending reactions requires the BlueBubbles Private API helper.");
            return;
        }
        let guid = selected.message.guid;
        let start = {
            let mut state = self.state.borrow_mut();
            let Some(thread) = state.threads.get_mut(&chat) else {
                return;
            };
            let pending = thread.pending_reactions.get(&guid).copied();
            let current = pending.unwrap_or(selected.my_reaction);
            thread
                .pending_reactions
                .insert(guid.clone(), (current != Some(kind)).then_some(kind));
            pending.is_none()
        };
        // Close the menu before rebinding its row. The new badge paints on the next frame.
        glib::idle_add_local_once({
            let (chat, guid, app) = (chat.clone(), guid.clone(), Rc::downgrade(self));
            move || {
                if let Some(app) = app.upgrade() {
                    app.paint_pending_reaction(&chat, &guid);
                    if start {
                        app.send_pending_reaction(chat, guid);
                    }
                }
            }
        });
    }

    pub(super) fn paint_pending_reaction(&self, chat: &str, guid: &str) {
        if self.active_guid().as_deref() != Some(chat) {
            return;
        }
        let _profile = crate::profile::span("reaction-row-ui");
        let Some(position) = self.message_position(guid) else {
            return;
        };
        let Some(mut row) = self.message_row_at(position) else {
            return;
        };
        let reaction = {
            let state = self.state.borrow();
            let Some(thread) = state.threads.get(chat) else {
                return;
            };
            thread
                .pending_reactions
                .get(guid)
                .copied()
                .unwrap_or_else(|| {
                    thread::own_reaction(thread.messages.as_deref().unwrap_or_default(), guid)
                })
        };
        if row.my_reaction != reaction {
            row.set_my_reaction(reaction);
            self.view().store.splice(position, 1, &boxed(vec![row]));
        }
    }

    pub(super) fn send_pending_reaction(self: &Rc<Self>, chat: String, guid: String) {
        let (server, session, selected, desired, confirmed) = {
            let state = self.state.borrow();
            let Some(server) = state.server.clone() else {
                return;
            };
            let Some(thread) = state.threads.get(&chat) else {
                return;
            };
            let Some(&desired) = thread.pending_reactions.get(&guid) else {
                return;
            };
            let Some(messages) = &thread.messages else {
                return;
            };
            let Some(selected) = messages
                .iter()
                .find(|message| message.guid == guid)
                .cloned()
            else {
                return;
            };
            (
                server,
                state.session,
                selected,
                desired,
                thread::own_reaction(messages, &guid),
            )
        };
        if desired == confirmed {
            if let Some(thread) = self.state.borrow_mut().threads.get_mut(&chat) {
                thread.pending_reactions.remove(&guid);
            }
            self.paint_pending_reaction(&chat, &guid);
            return;
        }
        let Some(kind) = desired.or(confirmed) else {
            return;
        };
        let remove = desired.is_none();
        let requested_chat = chat.clone();
        let requested_guid = guid.clone();
        background(
            move || {
                let matches = |message: &Message| {
                    message.outgoing
                        && message.tapback.as_ref().is_some_and(|tapback| {
                            tapback.target.guid.eq_ignore_ascii_case(&requested_guid)
                                && tapback.target.part == 0
                                && tapback.kind == kind
                                && tapback.removed == remove
                        })
                };
                let response = {
                    let _profile = crate::profile::span("reaction-post");
                    server.react(&requested_chat, &selected, kind, remove)?
                };
                if let Some(message) = response.filter(matches) {
                    server
                        .cache()
                        .merge_messages(&requested_chat, std::slice::from_ref(&message));
                    return Ok(message);
                }
                let _profile = crate::profile::span("reaction-confirmation-fetch");
                let (rows, _) = server.messages(&requested_chat, 0)?;
                rows.into_iter()
                    .rev()
                    .find(|message| {
                        message.outgoing
                            && message.tapback.as_ref().is_some_and(|tapback| {
                                tapback.target.guid.eq_ignore_ascii_case(&requested_guid)
                                    && tapback.target.part == 0
                            })
                    })
                    .filter(matches)
                    .ok_or_else(|| "The server did not confirm the reaction.".to_string())
            },
            {
                let weak = Rc::downgrade(self);
                move |result| {
                    let Some(app) = weak.upgrade() else { return };
                    app.finish_reaction(chat, guid, session, desired, result);
                }
            },
        );
    }

    fn finish_reaction(
        self: &Rc<Self>,
        chat: String,
        guid: String,
        session: u64,
        sent: Option<TapbackKind>,
        result: Result<Message, String>,
    ) {
        let (send_next, error) = {
            let mut state = self.state.borrow_mut();
            if state.session != session {
                return;
            }
            let visible = state
                .active
                .as_ref()
                .is_some_and(|active| active.guid == chat);
            let Some(thread) = state.threads.get_mut(&chat) else {
                return;
            };
            let Some(&desired) = thread.pending_reactions.get(&guid) else {
                return;
            };
            thread.view_dirty |= !visible;
            let Some(messages) = thread.messages.as_mut() else {
                return;
            };
            let error = match result {
                Ok(message) => {
                    let at_end = messages
                        .last()
                        .is_none_or(|last| message.sent_at >= last.sent_at);
                    let count = messages.len();
                    crate::model::merge_messages(messages, vec![message]);
                    if at_end
                        && messages.len() > count
                        && let Some(offset) = &mut thread.offset
                    {
                        *offset += 1;
                    }
                    None
                }
                // A live server event may already have confirmed the operation.
                Err(_) if thread::own_reaction(messages, &guid) == sent => None,
                Err(error) => Some(error),
            };
            let send_next = error.is_none() && desired != sent;
            if !send_next {
                thread.pending_reactions.remove(&guid);
            }
            (send_next, error)
        };
        self.paint_pending_reaction(&chat, &guid);
        if let Some(error) = error {
            self.notify_error(&format!("Could not update the reaction. {error}"));
        } else if send_next {
            self.send_pending_reaction(chat, guid);
        }
    }
}
