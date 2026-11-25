use std::io::Read;
use std::thread;
use std::time::Duration;

use reqwest::{Method, blocking::RequestBuilder};
use serde::Deserialize;
use serde_json::Value;

use crate::api::{BlueBubbles, realtime_messages};
use crate::model::{Conversation, Message};

pub enum Event {
    Connection(bool),
    Message(Box<Delivery>),
}

pub struct Delivery {
    pub chat: Conversation,
    pub message: Message,
    pub is_new: bool,
}

pub struct Listener {
    events: async_channel::Sender<Event>,
    worker: thread::Thread,
}

impl Listener {
    pub fn start(server: BlueBubbles) -> Option<(Self, async_channel::Receiver<Event>)> {
        let (events, receiver) = async_channel::bounded(64);
        let sender = events.clone();
        let worker = thread::Builder::new()
            .name("bluebubbles-realtime".into())
            .spawn(move || listen(&server, &sender))
            .ok()?
            .thread()
            .clone();
        Some((Self { events, worker }, receiver))
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.events.close();
        self.worker.unpark();
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Handshake {
    sid: String,
    ping_interval: u64,
    ping_timeout: u64,
}

fn listen(server: &BlueBubbles, events: &async_channel::Sender<Event>) {
    let mut delay = Duration::from_secs(2);
    while !events.is_closed() {
        let mut online = false;
        let _ = poll(server, events, &mut online);
        if online {
            let _ = events.send_blocking(Event::Connection(false));
            delay = Duration::from_secs(2);
        }
        if !events.is_closed() {
            thread::park_timeout(delay);
            delay = (delay * 2).min(Duration::from_secs(30));
        }
    }
}

fn poll(
    server: &BlueBubbles,
    events: &async_channel::Sender<Event>,
    online: &mut bool,
) -> Option<()> {
    let opening = read(
        server
            .realtime_request(Method::GET, None)
            .timeout(Duration::from_secs(6)),
    )?;
    let payload = opening.split('\u{1e}').next()?.strip_prefix('0')?;
    let handshake: Handshake = serde_json::from_str(payload).ok()?;
    if handshake.sid.is_empty() || events.is_closed() {
        return None;
    }
    let timeout = Duration::from_millis(
        handshake
            .ping_interval
            .saturating_add(handshake.ping_timeout)
            .clamp(5_000, 90_000)
            + 5_000,
    );
    let sid = Some(handshake.sid.as_str());
    let post = |packet: String| {
        read(
            server
                .realtime_request(Method::POST, sid)
                .timeout(Duration::from_secs(6))
                .header(reqwest::header::CONTENT_TYPE, "text/plain;charset=UTF-8")
                .body(packet),
        )
        .filter(|response| response == "ok")
    };
    post("40".into())?;
    let cache = server.cache();
    let mut empty = 0;
    // Engine.IO holds this request open until events or a heartbeat are available.
    while !events.is_closed() {
        let packets = read(server.realtime_request(Method::GET, sid).timeout(timeout))?;
        if packets.is_empty() {
            empty += 1;
            if empty > 2 {
                break;
            }
            continue;
        }
        empty = 0;
        for packet in packets.split('\u{1e}') {
            if let Some(ping) = packet.strip_prefix('2') {
                post(format!("3{ping}"))?;
            } else if packet == "1" || packet.starts_with("41") || packet.starts_with("44") {
                return None;
            } else if !*online && packet.starts_with("40") {
                *online = true;
                events.send_blocking(Event::Connection(true)).ok()?;
            } else if *online
                && let Some(json) = packet.strip_prefix("42")
                && let Ok((name, data)) = serde_json::from_str::<(String, Value)>(json)
                && matches!(name.as_str(), "new-message" | "updated-message")
            {
                for (chat, message) in realtime_messages(data) {
                    cache.merge_messages(&chat.guid, std::slice::from_ref(&message));
                    cache.save_live_chat_head(&chat, &message);
                    events
                        .send_blocking(Event::Message(Box::new(Delivery {
                            chat,
                            message,
                            is_new: name == "new-message",
                        })))
                        .ok()?;
                }
            }
        }
    }
    if events.is_closed() {
        let _ = post("1".into());
    }
    Some(())
}

fn read(request: RequestBuilder) -> Option<String> {
    const MAX_PAYLOAD: u64 = 8 * 1024 * 1024;
    let response = request.send().ok()?.error_for_status().ok()?;
    let mut data = String::new();
    response
        .take(MAX_PAYLOAD + 1)
        .read_to_string(&mut data)
        .ok()?;
    (data.len() as u64 <= MAX_PAYLOAD).then_some(data)
}
