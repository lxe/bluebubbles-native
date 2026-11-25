use std::collections::HashSet;
use std::io::Read;
use std::net::{IpAddr, ToSocketAddrs};
use std::time::Duration;

use reqwest::{Url, blocking::Client};
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};

use crate::cache::{Cache, read_json, write_json};

#[derive(Deserialize, Serialize)]
pub struct Preview {
    pub title: String,
    pub description: Option<String>,
}

pub fn first(text: &str) -> Option<Url> {
    url_spans(text).next().map(|(_, _, url)| url)
}

pub fn format_markup(text: &str) -> Option<String> {
    let mut spans = url_spans(text).peekable();
    spans.peek()?;
    let mut out = String::with_capacity(text.len() + 32);
    let mut cursor = 0;
    for (start, end, url) in spans {
        out.push_str(&gtk::glib::markup_escape_text(&text[cursor..start]));
        let href = gtk::glib::markup_escape_text(url.as_str());
        let display = gtk::glib::markup_escape_text(&text[start..end]);
        out.push_str("<a href=\"");
        out.push_str(&href);
        out.push_str("\">");
        out.push_str(&display);
        out.push_str("</a>");
        cursor = end;
    }
    out.push_str(&gtk::glib::markup_escape_text(&text[cursor..]));
    Some(out)
}

fn url_spans(text: &str) -> impl Iterator<Item = (usize, usize, Url)> + '_ {
    let mut index = 0;
    std::iter::from_fn(move || {
        while index < text.len() {
            let rest = &text[index..];
            let ws_len = rest
                .chars()
                .take_while(|c| c.is_whitespace())
                .map(char::len_utf8)
                .sum::<usize>();
            index += ws_len;
            if index >= text.len() {
                return None;
            }
            let token_start = index;
            let token_len = text[index..]
                .chars()
                .take_while(|c| !c.is_whitespace())
                .map(char::len_utf8)
                .sum::<usize>();
            index += token_len;
            let token = &text[token_start..token_start + token_len];
            let (trim_start, trim_end) = trim_url_bounds(token);
            let candidate = &token[trim_start..trim_end];
            if let Ok(url) = Url::parse(candidate)
                && valid_url(&url)
            {
                return Some((token_start + trim_start, token_start + trim_end, url));
            }
        }
        None
    })
}

fn trim_url_bounds(token: &str) -> (usize, usize) {
    let mut start = 0;
    let mut end = token.len();
    while let Some(ch) = token[start..end].chars().next() {
        if matches!(ch, '(' | '<' | '"' | '\'' | '[') {
            start += ch.len_utf8();
        } else {
            break;
        }
    }
    while let Some(ch) = token[start..end].chars().next_back() {
        let strip_paren = ch == ')'
            && token[start..end].matches(')').count() > token[start..end].matches('(').count();
        if strip_paren
            || matches!(
                ch,
                '>' | ',' | '.' | '"' | '\'' | '!' | '?' | ';' | ':' | ']'
            )
        {
            end -= ch.len_utf8();
        } else {
            break;
        }
    }
    (start, end)
}

pub fn fetch(url: &Url, cache: &Cache) -> Preview {
    let file = cache.link_path(url.as_str());
    if let Ok(preview) = read_json(&file) {
        return preview;
    }
    let fallback = || Preview {
        title: url.host_str().unwrap_or("Link").into(),
        description: None,
    };
    let Some(html) = download(url.clone()) else {
        return fallback();
    };
    let document = Html::parse_document(&html);
    let meta = Selector::parse("meta").unwrap();
    let title = Selector::parse("title").unwrap();
    let property = |names: &[&str]| {
        document.select(&meta).find_map(|element| {
            let element = element.value();
            let name = element.attr("property").or_else(|| element.attr("name"))?;
            names
                .contains(&name)
                .then(|| element.attr("content"))
                .flatten()
                .map(simplify)
                .filter(|value| !value.is_empty())
        })
    };
    let preview = Preview {
        title: property(&["og:title", "twitter:title"])
            .or_else(|| {
                document
                    .select(&title)
                    .next()
                    .map(|title| simplify(&title.text().collect::<String>()))
                    .filter(|value| !value.is_empty())
            })
            .unwrap_or_else(|| fallback().title),
        description: property(&["og:description", "description", "twitter:description"]),
    };
    let _ = write_json(&file, &preview);
    preview
}

fn download(mut url: Url) -> Option<String> {
    let mut visited = HashSet::new();
    for _ in 0..4 {
        if !valid_url(&url) || !visited.insert(url.clone()) {
            return None;
        }
        let host = url.host_str()?;
        let port = url.port_or_known_default()?;
        if !matches!(port, 80 | 443) {
            return None;
        }
        let addresses = (host, port).to_socket_addrs().ok()?.collect::<Vec<_>>();
        if addresses.is_empty() || addresses.iter().any(|address| !public(address.ip())) {
            return None;
        }
        let client = Client::builder()
            .timeout(Duration::from_secs(4))
            .connect_timeout(Duration::from_secs(2))
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .resolve_to_addrs(host, &addresses)
            .user_agent("BlueBubbles Native link preview")
            .build()
            .ok()?;
        let response = client.get(url.clone()).send().ok()?;
        if response.status().is_redirection() {
            url = url
                .join(
                    response
                        .headers()
                        .get(reqwest::header::LOCATION)?
                        .to_str()
                        .ok()?,
                )
                .ok()?;
            continue;
        }
        if !response.status().is_success()
            || !response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)?
                .to_str()
                .ok()?
                .starts_with("text/html")
        {
            return None;
        }
        let mut bytes = Vec::new();
        response.take(256 * 1024).read_to_end(&mut bytes).ok()?;
        return Some(String::from_utf8_lossy(&bytes).into_owned());
    }
    None
}

fn valid_url(url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https")
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
}

fn public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            !ip.is_private()
                && !ip.is_loopback()
                && !ip.is_link_local()
                && !ip.is_documentation()
                && a != 0
                && a < 224
                && !(a == 100 && (64..=127).contains(&b))
                && !(a == 192 && b == 0 && c == 0)
                && !(a == 198 && matches!(b, 18 | 19))
        }
        IpAddr::V6(ip) => {
            ip.segments()[0] & 0xe000 == 0x2000
                && !(ip.segments()[0] == 0x2001 && ip.segments()[1] == 0x0db8)
        }
    }
}

fn simplify(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(240)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    #[test]
    fn extracts_first_valid_url_and_strips_punctuation() {
        let url = first("Check out (https://example.com/docs?a=1), thanks!").unwrap();
        assert_eq!(url.as_str(), "https://example.com/docs?a=1");
        assert!(first("file:///etc/passwd").is_none());
        assert!(first("https://user:pass@example.com").is_none());
    }

    #[test]
    fn format_markup_links_urls_and_escapes_surrounding_text() {
        assert_eq!(format_markup("No links here <3 & >1"), None);
        let formatted = format_markup(
            "Apple Store: For more info: https://s.apple.com/cX9D0d9zZ5\nAnd (https://example.com/a?x=1&y=2) <ok>.",
        )
        .unwrap();
        assert_eq!(
            formatted,
            "Apple Store: For more info: <a href=\"https://s.apple.com/cX9D0d9zZ5\">https://s.apple.com/cX9D0d9zZ5</a>\nAnd (<a href=\"https://example.com/a?x=1&amp;y=2\">https://example.com/a?x=1&amp;y=2</a>) &lt;ok&gt;."
        );
        let wiki = format_markup("See https://en.wikipedia.org/wiki/Rust_(lang).").unwrap();
        assert!(wiki.contains("href=\"https://en.wikipedia.org/wiki/Rust_(lang)\""));
        assert!(wiki.ends_with("</a>."));
    }

    #[test]
    fn public_ip_guard_blocks_internal_ranges() {
        for blocked in [
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
            IpAddr::V4(Ipv4Addr::new(172, 16, 0, 1)),
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)),
            IpAddr::V4(Ipv4Addr::new(100, 64, 0, 1)),
            IpAddr::V4(Ipv4Addr::new(169, 254, 169, 254)),
            IpAddr::V4(Ipv4Addr::new(224, 0, 0, 1)),
            IpAddr::V6(Ipv6Addr::LOCALHOST),
            IpAddr::V6("fc00::1".parse().unwrap()),
            IpAddr::V6("fe80::1".parse().unwrap()),
            IpAddr::V6("2001:db8::1".parse().unwrap()),
        ] {
            assert!(!public(blocked), "expected {blocked} to be blocked");
        }
        assert!(public(IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34))));
        assert!(public(IpAddr::V6("2606:2800:220:1::1".parse().unwrap())));
    }
}
