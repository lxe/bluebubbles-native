use std::collections::HashMap;
use std::fs::{DirBuilder, File};
use std::io::{self, BufReader, Write};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};

use serde::{Serialize, de::DeserializeOwned};
use uuid::Uuid;

use crate::model::{ContactEntry, Conversation, Message, sort_conversations};

#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ConversationSnapshot {
    pub server_count: Option<usize>,
    pub conversations: Vec<Conversation>,
    #[serde(default)]
    pub has_participants: bool,
    #[serde(default)]
    pub has_group_photos: bool,
}

static THREAD_CACHE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
static LIVE_HEADS_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub struct Cache {
    directory: PathBuf,
}

impl Cache {
    pub fn new(server: &str) -> Self {
        let directory = gtk::glib::user_cache_dir()
            .join("bluebubbles-native")
            .join("v1")
            .join(key(server).to_string());
        Self { directory }
    }

    pub fn conversations(&self) -> ConversationSnapshot {
        let mut snapshot: ConversationSnapshot =
            read_json(&self.directory.join("conversations.json")).unwrap_or_default();
        let mut live: HashMap<String, Conversation> =
            read_json(&self.directory.join("live-chat-heads.json")).unwrap_or_default();
        let reorder = !live.is_empty();
        for chat in &mut snapshot.conversations {
            if let Some(head) = live.remove(&chat.guid)
                && head.last_activity >= chat.last_activity
            {
                chat.set_preview(head.last_activity, head.preview);
            }
            self.restore_group_avatar(chat);
        }
        for mut chat in live.into_values() {
            self.restore_group_avatar(&mut chat);
            snapshot.conversations.push(chat);
        }
        if reorder {
            sort_conversations(&mut snapshot.conversations);
        }
        snapshot
    }

    pub fn save_conversations(&self, rows: &ConversationSnapshot) {
        let _ = write_json(&self.directory.join("conversations.json"), rows);
    }

    pub fn contacts(&self) -> Vec<ContactEntry> {
        read_json(&self.directory.join("contacts.json")).unwrap_or_default()
    }

    pub fn save_contacts(&self, rows: &[ContactEntry]) {
        let _ = write_json(&self.directory.join("contacts.json"), rows);
    }

    pub fn save_live_chat_head(&self, chat: &Conversation, message: &Message) {
        if message.tapback.is_some() {
            return;
        }
        let _lock = LIVE_HEADS_LOCK.lock().unwrap();
        let path = self.directory.join("live-chat-heads.json");
        let mut heads: HashMap<String, Conversation> = read_json(&path).unwrap_or_default();
        let new_head = !heads.contains_key(&chat.guid);
        let head = heads
            .entry(chat.guid.clone())
            .or_insert_with(|| chat.clone());
        if !head.update_preview(message) && !new_head {
            return;
        }
        if heads.len() > 128
            && let Some(oldest) = heads
                .values()
                .min_by_key(|chat| chat.last_activity)
                .map(|chat| chat.guid.clone())
        {
            heads.remove(&oldest);
        }
        let _ = write_json(&path, &heads);
    }

    pub fn messages(&self, guid: &str) -> Vec<Message> {
        read_json(&self.thread(guid)).unwrap_or_default()
    }

    pub fn merge_messages(&self, guid: &str, rows: &[Message]) {
        if rows.is_empty() {
            return;
        }
        let _lock = THREAD_CACHE_LOCK.lock().unwrap();
        let mut cached = self.messages(guid);
        let mut rows = rows.to_vec();
        for message in &mut rows {
            message.temp_guid = None;
        }
        if crate::model::merge_messages(&mut cached, rows) {
            let _ = write_json(&self.thread(guid), &cached);
        }
    }

    pub fn media_path(&self, guid: &str, title: &str, original: bool) -> Result<PathBuf, String> {
        let dir = self.directory.join("media");
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)
            .map_err(|_| "Could not create the local media cache.".to_string())?;
        let variant = if original { "original" } else { "preview" };
        let extension = Path::new(title)
            .extension()
            .and_then(|ext| ext.to_str())
            .filter(|extension| {
                extension.len() < 10 && extension.bytes().all(|value| value.is_ascii_alphanumeric())
            })
            .unwrap_or("bin");
        Ok(dir.join(format!("{}-{variant}.{extension}", key(guid))))
    }

    pub fn link_path(&self, url: &str) -> PathBuf {
        self.directory
            .join("links")
            .join(format!("{}.json", key(url)))
    }

    pub fn avatar(&self, data: &[u8]) -> Option<PathBuf> {
        let id = Uuid::new_v5(&Uuid::NAMESPACE_OID, data);
        let path = self.directory.join("avatars").join(format!("{id}.png"));
        cache_avatar(path, data)
    }

    pub fn group_avatar_path(&self, photo: &str) -> PathBuf {
        self.directory
            .join("group-avatars")
            .join(format!("{}.png", key(photo)))
    }

    pub fn save_group_avatar(&self, photo: &str, data: &[u8]) -> Option<PathBuf> {
        cache_avatar(self.group_avatar_path(photo), data)
    }

    pub fn restore_group_avatar(&self, chat: &mut Conversation) {
        if chat.is_group() {
            chat.avatar = chat
                .group_photo
                .as_deref()
                .map(|photo| self.group_avatar_path(photo))
                .filter(|path| path.is_file());
        }
    }

    fn thread(&self, guid: &str) -> PathBuf {
        self.directory
            .join("threads")
            .join(format!("{}.json", key(guid)))
    }
}

pub const AVATAR_BYTE_LIMIT: usize = 4 * 1024 * 1024;

fn cache_avatar(path: PathBuf, data: &[u8]) -> Option<PathBuf> {
    if data.len() > AVATAR_BYTE_LIMIT {
        return None;
    }
    if path.is_file() {
        return Some(path);
    }
    let mut reader = image::ImageReader::new(io::Cursor::new(data))
        .with_guessed_format()
        .ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    let bitmap = reader.decode().ok()?.thumbnail(96, 96);
    let mut image = io::Cursor::new(Vec::new());
    bitmap.write_to(&mut image, image::ImageFormat::Png).ok()?;
    write_private(&path, &image.into_inner()).ok()?;
    Some(path)
}

pub fn read_json<T: DeserializeOwned>(path: &Path) -> io::Result<T> {
    Ok(serde_json::from_reader(BufReader::new(File::open(path)?))?)
}

pub fn write_json<T: Serialize + ?Sized>(path: &Path, value: &T) -> io::Result<()> {
    write_private(path, &serde_json::to_vec(value)?)
}

fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if std::fs::read(path).ok().as_deref() == Some(bytes) {
        return Ok(());
    }
    let dir = path.parent().ok_or(io::ErrorKind::InvalidInput)?;
    DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    let mut file = tempfile::NamedTempFile::new_in(dir)?;
    file.write_all(bytes)?;
    file.persist(path)?;
    Ok(())
}

fn key(value: &str) -> Uuid {
    Uuid::new_v5(&Uuid::NAMESPACE_URL, value.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_merges_live_heads_and_strips_temp_guids() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache {
            directory: dir.path().to_path_buf(),
        };
        let mut older = Conversation::new("c1".into(), "Maya".into(), "", "old".into());
        older.last_activity = 100;
        let mut newer = Conversation::new("c2".into(), "Sam".into(), "", "mid".into());
        newer.last_activity = 200;
        cache.save_conversations(&ConversationSnapshot {
            server_count: Some(2),
            conversations: vec![newer, older.clone()],
            has_participants: true,
            has_group_photos: true,
        });

        let mut live = Message {
            guid: "m1".into(),
            temp_guid: Some("temp-1".into()),
            text: "fresh push".into(),
            sent_at: 300,
            ..Message::default()
        };
        cache.save_live_chat_head(&older, &live);
        cache.merge_messages("c1", std::slice::from_ref(&live));

        let restored = cache.conversations();
        assert_eq!(restored.conversations[0].guid, "c1");
        assert_eq!(restored.conversations[0].preview, "fresh push");

        live.temp_guid = None;
        assert_eq!(cache.messages("c1"), vec![live]);
    }

    #[test]
    fn media_path_sanitizes_extensions() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache {
            directory: dir.path().to_path_buf(),
        };
        let ok = cache.media_path("g1", "photo.JPG", true).unwrap();
        assert_eq!(ok.extension().and_then(|e| e.to_str()), Some("JPG"));
        let bad = cache
            .media_path("g2", "weird.tar.bad_ext_too_long", false)
            .unwrap();
        assert_eq!(bad.extension().and_then(|e| e.to_str()), Some("bin"));
    }
}
