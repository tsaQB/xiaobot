//! Navigation inside one rich message (Bot API 10.1): links to its sections
//! (`[DNS](#dns)`, rendered as `anchor_link` to an `anchor` block placed
//! before the heading) and footnotes (`fact[^1]` with a `[^1]: note` line,
//! rendered as `reference_link` and `reference`).
//!
//! The parser only emits placeholders for these. [`resolve`] runs once on the
//! finished blocks, so it sees exactly the headings and notes that became
//! blocks: a link whose section is not there keeps only its text, and a
//! `[^…]` without a matching note (a regex such as `[^0-9]`, say) stays
//! literal text.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{json, Map, Value};

use crate::bot::models::{RichBlock, RichBlockCaption};

/// Placeholder rich-text types; [`resolve`] replaces every one of them.
const SECTION_LINK: &str = "xiao_section_link";
const FOOTNOTE_MARKER: &str = "xiao_footnote_marker";
const FOOTNOTE_NOTE: &str = "xiao_footnote_note";

/// A footnote definition line: `[^id]: note`.
pub(super) static RE_FOOTNOTE_DEFINITION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\[\^([^\]\s]+)\]:\s*(.*)$").expect("valid static regex"));
/// Leading enumeration of a heading ("2.", "2.1", "IV.") so `#dns` also
/// finds "2. DNS". Roman numerals need punctuation, so "MVC Pattern" keeps
/// its first word.
static RE_HEADING_NUMBER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(?:[0-9]+(?:\.[0-9]+)*[.):]?|[IVXLCDM]+[.):])\s+").expect("valid static regex")
});

/// Placeholder for `[label](#target)`.
pub(super) fn section_link(label: Value, target: &str) -> Value {
    json!({"type": SECTION_LINK, "text": label, "target": target})
}

/// Placeholder for a footnote marker `[^id]`.
pub(super) fn footnote_marker(id: &str) -> Value {
    json!({"type": FOOTNOTE_MARKER, "id": id})
}

/// A footnote definition line: its id and (possibly empty) note text.
pub(super) fn footnote_definition(line: &str) -> Option<(&str, &str)> {
    let caps = RE_FOOTNOTE_DEFINITION.captures(line)?;
    Some((caps.get(1)?.as_str(), caps.get(2)?.as_str().trim()))
}

/// Placeholder block for a footnote note.
pub(super) fn footnote_note_block(id: &str, note: Value) -> RichBlock {
    RichBlock::Paragraph {
        text: json!({"type": FOOTNOTE_NOTE, "id": id, "text": note}),
    }
}

/// Loose key used to match a link target to a heading: lowercase letters
/// and digits only, so `#2-dns`, `#2.-dns` and "2. DNS" all agree.
fn key(text: &str) -> String {
    text.chars()
        .filter(|ch| ch.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Visible text of rich text, for matching headings.
fn plain_text(value: &Value, out: &mut String) {
    match value {
        Value::String(text) => out.push_str(text),
        Value::Array(parts) => parts.iter().for_each(|part| plain_text(part, out)),
        Value::Object(object) => {
            if let Some(text) = object.get("text") {
                plain_text(text, out);
            }
        }
        _ => {}
    }
}

/// Calls `visit` on every rich-text value of a block, in reading order.
fn for_each_text(block: &mut RichBlock, visit: &mut impl FnMut(&mut Value)) {
    fn caption(caption: &mut Option<RichBlockCaption>, visit: &mut impl FnMut(&mut Value)) {
        if let Some(caption) = caption {
            visit(&mut caption.text);
            if let Some(credit) = caption.credit.as_mut() {
                visit(credit);
            }
        }
    }
    match block {
        RichBlock::Paragraph { text }
        | RichBlock::SectionHeading { text, .. }
        | RichBlock::Footer { text }
        | RichBlock::Thinking { text } => visit(text),
        RichBlock::List { items } => items
            .iter_mut()
            .flat_map(|item| item.blocks.iter_mut())
            .for_each(&mut *visit),
        RichBlock::BlockQuotation { blocks } => blocks.iter_mut().for_each(&mut *visit),
        RichBlock::ExpandableBlockQuotation { text, credit }
        | RichBlock::PullQuotation { text, credit } => {
            visit(text);
            if let Some(credit) = credit.as_mut() {
                visit(credit);
            }
        }
        RichBlock::Table { cells, .. } => cells
            .iter_mut()
            .flatten()
            .for_each(|cell| visit(&mut cell.text)),
        RichBlock::Details {
            summary, blocks, ..
        } => {
            visit(summary);
            blocks.iter_mut().for_each(&mut *visit);
        }
        RichBlock::Document { caption: c, .. }
        | RichBlock::Photo { caption: c, .. }
        | RichBlock::Video { caption: c, .. }
        | RichBlock::Audio { caption: c, .. }
        | RichBlock::VoiceNote { caption: c, .. }
        | RichBlock::Animation { caption: c, .. }
        | RichBlock::Collage { caption: c, .. }
        | RichBlock::Slideshow { caption: c, .. } => caption(c, visit),
        RichBlock::Preformatted { .. }
        | RichBlock::Divider {}
        | RichBlock::MathematicalExpression { .. }
        | RichBlock::Buttons { .. }
        | RichBlock::Map { .. }
        | RichBlock::Anchor { .. } => {}
    }
}

fn placeholder_kind(object: &Map<String, Value>) -> Option<&str> {
    object
        .get("type")
        .and_then(Value::as_str)
        .filter(|kind| [SECTION_LINK, FOOTNOTE_MARKER, FOOTNOTE_NOTE].contains(kind))
}

fn field<'a>(object: &'a Map<String, Value>, name: &str) -> &'a str {
    object.get(name).and_then(Value::as_str).unwrap_or("")
}

/// What the placeholders of a message refer to, in reading order.
#[derive(Default)]
struct Scan {
    targets: Vec<String>,
    markers: Vec<String>,
    notes: Vec<String>,
}

impl Scan {
    fn collect(&mut self, value: &Value) {
        match value {
            Value::Array(parts) => parts.iter().for_each(|part| self.collect(part)),
            Value::Object(object) => {
                match placeholder_kind(object) {
                    Some(SECTION_LINK) => self.targets.push(key(field(object, "target"))),
                    Some(FOOTNOTE_MARKER) => self.markers.push(field(object, "id").to_string()),
                    Some(FOOTNOTE_NOTE) => self.notes.push(field(object, "id").to_string()),
                    _ => {}
                }
                object.values().for_each(|inner| self.collect(inner));
            }
            _ => {}
        }
    }
}

/// How each placeholder is replaced.
struct Resolution {
    /// Link target key → anchor name (`""` is the top of the message).
    anchors: HashMap<String, String>,
    /// Footnote id → number, only for footnotes that have a note.
    footnotes: HashMap<String, usize>,
    /// Notes already placed; a repeated definition stays plain text.
    placed_notes: HashSet<String>,
}

/// Replacement for one value: a placeholder may become several parts.
enum Resolved {
    One(Value),
    Many(Vec<Value>),
}

impl Resolved {
    fn into_value(self) -> Value {
        match self {
            Resolved::One(value) => value,
            Resolved::Many(parts) => Value::Array(parts),
        }
    }
}

fn spread(value: Value) -> Resolved {
    match value {
        Value::Array(parts) => Resolved::Many(parts),
        other => Resolved::One(other),
    }
}

impl Resolution {
    fn resolve(&mut self, value: Value) -> Resolved {
        match value {
            Value::Array(parts) => {
                let mut out: Vec<Value> = Vec::with_capacity(parts.len());
                for part in parts {
                    let resolved = match self.resolve(part) {
                        Resolved::One(value) => vec![value],
                        Resolved::Many(values) => values,
                    };
                    for value in resolved {
                        // Adjacent strings are merged, as the parser does.
                        match (out.last_mut(), value) {
                            (Some(Value::String(previous)), Value::String(text)) => {
                                previous.push_str(&text)
                            }
                            (_, value) => out.push(value),
                        }
                    }
                }
                // A single part is not wrapped, as the parser does.
                Resolved::One(match out.len() {
                    0 => Value::String(String::new()),
                    1 => out.pop().unwrap_or_default(),
                    _ => Value::Array(out),
                })
            }
            Value::Object(mut object) => {
                let kind = placeholder_kind(&object).map(str::to_string);
                let text = object.remove("text").unwrap_or_default();
                let text = self.resolve(text).into_value();
                match kind.as_deref() {
                    Some(SECTION_LINK) => match self.anchors.get(&key(field(&object, "target"))) {
                        Some(name) => Resolved::One(
                            json!({"type": "anchor_link", "text": text, "anchor_name": name}),
                        ),
                        None => spread(text),
                    },
                    Some(FOOTNOTE_MARKER) => {
                        let id = field(&object, "id");
                        Resolved::One(match self.footnotes.get(id) {
                            Some(number) => json!({
                                "type": "reference_link",
                                "text": format!("[{number}]"),
                                "reference_name": format!("catatan-{number}"),
                            }),
                            None => Value::String(format!("[^{id}]")),
                        })
                    }
                    Some(FOOTNOTE_NOTE) => {
                        let id = field(&object, "id").to_string();
                        match self.footnotes.get(&id) {
                            Some(number) if self.placed_notes.insert(id.clone()) => {
                                Resolved::Many(vec![
                                    Value::String(format!("[{number}] ")),
                                    json!({
                                        "type": "reference",
                                        "name": format!("catatan-{number}"),
                                        "text": text,
                                    }),
                                ])
                            }
                            _ => {
                                let mut parts = vec![Value::String(format!("[^{id}]: "))];
                                match text {
                                    Value::Array(items) => parts.extend(items),
                                    other => parts.push(other),
                                }
                                Resolved::Many(parts)
                            }
                        }
                    }
                    _ => {
                        let mut resolved = Map::with_capacity(object.len() + 1);
                        for (name, inner) in object {
                            resolved.insert(name, self.resolve(inner).into_value());
                        }
                        if !text.is_null() {
                            resolved.insert("text".to_string(), text);
                        }
                        Resolved::One(Value::Object(resolved))
                    }
                }
            }
            other => Resolved::One(other),
        }
    }
}

/// Turns the navigation placeholders of a parsed message into Telegram rich
/// text and places an anchor before every linked heading.
pub(super) fn resolve(blocks: &mut Vec<RichBlock>) {
    let mut scan = Scan::default();
    for block in blocks.iter_mut() {
        for_each_text(block, &mut |value| scan.collect(value));
    }
    if scan.targets.is_empty() && scan.markers.is_empty() && scan.notes.is_empty() {
        return;
    }

    // Headings: an exact match wins over a match without the numbering.
    let headings: Vec<(usize, [String; 2])> = blocks
        .iter()
        .enumerate()
        .filter_map(|(index, block)| match block {
            RichBlock::SectionHeading { text, .. } => {
                let mut visible = String::new();
                plain_text(text, &mut visible);
                let short = key(&RE_HEADING_NUMBER.replace(&visible, ""));
                Some((index, [key(&visible), short]))
            }
            _ => None,
        })
        .collect();
    let mut target_heading: HashMap<String, usize> = HashMap::new();
    for target in &scan.targets {
        let exact = headings
            .iter()
            .find(|(_, keys)| !target.is_empty() && keys[0] == *target);
        let loose = || {
            headings
                .iter()
                .find(|(_, keys)| !target.is_empty() && keys[1] == *target)
        };
        if let Some((index, _)) = exact.or_else(loose) {
            target_heading.insert(target.clone(), *index);
        }
    }
    let mut linked: Vec<usize> = target_heading.values().copied().collect();
    linked.sort_unstable();
    linked.dedup();
    let anchor_names: HashMap<usize, String> = linked
        .iter()
        .enumerate()
        .map(|(n, index)| (*index, format!("bagian-{}", n + 1)))
        .collect();
    let mut anchors: HashMap<String, String> = target_heading
        .iter()
        .filter_map(|(target, index)| Some((target.clone(), anchor_names.get(index)?.clone())))
        .collect();
    for top in ["", "top", "atas", "awal"] {
        anchors.entry(top.to_string()).or_default();
    }

    // Footnotes with a note are numbered by first mention, then any note
    // that is never mentioned, in the order written.
    let defined: HashSet<&String> = scan.notes.iter().collect();
    let mut footnotes: HashMap<String, usize> = HashMap::new();
    for id in scan.markers.iter().chain(scan.notes.iter()) {
        if defined.contains(id) && !footnotes.contains_key(id) {
            let number = footnotes.len() + 1;
            footnotes.insert(id.clone(), number);
        }
    }

    let mut resolution = Resolution {
        anchors,
        footnotes,
        placed_notes: HashSet::new(),
    };
    for block in blocks.iter_mut() {
        for_each_text(block, &mut |value| {
            *value = resolution.resolve(std::mem::take(value)).into_value();
        });
    }
    for index in linked.into_iter().rev() {
        if let Some(name) = anchor_names.get(&index) {
            blocks.insert(index, RichBlock::Anchor { name: name.clone() });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paragraph(text: Value) -> RichBlock {
        RichBlock::Paragraph { text }
    }

    #[test]
    fn keys_ignore_case_punctuation_and_numbering() {
        assert_eq!(key("2. Apa itu DNS?"), "2apaitudns");
        assert_eq!(
            key(&RE_HEADING_NUMBER.replace("2. Apa itu DNS?", "")),
            "apaitudns"
        );
        assert_eq!(
            key(&RE_HEADING_NUMBER.replace("IV. Penutup", "")),
            "penutup"
        );
        assert_eq!(
            key(&RE_HEADING_NUMBER.replace("MVC Pattern", "")),
            "mvcpattern",
            "an all-caps word is not numbering"
        );
    }

    #[test]
    fn exact_heading_wins_over_a_numbered_one() {
        let mut blocks = vec![
            paragraph(section_link(json!("Pola"), "pattern")),
            RichBlock::SectionHeading {
                text: json!("1. Pattern"),
                level: 2,
            },
            RichBlock::SectionHeading {
                text: json!("Pattern"),
                level: 2,
            },
        ];
        resolve(&mut blocks);
        assert_eq!(
            blocks[2],
            RichBlock::Anchor {
                name: "bagian-1".into()
            }
        );
        assert!(matches!(&blocks[3], RichBlock::SectionHeading { text, .. } if *text == "Pattern"));
    }

    #[test]
    fn unresolved_links_flatten_into_their_parent_text() {
        let mut blocks = vec![paragraph(json!([
            "Lihat ",
            section_link(
                json!([{"type": "bold", "text": "DNS"}, " server"]),
                "hilang"
            ),
            " sekarang",
        ]))];
        resolve(&mut blocks);
        assert_eq!(
            blocks[0],
            paragraph(json!(["Lihat ", {"type": "bold", "text": "DNS"}, " server sekarang"]))
        );
    }

    #[test]
    fn tables_keep_their_header_flag() {
        let cell = |text: Value| crate::bot::models::RichBlockTableCell::new(text, false, None);
        let mut blocks = vec![RichBlock::Table {
            cells: vec![
                vec![cell(json!("A"))],
                vec![cell(section_link(json!("x"), "hilang"))],
            ],
            has_header: true,
            is_bordered: false,
            is_striped: false,
            is_compact: false,
            caption: None,
        }];
        resolve(&mut blocks);
        let RichBlock::Table {
            has_header, cells, ..
        } = &blocks[0]
        else {
            panic!("table kept");
        };
        assert!(*has_header);
        assert_eq!(cells[1][0].text, "x");
    }
}
