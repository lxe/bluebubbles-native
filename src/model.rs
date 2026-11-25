#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Conversation {
    pub guid: String,
    pub name: String,
    pub search_text: String,
    pub preview: String,
    #[serde(default)]
    pub last_activity: i64,
    #[serde(default)]
    pub avatar: Option<std::path::PathBuf>,
    #[serde(default)]
    pub group_photo: Option<String>,
    #[serde(default)]
    pub group_style: bool,
    #[serde(default)]
    pub participants: Vec<Participant>,
}

pub fn sort_conversations(conversations: &mut [Conversation]) {
    conversations.sort_unstable_by(|left, right| {
        right
            .last_activity
            .cmp(&left.last_activity)
            .then_with(|| left.guid.cmp(&right.guid))
    });
}

#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Participant {
    pub address: String,
    pub name: String,
    pub avatar: Option<std::path::PathBuf>,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Message {
    pub guid: String,
    #[serde(skip)]
    pub temp_guid: Option<String>,
    pub text: String,
    pub outgoing: bool,
    #[serde(default)]
    pub sent_at: i64,
    #[serde(default)]
    pub edited_at: i64,
    pub attachments: Vec<Attachment>,
    #[serde(default)]
    pub sender: Option<String>,
    #[serde(default)]
    pub sender_id: Option<i64>,
    #[serde(default, rename = "thread_reply")]
    pub reply_to: Option<MessagePointer>,
    #[serde(default)]
    pub tapback: Option<Tapback>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct MessagePointer {
    pub guid: String,
    pub part: u32,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Tapback {
    pub target: MessagePointer,
    pub kind: TapbackKind,
    pub removed: bool,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum TapbackKind {
    Love,
    Like,
    Dislike,
    Laugh,
    Emphasize,
    Question,
}

impl TapbackKind {
    pub const ALL: [Self; 6] = [
        Self::Love,
        Self::Like,
        Self::Dislike,
        Self::Laugh,
        Self::Emphasize,
        Self::Question,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Love => "love",
            Self::Like => "like",
            Self::Dislike => "dislike",
            Self::Laugh => "laugh",
            Self::Emphasize => "emphasize",
            Self::Question => "question",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Love => "Love",
            Self::Like => "Like",
            Self::Dislike => "Dislike",
            Self::Laugh => "Laugh",
            Self::Emphasize => "Emphasize",
            Self::Question => "Question",
        }
    }

    pub fn emoji(self) -> &'static str {
        match self {
            Self::Love => "❤️",
            Self::Like => "👍",
            Self::Dislike => "👎",
            Self::Laugh => "😂",
            Self::Emphasize => "‼️",
            Self::Question => "❓",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.name() == value)
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Attachment {
    pub guid: String,
    pub name: String,
    #[serde(default)]
    pub mime_type: Option<String>,
}

impl Attachment {
    pub fn is_link_payload(&self) -> bool {
        std::path::Path::new(&self.name)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("pluginPayloadAttachment"))
    }
}

pub fn merge_messages(cached: &mut Vec<Message>, incoming: Vec<Message>) -> bool {
    use std::collections::HashMap;

    let mut positions: HashMap<_, _> = cached
        .iter()
        .enumerate()
        .map(|(index, message)| (message.guid.clone(), index))
        .collect();
    let mut changed = false;
    for message in incoming {
        if let Some(&index) = positions.get(&message.guid) {
            if cached[index].edited_at <= message.edited_at && cached[index] != message {
                cached[index] = message;
                changed = true;
            }
        } else {
            positions.insert(message.guid.clone(), cached.len());
            cached.push(message);
            changed = true;
        }
    }
    if changed {
        cached.sort_by_key(|message| message.sent_at);
    }
    changed
}

impl Conversation {
    pub fn new(guid: String, name: String, addresses: &str, preview: String) -> Self {
        let search_text = format!("{name} {addresses} {preview}").to_lowercase();
        Self {
            guid,
            name,
            search_text,
            preview,
            last_activity: 0,
            avatar: None,
            group_photo: None,
            group_style: false,
            participants: Vec::new(),
        }
    }

    pub fn is_group(&self) -> bool {
        self.group_style
            || self.group_photo.is_some()
            || self.participants.len() > 1
            || self.guid.split(';').nth(1) == Some("+")
    }

    pub fn update_preview(&mut self, message: &Message) -> bool {
        if message.tapback.is_some() || message.sent_at < self.last_activity {
            return false;
        }
        let preview = message.preview();
        if self.last_activity == message.sent_at && self.preview == preview {
            return false;
        }
        self.set_preview(message.sent_at, preview);
        true
    }

    pub fn set_preview(&mut self, activity: i64, preview: String) {
        let suffix = format!(" {}", self.preview.to_lowercase());
        if self.search_text.ends_with(&suffix) {
            self.search_text
                .truncate(self.search_text.len() - suffix.len());
        }
        self.search_text.push(' ');
        self.search_text.push_str(&preview.to_lowercase());
        self.last_activity = activity;
        self.preview = preview;
    }

    pub fn participant(&self, address: &str) -> Participant {
        let key = address_key(address);
        self.participants
            .iter()
            .find(|person| address_key(&person.address) == key)
            .cloned()
            .unwrap_or_else(|| Participant {
                address: address.to_owned(),
                name: address.to_owned(),
                avatar: None,
            })
    }
}

impl Message {
    pub fn preview(&self) -> String {
        if self.text.trim().is_empty() {
            "Attachment".into()
        } else {
            self.text.split_whitespace().collect::<Vec<_>>().join(" ")
        }
    }
}

pub fn address_key(address: &str) -> String {
    if address.contains('@') {
        return address.trim().to_lowercase();
    }
    let digits: String = address.chars().filter(char::is_ascii_digit).collect();
    if digits.len() == 11 && digits.starts_with('1') {
        digits[1..].into()
    } else if digits.is_empty() {
        address.trim().to_lowercase()
    } else {
        digits
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(guid: &str, text: &str, sent_at: i64) -> Message {
        Message {
            guid: guid.into(),
            text: text.into(),
            sent_at,
            ..Message::default()
        }
    }

    #[test]
    fn address_key_normalizes_phones_and_emails() {
        assert_eq!(address_key("+1 (555) 123-4567"), "5551234567");
        assert_eq!(address_key("555.123.4567"), "5551234567");
        assert_eq!(address_key(" +44 20 7946 0958 "), "442079460958");
        assert_eq!(address_key("  Alice@Example.COM "), "alice@example.com");
    }

    #[test]
    fn merge_messages_deduplicates_orders_and_respects_edit_time() {
        let mut cached = vec![msg("b", "second", 2000), msg("a", "first", 1000)];
        let mut edited = msg("a", "first edited", 1000);
        edited.edited_at = 3000;
        let stale = msg("a", "stale", 1000);

        assert!(merge_messages(
            &mut cached,
            vec![edited.clone(), msg("c", "third", 3000)]
        ));
        assert_eq!(
            cached
                .iter()
                .map(|m| (m.guid.as_str(), m.text.as_str()))
                .collect::<Vec<_>>(),
            vec![("a", "first edited"), ("b", "second"), ("c", "third")]
        );
        // Older edit timestamp must not overwrite newer edited message.
        assert!(!merge_messages(&mut cached, vec![stale]));
        assert_eq!(cached[0].text, "first edited");
    }

    #[test]
    fn conversation_preview_and_sorting_behave_deterministically() {
        let mut chat = Conversation::new(
            "iMessage;-;+15550100100".into(),
            "Maya".into(),
            "+15550100100",
            "old text".into(),
        );
        chat.last_activity = 100;
        assert!(!chat.is_group());

        // Tapbacks and older messages do not change the preview.
        let mut tapback = msg("t1", "", 200);
        tapback.tapback = Some(Tapback {
            target: MessagePointer {
                guid: "m1".into(),
                part: 0,
            },
            kind: TapbackKind::Love,
            removed: false,
        });
        assert!(!chat.update_preview(&tapback));
        assert!(!chat.update_preview(&msg("old", "older", 50)));

        // Newer text updates preview and search_text.
        assert!(chat.update_preview(&msg("m2", "hello\n   world", 250)));
        assert_eq!(chat.preview, "hello world");
        assert!(chat.search_text.contains("hello world"));
        assert!(!chat.search_text.contains("old text"));

        let mut other = Conversation::new("iMessage;+;chat1".into(), "Group".into(), "", "".into());
        other.last_activity = 250;
        assert!(other.is_group());

        let mut list = vec![chat, other];
        sort_conversations(&mut list);
        assert_eq!(list[0].guid, "iMessage;+;chat1");
    }
}
