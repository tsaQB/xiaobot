//! Readable plain text for rich messages received from Telegram (Bot API 10.1
//! `Message.rich_message`), such as a bot answer the owner forwarded to Xiao.
//!
//! Unlike the generic text walker used for length validation, this keeps the
//! structure a reader relies on: headings, list items and table rows stay on
//! their own lines, link targets are kept, and media blocks are named instead
//! of dumping their file ids into the text.

use serde_json::{Map, Value};

/// Renders received rich blocks as Markdown-like plain text.
pub fn rich_blocks_to_plain_text(blocks: &[Value]) -> String {
    render_blocks(blocks).join("\n\n")
}

fn render_blocks(blocks: &[Value]) -> Vec<String> {
    blocks.iter().filter_map(render_block).collect()
}

fn array<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

fn string<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or_default()
}

fn entity_string<'a>(entity: &'a Map<String, Value>, key: &str) -> &'a str {
    entity.get(key).and_then(Value::as_str).unwrap_or_default()
}

fn is_true(value: &Value, key: &str) -> bool {
    value.get(key).and_then(Value::as_bool) == Some(true)
}

/// Inline rich text (`RichText`): a string, an array of parts, or an entity
/// (bold, link, custom emoji, ...) wrapping further rich text.
fn inline_text(value: &Value) -> String {
    let mut out = String::new();
    push_inline(value, &mut out);
    out
}

fn field_text(value: &Value, key: &str) -> String {
    value.get(key).map(inline_text).unwrap_or_default()
}

fn push_inline(value: &Value, out: &mut String) {
    match value {
        Value::String(text) => out.push_str(text),
        Value::Array(parts) => parts.iter().for_each(|part| push_inline(part, out)),
        Value::Object(entity) => {
            let text = entity.get("text").map(inline_text).unwrap_or_default();
            match entity_string(entity, "type") {
                // The id is meaningless to a reader; the fallback emoji is not.
                "custom_emoji" => out.push_str(entity_string(entity, "alternative_text")),
                "mathematical_expression" => {
                    out.push('$');
                    out.push_str(entity_string(entity, "expression"));
                    out.push('$');
                }
                "url" => {
                    out.push_str(&text);
                    let url = entity_string(entity, "url");
                    if !url.is_empty() && text.trim() != url {
                        out.push_str(" (");
                        out.push_str(url);
                        out.push(')');
                    }
                }
                _ => out.push_str(&text),
            }
        }
        _ => {}
    }
}

/// Caption of a media or collage block (`RichBlockCaption`): text and credit.
fn block_caption(block: &Value) -> String {
    let Some(caption) = block.get("caption") else {
        return String::new();
    };
    let text = field_text(caption, "text");
    let credit = field_text(caption, "credit");
    [text.trim(), credit.trim()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" — ")
}

fn media(label: &str, name: &str, block: &Value) -> String {
    let caption = block_caption(block);
    let details: Vec<&str> = [name.trim(), caption.as_str()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect();
    if details.is_empty() {
        format!("[{label}]")
    } else {
        format!("[{label}: {}]", details.join(" — "))
    }
}

fn quote(body: &str, credit: &str) -> String {
    let mut lines: Vec<String> = body
        .lines()
        .map(|line| {
            if line.is_empty() {
                ">".to_string()
            } else {
                format!("> {line}")
            }
        })
        .collect();
    if !credit.trim().is_empty() {
        lines.push(format!("> — {}", credit.trim()));
    }
    lines.join("\n")
}

/// Every line after the first is indented, so nested lists stay nested.
fn indent_continuation(text: &str) -> String {
    text.lines()
        .enumerate()
        .map(|(index, line)| {
            if index == 0 || line.is_empty() {
                line.to_string()
            } else {
                format!("  {line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_list(block: &Value) -> String {
    array(block, "items")
        .iter()
        .map(|item| {
            let label = string(item, "label").trim();
            let marker = if label.is_empty() { "-" } else { label };
            let checkbox = match (is_true(item, "has_checkbox"), is_true(item, "is_checked")) {
                (true, true) => "[x] ",
                (true, false) => "[ ] ",
                _ => "",
            };
            let body = indent_continuation(&render_blocks(array(item, "blocks")).join("\n"));
            format!("{marker} {checkbox}{body}")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_table(block: &Value) -> String {
    let mut lines = Vec::new();
    let caption = field_text(block, "caption");
    if !caption.trim().is_empty() {
        lines.push(caption.trim().to_string());
    }
    for (index, row) in array(block, "cells").iter().enumerate() {
        let cells = row.as_array().map(Vec::as_slice).unwrap_or(&[]);
        if cells.is_empty() {
            continue;
        }
        let texts: Vec<String> = cells
            .iter()
            .map(|cell| {
                field_text(cell, "text")
                    .replace('\n', " ")
                    .trim()
                    .to_string()
            })
            .collect();
        lines.push(format!("| {} |", texts.join(" | ")));
        if index == 0 && cells.iter().all(|cell| is_true(cell, "is_header")) {
            lines.push(format!("|{}|", vec!["---"; cells.len()].join("|")));
        }
    }
    lines.join("\n")
}

fn render_map(block: &Value) -> String {
    let location = block.get("location");
    let coordinate = |key: &str| location.and_then(|l| l.get(key)).and_then(Value::as_f64);
    let place = match (coordinate("latitude"), coordinate("longitude")) {
        (Some(latitude), Some(longitude)) => format!(
            "{latitude:.6}, {longitude:.6} · https://maps.google.com/?q={latitude:.6},{longitude:.6}"
        ),
        _ => String::new(),
    };
    media("Peta", &place, block)
}

fn render_buttons(block: &Value) -> String {
    let buttons: Vec<String> = array(block, "buttons")
        .iter()
        .map(|button| {
            let text = field_text(button, "text");
            let url = string(button, "url");
            if url.is_empty() {
                text.trim().to_string()
            } else {
                format!("{} ({url})", text.trim())
            }
        })
        .filter(|button| !button.is_empty())
        .collect();
    if buttons.is_empty() {
        String::new()
    } else {
        format!("[Tombol: {}]", buttons.join(" | "))
    }
}

fn audio_name(block: &Value) -> String {
    let Some(audio) = block.get("audio") else {
        return String::new();
    };
    let performer = string(audio, "performer").trim();
    let title = string(audio, "title").trim();
    match (performer.is_empty(), title.is_empty()) {
        (false, false) => format!("{performer} – {title}"),
        (true, false) => title.to_string(),
        (false, true) => performer.to_string(),
        (true, true) => string(audio, "file_name").trim().to_string(),
    }
}

fn render_block(block: &Value) -> Option<String> {
    let text = || field_text(block, "text");
    let rendered = match string(block, "type") {
        "paragraph" | "footer" => text(),
        "heading" => {
            let level = block
                .get("size")
                .and_then(Value::as_u64)
                .unwrap_or(2)
                .clamp(1, 6);
            let level = usize::try_from(level).unwrap_or(2);
            format!("{} {}", "#".repeat(level), text().trim())
        }
        "pre" => format!("```{}\n{}\n```", string(block, "language"), text()),
        "divider" => "---".to_string(),
        "mathematical_expression" => format!("$${}$$", string(block, "expression")),
        "list" => render_list(block),
        "blockquote" => quote(
            &render_blocks(array(block, "blocks")).join("\n\n"),
            &field_text(block, "credit"),
        ),
        "expandable_blockquote" | "pullquote" => quote(&text(), &field_text(block, "credit")),
        "collage" | "slideshow" => {
            let mut parts = render_blocks(array(block, "blocks"));
            let caption = block_caption(block);
            if !caption.is_empty() {
                parts.push(caption);
            }
            parts.join("\n")
        }
        "table" => render_table(block),
        "details" => {
            let mut parts = vec![format!("▸ {}", field_text(block, "summary").trim())];
            parts.extend(render_blocks(array(block, "blocks")));
            parts.join("\n")
        }
        "map" => render_map(block),
        "buttons" => render_buttons(block),
        "photo" => media("Foto", "", block),
        "video" => media("Video", "", block),
        "animation" => media("Animasi", "", block),
        "voice_note" => media("Pesan suara", "", block),
        "audio" => media("Audio", &audio_name(block), block),
        "document" => media(
            "Dokumen",
            block
                .get("document")
                .map(|document| string(document, "file_name"))
                .unwrap_or_default(),
            block,
        ),
        // Anchors are invisible; thinking blocks exist only in drafts.
        "anchor" | "thinking" => String::new(),
        // A block type added after Bot API 10.3: keep whatever text it has.
        _ => {
            let mut parts = vec![text()];
            parts.extend(render_blocks(array(block, "blocks")));
            parts.join("\n")
        }
    };
    let rendered = rendered.trim();
    (!rendered.is_empty()).then(|| rendered.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn render(blocks: Value) -> String {
        rich_blocks_to_plain_text(blocks.as_array().expect("blocks array"))
    }

    #[test]
    fn lists_keep_one_item_per_line_with_nesting_and_checkboxes() {
        let text = render(json!([{"type": "list", "items": [
            {"label": "1.", "blocks": [{"type": "paragraph", "text": "Siapkan"}]},
            {"label": "2.", "blocks": [
                {"type": "paragraph", "text": "Kerjakan"},
                {"type": "list", "items": [
                    {"label": "•", "blocks": [{"type": "paragraph", "text": "sub"}]}
                ]}
            ]},
            {"label": "•", "has_checkbox": true, "is_checked": true,
             "blocks": [{"type": "paragraph", "text": "Selesai"}]}
        ]}]));
        assert_eq!(text, "1. Siapkan\n2. Kerjakan\n  • sub\n• [x] Selesai");
    }

    #[test]
    fn tables_become_rows_with_a_header_separator() {
        let text = render(json!([{"type": "table", "caption": "Harga", "cells": [
            [{"text": "Barang", "is_header": true}, {"text": "Harga", "is_header": true}],
            [{"text": "Kopi"}, {"text": ["Rp", {"type": "bold", "text": "20.000"}]}]
        ]}]));
        assert_eq!(
            text,
            "Harga\n| Barang | Harga |\n|---|---|\n| Kopi | Rp20.000 |"
        );
    }

    #[test]
    fn inline_links_emoji_and_math_are_readable() {
        let text = render(json!([{"type": "paragraph", "text": [
            "Baca ",
            {"type": "url", "text": "dokumentasi", "url": "https://core.telegram.org/bots/api"},
            " ",
            {"type": "custom_emoji", "custom_emoji_id": "5368324170671202286",
             "alternative_text": "🔥"},
            " dan ",
            {"type": "mathematical_expression", "expression": "E=mc^2"}
        ]}]));
        assert_eq!(
            text,
            "Baca dokumentasi (https://core.telegram.org/bots/api) 🔥 dan $E=mc^2$"
        );
        assert!(!text.contains("5368324170671202286"));
    }

    #[test]
    fn media_blocks_are_named_without_leaking_file_ids() {
        let text = render(json!([
            {"type": "photo",
             "photo": [{"file_id": "AgACAgFILEID", "file_unique_id": "u", "width": 1, "height": 1}],
             "caption": {"text": "Pemandangan", "credit": "Budi"}},
            {"type": "document",
             "document": {"file_id": "BQACDOCID", "file_unique_id": "d", "file_name": "laporan.pdf",
                          "mime_type": "application/pdf"}},
            {"type": "audio", "audio": {"file_id": "CQAAUDIO", "file_unique_id": "a",
                                        "duration": 3, "performer": "Band", "title": "Lagu"}}
        ]));
        assert_eq!(
            text,
            "[Foto: Pemandangan — Budi]\n\n[Dokumen: laporan.pdf]\n\n[Audio: Band – Lagu]"
        );
        assert!(!text.contains("AgAC") && !text.contains("BQAC") && !text.contains("mime"));
    }

    #[test]
    fn quotes_details_code_and_headings_keep_their_shape() {
        let text = render(json!([
            {"type": "heading", "size": 1, "text": "Judul"},
            {"type": "blockquote", "blocks": [{"type": "paragraph", "text": "Kutipan"}],
             "credit": "Tokoh"},
            {"type": "details", "summary": "Rincian",
             "blocks": [{"type": "paragraph", "text": "Isi tersembunyi"}]},
            {"type": "pre", "language": "rust", "text": "fn main() {}"},
            {"type": "anchor", "name": "bagian-1"},
            {"type": "divider"}
        ]));
        assert_eq!(
            text,
            "# Judul\n\n> Kutipan\n> — Tokoh\n\n▸ Rincian\nIsi tersembunyi\n\n```rust\nfn main() {}\n```\n\n---"
        );
    }
}
