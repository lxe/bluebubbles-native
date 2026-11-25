use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::Path;
use std::time::Duration;

use reqwest::blocking::{Client, RequestBuilder, Response, multipart};
use reqwest::{Method, StatusCode, Url};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};

use crate::cache::{AVATAR_BYTE_LIMIT, Cache, ConversationSnapshot};
use crate::model::{
    Attachment, Conversation, Message, MessagePointer, Participant, Service, Tapback, TapbackKind,
    address_key, sort_conversations,
};

pub const PAGE_SIZE: usize = 50;
const CHAT_PAGE_SIZE: usize = 500;
const MAX_CHAT_PAGE_SIZE: usize = 1000;

#[derive(Clone)]
pub struct Credentials {
    pub server: Url,
    pub password: String,
}

impl Credentials {
    pub fn new(server: &str, password: String) -> Result<Self, String> {
        let server = Url::parse(server.trim().trim_end_matches('/'))
            .map_err(|_| "Enter a valid server URL, including http:// or https://.".to_string())?;
        if !matches!(server.scheme(), "http" | "https")
            || server.host_str().is_none()
            || !server.username().is_empty()
            || server.password().is_some()
            || server.query().is_some()
            || server.fragment().is_some()
        {
            return Err("Use an HTTP or HTTPS server URL without embedded credentials, a query, or a fragment.".into());
        }
        if password.is_empty() {
            return Err("Enter the BlueBubbles server password.".into());
        }
        Ok(Self { server, password })
    }
}

#[derive(Clone)]
pub struct BlueBubbles {
    http: Client,
    credentials: Credentials,
    private_api: bool,
}

#[derive(Deserialize)]
struct Envelope<T> {
    data: T,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ServerChat {
    guid: String,
    style: Option<u32>,
    #[serde(alias = "serviceName")]
    service: Option<String>,
    properties: Option<Value>,
    display_name: Option<String>,
    chat_identifier: Option<String>,
    participants: Option<Vec<Handle>>,
    handles: Option<Vec<Handle>>,
    last_message: Option<ServerMessage>,
    latest_message: Option<ServerMessage>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Handle {
    address: Option<String>,
    display_name: Option<String>,
    service: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Contact {
    display_name: Option<String>,
    first_name: Option<String>,
    last_name: Option<String>,
    phone_numbers: Option<Vec<ContactAddress>>,
    emails: Option<Vec<ContactAddress>>,
    avatar: Option<String>,
}

#[derive(Clone)]
struct ContactInfo {
    name: String,
    avatar: Option<std::path::PathBuf>,
}

#[derive(Deserialize)]
struct ContactAddress {
    address: Option<String>,
    value: Option<String>,
    phone: Option<String>,
    email: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ServerMessage {
    guid: Option<String>,
    temp_guid: Option<String>,
    text: Option<String>,
    full_text: Option<String>,
    subject: Option<String>,
    attributed_body: Option<Vec<BodyPart>>,
    is_from_me: Option<bool>,
    date_created: Option<Value>,
    date_edited: Option<i64>,
    #[serde(rename = "dateCreatedISO")]
    date_created_upper_iso: Option<String>,
    #[serde(rename = "dateCreatedIso")]
    date_created_iso: Option<String>,
    date: Option<Value>,
    has_attachments: Option<bool>,
    associated_message_guid: Option<String>,
    associated_message_type: Option<Value>,
    associated_message_part: Option<u32>,
    #[serde(default)]
    associated_messages: Option<Vec<ServerMessage>>,
    handle: Option<Handle>,
    handle_id: Option<i64>,
    #[serde(alias = "serviceName")]
    service: Option<String>,
    thread_originator_guid: Option<String>,
    thread_originator_part: Option<Value>,
    normalized_thread_part: Option<u32>,
    selected_message_guid: Option<String>,
    chats: Option<Vec<ServerChat>>,
    #[serde(default)]
    attachments: Option<Vec<ServerAttachment>>,
    #[serde(default)]
    attachment: Option<AttachmentField>,
}

#[derive(Deserialize)]
struct BodyPart {
    string: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum AttachmentField {
    Multiple(Vec<ServerAttachment>),
    Single(ServerAttachment),
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ServerAttachment {
    guid: String,
    transfer_name: Option<String>,
    mime_type: Option<String>,
}

impl BlueBubbles {
    pub fn new(credentials: Credentials) -> Result<Self, String> {
        let http = Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(20))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| "Could not initialize the HTTP client.".to_string())?;
        Ok(Self {
            http,
            credentials,
            private_api: false,
        })
    }

    pub fn info(&mut self) -> Result<String, String> {
        let value: Value = self.get(self.request(Method::GET, &["server", "info"]))?;
        self.private_api = value.get("private_api").and_then(Value::as_bool) == Some(true)
            && value.get("helper_connected").and_then(Value::as_bool) != Some(false);
        Ok(value
            .get("server_version")
            .and_then(Value::as_str)
            .unwrap_or("BlueBubbles")
            .to_owned())
    }

    pub fn supports_message_actions(&self) -> bool {
        self.private_api
    }

    fn conversations(&self, server_count: Option<usize>) -> Result<Vec<Conversation>, String> {
        let contacts = self.contacts().unwrap_or_default();
        let cache = self.cache();
        let mut chats = Vec::new();
        let mut seen = HashSet::new();
        let mut offset = 0;
        // BlueBubbles looks up the latest messages for the entire inventory on
        // every page. Its largest supported page reduces those repeated scans.
        let page_size = if server_count.is_some_and(|count| count > CHAT_PAGE_SIZE) {
            MAX_CHAT_PAGE_SIZE
        } else {
            CHAT_PAGE_SIZE
        };
        loop {
            let previous_size = chats.len();
            let page = self.chat_page(offset, page_size)?;
            let count = page.len();
            for chat in page {
                if seen.insert(chat.guid.clone()) {
                    let mut chat = chat.into_conversation(&contacts);
                    cache.restore_group_avatar(&mut chat);
                    chats.push(chat);
                }
            }
            if count == 0
                || (count < page_size && server_count.is_none_or(|total| offset + count >= total))
            {
                sort_conversations(&mut chats);
                return Ok(chats);
            }
            if chats.len() == previous_size {
                return Err("The server repeated the same conversation page.".into());
            }
            offset += count;
        }
    }

    pub fn sync_conversations(
        &self,
        mut snapshot: ConversationSnapshot,
        full: bool,
    ) -> Result<ConversationSnapshot, String> {
        const RECENT_PAGE: usize = 100;
        let count = self
            .get::<Value>(self.request(Method::GET, &["chat", "count"]))
            .ok()
            .and_then(|value| value.as_u64().or_else(|| value.get("total")?.as_u64()))
            .and_then(|count| usize::try_from(count).ok());
        if full
            || snapshot.conversations.is_empty()
            || !snapshot.has_participants
            || !snapshot.has_group_photos
            || matches!((snapshot.server_count, count), (Some(previous), Some(current)) if previous != current)
        {
            return Ok(ConversationSnapshot {
                server_count: count,
                conversations: self.conversations(count)?,
                has_participants: true,
                has_group_photos: true,
            });
        }
        let cached_latest = snapshot
            .conversations
            .iter()
            .map(|chat| chat.last_activity)
            .max()
            .unwrap_or_default();
        let positions = snapshot
            .conversations
            .iter()
            .enumerate()
            .map(|(index, chat)| (chat.guid.clone(), index))
            .collect::<HashMap<_, _>>();
        let empty_contacts = HashMap::new();
        let cache = self.cache();
        for offset in (0..400).step_by(RECENT_PAGE) {
            let page = self.chat_page(offset, RECENT_PAGE)?;
            if page
                .iter()
                .any(|entry| !positions.contains_key(&entry.guid))
            {
                break;
            }
            let reached_cache = page.len() < RECENT_PAGE
                || page
                    .last()
                    .and_then(ServerChat::latest_message)
                    .is_some_and(|message| message.timestamp() <= cached_latest);
            for entry in page {
                let display_name = entry
                    .display_name
                    .as_ref()
                    .is_some_and(|name| !name.trim().is_empty());
                let saved = &mut snapshot.conversations[positions[&entry.guid]];
                let mut latest = entry.into_conversation(&empty_contacts);
                if latest.last_activity >= saved.last_activity {
                    if !display_name {
                        latest.search_text =
                            format!("{} {}", saved.name.to_lowercase(), latest.search_text);
                        latest.name.clone_from(&saved.name);
                    }
                    if latest.is_group() {
                        cache.restore_group_avatar(&mut latest);
                    } else {
                        latest.avatar.clone_from(&saved.avatar);
                    }
                    for person in &mut latest.participants {
                        if let Some(saved_person) = saved.participants.iter().find(|cached| {
                            address_key(&cached.address) == address_key(&person.address)
                        }) {
                            person.clone_from(saved_person);
                        }
                    }
                    *saved = latest;
                }
            }
            if reached_cache {
                sort_conversations(&mut snapshot.conversations);
                snapshot.server_count = count.or(snapshot.server_count);
                return Ok(snapshot);
            }
        }
        Ok(ConversationSnapshot {
            server_count: count,
            conversations: self.conversations(count)?,
            has_participants: true,
            has_group_photos: true,
        })
    }

    pub fn cache(&self) -> Cache {
        Cache::new(self.credentials.server.as_str())
    }

    pub fn group_avatar(&self, chat: &str, photo: &str) -> Result<std::path::PathBuf, String> {
        let cache = self.cache();
        let path = cache.group_avatar_path(photo);
        if path.is_file() {
            return Ok(path);
        }
        let response = self.execute(
            self.request(Method::GET, &["chat", chat, "icon"])
                .timeout(Duration::from_secs(6)),
        )?;
        let mut data = Vec::new();
        response
            .take(AVATAR_BYTE_LIMIT as u64 + 1)
            .read_to_end(&mut data)
            .map_err(|_| "Could not read the group photo.".to_string())?;
        cache
            .save_group_avatar(photo, &data)
            .ok_or_else(|| "Could not render the group photo.".to_string())
    }

    fn chat_page(&self, offset: usize, limit: usize) -> Result<Vec<ServerChat>, String> {
        let _profile = crate::profile::span("sidebar-page-request-worker");
        self.get(self.request(Method::POST, &["chat", "query"]).json(&json!({
            "with": ["participants", "lastmessage"],
            "offset": offset,
            "limit": limit,
            "sort": "lastmessage",
        })))
    }

    fn contacts(&self) -> Result<HashMap<String, ContactInfo>, String> {
        let contacts: Vec<Contact> = {
            let _profile = crate::profile::span("sidebar-contacts-request-worker");
            self.get(
                self.request(Method::GET, &["contact"])
                    .query(&[("extraProperties", "avatar")]),
            )?
        };
        let _profile = crate::profile::span("sidebar-contacts-process-worker");
        let cache = self.cache();
        let mut names = HashMap::new();
        for contact in contacts {
            let name = contact
                .display_name
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| {
                    [contact.first_name, contact.last_name]
                        .into_iter()
                        .flatten()
                        .collect::<Vec<_>>()
                        .join(" ")
                });
            let avatar = contact.avatar.as_deref().and_then(|data| {
                let encoded = data.split_once(',').map(|(_, data)| data).unwrap_or(data);
                if encoded.len() > 2_000_000 {
                    return None;
                }
                cache.avatar(&gtk::glib::base64_decode(encoded))
            });
            if name.trim().is_empty() && avatar.is_none() {
                continue;
            }
            let person = ContactInfo { name, avatar };
            for address in contact
                .phone_numbers
                .into_iter()
                .flatten()
                .chain(contact.emails.into_iter().flatten())
            {
                if let Some(address) = address
                    .address
                    .or(address.value)
                    .or(address.phone)
                    .or(address.email)
                {
                    names.insert(address_key(&address), person.clone());
                }
            }
        }
        Ok(names)
    }

    pub fn messages(&self, guid: &str, offset: usize) -> Result<(Vec<Message>, usize), String> {
        let request = self
            .request(Method::GET, &["chat", guid, "message"])
            .timeout(Duration::from_secs(10))
            .query(&[
                (
                    "with",
                    "attachments,attachment,handle,message.attributedBody".to_owned(),
                ),
                ("sort", "DESC".into()),
                ("offset", offset.to_string()),
                ("limit", PAGE_SIZE.to_string()),
            ]);
        let response = {
            let _profile = crate::profile::span("thread-http-headers-worker");
            self.execute(request)?
        };
        let raw: Vec<ServerMessage> = {
            let _profile = crate::profile::span("thread-http-body-and-json-worker");
            Self::decode_response(response)?
        };
        let project = crate::profile::span("thread-project-rows-worker");
        let count = raw.len();
        let mut results = Vec::new();
        for mut row in raw.into_iter().rev() {
            let associated = std::mem::take(&mut row.associated_messages);
            let target = row.guid.clone();
            for mut event in associated.into_iter().flatten() {
                if event.associated_message_guid.is_none() {
                    event.associated_message_guid.clone_from(&target);
                }
                results.extend(event.into_message());
            }
            results.extend(row.into_message());
        }
        let mut unique = Vec::with_capacity(results.len());
        crate::model::merge_messages(&mut unique, results);
        let results = unique;
        drop(project);
        let _profile = crate::profile::span("thread-cache-write-worker");
        self.cache().merge_messages(guid, &results);
        Ok((results, count))
    }

    pub fn send_text(
        &self,
        guid: &str,
        message: &str,
        reply: Option<&MessagePointer>,
        temp_guid: &str,
    ) -> Result<Option<Message>, String> {
        let mut body = json!({
            "chatGuid": guid, "tempGuid": temp_guid,
            "message": message, "method": "apple-script",
        });
        if let Some(reply) = reply {
            body["method"] = json!("private-api");
            body["selectedMessageGuid"] = json!(reply.guid);
            body["partIndex"] = json!(reply.part);
        }
        let request = self.request(Method::POST, &["message", "text"]).json(&body);
        self.send_response(request.timeout(Duration::from_secs(130)))
    }

    pub fn send_attachment(
        &self,
        guid: &str,
        path: &Path,
        reply: Option<&MessagePointer>,
        temp_guid: &str,
    ) -> Result<Option<Message>, String> {
        let part = multipart::Part::file(path)
            .map_err(|_| "Could not read the attachment file.".to_string())?;
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or("The attachment needs a valid file name.")?;
        let mut form = multipart::Form::new()
            .part("attachment", part)
            .text("chatGuid", guid.to_owned())
            .text("tempGuid", temp_guid.to_owned())
            .text("name", name.to_owned());
        if let Some(reply) = reply {
            form = form
                .text("method", "private-api")
                .text("selectedMessageGuid", reply.guid.clone())
                .text("partIndex", reply.part.to_string());
        } else {
            form = form.text("method", "apple-script");
        }
        self.send_response(
            self.request(Method::POST, &["message", "attachment"])
                .timeout(Duration::from_secs(180))
                .multipart(form),
        )
    }

    pub fn react(
        &self,
        chat: &str,
        selected: &Message,
        reaction: TapbackKind,
        remove: bool,
    ) -> Result<Option<Message>, String> {
        let reaction = if remove {
            format!("-{}", reaction.name())
        } else {
            reaction.name().to_owned()
        };
        let mut response = self.write_response(
            self.request(Method::POST, &["message", "react"])
                .timeout(Duration::from_secs(70))
                .json(&json!({
                    "chatGuid": chat, "selectedMessageGuid": selected.guid,
                    "selectedMessageText": selected.text, "reaction": reaction, "partIndex": 0,
                })),
        )?;
        Ok(response.get_mut("data").and_then(|data| {
            serde_json::from_value::<ServerMessage>(Value::take(data))
                .ok()?
                .into_message()
        }))
    }

    pub fn download(&self, guid: &str, destination: &Path) -> Result<(), String> {
        self.download_variant(guid, destination, true, None)
    }

    pub fn media(
        &self,
        attachment: &Attachment,
        original: bool,
    ) -> Result<std::path::PathBuf, String> {
        let cache = self.cache();
        let path = cache.media_path(&attachment.guid, &attachment.name, original)?;
        if path.is_file() {
            return Ok(path);
        }
        let cap = (!original).then_some(16 * 1024 * 1024);
        let res = self.download_variant(&attachment.guid, &path, original, cap);
        if res.is_err() && !original {
            self.download_variant(&attachment.guid, &path, true, cap)?;
        } else {
            res?;
        }
        Ok(path)
    }

    fn download_variant(
        &self,
        guid: &str,
        destination: &Path,
        original: bool,
        cap: Option<u64>,
    ) -> Result<(), String> {
        let mut response = self.execute(
            self.request(Method::GET, &["attachment", guid, "download"])
                .query(&[("original", if original { "true" } else { "false" })])
                .timeout(Duration::from_secs(if cap.is_some() { 10 } else { 180 })),
        )?;
        if response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| {
                value.starts_with("application/json") || value.starts_with("text/html")
            })
        {
            // JSON and HTML can also be valid user files when the server serves them as attachments.
            if !response
                .headers()
                .contains_key(reqwest::header::CONTENT_DISPOSITION)
            {
                return Err("The server returned a response instead of the attachment.".into());
            }
        }
        let parent = destination
            .parent()
            .ok_or("Choose a valid save location.")?;
        let mut file = tempfile::NamedTempFile::new_in(parent)
            .map_err(|_| "Could not write to the selected folder.".to_string())?;
        if let Some(cap) = cap {
            use std::io::Read;
            let size = std::io::copy(&mut response.take(cap + 1), &mut file)
                .map_err(|_| "The attachment preview was interrupted.".to_string())?;
            if size > cap {
                return Err("This attachment is too large for an inline preview.".into());
            }
        } else {
            std::io::copy(&mut response, &mut file)
                .map_err(|_| "The attachment download was interrupted.".to_string())?;
        }
        file.persist(destination)
            .map_err(|_| "Could not save the attachment at the selected location.".to_string())?;
        Ok(())
    }

    fn request(&self, method: Method, parts: &[&str]) -> RequestBuilder {
        let mut url = self.credentials.server.clone();
        url.path_segments_mut()
            .unwrap()
            .pop_if_empty()
            .extend(["api", "v1"])
            .extend(parts);
        self.authenticated_request(method, url)
    }

    pub fn realtime_request(&self, method: Method, sid: Option<&str>) -> RequestBuilder {
        let mut url = self.credentials.server.clone();
        url.path_segments_mut()
            .unwrap()
            .pop_if_empty()
            .extend(["socket.io", ""]);
        let request = self
            .authenticated_request(method, url)
            .header(reqwest::header::CACHE_CONTROL, "no-cache")
            .query(&[("EIO", "4"), ("transport", "polling")]);
        if let Some(sid) = sid {
            request.query(&[("sid", sid)])
        } else {
            request
        }
    }

    fn authenticated_request(&self, method: Method, url: Url) -> RequestBuilder {
        let mut request = self
            .http
            .request(method, url)
            .query(&[("guid", &self.credentials.password)]);
        let host = self.credentials.server.host_str().unwrap_or("");
        if host.contains("ngrok") {
            request = request.header("ngrok-skip-browser-warning", "true");
        }
        if host.contains("zrok") {
            request = request.header("skip_zrok_interstitial", "true");
        }
        request
    }

    fn execute(&self, request: RequestBuilder) -> Result<Response, String> {
        let response = request.send().map_err(|error| {
            if error.is_timeout() {
                "The server did not respond in time.".to_string()
            } else {
                "Could not reach the BlueBubbles server. Check the URL and connection.".to_string()
            }
        })?;
        match response.status() {
            status if status.is_success() => Ok(response),
            StatusCode::UNAUTHORIZED => {
                Err("The server rejected the password. Check it and try again.".into())
            }
            StatusCode::FORBIDDEN => {
                Err("The server denied access. Check the password and server permissions.".into())
            }
            status => Err(format!(
                "The BlueBubbles server returned HTTP {}.",
                status.as_u16()
            )),
        }
    }

    fn get<T: DeserializeOwned>(&self, request: RequestBuilder) -> Result<T, String> {
        Self::decode_response(self.execute(request)?)
    }

    fn decode_response<T: DeserializeOwned>(response: Response) -> Result<T, String> {
        response
            .json::<Envelope<T>>()
            .map(|body| body.data)
            .map_err(|_| "The server returned an unexpected response.".into())
    }

    fn send_response(&self, request: RequestBuilder) -> Result<Option<Message>, String> {
        let mut response = self.write_response(request)?;
        Ok(response.get_mut("data").and_then(|data| {
            serde_json::from_value::<ServerMessage>(Value::take(data))
                .ok()?
                .into_message()
        }))
    }

    fn write_response(&self, request: RequestBuilder) -> Result<Value, String> {
        let response = self.execute(request)?;
        let bytes = response.bytes().map_err(|_| {
            "The server did not confirm the send. Check the conversation before retrying."
                .to_string()
        })?;
        if bytes.is_empty() {
            return Ok(Value::Null);
        }
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| "The server sent an unexpected confirmation. Check the conversation before retrying.".to_string())?;
        if value.get("error").is_some_and(|value| !value.is_null())
            || value
                .get("status")
                .and_then(Value::as_u64)
                .is_some_and(|status| status >= 400)
        {
            return Err(
                "The BlueBubbles server reported that it could not send the message.".into(),
            );
        }
        Ok(value)
    }
}

pub fn check_connection() -> Result<(), String> {
    let credentials = crate::settings::load()?.ok_or(
        "Configure a server in Preferences or set BLUEBUBBLES_NATIVE_SERVER and BLUEBUBBLES_NATIVE_PASSWORD.",
    )?;
    let mut server = BlueBubbles::new(credentials)?;
    let version = server.info()?;
    let cache = server.cache();
    let snapshot = server.sync_conversations(cache.conversations(), false)?;
    cache.save_conversations(&snapshot);
    let chats = snapshot.conversations;
    println!(
        "Authenticated to BlueBubbles {version}. Loaded {} conversations.",
        chats.len()
    );
    if let Some(chat) = chats.first() {
        let (messages, _) = server.messages(&chat.guid, 0)?;
        println!(
            "Read {} messages in the first conversation.",
            messages.len()
        );
    }
    Ok(())
}

pub fn realtime_messages(mut data: Value) -> Vec<(Conversation, Message)> {
    let raw = data
        .get_mut("data")
        .filter(|item| item.is_object())
        .map(Value::take)
        .unwrap_or(data);
    let chat_guid = raw
        .get("chatGuid")
        .and_then(Value::as_str)
        .filter(|guid| !guid.is_empty())
        .map(str::to_owned);
    let Ok(mut message) = serde_json::from_value::<ServerMessage>(raw) else {
        return Vec::new();
    };
    let chats = message.chats.take().unwrap_or_default();
    let Some(message) = message.into_message() else {
        return Vec::new();
    };
    let contacts = HashMap::new();
    let mut results: Vec<_> = chats
        .into_iter()
        .map(|chat| chat.into_conversation(&contacts))
        .collect();
    if results.is_empty()
        && let Some(guid) = chat_guid
    {
        let name = message
            .sender
            .clone()
            .unwrap_or_else(|| guid.split(';').next_back().unwrap_or(&guid).to_owned());
        results.push(Conversation::new(guid, name, "", String::new()));
    }
    results
        .into_iter()
        .map(|chat| (chat, message.clone()))
        .collect()
}

impl ServerChat {
    fn latest_message(&self) -> Option<&ServerMessage> {
        [self.last_message.as_ref(), self.latest_message.as_ref()]
            .into_iter()
            .flatten()
            .max_by_key(|message| message.timestamp())
    }

    fn into_conversation(self, contacts: &HashMap<String, ContactInfo>) -> Conversation {
        let latest = self.latest_message();
        let last_activity = latest.map(ServerMessage::timestamp).unwrap_or_default();
        let latest_service = latest.and_then(ServerMessage::resolved_service);
        let preview = latest
            .map(|message| {
                let text = message.display_text();
                if text.is_empty()
                    && (message.has_attachments.unwrap_or(false)
                        || message
                            .attachments
                            .as_ref()
                            .is_some_and(|files| !files.is_empty())
                        || message.attachment.is_some())
                {
                    "Attachment".into()
                } else {
                    text
                }
            })
            .unwrap_or_default();
        let people = self
            .participants
            .filter(|people| !people.is_empty())
            .or(self.handles)
            .unwrap_or_default();
        let lookup = |person: &Handle| {
            person
                .address
                .as_ref()
                .and_then(|address| contacts.get(&address_key(address)))
        };
        let names = people
            .iter()
            .filter_map(|person| {
                lookup(person)
                    .filter(|contact| !contact.name.is_empty())
                    .map(|contact| contact.name.clone())
                    .or_else(|| person.display_name.clone())
                    .or_else(|| person.address.clone())
            })
            .collect::<Vec<_>>();
        let avatar = people
            .iter()
            .filter_map(lookup)
            .find_map(|contact| contact.avatar.clone());
        let addresses = people
            .iter()
            .filter_map(|person| person.address.as_deref())
            .chain(self.chat_identifier.as_deref())
            .collect::<Vec<_>>()
            .join(" ");
        let name = self
            .display_name
            .filter(|value| !value.trim().is_empty())
            .or_else(|| (!names.is_empty()).then(|| names.join(", ")))
            .or(self.chat_identifier)
            .unwrap_or_else(|| "Conversation".into());
        let mut row = Conversation::new(
            self.guid,
            name,
            &format!("{addresses} {}", names.join(" ")),
            preview,
        );
        row.last_activity = last_activity;
        row.group_style = self.style == Some(43);
        row.service = Service::from_chat_guid(&row.guid)
            .or_else(|| self.service.as_deref().and_then(Service::parse))
            .or(latest_service);
        row.group_photo = self
            .properties
            .as_ref()
            .and_then(Value::as_array)
            .and_then(|items| {
                items
                    .iter()
                    .rev()
                    .filter_map(|item| item.get("groupPhotoGuid").and_then(Value::as_str))
                    .find(|guid| !guid.trim().is_empty())
            })
            .map(str::to_owned);
        row.participants = people
            .iter()
            .filter_map(|person| {
                let address = person.address.as_ref()?;
                let contact = lookup(person);
                let name = contact
                    .filter(|contact| !contact.name.is_empty())
                    .map(|contact| contact.name.clone())
                    .or_else(|| person.display_name.clone())
                    .unwrap_or_else(|| address.clone());
                Some(Participant {
                    address: address.clone(),
                    name,
                    avatar: contact.and_then(|contact| contact.avatar.clone()),
                })
            })
            .collect();
        if !row.is_group() {
            row.avatar = avatar;
        }
        row
    }
}

impl ServerMessage {
    fn timestamp(&self) -> i64 {
        let from_json = |value: &Value| {
            value
                .as_i64()
                .map(normalize_timestamp)
                .or_else(|| value.as_str().and_then(parse_timestamp))
        };
        self.date_created
            .as_ref()
            .and_then(from_json)
            .or_else(|| self.date_created_iso.as_deref().and_then(parse_timestamp))
            .or_else(|| {
                self.date_created_upper_iso
                    .as_deref()
                    .and_then(parse_timestamp)
            })
            .or_else(|| self.date.as_ref().and_then(from_json))
            .unwrap_or_default()
    }

    fn display_text(&self) -> String {
        let attributed = self.attributed_body.as_ref().map(|parts| {
            parts
                .iter()
                .filter_map(|part| part.string.as_deref())
                .collect::<String>()
        });
        [
            self.full_text.as_deref(),
            self.text.as_deref(),
            attributed.as_deref(),
            self.subject.as_deref(),
        ]
        .into_iter()
        .flatten()
        .map(|text| {
            text.replace(
                [char::from_u32(0xfffc).unwrap(), char::REPLACEMENT_CHARACTER],
                "",
            )
            .trim()
            .to_owned()
        })
        .find(|text| !text.is_empty())
        .unwrap_or_default()
    }

    fn files(self) -> Vec<Attachment> {
        let additional = match self.attachment {
            Some(AttachmentField::Single(value)) => vec![value],
            Some(AttachmentField::Multiple(values)) => values,
            None => Vec::new(),
        };
        let mut seen = HashSet::new();
        self.attachments
            .into_iter()
            .flatten()
            .chain(additional)
            .filter(|file| seen.insert(file.guid.clone()))
            .map(|file| Attachment {
                guid: file.guid,
                name: file.transfer_name.unwrap_or_else(|| "Attachment".into()),
                mime_type: file.mime_type,
            })
            .collect()
    }

    fn resolved_service(&self) -> Option<Service> {
        self.service
            .as_deref()
            .and_then(Service::parse)
            .or_else(|| {
                self.handle
                    .as_ref()
                    .and_then(|handle| handle.service.as_deref())
                    .and_then(Service::parse)
            })
    }

    fn into_message(mut self) -> Option<Message> {
        let guid = self.guid.take()?;
        let service = self.resolved_service();
        let text = self.display_text();
        let outgoing = self.is_from_me.unwrap_or(false);
        let sent_at = self.timestamp();
        let edited_at = self
            .date_edited
            .map(normalize_timestamp)
            .unwrap_or_default();
        let sender = self
            .handle
            .as_ref()
            .and_then(|handle| handle.address.clone());
        let sender_id = self.handle_id;
        let tapback = if let Some(target) = self
            .associated_message_guid
            .as_deref()
            .filter(|value| !value.is_empty())
        {
            let (kind, removed) = parse_tapback(self.associated_message_type.as_ref()?)?;
            Some(Tapback {
                target: parse_pointer(target, self.associated_message_part)?,
                kind,
                removed,
            })
        } else {
            None
        };
        let reply_part = self.normalized_thread_part.or_else(|| {
            let part = self.thread_originator_part.as_ref()?;
            part.as_u64()
                .and_then(|number| u32::try_from(number).ok())
                .or_else(|| part.as_str()?.split(':').next()?.parse().ok())
        });
        let reply_to = [
            self.thread_originator_guid.as_deref(),
            self.selected_message_guid.as_deref(),
        ]
        .into_iter()
        .flatten()
        .find_map(|target| parse_pointer(target, reply_part))
        .filter(|target| !target.guid.eq_ignore_ascii_case(&guid));
        let temp_guid = self.temp_guid.take();
        let attachments = self.files();
        if tapback.is_none() && text.is_empty() && attachments.is_empty() {
            return None;
        }
        Some(Message {
            guid,
            temp_guid,
            text,
            outgoing,
            sent_at,
            edited_at,
            attachments,
            sender,
            sender_id,
            reply_to,
            tapback,
            service,
        })
    }
}

fn parse_pointer(value: &str, part: Option<u32>) -> Option<MessagePointer> {
    let value = value.trim();
    let (guid, index) = if let Some((prefix, guid)) = value.split_once('/') {
        let index = prefix
            .strip_prefix("p:")
            .or_else(|| prefix.strip_prefix("P:"))
            .and_then(|index| index.parse().ok());
        (guid, index)
    } else {
        (value.strip_prefix("bp:").unwrap_or(value), None)
    };
    (!guid.is_empty()).then(|| MessagePointer {
        guid: guid.to_owned(),
        part: part.or(index).unwrap_or_default(),
    })
}

fn parse_tapback(value: &Value) -> Option<(TapbackKind, bool)> {
    if let Some(number) = value.as_u64().or_else(|| value.as_str()?.parse().ok()) {
        let (index, removed) = match number {
            2000..=2005 => (number - 2000, false),
            3000..=3005 => (number - 3000, true),
            _ => return None,
        };
        return Some((TapbackKind::ALL[index as usize], removed));
    }
    let lower = value.as_str()?.trim().to_ascii_lowercase();
    let (name, removed) = lower
        .strip_prefix('-')
        .map_or((lower.as_str(), false), |name| (name, true));
    TapbackKind::parse(name).map(|kind| (kind, removed))
}

fn parse_timestamp(value: &str) -> Option<i64> {
    value.parse().map(normalize_timestamp).ok().or_else(|| {
        gtk::glib::DateTime::from_iso8601(value, None)
            .ok()
            .map(|date| date.to_unix() * 1_000 + i64::from(date.microsecond()) / 1_000)
    })
}

fn normalize_timestamp(value: i64) -> i64 {
    const APPLE_EPOCH_MS: i64 = 978_307_200_000;
    match value {
        100_000_000_000_000_001.. => value / 1_000_000 + APPLE_EPOCH_MS,
        100_000_000_000_001.. => value / 1_000 + APPLE_EPOCH_MS,
        10_000_000_001.. => value,
        1_000_000_001.. => value * 1_000,
        _ => value,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BlueBubbles, Credentials, ServerChat, ServerMessage, normalize_timestamp, parse_pointer,
        parse_tapback, realtime_messages,
    };
    use crate::model::TapbackKind;
    use serde_json::json;
    use std::path::Path;
    use std::process::{Child, Command, Stdio};
    use std::time::{Duration, Instant};

    #[test]
    fn credentials_validate_url_and_password() {
        let valid = Credentials::new("https://bb.example.com///", "secret".into()).unwrap();
        assert_eq!(valid.server.as_str(), "https://bb.example.com/");
        assert!(Credentials::new("ftp://bb.example.com", "secret".into()).is_err());
        assert!(Credentials::new("https://user@bb.example.com", "secret".into()).is_err());
        assert!(Credentials::new("https://bb.example.com?a=1", "secret".into()).is_err());
        assert!(Credentials::new("https://bb.example.com", String::new()).is_err());
    }

    #[test]
    fn parses_pointers_tapbacks_timestamps_and_realtime_envelopes() {
        let pointer = parse_pointer("p:2/abc-123", None).unwrap();
        assert_eq!((pointer.guid.as_str(), pointer.part), ("abc-123", 2));
        let bp = parse_pointer("bp:xyz", None).unwrap();
        assert_eq!((bp.guid.as_str(), bp.part), ("xyz", 0));

        assert_eq!(
            parse_tapback(&json!(2000)),
            Some((TapbackKind::Love, false))
        );
        assert_eq!(parse_tapback(&json!(3001)), Some((TapbackKind::Like, true)));
        assert_eq!(
            parse_tapback(&json!("-laugh")),
            Some((TapbackKind::Laugh, true))
        );
        assert_eq!(parse_tapback(&json!(9999)), None);

        assert_eq!(normalize_timestamp(1_700_000_000), 1_700_000_000_000);
        assert_eq!(normalize_timestamp(1_700_000_000_000), 1_700_000_000_000);

        let delivered = realtime_messages(json!({
            "data": {
                "guid": "m-live",
                "chatGuid": "iMessage;-;+15550100100",
                "text": "Hello live",
                "isFromMe": false,
                "dateCreated": 1795500000000_i64
            }
        }));
        assert_eq!(delivered.len(), 1);
        assert_eq!(delivered[0].0.guid, "iMessage;-;+15550100100");
        assert_eq!(delivered[0].1.text, "Hello live");
    }

    struct Fixture(Child);

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[test]
    fn uses_newer_of_both_chat_timestamps() {
        let chat: ServerChat = serde_json::from_str(
            r#"{"guid":"example","lastMessage":{"dateCreated":1795500000000},"latestMessage":{"dateCreatedISO":"2026-11-24T18:00:00Z","text":"Latest"}}"#,
        ).unwrap();
        let conversation = chat.into_conversation(&std::collections::HashMap::new());
        assert_eq!(conversation.preview, "Latest");
        assert!(conversation.last_activity > 1_795_500_000_000);
    }

    #[test]
    fn only_explicit_thread_fields_create_quotes() {
        let ordinary: ServerMessage = serde_json::from_str(
            r#"{"guid":"ordinary","text":"Next message","replyToGuid":"previous"}"#,
        )
        .unwrap();
        assert!(ordinary.into_message().unwrap().reply_to.is_none());

        let threaded: ServerMessage = serde_json::from_str(
            r#"{"guid":"threaded","text":"A real reply","replyToGuid":"previous","threadOriginatorGuid":"p:2/parent"}"#,
        )
        .unwrap();
        let reply = threaded.into_message().unwrap().reply_to.unwrap();
        assert_eq!((reply.guid.as_str(), reply.part), ("parent", 2));

        let rcs_chat: ServerChat = serde_json::from_str(
            r#"{"guid":"any;+;chat-rcs","lastMessage":{"guid":"m1","text":"hi","service":"RCS","dateCreated":1795500000000}}"#,
        )
        .unwrap();
        let conversation = rcs_chat.into_conversation(&std::collections::HashMap::new());
        assert_eq!(conversation.service(), crate::model::Service::Rcs);

        let sms_message: ServerMessage = serde_json::from_str(
            r#"{"guid":"m-sms","text":"fallback","isFromMe":true,"handle":{"address":"+15550100100","service":"SMS"}}"#,
        )
        .unwrap();
        assert_eq!(
            sms_message.into_message().unwrap().service,
            Some(crate::model::Service::Sms)
        );
    }

    #[test]
    fn fixture_accepts_text_and_file_and_returns_download() {
        let directory = tempfile::tempdir().unwrap();
        let ready = directory.path().join("port");
        let receipts = directory.path().join("receipts");
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixture.py");
        let _process = Fixture(
            Command::new("python3")
                .arg(fixture)
                .arg("--ready")
                .arg(&ready)
                .arg("--receipts")
                .arg(&receipts)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let timeout = Instant::now() + Duration::from_secs(4);
        let port = loop {
            if let Ok(port) = std::fs::read_to_string(&ready)
                && !port.is_empty()
            {
                break port;
            }
            assert!(Instant::now() < timeout, "The fixture did not start");
            std::thread::sleep(Duration::from_millis(20));
        };
        let host = format!("http://127.0.0.1:{port}");
        let mut bad_password =
            BlueBubbles::new(Credentials::new(&host, "wrong".into()).unwrap()).unwrap();
        assert!(bad_password.info().is_err());
        let mut client =
            BlueBubbles::new(Credentials::new(&host, "fixture-secret".into()).unwrap()).unwrap();
        assert_eq!(client.info().unwrap(), "fixture");

        let chat = "iMessage;-;+15550100100";
        client
            .send_text(chat, "Test text", None, "temp-test-text")
            .unwrap();
        let source = directory.path().join("upload.txt");
        std::fs::write(&source, b"hello").unwrap();
        client
            .send_attachment(chat, &source, None, "temp-test-file")
            .unwrap();
        let destination = directory.path().join("download.txt");
        client.download("fixture-attachment", &destination).unwrap();
        assert_eq!(std::fs::read(destination).unwrap(), b"Attachment fixture\n");

        let events = std::fs::read_to_string(receipts)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert!(
            events
                .iter()
                .any(|event| event["event"] == "text" && event["text"] == "Test text")
        );
        assert!(events.iter().any(|event| event["event"] == "upload"
            && event["name"] == "upload.txt"
            && event["size"] == 5));
    }
}
