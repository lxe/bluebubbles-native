use gtk::glib;
use std::collections::{BTreeMap, HashMap};

use crate::model::{Conversation, Message, Participant, TapbackKind, address_key};

#[derive(Clone, PartialEq)]
pub struct Quote {
    pub author: String,
    pub text: String,
    pub available: bool,
}

#[derive(Clone, PartialEq)]
pub struct ReactionBadge {
    pub kind: TapbackKind,
    pub count: usize,
    pub own_count: usize,
}

#[derive(Clone, Copy, PartialEq)]
pub enum SendState {
    Sending,
    Unconfirmed,
    Failed,
}

#[derive(Clone, PartialEq)]
pub struct MessageRow {
    pub message: Message,
    pub group: bool,
    pub starts_group: bool,
    pub show_time: bool,
    pub sender: Option<Participant>,
    pub quote: Option<Quote>,
    pub reactions: Vec<ReactionBadge>,
    pub my_reaction: Option<TapbackKind>,
    pub send_state: Option<SendState>,
    pub confirmed_at: Option<std::time::Instant>,
}

impl MessageRow {
    pub fn set_my_reaction(&mut self, reaction: Option<TapbackKind>) {
        if self.my_reaction == reaction {
            return;
        }
        if let Some(previous) = self.my_reaction
            && let Some(index) = self
                .reactions
                .iter()
                .position(|badge| badge.kind == previous)
        {
            let badge = &mut self.reactions[index];
            badge.count -= 1;
            badge.own_count -= 1;
            if badge.count == 0 {
                self.reactions.remove(index);
            }
        }
        if let Some(kind) = reaction {
            match self
                .reactions
                .binary_search_by_key(&kind, |badge| badge.kind)
            {
                Ok(index) => {
                    self.reactions[index].count += 1;
                    self.reactions[index].own_count += 1;
                }
                Err(index) => self.reactions.insert(
                    index,
                    ReactionBadge {
                        kind,
                        count: 1,
                        own_count: 1,
                    },
                ),
            }
        }
        self.my_reaction = reaction;
    }
}

pub fn own_reaction(messages: &[Message], guid: &str) -> Option<TapbackKind> {
    let mut own = None;
    for message in messages.iter().filter(|message| message.outgoing) {
        let Some(tapback) = message.tapback.as_ref().filter(|tapback| {
            tapback.target.part == 0 && tapback.target.guid.eq_ignore_ascii_case(guid)
        }) else {
            continue;
        };
        if !tapback.removed {
            own = Some(tapback.kind);
        } else if own == Some(tapback.kind) {
            own = None;
        }
    }
    own
}

pub fn quote(message: &Message, chat: &Conversation) -> Quote {
    let author = if message.outgoing {
        "You".into()
    } else if !chat.is_group() {
        chat.name.clone()
    } else {
        message
            .sender
            .as_deref()
            .map(|address| chat.participant(address).name)
            .unwrap_or_else(|| "Participant".into())
    };
    let text = if message.text.is_empty() {
        message
            .attachments
            .first()
            .map(|file| file.name.clone())
            .unwrap_or_else(|| "Message".into())
    } else {
        message
            .text
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    };
    Quote {
        author,
        text,
        available: true,
    }
}

pub fn timeline(messages: &[Message], chat: &Conversation) -> Vec<MessageRow> {
    let by_guid: HashMap<_, _> = messages
        .iter()
        .filter(|message| message.tapback.is_none())
        .map(|message| (message.guid.to_ascii_lowercase(), message))
        .collect();
    let mut reactions = HashMap::<String, HashMap<(u32, String), TapbackKind>>::new();
    for message in messages {
        let Some(event) = &message.tapback else {
            continue;
        };
        let sender = if message.outgoing {
            "self".into()
        } else if let Some(address) = &message.sender {
            format!("address:{}", address_key(address))
        } else if let Some(id) = message.sender_id {
            format!("handle:{id}")
        } else {
            format!("event:{}", message.guid)
        };
        let entries = reactions
            .entry(event.target.guid.to_ascii_lowercase())
            .or_default();
        let key = (event.target.part, sender);
        if event.removed {
            if entries.get(&key) == Some(&event.kind) {
                entries.remove(&key);
            }
        } else {
            entries.insert(key, event.kind);
        }
    }

    let group = chat.is_group();
    let mut previous: Option<&Message> = None;
    let mut previous_date: Option<glib::DateTime> = None;
    messages
        .iter()
        .filter(|message| message.tapback.is_none())
        .map(|message| {
            let date = local_date(message.sent_at);
            let show_time = date.as_ref().is_some_and(|date| {
                previous_date.as_ref().is_none_or(|previous| {
                    date.to_unix() - previous.to_unix() >= 30 * 60 || !same_day(date, previous)
                })
            });
            if let Some(date) = date {
                previous_date = Some(date);
            }
            let starts_group = show_time
                || previous.is_none_or(|earlier| {
                    earlier.outgoing != message.outgoing
                        || (!message.outgoing && !same_sender(earlier, message))
                });
            previous = Some(message);
            let reply = message.reply_to.as_ref().map(|target| {
                by_guid
                    .get(&target.guid.to_ascii_lowercase())
                    .map(|parent| quote(parent, chat))
                    .unwrap_or_else(|| Quote {
                        author: "Reply".into(),
                        text: "Earlier message".into(),
                        available: false,
                    })
            });
            let mut by_kind = BTreeMap::new();
            let mut my_reaction = None;
            if let Some(entries) = reactions.remove(&message.guid.to_ascii_lowercase()) {
                for ((part, sender), kind) in entries {
                    let badge = by_kind.entry(kind).or_insert(ReactionBadge {
                        kind,
                        count: 0,
                        own_count: 0,
                    });
                    badge.count += 1;
                    if sender == "self" {
                        badge.own_count += 1;
                        if part == 0 {
                            my_reaction = Some(kind);
                        }
                    }
                }
            }
            let sender = if group && !message.outgoing && starts_group {
                message
                    .sender
                    .as_deref()
                    .map(|address| chat.participant(address))
            } else {
                None
            };
            MessageRow {
                message: message.clone(),
                group,
                starts_group,
                show_time,
                sender,
                quote: reply,
                reactions: by_kind.into_values().collect(),
                my_reaction,
                send_state: None,
                confirmed_at: None,
            }
        })
        .collect()
}

fn same_sender(first: &Message, second: &Message) -> bool {
    match (first.sender.as_deref(), second.sender.as_deref()) {
        (Some(first), Some(second)) => address_key(first) == address_key(second),
        _ => first.sender_id.is_some() && first.sender_id == second.sender_id,
    }
}

fn local_date(milliseconds: i64) -> Option<glib::DateTime> {
    (milliseconds > 0)
        .then(|| glib::DateTime::from_unix_local(milliseconds / 1000).ok())
        .flatten()
}

fn same_day(left: &glib::DateTime, right: &glib::DateTime) -> bool {
    left.year() == right.year() && left.day_of_year() == right.day_of_year()
}

pub fn timestamp(milliseconds: i64) -> Option<(String, glib::GString)> {
    let date = local_date(milliseconds)?;
    let now = glib::DateTime::now_local().ok()?;
    let elapsed = now.to_unix() - date.to_unix();
    let label = if same_day(&date, &now) && elapsed >= 0 {
        let (count, unit) = if elapsed >= 3600 {
            (elapsed / 3600, "hour")
        } else {
            (elapsed / 60, "minute")
        };
        if count == 0 {
            "Just now".into()
        } else {
            let suffix = if count == 1 { "" } else { "s" };
            format!("{count} {unit}{suffix} ago")
        }
    } else if now
        .add_days(-1)
        .is_ok_and(|yesterday| same_day(&date, &yesterday))
    {
        "Yesterday".into()
    } else {
        date.format(if now.year() == date.year() {
            "%b %-d"
        } else {
            "%b %-d, %Y"
        })
        .ok()?
        .into()
    };
    Some((label, date.format("%c").ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{MessagePointer, Tapback};

    fn group_chat() -> Conversation {
        let mut chat = Conversation::new("iMessage;+;chat1".into(), "Crew".into(), "", "".into());
        chat.participants = vec![
            Participant {
                address: "+15550100100".into(),
                name: "Maya".into(),
                avatar: None,
            },
            Participant {
                address: "+15550100101".into(),
                name: "Sam".into(),
                avatar: None,
            },
        ];
        chat
    }

    #[test]
    fn timeline_groups_senders_resolves_replies_and_aggregates_tapbacks() {
        let chat = group_chat();
        let messages = vec![
            Message {
                guid: "m1".into(),
                text: "First message".into(),
                sent_at: 1_700_000_000_000,
                sender: Some("+15550100100".into()),
                ..Message::default()
            },
            Message {
                guid: "m2".into(),
                text: "Follow up".into(),
                sent_at: 1_700_000_060_000,
                sender: Some("555-010-0100".into()),
                ..Message::default()
            },
            Message {
                guid: "m3".into(),
                text: "Reply after gap".into(),
                outgoing: true,
                sent_at: 1_700_002_000_000,
                reply_to: Some(MessagePointer {
                    guid: "m1".into(),
                    part: 0,
                }),
                ..Message::default()
            },
            Message {
                guid: "t1".into(),
                outgoing: true,
                sent_at: 1_700_002_001_000,
                tapback: Some(Tapback {
                    target: MessagePointer {
                        guid: "m1".into(),
                        part: 0,
                    },
                    kind: TapbackKind::Like,
                    removed: false,
                }),
                ..Message::default()
            },
            Message {
                guid: "t2".into(),
                outgoing: true,
                sent_at: 1_700_002_002_000,
                tapback: Some(Tapback {
                    target: MessagePointer {
                        guid: "m1".into(),
                        part: 0,
                    },
                    kind: TapbackKind::Love,
                    removed: false,
                }),
                ..Message::default()
            },
            Message {
                guid: "t3".into(),
                outgoing: false,
                sender: Some("+15550100101".into()),
                sent_at: 1_700_002_003_000,
                tapback: Some(Tapback {
                    target: MessagePointer {
                        guid: "m1".into(),
                        part: 0,
                    },
                    kind: TapbackKind::Love,
                    removed: false,
                }),
                ..Message::default()
            },
        ];

        let rows = timeline(&messages, &chat);
        assert_eq!(rows.len(), 3);
        assert!(rows[0].show_time);
        assert!(rows[0].starts_group);
        assert_eq!(rows[0].sender.as_ref().unwrap().name, "Maya");
        assert!(!rows[1].show_time);
        assert!(!rows[1].starts_group);
        assert!(rows[1].sender.is_none());
        assert!(rows[2].show_time);
        assert_eq!(
            rows[2]
                .quote
                .as_ref()
                .map(|q| (q.author.as_str(), q.text.as_str(), q.available)),
            Some(("Maya", "First message", true))
        );
        // Outgoing Love replaced Like, combined with Sam's Love => count 2, own_count 1.
        assert_eq!(rows[0].my_reaction, Some(TapbackKind::Love));
        assert_eq!(rows[0].reactions.len(), 1);
        assert_eq!(rows[0].reactions[0].count, 2);
        assert_eq!(rows[0].reactions[0].own_count, 1);
        assert_eq!(own_reaction(&messages, "m1"), Some(TapbackKind::Love));

        // Optimistic reaction mutation switches and removes cleanly.
        let mut row = rows[0].clone();
        row.set_my_reaction(Some(TapbackKind::Laugh));
        assert_eq!(row.my_reaction, Some(TapbackKind::Laugh));
        assert_eq!(row.reactions.len(), 2);
        assert_eq!(row.reactions[0].count, 1);
        assert_eq!(row.reactions[0].own_count, 0);
        row.set_my_reaction(None);
        assert_eq!(row.my_reaction, None);
        assert_eq!(row.reactions.len(), 1);
    }
}
