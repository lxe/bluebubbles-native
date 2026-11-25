use std::collections::BTreeMap;

use crate::model::Conversation;

#[derive(Clone, Default, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct ChatOrganization {
    pinned: Vec<String>,
    archived: BTreeMap<String, String>,
    pub last_open: Option<String>,
}

#[derive(Clone, Copy)]
pub enum ChatAction {
    TogglePin,
    Archive,
    Restore,
}

#[derive(PartialEq)]
pub struct SidebarRow {
    pub chat: Conversation,
    pub pinned: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ArchivedRow {
    pub guid: String,
    pub name: String,
}

impl ChatOrganization {
    pub fn is_archived(&self, guid: &str) -> bool {
        self.archived.contains_key(guid)
    }

    pub fn apply(&mut self, action: ChatAction, guid: &str, chat: Option<&Conversation>) -> bool {
        match action {
            ChatAction::TogglePin if chat.is_some() && !self.archived.contains_key(guid) => {
                if let Some(index) = self.pinned.iter().position(|item| item == guid) {
                    self.pinned.remove(index);
                } else {
                    self.pinned.insert(0, guid.to_owned());
                }
                true
            }
            ChatAction::Archive if !self.archived.contains_key(guid) => {
                let Some(chat) = chat else { return false };
                self.pinned.retain(|item| item != guid);
                if self.last_open.as_deref() == Some(guid) {
                    self.last_open = None;
                }
                self.archived.insert(guid.to_owned(), chat.name.clone());
                true
            }
            ChatAction::Restore => self.archived.remove(guid).is_some(),
            _ => false,
        }
    }

    pub fn sidebar(&self, chats: &[Conversation]) -> Vec<SidebarRow> {
        let ranks: BTreeMap<_, _> = self
            .pinned
            .iter()
            .enumerate()
            .map(|(i, id)| (id, i))
            .collect();
        let mut rows: Vec<_> = chats
            .iter()
            .filter(|chat| !self.archived.contains_key(&chat.guid))
            .map(|chat| (ranks.get(&chat.guid).copied().unwrap_or(usize::MAX), chat))
            .collect();
        // Stable sorting preserves the server's recency order for unpinned chats.
        rows.sort_by_key(|(rank, _)| *rank);
        rows.into_iter()
            .map(|(rank, chat)| SidebarRow {
                chat: chat.clone(),
                pinned: rank != usize::MAX,
            })
            .collect()
    }

    pub fn archive(&self, chats: &[Conversation]) -> Vec<ArchivedRow> {
        if self.archived.is_empty() {
            return Vec::new();
        }
        let current: BTreeMap<_, _> = chats
            .iter()
            .filter(|chat| self.archived.contains_key(&chat.guid))
            .map(|chat| (&chat.guid, &chat.name))
            .collect();
        let mut rows: Vec<_> = self
            .archived
            .iter()
            .map(|(guid, saved_name)| ArchivedRow {
                guid: guid.clone(),
                name: current.get(guid).copied().unwrap_or(saved_name).clone(),
            })
            .collect();
        rows.sort_by_cached_key(|row| row.name.to_lowercase());
        rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chat(guid: &str, name: &str) -> Conversation {
        Conversation::new(guid.into(), name.into(), "", String::new())
    }

    #[test]
    fn pin_archive_and_restore_maintain_order_and_invariants() {
        let chats = vec![chat("c1", "Zoe"), chat("c2", "Alice"), chat("c3", "Maya")];
        let mut org = ChatOrganization {
            last_open: Some("c2".into()),
            ..ChatOrganization::default()
        };

        assert!(org.apply(ChatAction::TogglePin, "c3", Some(&chats[2])));
        assert!(org.apply(ChatAction::TogglePin, "c2", Some(&chats[1])));
        let ids: Vec<_> = org
            .sidebar(&chats)
            .into_iter()
            .map(|r| (r.chat.guid, r.pinned))
            .collect();
        assert_eq!(
            ids,
            vec![
                ("c2".into(), true),
                ("c3".into(), true),
                ("c1".into(), false),
            ]
        );

        // Archiving unpins, clears last_open, and hides from sidebar.
        assert!(org.apply(ChatAction::Archive, "c2", Some(&chats[1])));
        assert!(org.is_archived("c2"));
        assert!(org.last_open.is_none());
        assert_eq!(
            org.sidebar(&chats)
                .into_iter()
                .map(|r| r.chat.guid)
                .collect::<Vec<_>>(),
            vec!["c3", "c1"]
        );
        assert_eq!(
            org.archive(&chats),
            vec![ArchivedRow {
                guid: "c2".into(),
                name: "Alice".into(),
            }]
        );

        // Restoring puts it back in normal server order (unpinned).
        assert!(org.apply(ChatAction::Restore, "c2", None));
        assert!(!org.is_archived("c2"));
        assert_eq!(
            org.sidebar(&chats)
                .into_iter()
                .map(|r| r.chat.guid)
                .collect::<Vec<_>>(),
            vec!["c3", "c1", "c2"]
        );
    }
}
