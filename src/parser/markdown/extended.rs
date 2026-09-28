//! Telegram rich-text entities that have no standard Markdown syntax:
//! highlighted ("marked") text, superscript, subscript, date-time and custom
//! emoji (Bot API 10.1 `RichText`).
//!
//! Models write them as HTML (`<mark>`, `<sup>`, `<sub>`,
//! `<time datetime="…">`, `<tg-time>`, `<tg-emoji>`), as `==text==`, or with
//! Telegram's own MarkdownV2 links (`![22:45](tg://time?unix=…&format=wDT)`,
//! `![👍](tg://emoji?id=…)`). Telegram gets real entities; plain-text
//! channels (WhatsApp, the terminal) get a readable flattening instead.

use std::sync::LazyLock;

use regex::{Captures, Regex};
use serde_json::{json, Value};

const CODE_OPEN: char = '\u{E000}';
const CODE_CLOSE: char = '\u{E001}';
const MARK_OPEN: char = '\u{E002}';
const MARK_CLOSE: char = '\u{E003}';
const SUP_OPEN: char = '\u{E004}';
const SUP_CLOSE: char = '\u{E005}';
const SUB_OPEN: char = '\u{E006}';
const SUB_CLOSE: char = '\u{E007}';

/// Private-use markers standing in for converted HTML spans while inline
/// text is tokenized: (opening marker, closing marker, rich-text type).
pub(super) const SPAN_MARKERS: [(char, char, &str); 3] = [
    (MARK_OPEN, MARK_CLOSE, "marked"),
    (SUP_OPEN, SUP_CLOSE, "superscript"),
    (SUB_OPEN, SUB_CLOSE, "subscript"),
];
/// The opening markers as token delimiters for plain-text chunking.
pub(super) const SPAN_MARKER_DELIMITERS: [&str; 3] = ["\u{E002}", "\u{E004}", "\u{E006}"];

pub(super) fn is_marker(ch: char) -> bool {
    ('\u{E000}'..='\u{E007}').contains(&ch)
}

static RE_INLINE_CODE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"`[^`]*`").expect("valid static regex"));
static RE_CODE_PLACEHOLDER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("\u{E000}([0-9]+)\u{E001}").expect("valid static regex"));
/// Fenced blocks or inline code, left alone when flattening plain text.
static RE_CODE_SEGMENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)```.*?```|`[^`\n]+`").expect("valid static regex"));
static RE_MARK_TAG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?is)<mark(?:\s+[^>]*)?>(.*?)</mark>").expect("valid static regex")
});
static RE_SUP_TAG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?is)<sup(?:\s+[^>]*)?>(.*?)</sup>").expect("valid static regex")
});
static RE_SUB_TAG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?is)<sub(?:\s+[^>]*)?>(.*?)</sub>").expect("valid static regex")
});
static RE_TIME_TAG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?is)<(time|tg-time)\b([^>]*)>(.*?)</(?:time|tg-time)>")
        .expect("valid static regex")
});
static RE_EMOJI_TAG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?is)<tg-emoji\b([^>]*)>(.*?)</tg-emoji>").expect("valid static regex")
});
static RE_ATTRIBUTE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"([A-Za-z][A-Za-z0-9_-]*)\s*=\s*["']([^"']*)["']"#).expect("valid static regex")
});
/// `==text==` highlight; the text may not start or end with a space, so
/// comparisons such as `a == b == c` stay literal.
static RE_EQ_MARK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"==([^\s=](?:[^=\n]*?[^\s=])?)==").expect("valid static regex"));
static RE_TG_LINK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"!?\[([^\]]*)\]\(tg://(?:time|emoji)\?[^)\s]*\)").expect("valid static regex")
});
/// A link to a section of the same message: `[text](#section)`.
static RE_SECTION_LINK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[([^\]]+)\]\(\s*#[^)\s]*\s*\)").expect("valid static regex"));
static RE_FOOTNOTE_DEFINITION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^([ \t]*)\[\^([^\]\s]+)\]:[ \t]*").expect("valid static regex")
});
static RE_FOOTNOTE_MARKER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[\^([^\]\s]+)\]").expect("valid static regex"));
/// A `](tg://time?…` or `](tg://emoji?…` link target inside image syntax.
static RE_ENTITY_LINK_TARGET: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\]\s*\(\s*<?tg://(?:time|emoji)\?").expect("valid static regex"));
/// Date-time entity formatting (Bot API): `r|w?[dD]?[tT]?`.
static RE_TIME_FORMAT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:r|w?[dD]?[tT]?)$").expect("valid static regex"));

/// Whether image-style markup (`![…](…)`) is a date-time or custom emoji
/// entity, which is inline text rather than a media block.
pub(super) fn is_entity_link_markup(markup: &str) -> bool {
    RE_ENTITY_LINK_TARGET.is_match(markup)
}

/// Replaces inline code spans with numbered placeholders, so tag conversion
/// cannot touch code: `<sup>` inside backticks stays literal.
pub(super) fn protect_inline_code(text: &str) -> (String, Vec<String>) {
    let mut spans = Vec::new();
    let protected = RE_INLINE_CODE.replace_all(text, |caps: &Captures| {
        spans.push(caps[0].to_string());
        format!("{CODE_OPEN}{}{CODE_CLOSE}", spans.len() - 1)
    });
    (protected.into_owned(), spans)
}

pub(super) fn restore_inline_code(text: &str, spans: &[String]) -> String {
    if spans.is_empty() {
        return text.to_string();
    }
    RE_CODE_PLACEHOLDER
        .replace_all(text, |caps: &Captures| {
            caps[1]
                .parse::<usize>()
                .ok()
                .and_then(|index| spans.get(index))
                .cloned()
                .unwrap_or_default()
        })
        .into_owned()
}

fn attributes(raw: &str) -> Vec<(String, String)> {
    RE_ATTRIBUTE
        .captures_iter(raw)
        .map(|caps| (caps[1].to_ascii_lowercase(), caps[2].trim().to_string()))
        .collect()
}

fn attribute<'a>(attributes: &'a [(String, String)], names: &[&str]) -> Option<&'a str> {
    attributes
        .iter()
        .find(|(name, _)| names.contains(&name.as_str()))
        .map(|(_, value)| value.as_str())
}

/// Unix time of an ISO 8601 date-time. A time zone is required: without one
/// the moment is ambiguous, and a wrong time is worse than plain text.
pub(super) fn parse_datetime(value: &str) -> Option<i64> {
    let value = value.trim();
    let value = match value.strip_suffix(['Z', 'z']) {
        Some(utc) => format!("{utc}+00:00"),
        None => value.to_string(),
    };
    if let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(&value) {
        return Some(parsed.timestamp());
    }
    [
        "%Y-%m-%dT%H:%M%:z",
        "%Y-%m-%d %H:%M:%S%:z",
        "%Y-%m-%d %H:%M%:z",
    ]
    .iter()
    .find_map(|format| chrono::DateTime::parse_from_str(&value, format).ok())
    .map(|parsed| parsed.timestamp())
}

fn time_link(label: &str, unix: i64, format: &str) -> String {
    format!("![{label}](tg://time?unix={unix}&format={format})")
}

/// HTML forms of the extended entities become span markers (marked,
/// superscript, subscript) or `tg://` links (date-time, custom emoji). Tags
/// that cannot be converted safely keep only their text.
pub(super) fn convert_extended_tags(text: &str) -> String {
    if !text.contains('<') {
        return text.to_string();
    }
    let text = RE_MARK_TAG.replace_all(text, format!("{MARK_OPEN}${{1}}{MARK_CLOSE}"));
    let text = RE_SUP_TAG.replace_all(&text, format!("{SUP_OPEN}${{1}}{SUP_CLOSE}"));
    let text = RE_SUB_TAG.replace_all(&text, format!("{SUB_OPEN}${{1}}{SUB_CLOSE}"));
    let text = RE_TIME_TAG.replace_all(&text, |caps: &Captures| {
        let attrs = attributes(&caps[2]);
        let label = &caps[3];
        let unix = if caps[1].eq_ignore_ascii_case("tg-time") {
            attribute(&attrs, &["unix"]).and_then(|unix| unix.parse::<i64>().ok())
        } else {
            attribute(&attrs, &["datetime"]).and_then(parse_datetime)
        };
        let format = attribute(&attrs, &["format", "data-format"]).unwrap_or("");
        match unix {
            Some(unix) if RE_TIME_FORMAT.is_match(format) => time_link(label, unix, format),
            _ => label.to_string(),
        }
    });
    let text = RE_EMOJI_TAG.replace_all(&text, |caps: &Captures| {
        let attrs = attributes(&caps[1]);
        let alternative = &caps[2];
        match attribute(&attrs, &["emoji-id", "id"]) {
            Some(id) if is_custom_emoji_id(id) => format!("![{alternative}](tg://emoji?id={id})"),
            _ => alternative.to_string(),
        }
    });
    text.into_owned()
}

fn is_custom_emoji_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 32 && id.bytes().all(|byte| byte.is_ascii_digit())
}

/// `==text==` at the start of `rest`: the highlighted text and what follows.
pub(super) fn take_eq_mark(rest: &str) -> Option<(&str, &str)> {
    let found = RE_EQ_MARK.captures(rest)?;
    let whole = found.get(0)?;
    if whole.start() != 0 {
        return None;
    }
    Some((found.get(1)?.as_str(), &rest[whole.end()..]))
}

/// A marker span at the start of `rest`: its rich-text type, inner text and
/// what follows. An opening marker without its closing one is dropped.
pub(super) fn take_marker_span(rest: &str) -> Option<(Option<&'static str>, &str, &str)> {
    let first = rest.chars().next()?;
    let (_, close, kind) = SPAN_MARKERS.iter().find(|(open, _, _)| *open == first)?;
    let after = &rest[first.len_utf8()..];
    Some(match after.find(*close) {
        Some(end) => (Some(*kind), &after[..end], &after[end + close.len_utf8()..]),
        None => (None, "", after),
    })
}

/// The rich-text entity for a `tg://time` or `tg://emoji` link, or `None`
/// for any other URL. An invalid link keeps only its (already parsed) label.
pub(super) fn tg_link_entity(url: &str, label_text: &str, label: Value) -> Option<Value> {
    let (kind, query) = url.strip_prefix("tg://")?.split_once('?')?;
    let param = |key: &str| {
        query
            .split('&')
            .filter_map(|pair| pair.split_once('='))
            .find(|(name, _)| *name == key)
            .map(|(_, value)| value.trim())
    };
    match kind {
        "time" => {
            let unix = param("unix").and_then(|unix| unix.parse::<i64>().ok());
            let format = param("format").unwrap_or("");
            Some(match unix {
                Some(unix) if RE_TIME_FORMAT.is_match(format) => json!({
                    "type": "date_time",
                    "text": label,
                    "unix_time": unix,
                    "date_time_format": format,
                }),
                _ => label,
            })
        }
        "emoji" => {
            let alternative = label_text.trim();
            Some(match param("id") {
                Some(id) if is_custom_emoji_id(id) && !alternative.is_empty() => json!({
                    "type": "custom_emoji",
                    "custom_emoji_id": id,
                    "alternative_text": alternative,
                }),
                _ => label,
            })
        }
        _ => None,
    }
}

fn superscript_char(ch: char) -> Option<char> {
    Some(match ch {
        '0' => '⁰',
        '1' => '¹',
        '2' => '²',
        '3' => '³',
        '4' => '⁴',
        '5' => '⁵',
        '6' => '⁶',
        '7' => '⁷',
        '8' => '⁸',
        '9' => '⁹',
        '+' => '⁺',
        '-' => '⁻',
        '=' => '⁼',
        '(' => '⁽',
        ')' => '⁾',
        'n' => 'ⁿ',
        'i' => 'ⁱ',
        _ => return None,
    })
}

fn subscript_char(ch: char) -> Option<char> {
    Some(match ch {
        '0' => '₀',
        '1' => '₁',
        '2' => '₂',
        '3' => '₃',
        '4' => '₄',
        '5' => '₅',
        '6' => '₆',
        '7' => '₇',
        '8' => '₈',
        '9' => '₉',
        '+' => '₊',
        '-' => '₋',
        '=' => '₌',
        '(' => '₍',
        ')' => '₎',
        _ => return None,
    })
}

/// Unicode super/subscript when every character has one (`x²`, `H₂O`),
/// otherwise `^(…)` / `_(…)`.
fn scripted(text: &str, map: fn(char) -> Option<char>, sign: char) -> String {
    text.chars()
        .map(map)
        .collect::<Option<String>>()
        .filter(|mapped| !mapped.is_empty())
        .unwrap_or_else(|| {
            if text.chars().count() == 1 {
                format!("{sign}{text}")
            } else {
                format!("{sign}({text})")
            }
        })
}

/// Readable form of the extended entities for channels without them:
/// highlight becomes bold, super/subscript become Unicode where possible,
/// date-times and custom emoji keep their visible text.
pub fn flatten_extended_inline(text: &str) -> String {
    let text = RE_MARK_TAG.replace_all(text, "**$1**");
    let text = RE_EQ_MARK.replace_all(&text, "**$1**");
    let text = RE_SUP_TAG.replace_all(&text, |caps: &Captures| {
        scripted(&caps[1], superscript_char, '^')
    });
    let text = RE_SUB_TAG.replace_all(&text, |caps: &Captures| {
        scripted(&caps[1], subscript_char, '_')
    });
    let text = RE_TIME_TAG.replace_all(&text, "$3");
    let text = RE_EMOJI_TAG.replace_all(&text, "$2");
    let text = RE_TG_LINK.replace_all(&text, "$1");
    // In-message navigation cannot jump anywhere in plain text: section
    // links keep their text and footnotes read as `[1]`.
    let text = RE_SECTION_LINK.replace_all(&text, "$1");
    let text = RE_FOOTNOTE_DEFINITION.replace_all(&text, "$1[$2] ");
    RE_FOOTNOTE_MARKER.replace_all(&text, "[$1]").into_owned()
}

/// [`flatten_extended_inline`] outside fenced and inline code.
pub fn flatten_extended_inline_outside_code(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for code in RE_CODE_SEGMENT.find_iter(text) {
        out.push_str(&flatten_extended_inline(&text[last..code.start()]));
        out.push_str(code.as_str());
        last = code.end();
    }
    out.push_str(&flatten_extended_inline(&text[last..]));
    out
}
