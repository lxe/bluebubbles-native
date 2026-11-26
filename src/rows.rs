use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib, pango};

use crate::avatar::Avatar;
use crate::bubble_lane::BubbleLane;
use crate::icons::Icon;
use crate::layout::{SIDEBAR_INSET, SIDEBAR_ROW_GAP, thread_clamp};
use crate::media;
use crate::model::Conversation;
use crate::organization::SidebarRow;
use crate::reaction_stack::{HEADROOM, ReactionStack};
use crate::squircle::Squircle;
use crate::thread::{MessageRow, SendState};

#[derive(Default)]
pub struct Counts {
    conversations: Cell<u32>,
    messages: Cell<u32>,
}

impl Counts {
    pub fn report(&self) {
        eprintln!(
            "Row widgets created: {} conversation, {} message",
            self.conversations.get(),
            self.messages.get()
        );
    }
}

pub type GroupAvatarHandler =
    Rc<RefCell<Option<Box<dyn Fn(String, String, glib::WeakRef<Avatar>)>>>>;

struct ConversationRowWidgets {
    shape: Squircle,
    avatar: Avatar,
    name: gtk::Label,
    preview: gtk::Label,
    pin: gtk::DrawingArea,
    menu: gtk::Popover,
    active_signal: glib::SignalHandlerId,
}

struct MessageRowWidgets {
    timestamp: gtk::Label,
    lane: BubbleLane,
    reactions: ReactionStack,
    row: gtk::Box,
    sender: gtk::Box,
    sender_avatar: Avatar,
    sender_name: gtk::Label,
    bubble: Squircle,
    quote: gtk::Button,
    quote_author: gtk::Label,
    quote_text: gtk::Label,
    label: gtk::Label,
    files: gtk::Box,
    send_status: gtk::Label,
    menu: gtk::Popover,
}

pub fn conversations(
    counts: &Rc<Counts>,
    active: &gtk::EntryBuffer,
    group_avatar: &GroupAvatarHandler,
) -> gtk::SignalListItemFactory {
    let factory = gtk::SignalListItemFactory::new();
    let rows = Rc::new(RefCell::new(HashMap::<usize, ConversationRowWidgets>::new()));
    factory.connect_setup({
        let counts = counts.clone();
        let active = active.clone();
        let rows = rows.clone();
        move |_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            let avatar = Avatar::new(36);
            let name = row_label("heading");
            let preview = row_label("dim-label");
            let labels = gtk::Box::new(gtk::Orientation::Vertical, 3);
            labels.set_hexpand(true);
            labels.append(&name);
            labels.append(&preview);
            let pin = Icon::Pin.widget();
            pin.set_tooltip_text(Some("Pinned"));
            pin.update_property(&[gtk::accessible::Property::Label("Pinned")]);

            let row = gtk::Box::new(gtk::Orientation::Horizontal, 11);
            row.add_css_class("conversation-row");
            row.append(&avatar);
            row.append(&labels);
            row.append(&pin);

            let shape = Squircle::new(&row, "conversation-shape");
            let host = gtk::Overlay::new();
            host.set_margin_start(SIDEBAR_INSET);
            host.set_margin_end(SIDEBAR_INSET);
            host.set_margin_top(SIDEBAR_ROW_GAP);
            host.set_margin_bottom(SIDEBAR_ROW_GAP);
            host.set_child(Some(&shape));

            let menu = crate::context_menu::attach(&host, &host);
            let weak_item = item.downgrade();
            menu.connect_show(move |menu| {
                let Some(data) = weak_item
                    .upgrade()
                    .and_then(|item| item.item().and_downcast::<glib::BoxedAnyObject>())
                else {
                    return;
                };
                let Ok(data) = data.try_borrow::<SidebarRow>() else {
                    return;
                };
                let target = data.chat.guid.to_variant();
                crate::context_menu::set_commands(
                    menu,
                    &[
                        (
                            if data.pinned {
                                "Unpin Conversation"
                            } else {
                                "Pin Conversation"
                            },
                            "chat.pin",
                            Some(&target),
                        ),
                        ("Archive Conversation", "chat.archive", Some(&target)),
                    ],
                );
            });

            item.set_child(Some(&host));
            let weak_item = item.downgrade();
            let weak_shape = shape.downgrade();
            let active_signal = active.connect_text_notify(move |active| {
                if let (Some(item), Some(shape)) = (weak_item.upgrade(), weak_shape.upgrade()) {
                    update_highlight(&item, &shape, active);
                }
            });
            rows.borrow_mut().insert(
                item.as_ptr() as usize,
                ConversationRowWidgets {
                    shape,
                    avatar,
                    name,
                    preview,
                    pin,
                    menu,
                    active_signal,
                },
            );
            counts.conversations.set(counts.conversations.get() + 1);
        }
    });
    factory.connect_bind({
        let active = active.clone();
        let group_avatar = group_avatar.clone();
        let rows = rows.clone();
        move |_, item| {
            let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let Some(data) = item.item().and_downcast::<glib::BoxedAnyObject>() else {
                return;
            };
            let Ok(data) = data.try_borrow::<SidebarRow>() else {
                return;
            };
            let rows = rows.borrow();
            let Some(widgets) = rows.get(&(item.as_ptr() as usize)) else {
                return;
            };
            set_avatar(&widgets.avatar, &data.chat);
            if let Some(photo) = &data.chat.group_photo
                && data.chat.avatar.is_none()
                && let Some(request) = group_avatar.borrow().as_ref()
            {
                request(
                    data.chat.guid.clone(),
                    photo.clone(),
                    widgets.avatar.downgrade(),
                );
            }
            widgets.name.set_label(&data.chat.name);
            widgets.pin.set_visible(data.pinned);
            widgets.preview.set_label(
                &data
                    .chat
                    .preview
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" "),
            );
            update_highlight(item, &widgets.shape, &active);
        }
    });
    factory.connect_unbind({
        let rows = rows.clone();
        move |_, item| {
            if let Some(widgets) = rows.borrow().get(&(item.as_ptr() as usize)) {
                widgets.menu.popdown();
            }
        }
    });
    factory.connect_teardown({
        let active = active.clone();
        move |_, item| {
            if let Some(widgets) = rows.borrow_mut().remove(&(item.as_ptr() as usize)) {
                active.disconnect(widgets.active_signal);
            }
        }
    });
    factory
}

fn update_highlight(item: &gtk::ListItem, shape: &Squircle, active: &gtk::EntryBuffer) {
    let is_active = item
        .item()
        .and_downcast::<glib::BoxedAnyObject>()
        .is_some_and(|row| {
            row.try_borrow::<SidebarRow>()
                .is_ok_and(|row| row.chat.guid == active.text().as_str())
        });
    if is_active {
        shape.add_css_class("active-conversation");
    } else {
        shape.remove_css_class("active-conversation");
    }
}

pub fn open_focused_chat_menu(list: &gtk::ListView) -> bool {
    let Some(host) = list
        .focus_child()
        .and_then(|row| row.first_child())
        .and_downcast::<gtk::Overlay>()
    else {
        return false;
    };
    let Some(menu) = host.last_child().and_downcast::<gtk::Popover>() else {
        return false;
    };
    crate::context_menu::popup(&menu, None);
    true
}

pub fn messages(counts: &Rc<Counts>, handler: &media::Handler) -> gtk::SignalListItemFactory {
    let factory = gtk::SignalListItemFactory::new();
    let rows = Rc::new(RefCell::new(HashMap::<usize, MessageRowWidgets>::new()));
    factory.connect_setup({
        let counts = counts.clone();
        let rows = rows.clone();
        move |_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            let sender = gtk::Box::new(gtk::Orientation::Horizontal, 5);
            sender.add_css_class("message-sender");
            sender.set_halign(gtk::Align::Start);
            let sender_avatar = Avatar::new(18);
            sender.append(&sender_avatar);
            let sender_name = row_label("dim-label");
            sender_name.set_valign(gtk::Align::Center);
            sender.append(&sender_name);

            let quote = gtk::Button::new();
            quote.set_margin_start(9);
            quote.set_margin_end(9);
            quote.set_margin_top(7);
            quote.add_css_class("thread-quote");
            let quote_author = row_label("heading");
            let quote_text = row_label("dim-label");
            let excerpt = gtk::Box::new(gtk::Orientation::Vertical, 2);
            excerpt.append(&quote_author);
            excerpt.append(&quote_text);
            quote.set_child(Some(&excerpt));

            let label = gtk::Label::builder()
                .xalign(0.0)
                .wrap(true)
                .wrap_mode(pango::WrapMode::WordChar)
                .max_width_chars(56)
                .selectable(true)
                .build();
            label.add_css_class("message");
            let text = gtk::Box::new(gtk::Orientation::Vertical, 0);
            text.append(&quote);
            text.append(&label);
            let bubble = Squircle::new(&text, "thread-bubble");

            let files = gtk::Box::new(gtk::Orientation::Vertical, 4);
            let send_status = gtk::Label::new(None);
            send_status.add_css_class("message-send-status");
            send_status.set_halign(gtk::Align::End);

            let row = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(4)
                .margin_top(3)
                .margin_bottom(3)
                .build();
            row.append(&sender);
            row.append(&bubble);
            row.append(&files);
            row.append(&send_status);

            let host = gtk::Overlay::new();
            host.set_child(Some(&row));
            let menu = crate::context_menu::attach(&host, &row);
            crate::context_menu::bind_pointer(&menu, &bubble, gtk::PropagationPhase::Capture);
            let weak_item = item.downgrade();
            menu.connect_show(move |menu| {
                let Some(data) = weak_item
                    .upgrade()
                    .and_then(|item| item.item().and_downcast::<glib::BoxedAnyObject>())
                else {
                    return;
                };
                let Ok(data) = data.try_borrow::<MessageRow>() else {
                    return;
                };
                let guid = &data.message.guid;
                let target = guid.to_variant();
                let remove_target = (data.send_state.is_none())
                    .then_some(data.my_reaction)
                    .flatten()
                    .map(|kind| (guid.as_str(), kind.name()).to_variant());
                let text = &data.message.text;
                let url = crate::links::first(text);
                let address = url.as_ref().map(|url| url.as_str().to_variant());
                let text_target = text.to_variant();
                let mut entries = Vec::new();
                if let Some(remove_target) = &remove_target {
                    entries.push(("Remove Reaction", "message.react", Some(remove_target)));
                }
                if !text.trim().is_empty()
                    && (url.is_none() || reqwest::Url::parse(text.trim()).ok() != url)
                {
                    entries.push(("Copy Text", "message.copy", Some(&text_target)));
                }
                if let Some(address) = &address {
                    entries.push(("Copy URL", "message.copy", Some(address)));
                }
                if data.send_state == Some(SendState::Failed) {
                    entries.push(("Restore Draft", "message.restore", Some(&target)));
                } else if data.send_state.is_none() {
                    entries.push(("Reply", "message.reply", Some(&target)));
                }
                crate::context_menu::set_commands(menu, &entries);
                if data.send_state.is_none() {
                    crate::context_menu::prepend_reactions(menu, guid, data.my_reaction);
                }
            });
            let keyboard = gtk::EventControllerKey::new();
            let menu_ref = menu.downgrade();
            keyboard.connect_key_pressed(move |_, key, _, modifiers| {
                if crate::context_menu::keyboard_shortcut(key, modifiers)
                    && let Some(menu) = menu_ref.upgrade()
                {
                    crate::context_menu::popup(&menu, None);
                    glib::Propagation::Stop
                } else {
                    glib::Propagation::Proceed
                }
            });
            row.add_controller(keyboard);

            item.set_activatable(false);
            item.set_selectable(false);
            item.set_focusable(false);
            let lane = crate::bubble_lane::new(&host);
            let surface = gtk::Overlay::new();
            surface.add_css_class("thread-content-inset");
            surface.set_child(Some(&lane));
            let reactions = ReactionStack::new();
            crate::context_menu::bind_pointer(&menu, &reactions, gtk::PropagationPhase::Capture);
            surface.add_overlay(&reactions);
            surface.connect_get_child_position({
                let (bubble, files) = (bubble.downgrade(), files.downgrade());
                move |surface, child| {
                    let reactions = child.downcast_ref::<ReactionStack>()?;
                    let anchor = match bubble.upgrade().filter(gtk::prelude::WidgetExt::is_visible)
                    {
                        Some(bubble) => bubble.upcast(),
                        None => files.upgrade()?.first_child()?,
                    };
                    let bounds = anchor.compute_bounds(surface)?;
                    Some(reactions.position(&bounds, surface.width()))
                }
            });
            let timestamp = gtk::Label::new(None);
            timestamp.add_css_class("message-time");
            timestamp.set_halign(gtk::Align::Center);
            let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
            column.append(&timestamp);
            column.append(&surface);
            item.set_child(Some(&thread_clamp(&column)));
            rows.borrow_mut().insert(
                item.as_ptr() as usize,
                MessageRowWidgets {
                    timestamp,
                    lane,
                    reactions,
                    row,
                    sender,
                    sender_avatar,
                    sender_name,
                    bubble,
                    quote,
                    quote_author,
                    quote_text,
                    label,
                    files,
                    send_status,
                    menu,
                },
            );
            counts.messages.set(counts.messages.get() + 1);
        }
    });
    factory.connect_bind({
        let handler = handler.clone();
        let rows = rows.clone();
        move |_, item| {
            let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let Some(data) = item.item().and_downcast::<glib::BoxedAnyObject>() else {
                return;
            };
            let Ok(data) = data.try_borrow::<MessageRow>() else {
                return;
            };
            let rows = rows.borrow();
            let Some(widgets) = rows.get(&(item.as_ptr() as usize)) else {
                return;
            };
            bind_message(widgets, &data, &handler);
        }
    });
    factory.connect_unbind({
        let rows = rows.clone();
        move |_, item| {
            if let Some(widgets) = rows.borrow().get(&(item.as_ptr() as usize)) {
                widgets.menu.popdown();
            }
        }
    });
    factory.connect_teardown(move |_, item| {
        rows.borrow_mut().remove(&(item.as_ptr() as usize));
    });
    factory
}

fn bind_message(widgets: &MessageRowWidgets, data: &MessageRow, handler: &media::Handler) {
    let time = data
        .show_time
        .then(|| crate::thread::timestamp(data.message.sent_at))
        .flatten();
    widgets.timestamp.set_visible(time.is_some());
    if let Some((text, full)) = time {
        widgets.timestamp.set_label(&text);
        widgets.timestamp.set_tooltip_text(Some(&full));
    }

    let message = &data.message;
    let failed = data.send_state == Some(SendState::Failed);
    widgets.send_status.set_visible(failed);
    widgets.send_status.set_label("Send unconfirmed");
    if failed {
        widgets.send_status.add_css_class("error");
    } else {
        widgets.send_status.remove_css_class("error");
    }

    widgets
        .row
        .set_margin_top(if data.group && data.starts_group {
            9
        } else {
            3
        });
    widgets
        .row
        .set_margin_bottom(if data.group { 5 } else { 3 });
    widgets.row.set_spacing(if data.group { 6 } else { 4 });

    widgets.sender.set_visible(data.sender.is_some());
    if let Some(person) = &data.sender {
        widgets
            .sender_avatar
            .bind(&person.name, false, person.avatar.as_deref());
        widgets.sender_name.set_label(&person.name);
    }

    widgets.quote.set_visible(data.quote.is_some());
    if let Some(reply) = &data.quote {
        widgets.quote_author.set_label(&reply.author);
        widgets.quote_text.set_label(&reply.text);
        widgets.quote.set_sensitive(reply.available);
        widgets.quote.set_action_target_value(
            message
                .reply_to
                .as_ref()
                .map(|target| target.guid.to_variant())
                .as_ref(),
        );
        widgets.quote.set_action_name(Some("message.jump"));
    }

    let align = if message.outgoing {
        gtk::Align::End
    } else {
        gtk::Align::Start
    };
    make_outgoing(&widgets.lane, message.outgoing);
    widgets.lane.confirm(data.confirmed_at);
    let pending = matches!(
        data.send_state,
        Some(SendState::Sending | SendState::Unconfirmed)
    );
    let description = if pending { "Sending" } else { "" };
    widgets
        .bubble
        .update_property(&[gtk::accessible::Property::Description(description)]);
    let green = data.service.is_green();
    widgets.bubble.set_pending(pending);
    make_outgoing(&widgets.bubble, message.outgoing);
    make_sms(&widgets.bubble, message.outgoing && green);
    widgets.bubble.set_halign(align);

    let link = (!pending)
        .then(|| crate::links::first(&message.text))
        .flatten();
    let standalone_link = link.is_some() && reqwest::Url::parse(message.text.trim()).ok() == link;
    let show_label = !message.text.is_empty() && !standalone_link;
    if show_label
        && !pending
        && let Some(markup) = crate::links::format_markup(&message.text)
    {
        widgets.label.set_markup(&markup);
    } else {
        widgets.label.set_use_markup(false);
        widgets.label.set_label(&message.text);
    }
    widgets.label.set_visible(show_label);
    widgets
        .bubble
        .set_visible(show_label || data.quote.is_some());

    while let Some(child) = widgets.files.first_child() {
        widgets.files.remove(&child);
    }
    let has_preview = link.is_some();
    let mut visible_files = standalone_link;
    if let Some(link) = link {
        let card = media::link_row(
            link,
            message.outgoing,
            green,
            standalone_link,
            &widgets.files,
            handler,
        );
        card.set_halign(align);
        crate::context_menu::bind_pointer(&widgets.menu, &card, gtk::PropagationPhase::Capture);
        widgets.files.append(&card);
    }
    let attachment_menu = (!widgets.bubble.is_visible()).then_some(&widgets.menu);
    for attachment in &message.attachments {
        if has_preview && attachment.is_link_payload() {
            continue;
        }
        let widget = media::attachment_row(attachment, handler, attachment_menu);
        widget.set_halign(align);
        widgets.files.append(&widget);
        visible_files = true;
    }
    widgets.files.set_visible(visible_files);

    let headroom = if data.reactions.is_empty() {
        0
    } else {
        HEADROOM
    };
    widgets
        .bubble
        .set_margin_top(if widgets.bubble.is_visible() {
            headroom
        } else {
            0
        });
    widgets
        .files
        .set_margin_top(if widgets.bubble.is_visible() {
            0
        } else {
            headroom
        });
    widgets
        .reactions
        .bind(&message.guid, message.outgoing, &data.reactions);
}

pub fn replace_changed<T: PartialEq + 'static>(store: &gio::ListStore, rows: Vec<T>) {
    // A shared prefix or suffix can keep its current GTK row bindings.
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
    if remove > 0 || insert_end > prefix {
        let changed: Vec<_> = rows
            .into_iter()
            .skip(prefix)
            .take(insert_end - prefix)
            .map(glib::BoxedAnyObject::new)
            .collect();
        store.splice(prefix as u32, remove as u32, &changed);
    }
}

fn make_outgoing(widget: &impl IsA<gtk::Widget>, outgoing: bool) {
    if outgoing {
        widget.add_css_class("outgoing");
    } else {
        widget.remove_css_class("outgoing");
    }
}

fn make_sms(widget: &impl IsA<gtk::Widget>, sms: bool) {
    if sms {
        widget.add_css_class("sms");
    } else {
        widget.remove_css_class("sms");
    }
}

fn row_label(class: &str) -> gtk::Label {
    let label = gtk::Label::builder()
        .xalign(0.0)
        .ellipsize(pango::EllipsizeMode::End)
        .single_line_mode(true)
        .hexpand(true)
        .build();
    label.add_css_class(class);
    label
}

pub fn set_avatar(avatar: &Avatar, conversation: &Conversation) {
    avatar.bind(
        &conversation.name,
        conversation.is_group(),
        conversation.avatar.as_deref(),
    );
    avatar.set_widget_name(conversation.group_photo.as_deref().unwrap_or(""));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(store: &gio::ListStore) -> Vec<i32> {
        (0..store.n_items())
            .map(|i| {
                *store
                    .item(i)
                    .unwrap()
                    .downcast::<glib::BoxedAnyObject>()
                    .unwrap()
                    .borrow::<i32>()
            })
            .collect()
    }

    #[test]
    fn replace_changed_preserves_matching_prefix_and_suffix() {
        let store = gio::ListStore::new::<glib::BoxedAnyObject>();
        replace_changed(&store, vec![1, 2, 3, 4]);
        assert_eq!(items(&store), vec![1, 2, 3, 4]);
        let first_ptr = store.item(0).unwrap().as_ptr();
        let last_ptr = store.item(3).unwrap().as_ptr();

        // Splice middle while keeping index 0 and last element untouched.
        replace_changed(&store, vec![1, 20, 30, 31, 4]);
        assert_eq!(items(&store), vec![1, 20, 30, 31, 4]);
        assert_eq!(store.item(0).unwrap().as_ptr(), first_ptr);
        assert_eq!(store.item(4).unwrap().as_ptr(), last_ptr);

        // No-op update changes nothing.
        replace_changed(&store, vec![1, 20, 30, 31, 4]);
        assert_eq!(store.item(0).unwrap().as_ptr(), first_ptr);

        // Clear to empty.
        replace_changed::<i32>(&store, vec![]);
        assert!(items(&store).is_empty());
    }
}
