//! Inline Markdown and HTML to Telegram `RichText`: emphasis, links, code
//! and math, plus highlighted (marked) text, superscript and subscript,
//! date-time entities and custom emoji (Bot API 10.1 rich text).

use std::sync::LazyLock;

use regex::Regex;
use serde_json::{json, Value};

use super::extended;
use super::{find_bold_marker, is_exponent_marker, normalize_inline_media_label};
use crate::parser::latex::sanitize_latex_for_telegram;
use crate::parser::rtl;

static RE_HTML_SPOILER_TG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?is)<tg-spoiler(?:\s+[^>]*)?>(.*?)</tg-spoiler>").expect("valid static regex")
});
static RE_HTML_SPOILER_SPAN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?is)<span\s+class=["']?(?:tg-)?spoiler["']?>(.*?)</span>"#)
        .expect("valid static regex")
});
static RE_HTML_STRIKE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?is)<(?:s|strike|del)(?:\s+[^>]*)?>(.*?)</(?:s|strike|del)>")
        .expect("valid static regex")
});
static RE_HTML_UNDERLINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?is)<(?:u|ins)(?:\s+[^>]*)?>(.*?)</(?:u|ins)>").expect("valid static regex")
});
static RE_HTML_BOLD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?is)<(?:b|strong)(?:\s+[^>]*)?>(.*?)</(?:b|strong)>").expect("valid static regex")
});
static RE_HTML_ITALIC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?is)<(?:i|em)(?:\s+[^>]*)?>(.*?)</(?:i|em)>").expect("valid static regex")
});
static RE_HTML_CODE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?is)<code(?:\s+[^>]*)?>(.*?)</code>").expect("valid static regex")
});
static RE_HTML_LINK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?is)<a\s+[^>]*href=["']([^"']+)["'][^>]*>(.*?)</a>"#)
        .expect("valid static regex")
});
static RE_HTML_BR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)<br\s*/?>").expect("valid static regex"));
static RE_HTML_LEAKED_TAGS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)</?(?:b|strong|i|em|s|strike|del|u|ins|code|pre|blockquote|a|tg-spoiler|span|p|div|mark|kbd|sup|sub|time|tg-time|tg-emoji)(?:\s+[^>]*)?>").expect("valid static regex")
});

fn try_format_standalone_logic_symbol(inner: &str) -> Option<&'static str> {
    let clean_cmd = inner.trim_end_matches(r"\ ").trim();
    match clean_cmd {
        r"\therefore" => Some("∴"),
        r"\because" => Some("∵"),
        _ => None,
    }
}

/// Maximum nesting of inline formatting spans. Real model output rarely nests
/// more than three levels; the cap keeps adversarial input from turning into
/// deep recursion or runaway work.
const MAX_INLINE_DEPTH: usize = 12;

/// Characters that a backslash turns into literal text, following CommonMark.
/// `(`, `)`, `[` and `]` are deliberately excluded because `\(` / `\[` open
/// LaTeX math that the parsers below must still see.
const INLINE_ESCAPABLE: &str = "\\*_~|`$+!#";

pub fn parse_inline(input_str: &str) -> Value {
    if input_str.is_empty() {
        return Value::String(String::new());
    }
    let unescaped = normalize_inline_html(input_str);
    parse_inline_tokens(&unescaped, 0)
}

/// Converts inline HTML to Markdown markers, strips leaked tags and decodes
/// entities exactly once. Nested spans reuse the decoded text, so escaped
/// sequences like `&amp;lt;b&amp;gt;` stay literal instead of being decoded
/// again at every nesting level.
fn normalize_inline_html(input_str: &str) -> String {
    // Inline code is shown verbatim, so tags inside backticks stay literal.
    let (protected, code_spans) = extended::protect_inline_code(input_str);
    // Normalize well-formed inline HTML tags before cleaning leaked residual HTML
    let mut normalized = extended::convert_extended_tags(&protected);
    if normalized.contains("spoiler") || normalized.contains("<tg-spoiler") {
        normalized = RE_HTML_SPOILER_TG
            .replace_all(&normalized, "||$1||")
            .into_owned();
        normalized = RE_HTML_SPOILER_SPAN
            .replace_all(&normalized, "||$1||")
            .into_owned();
    }
    if normalized.contains("<s") || normalized.contains("<strike") || normalized.contains("<del") {
        normalized = RE_HTML_STRIKE
            .replace_all(&normalized, "~~$1~~")
            .into_owned();
    }
    if normalized.contains("<u") || normalized.contains("<ins") {
        normalized = RE_HTML_UNDERLINE
            .replace_all(&normalized, "++${1}++")
            .into_owned();
    }
    if normalized.contains("<b") || normalized.contains("<strong") {
        normalized = RE_HTML_BOLD.replace_all(&normalized, "**$1**").into_owned();
    }
    if normalized.contains("<i") || normalized.contains("<em") {
        normalized = RE_HTML_ITALIC.replace_all(&normalized, "*$1*").into_owned();
    }
    if normalized.contains("<code") {
        normalized = RE_HTML_CODE.replace_all(&normalized, "`$1`").into_owned();
    }
    if normalized.contains("<a ") || normalized.contains("<a>") {
        normalized = RE_HTML_LINK
            .replace_all(&normalized, "[$2]($1)")
            .into_owned();
    }
    if normalized.contains("<br") {
        normalized = RE_HTML_BR.replace_all(&normalized, "\n").into_owned();
    }

    // Clean leaked HTML tags
    let cleaned = RE_HTML_LEAKED_TAGS.replace_all(&normalized, "");
    let restored = extended::restore_inline_code(&cleaned, &code_spans);
    html_escape::decode_html_entities(&restored).to_string()
}

/// Returns the offset of the `*` that closes a single-star italic span,
/// stepping over complete `**bold**` pairs so `*a **b** c*` stays one italic
/// span that contains bold text.
fn find_closing_single_star(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'*' {
            if bytes.get(index + 1) == Some(&b'*') {
                if let Some(close) = text[index + 2..].find("**") {
                    index += 2 + close + 2;
                    continue;
                }
            }
            return Some(index);
        }
        index += 1;
    }
    None
}

fn parse_inline_tokens(input: &str, depth: usize) -> Value {
    if input.is_empty() {
        return Value::String(String::new());
    }
    if depth >= MAX_INLINE_DEPTH {
        return Value::String(input.to_string());
    }
    let parse_inline = |text: &str| parse_inline_tokens(text, depth + 1);

    let mut out: Vec<Value> = Vec::new();
    let mut rest = input;

    while !rest.is_empty() {
        // 0. Backslash escape (`\*` renders a literal asterisk)
        if let Some(after) = rest.strip_prefix('\\') {
            if let Some(ch) = after
                .chars()
                .next()
                .filter(|ch| INLINE_ESCAPABLE.contains(*ch))
            {
                out.push(Value::String(ch.to_string()));
                rest = &after[ch.len_utf8()..];
                continue;
            }
        }

        // 0a. Highlight, superscript and subscript converted from HTML
        if let Some((kind, inner, after)) = extended::take_marker_span(rest) {
            if let Some(kind) = kind {
                out.push(json!({"type": kind, "text": parse_inline(inner)}));
            }
            rest = after;
            continue;
        }

        // 0b. Highlight ==text==
        if rest.starts_with("==") {
            if let Some((inner, after)) = extended::take_eq_mark(rest) {
                out.push(json!({"type": "marked", "text": parse_inline(inner)}));
                rest = after;
                continue;
            }
        }

        // 1. Bold **text** (but not the exponent operator in `a**2`)
        let offset = input.len() - rest.len();
        if rest.starts_with("**") && !is_exponent_marker(input, offset) {
            if let Some(close) = find_bold_marker(input, offset + 2) {
                let end = close - offset - 2;
                let inner = &rest[2..2 + end];
                out.push(json!({
                    "type": "bold",
                    "text": parse_inline(inner)
                }));
                rest = &rest[2 + end + 2..];
                continue;
            }
        }

        // 2c. Underline <u>text</u> (++text++)
        if rest.starts_with("++") {
            if let Some(end) = rest[2..].find("++") {
                let inner = &rest[2..2 + end];
                out.push(json!({
                    "type": "underline",
                    "text": parse_inline(inner)
                }));
                rest = &rest[2 + end + 2..];
                continue;
            }
        }

        // 2. Bold __text__
        if rest.starts_with("__") {
            if let Some(end) = rest[2..].find("__") {
                let inner = &rest[2..2 + end];
                out.push(json!({
                    "type": "bold",
                    "text": parse_inline(inner)
                }));
                rest = &rest[2 + end + 2..];
                continue;
            }
        }

        // 2a. Spoiler ||text||
        if rest.starts_with("||") {
            if let Some(end) = rest[2..].find("||") {
                let inner = &rest[2..2 + end];
                out.push(json!({
                    "type": "spoiler",
                    "text": parse_inline(inner)
                }));
                rest = &rest[2 + end + 2..];
                continue;
            }
        }

        // 2b. Strikethrough ~~text~~
        if rest.starts_with("~~") {
            if let Some(end) = rest[2..].find("~~") {
                let inner = &rest[2..2 + end];
                out.push(json!({
                    "type": "strikethrough",
                    "text": parse_inline(inner)
                }));
                rest = &rest[2 + end + 2..];
                continue;
            }
        }

        // 3. Inline code `code`
        if rest.starts_with('`') {
            if let Some(end) = rest[1..].find('`') {
                let inner = &rest[1..1 + end];
                out.push(json!({
                    "type": "code",
                    "text": inner
                }));
                rest = &rest[1 + end + 1..];
                continue;
            }
        }

        // 4. Italic *text*
        if rest.starts_with('*') && !rest.starts_with("**") {
            if let Some(end) = find_closing_single_star(&rest[1..]) {
                if end > 0 && !rest[1..].starts_with('*') {
                    let inner = &rest[1..1 + end];
                    out.push(json!({
                        "type": "italic",
                        "text": parse_inline(inner)
                    }));
                    rest = &rest[1 + end + 1..];
                    continue;
                }
            }
        }

        // 5. Italic _text_
        if rest.starts_with('_') && !rest.starts_with("__") {
            if let Some(end) = rest[1..].find('_') {
                if end > 0 {
                    let inner = &rest[1..1 + end];
                    out.push(json!({
                        "type": "italic",
                        "text": parse_inline(inner)
                    }));
                    rest = &rest[1 + end + 1..];
                    continue;
                }
            }
        }

        // 6a. Markdown image ![alt](url)
        if rest.starts_with("![") {
            if let Some(close) = rest.find("](") {
                if let Some(end) = rest[close + 2..].find(')') {
                    let raw_url = &rest[close + 2..close + 2 + end];
                    let url = raw_url
                        .trim()
                        .trim_start_matches('<')
                        .trim_end_matches('>')
                        .trim();
                    let label = &rest[2..close];
                    if let Some(entity) = extended::tg_link_entity(url, label, parse_inline(label))
                    {
                        out.push(entity);
                        rest = &rest[close + 3 + end..];
                        continue;
                    }
                    if url.starts_with("https://")
                        || url.starts_with("http://")
                        || url.starts_with("tg://")
                    {
                        let alt = label;
                        let alt_trimmed = alt.trim();
                        let display_label = if alt_trimmed.is_empty() {
                            "📷 Foto".to_string()
                        } else {
                            format!("📷 {alt_trimmed}")
                        };
                        out.push(json!({
                            "type": "url",
                            "text": parse_inline(&display_label),
                            "url": url
                        }));
                        rest = &rest[close + 3 + end..];
                        continue;
                    }
                }
            }
        }

        // 6b. Links [text](url)
        if rest.starts_with('[') {
            if let Some(close) = rest.find("](") {
                if let Some(end) = rest[close + 2..].find(')') {
                    let raw_url = &rest[close + 2..close + 2 + end];
                    let url = raw_url
                        .trim()
                        .trim_start_matches('<')
                        .trim_end_matches('>')
                        .trim();
                    let inner = &rest[1..close];
                    if let Some(entity) = extended::tg_link_entity(url, inner, parse_inline(inner))
                    {
                        out.push(entity);
                        rest = &rest[close + 3 + end..];
                        continue;
                    }
                    if url.starts_with("https://")
                        || url.starts_with("http://")
                        || url.starts_with("tg://")
                    {
                        let display_label = normalize_inline_media_label(inner);
                        out.push(json!({
                            "type": "url",
                            "text": parse_inline(&display_label),
                            "url": url
                        }));
                        rest = &rest[close + 3 + end..];
                        continue;
                    }
                }
            }
        }

        // 7. Inline math $...$
        if rest.starts_with('$') && !rest.starts_with("$$") {
            if let Some(end) = rest[1..].find('$') {
                if end > 0 && !rest[1..].starts_with('$') {
                    let inner = rest[1..1 + end].trim();
                    if !inner.is_empty() {
                        if let Some(sym) = try_format_standalone_logic_symbol(inner) {
                            out.push(Value::String(sym.to_string()));
                            rest = &rest[1 + end + 1..];
                            continue;
                        }
                        if rtl::has_rtl_characters(inner) {
                            let clean_text = rtl::extract_text_from_pseudo_math(inner);
                            let parsed = parse_inline(&clean_text);
                            match parsed {
                                Value::Array(arr) => out.extend(arr),
                                other => out.push(other),
                            }
                            rest = &rest[1 + end + 1..];
                            continue;
                        }
                        let sanitized = sanitize_latex_for_telegram(inner);
                        out.push(json!({
                            "type": "mathematical_expression",
                            "expression": sanitized
                        }));
                        rest = &rest[1 + end + 1..];
                        continue;
                    }
                }
            }
        }

        // 8. Inline math \( ... \)
        if rest.starts_with(r"\(") {
            if let Some(end) = rest[2..].find(r"\)") {
                let inner = rest[2..2 + end].trim();
                if !inner.is_empty() {
                    if let Some(sym) = try_format_standalone_logic_symbol(inner) {
                        out.push(Value::String(sym.to_string()));
                        rest = &rest[2 + end + 2..];
                        continue;
                    }
                    if rtl::has_rtl_characters(inner) {
                        let clean_text = rtl::extract_text_from_pseudo_math(inner);
                        let parsed = parse_inline(&clean_text);
                        match parsed {
                            Value::Array(arr) => out.extend(arr),
                            other => out.push(other),
                        }
                        rest = &rest[2 + end + 2..];
                        continue;
                    }
                    let sanitized = sanitize_latex_for_telegram(inner);
                    out.push(json!({
                        "type": "mathematical_expression",
                        "expression": sanitized
                    }));
                    rest = &rest[2 + end + 2..];
                    continue;
                }
            }
        }

        // 9. Plain text chunk until next token
        let mut next_pos = rest.len();
        for delim in [
            "**", "__", "||", "~~", "++", "`", "*", "_", "![", "[", "$", r"\(", r"\", "==",
        ]
        .iter()
        .chain(extended::SPAN_MARKER_DELIMITERS.iter())
        {
            if let Some(idx) = rest.find(delim) {
                if idx > 0 && idx < next_pos {
                    next_pos = idx;
                }
            }
        }

        if next_pos == rest.len() {
            out.push(Value::String(rest.to_string()));
            break;
        } else {
            out.push(Value::String(rest[..next_pos].to_string()));
            rest = &rest[next_pos..];
        }
    }

    // Merge adjacent strings; stray span markers never reach the output.
    let mut merged: Vec<Value> = Vec::new();
    for item in out {
        if let Value::String(mut s) = item {
            s.retain(|ch| !extended::is_marker(ch));
            if let Some(Value::String(prev)) = merged.last_mut() {
                prev.push_str(&s);
            } else if !s.is_empty() {
                merged.push(Value::String(s));
            }
        } else {
            merged.push(item);
        }
    }

    if merged.is_empty() {
        Value::String(String::new())
    } else if merged.len() == 1 {
        merged.pop().unwrap_or_else(|| Value::String(String::new()))
    } else {
        Value::Array(merged)
    }
}
