#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum Service {
    #[default]
    IMessage,
    Sms,
    Rcs,
}

impl Service {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "imessage" => Some(Self::IMessage),
            "sms" | "mms" => Some(Self::Sms),
            "rcs" => Some(Self::Rcs),
            _ => None,
        }
    }

    pub fn from_chat_guid(guid: &str) -> Option<Self> {
        guid.split(';').next().and_then(Self::parse)
    }

    pub fn is_green(self) -> bool {
        matches!(self, Self::Sms | Self::Rcs)
    }

    pub fn composer_placeholder(self) -> &'static str {
        match self {
            Self::IMessage => "Message",
            Self::Sms => "Text Message · SMS",
            Self::Rcs => "Text Message · RCS",
        }
    }
}

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
    pub service: Option<Service>,
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

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ContactEntry {
    pub name: String,
    pub address: String,
    #[serde(default)]
    pub avatar: Option<std::path::PathBuf>,
}

impl ContactEntry {
    pub fn matches_query(&self, query: &str) -> bool {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return true;
        }
        let words: Vec<String> = trimmed.split_whitespace().map(str::to_lowercase).collect();
        let name = self.name.to_lowercase();
        let address = self.address.to_lowercase();
        let address_digits: String = self.address.chars().filter(char::is_ascii_digit).collect();
        words.iter().all(|word| {
            if name.contains(word) || address.contains(word) {
                return true;
            }
            let word_digits: String = word.chars().filter(char::is_ascii_digit).collect();
            !word_digits.is_empty() && address_digits.contains(&word_digits)
        })
    }
}

pub fn looks_like_address(raw: &str) -> bool {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return false;
    }
    if let Some((local, domain)) = trimmed.split_once('@') {
        return !local.is_empty()
            && !domain.is_empty()
            && domain.contains('.')
            && !trimmed.chars().any(char::is_whitespace);
    }
    let digits = trimmed.chars().filter(char::is_ascii_digit).count();
    digits >= 3
        && trimmed
            .chars()
            .all(|ch| ch.is_ascii_digit() || matches!(ch, '+' | '-' | '(' | ')' | ' ' | '.'))
}

pub fn merge_contacts(
    directory: &[ContactEntry],
    conversations: &[Conversation],
) -> Vec<ContactEntry> {
    use std::collections::HashMap;

    let mut by_key: HashMap<String, ContactEntry> = HashMap::new();
    for entry in directory {
        let address = entry.address.trim();
        if address.is_empty() {
            continue;
        }
        let key = address_key(address);
        let name = if entry.name.trim().is_empty() {
            address.to_owned()
        } else {
            entry.name.trim().to_owned()
        };
        by_key
            .entry(key)
            .and_modify(|existing| {
                if existing.name == existing.address && name != address {
                    existing.name = name.clone();
                }
                if existing.avatar.is_none() && entry.avatar.is_some() {
                    existing.avatar.clone_from(&entry.avatar);
                }
            })
            .or_insert(ContactEntry {
                name,
                address: address.to_owned(),
                avatar: entry.avatar.clone(),
            });
    }
    for chat in conversations {
        for person in &chat.participants {
            let address = person.address.trim();
            if address.is_empty() {
                continue;
            }
            let key = address_key(address);
            let name = if person.name.trim().is_empty() {
                address.to_owned()
            } else {
                person.name.trim().to_owned()
            };
            by_key
                .entry(key)
                .and_modify(|existing| {
                    if existing.name == existing.address && name != address {
                        existing.name = name.clone();
                    }
                    if existing.avatar.is_none() && person.avatar.is_some() {
                        existing.avatar.clone_from(&person.avatar);
                    }
                })
                .or_insert(ContactEntry {
                    name,
                    address: address.to_owned(),
                    avatar: person.avatar.clone(),
                });
        }
        if !chat.is_group()
            && chat.participants.is_empty()
            && let Some(address) = chat.guid.rsplit(';').next()
            && looks_like_address(address)
        {
            let key = address_key(address);
            by_key.entry(key).or_insert_with(|| ContactEntry {
                name: chat.name.clone(),
                address: address.to_owned(),
                avatar: chat.avatar.clone(),
            });
        }
    }
    let mut merged: Vec<ContactEntry> = by_key.into_values().collect();
    merged.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.address.cmp(&right.address))
    });
    merged
}

pub fn find_existing_conversation<'a>(
    conversations: &'a [Conversation],
    addresses: &[String],
) -> Option<&'a Conversation> {
    use std::collections::BTreeSet;

    let wanted: BTreeSet<String> = addresses
        .iter()
        .map(|address| address_key(address))
        .filter(|key| !key.is_empty())
        .collect();
    if wanted.is_empty() {
        return None;
    }
    conversations
        .iter()
        .filter(|chat| {
            let mut keys: BTreeSet<String> = chat
                .participants
                .iter()
                .map(|person| address_key(&person.address))
                .filter(|key| !key.is_empty())
                .collect();
            if keys.is_empty()
                && !chat.is_group()
                && let Some(address) = chat.guid.rsplit(';').next()
                && looks_like_address(address)
            {
                keys.insert(address_key(address));
            }
            if wanted.len() == 1 && chat.is_group() {
                return false;
            }
            keys == wanted
        })
        .max_by_key(|chat| chat.last_activity)
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
    #[serde(default)]
    pub service: Option<Service>,
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
        let service = Service::from_chat_guid(&guid);
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
            service,
            participants: Vec::new(),
        }
    }

    pub fn service(&self) -> Service {
        self.service
            .or_else(|| Service::from_chat_guid(&self.guid))
            .unwrap_or(Service::IMessage)
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
        if let Some(service) = message.service
            && (self.service.is_none() || Service::from_chat_guid(&self.guid).is_none())
        {
            self.service = Some(service);
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

    #[test]
    fn service_detects_sms_rcs_and_imessage() {
        assert_eq!(
            Service::from_chat_guid("SMS;-;+15550100100"),
            Some(Service::Sms)
        );
        assert_eq!(Service::from_chat_guid("RCS;+;chat42"), Some(Service::Rcs));
        assert_eq!(
            Service::from_chat_guid("iMessage;-;+15550100100"),
            Some(Service::IMessage)
        );
        assert_eq!(Service::from_chat_guid("any;+;chat99"), None);
        assert!(Service::Sms.is_green());
        assert!(Service::Rcs.is_green());
        assert!(!Service::IMessage.is_green());

        let mut unified =
            Conversation::new("any;-;+15550100100".into(), "Alex".into(), "", "".into());
        assert_eq!(unified.service(), Service::IMessage);
        let mut rcs_msg = msg("m1", "hi", 100);
        rcs_msg.service = Some(Service::Rcs);
        assert!(unified.update_preview(&rcs_msg));
        assert_eq!(unified.service(), Service::Rcs);
    }

    #[test]
    fn contacts_match_merge_and_find_existing_conversations() {
        let maya = ContactEntry {
            name: "Maya Chen".into(),
            address: "+1 (555) 010-0100".into(),
            avatar: None,
        };
        assert!(maya.matches_query("maya"));
        assert!(maya.matches_query("chen 555"));
        assert!(maya.matches_query("5550100100"));
        assert!(!maya.matches_query("sam"));

        assert!(looks_like_address("+1 555-010-0100"));
        assert!(looks_like_address("maya@icloud.com"));
        assert!(!looks_like_address("Maya Chen"));
        assert!(!looks_like_address("12"));

        let mut dm = Conversation::new(
            "iMessage;-;+15550100100".into(),
            "Maya".into(),
            "",
            "".into(),
        );
        dm.participants.push(Participant {
            address: "+15550100100".into(),
            name: "Maya Chen".into(),
            avatar: None,
        });
        dm.last_activity = 200;
        let mut group = Conversation::new("iMessage;+;chat1".into(), "Crew".into(), "", "".into());
        group.group_style = true;
        group.participants.push(Participant {
            address: "+15550100100".into(),
            name: "Maya Chen".into(),
            avatar: None,
        });
        group.participants.push(Participant {
            address: "+15550100101".into(),
            name: "Sam Rivera".into(),
            avatar: None,
        });
        let merged = merge_contacts(&[maya], &[dm.clone(), group.clone()]);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].name, "Maya Chen");
        assert_eq!(merged[1].name, "Sam Rivera");

        let chats = vec![dm, group];
        assert_eq!(
            find_existing_conversation(&chats, &["555-010-0100".into()]).map(|c| c.guid.as_str()),
            Some("iMessage;-;+15550100100")
        );
        assert_eq!(
            find_existing_conversation(&chats, &["+15550100101".into(), "+15550100100".into()])
                .map(|c| c.guid.as_str()),
            Some("iMessage;+;chat1")
        );
        assert!(find_existing_conversation(&chats, &["+15559999999".into()]).is_none());
    }
}
