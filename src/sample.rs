use crate::model::{Conversation, Message};

const STRESS_SIZE: usize = 10_000;
const FIELD_NOTES: &[(&str, bool)] = &[
    ("The trail map is up to date now.", false),
    ("Nice. Did you mark the fork by the creek?", true),
    (
        "Yes, and I added a note about the bridge. The route stays on the east bank until the second crossing.",
        false,
    ),
    ("That helps. I had the two crossings backwards.", true),
    ("I can bring a paper copy on Saturday.", false),
    ("Great. See you at the trailhead at nine.", true),
    ("I’ll bring coffee.", false),
];
const CONVERSATIONS: &[(&str, &[(&str, bool)])] = &[
    ("Field Notes", FIELD_NOTES),
    ("Maya Chen", &[("Friday still works for me.", false)]),
    (
        "Bike Workshop",
        &[("The stand is free after lunch.", false)],
    ),
    ("Sam Rivera", &[("I’ll send the route tonight.", true)]),
    ("Book Club", &[("Let’s start with chapter three.", false)]),
    ("Jordan Lee", &[("Thanks for the heads-up.", true)]),
];
const STRESS_LINES: &[(&str, bool)] = &[
    ("Short messages should stay compact.", false),
    (
        "Longer messages wrap at narrow widths. Scroll through the thread and check that recycled rows keep the correct text and alignment.",
        true,
    ),
    ("The next row has different content.", false),
];
const SAMPLE_LINE: &str = "This conversation is generated for the list test.";

pub fn conversations() -> Vec<Conversation> {
    let mut rows = CONVERSATIONS
        .iter()
        .map(|&(name, lines)| {
            Conversation::new(name.into(), name.into(), "", lines.last().unwrap().0.into())
        })
        .collect::<Vec<_>>();
    rows.push(Conversation::new(
        "Long thread".into(),
        "Long thread".into(),
        "",
        "10,000 mixed-length messages".into(),
    ));
    rows.extend((rows.len() + 1..=STRESS_SIZE).map(|number| {
        let name = format!("Sample conversation {number:05}");
        Conversation::new(name.clone(), name, "", SAMPLE_LINE.into())
    }));
    rows
}

pub fn messages(conversation: &Conversation) -> Vec<Message> {
    if conversation.guid == "Long thread" {
        return (0..STRESS_SIZE)
            .map(|index| {
                let (line, outgoing) = STRESS_LINES[index % STRESS_LINES.len()];
                Message {
                    guid: index.to_string(),
                    text: format!("{} · {line}", index + 1),
                    outgoing,
                    sent_at: 0,
                    ..Message::default()
                }
            })
            .collect();
    }
    let lines = CONVERSATIONS
        .iter()
        .find(|(name, _)| *name == conversation.guid)
        .map(|(_, lines)| *lines)
        .unwrap_or(&[(SAMPLE_LINE, false)]);
    lines
        .iter()
        .enumerate()
        .map(|(index, &(text, outgoing))| Message {
            guid: index.to_string(),
            text: text.into(),
            outgoing,
            sent_at: 0,
            ..Message::default()
        })
        .collect()
}
