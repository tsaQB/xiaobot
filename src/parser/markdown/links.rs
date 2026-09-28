//! Navigation inside one rich message (Bot API 10.1): links to its sections
//! (`[DNS](#dns)`, rendered as `anchor_link` to an `anchor` block placed
//! before the heading) and footnotes (`fact[^1]` with a `[^1]: note` line,
//! rendered as `reference_link` and `reference`).
//!
//! Targets are collected from the whole text before it is parsed, so a link
//! is emitted only when its target exists in the same message; otherwise only
//! its text is kept. Anchors are placed only on headings that are linked to.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{json, Value};

use crate::bot::models::RichBlock;

static RE_MARKDOWN_HEADING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^#{1,6}\s*([^\s#].*)$").expect("valid static regex"));
static RE_HTML_HEADING_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?is)^<h[1-6](?:\s+[^>]*)?>(.*?)</h[1-6]>$").expect("valid static regex")
});
static RE_SECTION_LINK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"\]\(\s*#([^)\s]*)\s*\)|href=["']#([^"']*)["']"#).expect("valid static regex")
});
static RE_FOOTNOTE_DEFINITION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\[\^([^\]\s]+)\]:\s*(.*)$").expect("valid static regex"));
static RE_FOOTNOTE_MARKER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[\^([^\]\s]+)\](:?)").expect("valid static regex"));
/// Leading enumeration of a heading ("2.", "2.1", "IV.") so `#dns` also
/// finds "2. DNS".
static RE_HEADING_NUMBER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(?:[0-9]+(?:\.[0-9]+)*|[IVXLCDM]+)[.):]?\s+").expect("valid static regex")
});

/// Link targets that exist in the message being parsed.
#[derive(Debug, Default)]
struct LinkTargets {
    /// Heading key → anchor name, only for headings that are linked to.
    headings: HashMap<String, String>,
    /// Footnote id → (number, reference name), numbered by first mention.
    footnotes: HashMap<String, (usize, String)>,
    /// Footnote ids that have a definition.
    defined: HashSet<String>,
    /// Anchor and reference names already placed; each is placed once.
    placed: HashSet<String>,
    /// Anchor and reference names that links were emitted for.
    linked_names: HashSet<String>,
}

thread_local! {
    // The initializer is already `const`; clippy 0.1.98 still reports the
    // expanded macro, whichever form is written.
    #[allow(clippy::missing_const_for_thread_local)]
    static TARGETS: RefCell<Option<LinkTargets>> = const { RefCell::new(None) };
}

/// Clears the targets when parsing ends, even if it panics.
struct ClearTargets;

impl Drop for ClearTargets {
    fn drop(&mut self) {
        TARGETS.with(|targets| *targets.borrow_mut() = None);
    }
}

/// Runs `parse` with the link targets of `text` in scope. A nested parse
/// reuses the targets of the outer message.
pub(super) fn with_link_targets(
    text: &str,
    parse: impl FnOnce() -> Vec<RichBlock>,
) -> Vec<RichBlock> {
    if TARGETS.with(|targets| targets.borrow().is_some()) {
        return parse();
    }
    TARGETS.with(|targets| *targets.borrow_mut() = Some(collect(text)));
    let _clear = ClearTargets;
    let blocks = parse();
    let dangling: HashSet<String> = TARGETS.with(|targets| {
        targets
            .borrow()
            .as_ref()
            .map(|targets| {
                targets
                    .linked_names
                    .difference(&targets.placed)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    });
    if dangling.is_empty() {
        blocks
    } else {
        unlink_anchors(blocks, &dangling)
    }
}

/// A heading or footnote that did not end up as its own block (for example
/// inside a collapsible block) has no target; links to it keep their text.
fn unlink_anchors(blocks: Vec<RichBlock>, dangling: &HashSet<String>) -> Vec<RichBlock> {
    fn walk(value: &mut Value, dangling: &HashSet<String>) {
        match value {
            Value::Array(items) => items.iter_mut().for_each(|item| walk(item, dangling)),
            Value::Object(object) => {
                let name_field = match object.get("type").and_then(Value::as_str) {
                    Some("anchor_link") => "anchor_name",
                    Some("reference_link") => "reference_name",
                    _ => "",
                };
                let is_dangling = !name_field.is_empty()
                    && object
                        .get(name_field)
                        .and_then(Value::as_str)
                        .is_some_and(|name| dangling.contains(name));
                if is_dangling {
                    *value = object.get("text").cloned().unwrap_or_default();
                    walk(value, dangling);
                } else {
                    object.values_mut().for_each(|item| walk(item, dangling));
                }
            }
            _ => {}
        }
    }
    let Ok(mut value) = serde_json::to_value(&blocks) else {
        return blocks;
    };
    walk(&mut value, dangling);
    serde_json::from_value(value).unwrap_or(blocks)
}

/// Loose key used to match a link target to a heading: lowercase letters
/// and digits only, so `#2-dns`, `#2.-dns` and "2. DNS" all agree.
fn key(text: &str) -> String {
    text.chars()
        .filter(|ch| ch.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn heading_text(line: &str) -> Option<&str> {
    RE_MARKDOWN_HEADING
        .captures(line)
        .or_else(|| RE_HTML_HEADING_LINE.captures(line))
        .and_then(|caps| caps.get(1))
        .map(|text| text.as_str().trim())
}

/// Both keys of a heading: the full text and the text without numbering.
fn heading_keys(text: &str) -> [String; 2] {
    [key(text), key(&RE_HEADING_NUMBER.replace(text, ""))]
}

fn collect(text: &str) -> LinkTargets {
    let mut in_code = false;
    let mut headings = Vec::new();
    let mut linked = HashSet::new();
    let mut targets = LinkTargets::default();
    let mut next_footnote = 1;
    let mut number_footnote = |id: &str, targets: &mut LinkTargets| {
        if !targets.footnotes.contains_key(id) {
            let name = format!("catatan-{next_footnote}");
            targets
                .footnotes
                .insert(id.to_string(), (next_footnote, name));
            next_footnote += 1;
        }
    };

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            in_code = !in_code;
            continue;
        }
        if in_code {
            continue;
        }
        if let Some(heading) = heading_text(trimmed) {
            headings.push(heading_keys(heading));
        }
        for caps in RE_SECTION_LINK.captures_iter(trimmed) {
            if let Some(target) = caps.get(1).or_else(|| caps.get(2)) {
                linked.insert(key(target.as_str()));
            }
        }
        let definition = RE_FOOTNOTE_DEFINITION.captures(trimmed);
        for caps in RE_FOOTNOTE_MARKER.captures_iter(trimmed) {
            let is_definition_head = caps.get(0).is_some_and(|m| m.start() == 0)
                && definition.is_some()
                && caps.get(2).is_some_and(|colon| !colon.as_str().is_empty());
            if !is_definition_head {
                number_footnote(&caps[1], &mut targets);
            }
        }
        if let Some(caps) = definition {
            targets.defined.insert(caps[1].to_string());
        }
    }
    // Footnotes defined but never mentioned still get a number.
    let mut unmentioned: Vec<String> = targets
        .defined
        .iter()
        .filter(|id| !targets.footnotes.contains_key(*id))
        .cloned()
        .collect();
    unmentioned.sort();
    for id in unmentioned {
        number_footnote(&id, &mut targets);
    }

    // Each key belongs to the first heading that has it; a heading gets an
    // anchor only when a link points to one of its keys.
    let mut anchors = 0;
    for keys in headings {
        let free: Vec<&String> = keys
            .iter()
            .filter(|k| !k.is_empty() && !targets.headings.contains_key(*k))
            .collect();
        if !free.iter().any(|k| linked.contains(*k)) {
            continue;
        }
        anchors += 1;
        let name = format!("bagian-{anchors}");
        for key in free {
            targets.headings.insert(key.clone(), name.clone());
        }
    }
    targets
}

/// Anchor name to place before a heading, when some link points to it.
/// Each name is returned once, so a repeated heading gets no second anchor.
pub(super) fn heading_anchor(heading: &str) -> Option<String> {
    let [full, short] = heading_keys(heading);
    TARGETS.with(|targets| {
        let mut targets = targets.borrow_mut();
        let targets = targets.as_mut()?;
        let name = targets
            .headings
            .get(&full)
            .or_else(|| targets.headings.get(&short))
            .cloned()?;
        targets.placed.insert(name.clone()).then_some(name)
    })
}

/// Anchor name for a `#target` link: a linked heading, or `""` (the top of
/// the message) for `#`, `#top` and `#atas`. `None` when the target is not in
/// this message.
pub(super) fn section_link_target(target: &str) -> Option<String> {
    let target = key(target);
    TARGETS.with(|targets| {
        let mut targets = targets.borrow_mut();
        let targets = targets.as_mut()?;
        if let Some(name) = targets.headings.get(&target).cloned() {
            targets.linked_names.insert(name.clone());
            return Some(name);
        }
        matches!(target.as_str(), "" | "top" | "atas" | "awal").then(String::new)
    })
}

/// The rich text for a footnote marker `[^id]`: a `reference_link` showing
/// `[n]` when the footnote is defined, plain `[n]` otherwise. `None` outside
/// a message parse.
pub(super) fn footnote_marker(id: &str) -> Option<Value> {
    TARGETS.with(|targets| {
        let mut targets = targets.borrow_mut();
        let targets = targets.as_mut()?;
        let (number, name) = targets.footnotes.get(id)?.clone();
        let label = format!("[{number}]");
        if !targets.defined.contains(id) {
            return Some(Value::String(label));
        }
        targets.linked_names.insert(name.clone());
        Some(json!({"type": "reference_link", "text": label, "reference_name": name}))
    })
}

/// Whether a line defines a footnote (`[^id]: text`).
pub(super) fn is_footnote_definition(line: &str) -> bool {
    RE_FOOTNOTE_DEFINITION.is_match(line)
}

/// The block for a footnote definition line: `[n]` followed by the note as
/// the `reference` its markers link to. `None` for other lines, outside a
/// message parse, or for a repeated definition.
pub(super) fn footnote_definition_block(line: &str) -> Option<RichBlock> {
    let caps = RE_FOOTNOTE_DEFINITION.captures(line)?;
    let id = caps.get(1)?.as_str();
    let note = caps.get(2)?.as_str().trim();
    let (number, name) = TARGETS.with(|targets| {
        let mut targets = targets.borrow_mut();
        let targets = targets.as_mut()?;
        let (number, name) = targets.footnotes.get(id)?.clone();
        targets
            .placed
            .insert(name.clone())
            .then_some((number, name))
    })?;
    Some(RichBlock::Paragraph {
        text: json!([
            format!("[{number}] "),
            {"type": "reference", "name": name, "text": super::parse_inline(note)},
        ]),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_to_targets_that_never_became_blocks_keep_their_text() {
        let blocks = vec![RichBlock::Paragraph {
            text: json!([
                "Lihat ",
                {"type": "anchor_link", "text": "rincian", "anchor_name": "bagian-9"},
                " dan ",
                {"type": "anchor_link", "text": "atas", "anchor_name": ""},
                {"type": "reference_link", "text": "[1]", "reference_name": "catatan-1"},
            ]),
        }];
        let dangling: HashSet<String> = ["bagian-9".to_string(), "catatan-1".to_string()].into();
        let cleaned = unlink_anchors(blocks, &dangling);
        let RichBlock::Paragraph { text } = &cleaned[0] else {
            panic!("paragraph kept");
        };
        assert_eq!(text[1], "rincian");
        assert_eq!(text[3]["type"], "anchor_link", "top-of-message links stay");
        assert_eq!(text[4], "[1]");
    }

    #[test]
    fn keys_ignore_case_punctuation_and_numbering() {
        assert_eq!(key("2. Apa itu DNS?"), "2apaitudns");
        assert_eq!(heading_keys("2. Apa itu DNS?")[1], "apaitudns");
        assert_eq!(heading_keys("IV. Penutup")[1], "penutup");
    }
}
