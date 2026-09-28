use serde_json::Value;

use super::TelegramBotClient;
use crate::bot::models::{RichBlock, RichBlockCaption, RichBlockTableCell};

/// Compiled once instead of on every render call.
static RE_HTML_TAG: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| regex::Regex::new(r"</?[^>]+>").expect("valid static regex"));

impl TelegramBotClient {
    pub fn render_blocks_to_html_chunks(
        &self,
        blocks: &[RichBlock],
        max_chars: usize,
    ) -> Vec<String> {
        let mut chunks = Vec::new();
        let mut current_text = String::new();
        let tag_clean_re = Some(&*RE_HTML_TAG);

        for block in blocks {
            let b_html = self.render_single_block_html(block);
            if b_html.is_empty() {
                continue;
            }

            if b_html.chars().count() > max_chars {
                if !current_text.is_empty() {
                    chunks.push(current_text.trim().to_string());
                    current_text.clear();
                }

                // Never split raw Telegram HTML in the middle of a tag/entity.
                // Oversized single blocks degrade to escaped plain text chunks;
                // correctness is preferable to a parse-mode rejection.
                let stripped = tag_clean_re
                    .as_ref()
                    .map(|regex| regex.replace_all(&b_html, "").into_owned())
                    .unwrap_or_else(|| b_html.clone());
                let plain = html_escape::decode_html_entities(&stripped).into_owned();
                let mut escaped_chunk = String::new();
                let mut escaped_len = 0usize;
                for ch in plain.chars() {
                    let encoded = html_escape::encode_text(&ch.to_string()).into_owned();
                    let encoded_len = encoded.chars().count();
                    if !escaped_chunk.is_empty() && escaped_len + encoded_len > max_chars {
                        chunks.push(std::mem::take(&mut escaped_chunk));
                        escaped_len = 0;
                    }
                    escaped_chunk.push_str(&encoded);
                    escaped_len += encoded_len;
                }
                if !escaped_chunk.is_empty() {
                    chunks.push(escaped_chunk);
                }
                continue;
            }

            if current_text.chars().count() + b_html.chars().count() + 2 > max_chars
                && !current_text.is_empty()
            {
                chunks.push(current_text.trim().to_string());
                current_text.clear();
            }

            if !current_text.is_empty() {
                current_text.push('\n');
            }
            current_text.push_str(&b_html);
        }

        if !current_text.trim().is_empty() {
            chunks.push(current_text.trim().to_string());
        }

        if chunks.is_empty() {
            vec!["".to_string()]
        } else {
            chunks
        }
    }

    pub fn render_blocks_to_plain_chunks(
        &self,
        blocks: &[RichBlock],
        max_chars: usize,
    ) -> Vec<String> {
        let mut chunks = Vec::new();
        let mut current = String::new();
        for block in blocks {
            let rendered = self.render_single_block_plain(block);
            if rendered.trim().is_empty() {
                continue;
            }
            if rendered.chars().count() > max_chars {
                if !current.trim().is_empty() {
                    chunks.push(std::mem::take(&mut current));
                }
                chunks.extend(self.split_text_chunks(&rendered, max_chars));
                continue;
            }
            if !current.is_empty()
                && current.chars().count() + rendered.chars().count() + 1 > max_chars
            {
                chunks.push(std::mem::take(&mut current));
            }
            if !current.is_empty() {
                current.push('\n');
            }
            current.push_str(rendered.trim());
        }
        if !current.trim().is_empty() {
            chunks.push(current);
        }
        if chunks.is_empty() {
            vec![String::new()]
        } else {
            chunks
        }
    }

    fn render_single_block_plain(&self, block: &RichBlock) -> String {
        match block {
            RichBlock::Paragraph { text }
            | RichBlock::SectionHeading { text, .. }
            | RichBlock::Thinking { text } => self.rich_value_to_plain(text),
            RichBlock::Footer { text } => format!("— {}", self.rich_value_to_plain(text)),
            RichBlock::Preformatted { text, .. } => text.clone(),
            RichBlock::List { items } => items
                .iter()
                .map(|item| {
                    let marker = item
                        .value
                        .map(|value| format!("{value}."))
                        .or_else(|| item.kind.as_deref().map(|_| "1.".to_string()))
                        .unwrap_or_else(|| "•".to_string());
                    let body = item
                        .blocks
                        .iter()
                        .map(|value| self.rich_value_to_plain(value))
                        .collect::<Vec<_>>()
                        .join("");
                    format!("{marker} {body}")
                })
                .collect::<Vec<_>>()
                .join(
                    "
",
                ),
            RichBlock::BlockQuotation { blocks } => blocks
                .iter()
                .map(|value| self.rich_value_to_plain(value))
                .collect::<Vec<_>>()
                .join(
                    "
",
                ),
            RichBlock::ExpandableBlockQuotation { text, credit }
            | RichBlock::PullQuotation { text, credit } => {
                let mut output = self.rich_value_to_plain(text);
                if let Some(credit) = credit {
                    let credit = self.rich_value_to_plain(credit);
                    if !credit.is_empty() {
                        output.push_str(
                            "
— ",
                        );
                        output.push_str(&credit);
                    }
                }
                output
            }
            RichBlock::Divider {} => "────────────────────────".to_string(),
            RichBlock::MathematicalExpression { expression } => expression.clone(),
            RichBlock::Table {
                cells,
                has_header,
                caption,
                ..
            } => {
                let ascii = self.render_table_to_ascii(cells, *has_header);
                if let Some(cap) = caption {
                    if !ascii.is_empty() {
                        format!("{cap}\n{ascii}")
                    } else {
                        cap.clone()
                    }
                } else {
                    ascii
                }
            }
            RichBlock::Buttons { buttons, .. } => buttons
                .iter()
                .map(|button| self.rich_value_to_plain(&button.text))
                .collect::<Vec<_>>()
                .join(" | "),
            RichBlock::Document { document, caption } => {
                let name = caption
                    .as_ref()
                    .map(|cap| self.rich_value_to_plain(&cap.text))
                    .filter(|val| !val.trim().is_empty())
                    .unwrap_or_else(|| {
                        document
                            .get("media")
                            .and_then(Value::as_str)
                            .map(|m| m.strip_prefix("tg://document?id=").unwrap_or(m))
                            .unwrap_or("document")
                            .to_string()
                    });
                format!("📄 {name}")
            }
            RichBlock::Photo { caption, .. } => {
                let cap = self.rich_caption_to_plain(caption);
                if cap.is_empty() {
                    "[Photo]".to_string()
                } else {
                    format!("[Photo] {cap}")
                }
            }
            RichBlock::Video { caption, .. } => {
                let cap = self.rich_caption_to_plain(caption);
                if cap.is_empty() {
                    "[Video]".to_string()
                } else {
                    format!("[Video] {cap}")
                }
            }
            RichBlock::Audio { caption, .. } => {
                let cap = self.rich_caption_to_plain(caption);
                if cap.is_empty() {
                    "[Audio]".to_string()
                } else {
                    format!("[Audio] {cap}")
                }
            }
            RichBlock::VoiceNote { caption, .. } => {
                let cap = self.rich_caption_to_plain(caption);
                if cap.is_empty() {
                    "[Voice Note]".to_string()
                } else {
                    format!("[Voice Note] {cap}")
                }
            }
            RichBlock::Animation { caption, .. } => {
                let cap = self.rich_caption_to_plain(caption);
                if cap.is_empty() {
                    "[Animation]".to_string()
                } else {
                    format!("[Animation] {cap}")
                }
            }
            RichBlock::Collage { blocks, caption } => {
                let items = blocks
                    .iter()
                    .map(|value| self.rich_value_to_plain(value))
                    .collect::<Vec<_>>()
                    .join(" ");
                let cap = self.rich_caption_to_plain(caption);
                if cap.is_empty() {
                    format!("[Collage: {items}]")
                } else {
                    format!(
                        "[Collage: {items}]
{cap}"
                    )
                }
            }
            RichBlock::Slideshow { blocks, caption } => {
                let items = blocks
                    .iter()
                    .map(|value| self.rich_value_to_plain(value))
                    .collect::<Vec<_>>()
                    .join(" ");
                let cap = self.rich_caption_to_plain(caption);
                if cap.is_empty() {
                    format!("[Slideshow: {items}]")
                } else {
                    format!(
                        "[Slideshow: {items}]
{cap}"
                    )
                }
            }
            RichBlock::Map { location, zoom, .. } => {
                let zoom_str = zoom.map(|z| format!(" zoom={z}")).unwrap_or_default();
                format!(
                    "[Map: lat={}, lon={}{}]",
                    location.latitude, location.longitude, zoom_str
                )
            }
            RichBlock::Details {
                summary, blocks, ..
            } => {
                let body = blocks
                    .iter()
                    .map(|value| self.rich_value_to_plain(value))
                    .collect::<Vec<_>>()
                    .join(
                        "
",
                    );
                format!(
                    "{}
{}",
                    self.rich_value_to_plain(summary),
                    body
                )
                .trim()
                .to_string()
            }
            RichBlock::Anchor { .. } => String::new(),
        }
    }

    pub fn render_blocks_to_html(&self, blocks: &[RichBlock]) -> String {
        let mut lines = Vec::new();
        for block in blocks {
            let h = self.render_single_block_html(block);
            if !h.is_empty() {
                lines.push(h);
            }
        }
        lines.join("\n").trim().to_string()
    }

    fn render_single_block_html(&self, block: &RichBlock) -> String {
        match block {
            RichBlock::Paragraph { text } => {
                let inner = self.rich_value_to_html(text);
                format!(
                    "{inner}
"
                )
            }
            RichBlock::Footer { text } => {
                let inner = self.rich_value_to_html(text);
                format!(
                    "
— <i>{inner}</i>
"
                )
            }
            RichBlock::SectionHeading { text, .. } => {
                let inner = self.rich_value_to_html(text);
                format!(
                    "
<b>{inner}</b>
"
                )
            }
            RichBlock::Preformatted { text, language } => {
                let lang_attr = language
                    .as_ref()
                    .map(|language| html_escape::encode_double_quoted_attribute(language))
                    .map(|language| format!(" class=\"language-{language}\""))
                    .unwrap_or_default();
                let esc = html_escape::encode_text(text);
                format!(
                    "<pre{lang_attr}>{esc}</pre>
"
                )
            }
            RichBlock::List { items } => {
                let mut list_lines = Vec::new();
                for item in items {
                    let item_str = item
                        .blocks
                        .iter()
                        .map(|block| self.rich_value_to_html(block))
                        .collect::<Vec<_>>()
                        .join("");
                    let marker = item
                        .value
                        .map(|value| format!("{value}."))
                        .or_else(|| item.kind.as_deref().map(|_| "1.".to_string()))
                        .unwrap_or_else(|| "•".to_string());
                    list_lines.push(format!("{marker} {item_str}"));
                }
                list_lines.join(
                    "
",
                )
            }
            RichBlock::BlockQuotation { blocks } => {
                let mut q_text = String::new();
                for b in blocks {
                    q_text.push_str(&self.rich_value_to_html(b));
                }
                format!(
                    "<blockquote>{q_text}</blockquote>
"
                )
            }
            RichBlock::ExpandableBlockQuotation { text, credit }
            | RichBlock::PullQuotation { text, credit } => {
                let q_text = self.rich_value_to_html(text);
                let credit_html = credit
                    .as_ref()
                    .map(|value| self.rich_value_to_html(value))
                    .filter(|value| !value.is_empty())
                    .map(|value| format!("<cite>{value}</cite>"))
                    .unwrap_or_default();
                format!(
                    "<blockquote expandable>{q_text}{credit_html}</blockquote>
"
                )
            }
            RichBlock::Divider {} => "────────────────────────
"
            .to_string(),
            RichBlock::MathematicalExpression { expression } => {
                let esc = html_escape::encode_text(expression);
                format!(
                    "<code>{esc}</code>
"
                )
            }
            RichBlock::Table {
                cells,
                has_header,
                caption,
                ..
            } => {
                let ascii_tbl = self.render_table_to_ascii(cells, *has_header);
                if !ascii_tbl.is_empty() {
                    if let Some(cap) = caption {
                        let safe_cap = html_escape::encode_text(cap);
                        format!(
                            "
<b>{safe_cap}</b>
{ascii_tbl}
"
                        )
                    } else {
                        format!(
                            "
{ascii_tbl}
"
                        )
                    }
                } else {
                    String::new()
                }
            }
            RichBlock::Buttons { buttons, .. } => buttons
                .iter()
                .map(|button| format!("[{}]", self.rich_value_to_html(&button.text)))
                .collect::<Vec<_>>()
                .join(" "),
            RichBlock::Document { document, caption } => {
                let name = caption
                    .as_ref()
                    .map(|cap| self.rich_value_to_html(&cap.text))
                    .filter(|val| !val.trim().is_empty())
                    .unwrap_or_else(|| {
                        let m = document
                            .get("media")
                            .and_then(Value::as_str)
                            .map(|m| m.strip_prefix("tg://document?id=").unwrap_or(m))
                            .unwrap_or("document");
                        html_escape::encode_text(m).to_string()
                    });
                let media = document
                    .get("media")
                    .and_then(Value::as_str)
                    .or_else(|| document.as_str())
                    .unwrap_or_default();
                if !media.is_empty() {
                    let safe_url = html_escape::encode_double_quoted_attribute(media);
                    format!("📄 <b><a href=\"{safe_url}\">{name}</a></b>\n")
                } else {
                    format!("📄 <b>{name}</b>\n")
                }
            }
            RichBlock::Details {
                summary, blocks, ..
            } => {
                let summary_html = self.rich_value_to_html(summary);
                let body_html = blocks
                    .iter()
                    .map(|block| self.rich_value_to_html(block))
                    .collect::<Vec<_>>()
                    .join("\n");
                format!("<blockquote expandable><b>{summary_html}</b>\n{body_html}</blockquote>\n")
            }
            RichBlock::Thinking { text } => {
                let t_esc = self.rich_value_to_html(text);
                format!("{t_esc}\n")
            }
            RichBlock::Photo { photo, caption } => {
                let cap = self.rich_caption_to_html(caption);
                let media = photo
                    .get("media")
                    .and_then(Value::as_str)
                    .or_else(|| photo.as_str())
                    .unwrap_or_default();
                if !media.is_empty() {
                    let safe_url = html_escape::encode_double_quoted_attribute(media);
                    let label = if cap.is_empty() {
                        "Lihat Foto".to_string()
                    } else {
                        cap
                    };
                    format!("🖼️ <b><a href=\"{safe_url}\">{label}</a></b>\n")
                } else if cap.is_empty() {
                    "🖼️ <b>[Photo]</b>\n".to_string()
                } else {
                    format!("🖼️ <b>[Photo]</b>\n{cap}\n")
                }
            }
            RichBlock::Video { video, caption } => {
                let cap = self.rich_caption_to_html(caption);
                let media = video
                    .get("media")
                    .and_then(Value::as_str)
                    .or_else(|| video.as_str())
                    .unwrap_or_default();
                if !media.is_empty() {
                    let safe_url = html_escape::encode_double_quoted_attribute(media);
                    let label = if cap.is_empty() {
                        "Lihat Video".to_string()
                    } else {
                        cap
                    };
                    format!("🎥 <b><a href=\"{safe_url}\">{label}</a></b>\n")
                } else if cap.is_empty() {
                    "🎥 <b>[Video]</b>\n".to_string()
                } else {
                    format!("🎥 <b>[Video]</b>\n{cap}\n")
                }
            }
            RichBlock::Audio { audio, caption } => {
                let cap = self.rich_caption_to_html(caption);
                let media = audio
                    .get("media")
                    .and_then(Value::as_str)
                    .or_else(|| audio.as_str())
                    .unwrap_or_default();
                if !media.is_empty() {
                    let safe_url = html_escape::encode_double_quoted_attribute(media);
                    let label = if cap.is_empty() {
                        "Dengarkan Audio".to_string()
                    } else {
                        cap
                    };
                    format!("🎵 <b><a href=\"{safe_url}\">{label}</a></b>\n")
                } else if cap.is_empty() {
                    "🎵 <b>[Audio]</b>\n".to_string()
                } else {
                    format!("🎵 <b>[Audio]</b>\n{cap}\n")
                }
            }
            RichBlock::VoiceNote {
                voice_note,
                caption,
            } => {
                let cap = self.rich_caption_to_html(caption);
                let media = voice_note
                    .get("media")
                    .and_then(Value::as_str)
                    .or_else(|| voice_note.as_str())
                    .unwrap_or_default();
                if !media.is_empty() {
                    let safe_url = html_escape::encode_double_quoted_attribute(media);
                    let label = if cap.is_empty() {
                        "Pesan Suara".to_string()
                    } else {
                        cap
                    };
                    format!("🎤 <b><a href=\"{safe_url}\">{label}</a></b>\n")
                } else if cap.is_empty() {
                    "🎤 <b>[Voice Note]</b>\n".to_string()
                } else {
                    format!("🎤 <b>[Voice Note]</b>\n{cap}\n")
                }
            }
            RichBlock::Animation { animation, caption } => {
                let cap = self.rich_caption_to_html(caption);
                let media = animation
                    .get("media")
                    .and_then(Value::as_str)
                    .or_else(|| animation.as_str())
                    .unwrap_or_default();
                if !media.is_empty() {
                    let safe_url = html_escape::encode_double_quoted_attribute(media);
                    let label = if cap.is_empty() {
                        "Lihat Animasi".to_string()
                    } else {
                        cap
                    };
                    format!("🎞️ <b><a href=\"{safe_url}\">{label}</a></b>\n")
                } else if cap.is_empty() {
                    "🎞️ <b>[Animation]</b>\n".to_string()
                } else {
                    format!("🎞️ <b>[Animation]</b>\n{cap}\n")
                }
            }
            RichBlock::Collage { blocks, caption } | RichBlock::Slideshow { blocks, caption } => {
                let cap = self.rich_caption_to_html(caption);
                let label = if cap.is_empty() {
                    "Galeri Foto".to_string()
                } else {
                    cap
                };
                let mut links = Vec::new();
                for (i, b) in blocks.iter().enumerate() {
                    let url = b
                        .get("photo")
                        .and_then(|p| p.get("media"))
                        .and_then(Value::as_str)
                        .or_else(|| b.get("media").and_then(Value::as_str))
                        .or_else(|| b.as_str())
                        .unwrap_or_default();
                    if !url.is_empty() {
                        let safe_url = html_escape::encode_double_quoted_attribute(url);
                        links.push(format!("<a href=\"{safe_url}\">Foto #{}</a>", i + 1));
                    }
                }
                if links.is_empty() {
                    format!("🖼️ <b>{label}</b>\n")
                } else {
                    format!("🖼️ <b>{label}</b>: {}\n", links.join(" • "))
                }
            }
            RichBlock::Map { location, zoom, .. } => {
                let zoom_str = zoom.map(|z| format!("?z={z}")).unwrap_or_default();
                let url = format!(
                    "https://www.google.com/maps?q={},{}",
                    location.latitude, location.longitude
                );
                let safe_url = html_escape::encode_double_quoted_attribute(&url);
                format!(
                    "📍 <b><a href=\"{safe_url}\">Lokasi Peta ({}, {}{})</a></b>\n",
                    location.latitude, location.longitude, zoom_str
                )
            }
            RichBlock::Anchor { .. } => String::new(),
        }
    }

    pub(crate) fn rich_caption_to_plain(&self, caption: &Option<RichBlockCaption>) -> String {
        let Some(caption) = caption else {
            return String::new();
        };
        let mut text = self.rich_value_to_plain(&caption.text);
        if let Some(credit) = &caption.credit {
            let credit_text = self.rich_value_to_plain(credit);
            if !credit_text.is_empty() {
                text.push_str(" — ");
                text.push_str(&credit_text);
            }
        }
        text
    }

    fn rich_caption_to_html(&self, caption: &Option<RichBlockCaption>) -> String {
        let Some(caption) = caption else {
            return String::new();
        };
        let mut text = self.rich_value_to_html(&caption.text);
        if let Some(credit) = &caption.credit {
            let credit_text = self.rich_value_to_html(credit);
            if !credit_text.is_empty() {
                text.push_str("<cite>");
                text.push_str(&credit_text);
                text.push_str("</cite>");
            }
        }
        text
    }

    fn rich_value_to_html(&self, v: &Value) -> String {
        match v {
            Value::String(s) => html_escape::encode_text(s).into_owned(),
            Value::Array(arr) => arr
                .iter()
                .map(|item| self.rich_value_to_html(item))
                .collect(),
            Value::Object(obj) => {
                let t = obj.get("type").and_then(|s| s.as_str()).unwrap_or("");
                let inner = obj
                    .get("text")
                    .map(|sub| self.rich_value_to_html(sub))
                    .unwrap_or_default();
                match t {
                    "bold" => format!("<b>{inner}</b>"),
                    "italic" => format!("<i>{inner}</i>"),
                    "code" => format!("<code>{inner}</code>"),
                    "url" => {
                        let url = obj.get("url").and_then(|s| s.as_str()).unwrap_or("#");
                        let safe_url = html_escape::encode_double_quoted_attribute(url);
                        format!("<a href=\"{safe_url}\">{inner}</a>")
                    }
                    "spoiler" => format!("<tg-spoiler>{inner}</tg-spoiler>"),
                    "strikethrough" | "strike" => format!("<s>{inner}</s>"),
                    "underline" => format!("<u>{inner}</u>"),
                    "paragraph" => inner,
                    _ => inner,
                }
            }
            _ => v.to_string(),
        }
    }

    fn rich_value_to_plain(&self, v: &Value) -> String {
        match v {
            Value::String(s) => s.clone(),
            Value::Array(arr) => arr
                .iter()
                .map(|item| self.rich_value_to_plain(item))
                .collect(),
            Value::Object(obj) => {
                if let Some(text) = obj.get("text") {
                    self.rich_value_to_plain(text)
                } else if let Some(expr) = obj.get("expression") {
                    self.rich_value_to_plain(expr)
                } else {
                    String::new()
                }
            }
            _ => v.to_string(),
        }
    }

    fn char_display_width(c: char) -> usize {
        match c {
            '\u{1100}'..='\u{115F}'
            | '\u{2329}'
            | '\u{232A}'
            | '\u{2E80}'..='\u{303E}'
            | '\u{3040}'..='\u{A4CF}'
            | '\u{AC00}'..='\u{D7A3}'
            | '\u{F900}'..='\u{FAFF}'
            | '\u{FE10}'..='\u{FE19}'
            | '\u{FE30}'..='\u{FE6F}'
            | '\u{FF00}'..='\u{FF60}'
            | '\u{FFE0}'..='\u{FFE6}'
            | '\u{1F300}'..='\u{1FAFF}'
            | '\u{2600}'..='\u{27BF}' => 2,
            _ => 1,
        }
    }

    fn str_display_width(s: &str) -> usize {
        s.chars().map(Self::char_display_width).sum()
    }

    fn truncate_display_width(s: &str, max_width: usize) -> String {
        if Self::str_display_width(s) <= max_width {
            return s.to_string();
        }
        if max_width <= 1 {
            return "…".to_string();
        }
        let mut out = String::new();
        let mut width = 0;
        for c in s.chars() {
            let c_width = Self::char_display_width(c);
            if width + c_width > max_width - 1 {
                break;
            }
            out.push(c);
            width += c_width;
        }
        out.push('…');
        out
    }

    fn render_table_to_ascii(&self, rows: &[Vec<RichBlockTableCell>], has_header: bool) -> String {
        if rows.is_empty() {
            return String::new();
        }

        let num_cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
        if num_cols == 0 {
            return String::new();
        }

        let tag_clean_re = Some(&*RE_HTML_TAG);
        let mut norm_rows: Vec<Vec<(String, String)>> = Vec::new();
        for r in rows {
            let mut row_cells: Vec<(String, String)> = r
                .iter()
                .map(|c| {
                    let plain = self.rich_value_to_plain(&c.text);
                    let cleaned = tag_clean_re
                        .as_ref()
                        .map(|regex| regex.replace_all(&plain, "").into_owned())
                        .unwrap_or(plain);
                    let decoded = html_escape::decode_html_entities(&cleaned)
                        .replace(['\n', '\r', '\t'], " ")
                        .to_string();
                    let align = c.align.clone().unwrap_or_else(|| "left".to_string());
                    (decoded, align)
                })
                .collect();
            while row_cells.len() < num_cols {
                row_cells.push((String::new(), "left".to_string()));
            }
            norm_rows.push(row_cells);
        }

        let mut col_widths = vec![2usize; num_cols];
        for r in &norm_rows {
            for (i, (c, _)) in r.iter().enumerate() {
                col_widths[i] = col_widths[i].max(Self::str_display_width(c));
            }
        }

        const MAX_TABLE_WIDTH: usize = 64;
        let border_overhead = num_cols + 1 + (num_cols * 2);
        let available = MAX_TABLE_WIDTH.saturating_sub(border_overhead);
        let current: usize = col_widths.iter().sum();
        if current > available && available >= num_cols {
            let min_width = 3usize;
            let mut excess = current.saturating_sub(available);
            while excess > 0 {
                let mut changed = false;
                for width in &mut col_widths {
                    if excess == 0 {
                        break;
                    }
                    if *width > min_width {
                        *width -= 1;
                        excess -= 1;
                        changed = true;
                    }
                }
                if !changed {
                    break;
                }
            }
        }

        let separator = format!(
            "|-{}-|",
            col_widths
                .iter()
                .map(|w| "-".repeat(*w + 2))
                .collect::<Vec<_>>()
                .join("-|-")
        );

        let mut table_lines = Vec::new();
        for (idx, r) in norm_rows.iter().enumerate() {
            let mut line_cells = Vec::new();
            for i in 0..num_cols {
                let (cell_txt, align) = &r[i];
                let display_txt = Self::truncate_display_width(cell_txt, col_widths[i]);
                let width = Self::str_display_width(&display_txt);
                let total_pad = col_widths[i].saturating_sub(width);
                let esc_txt = html_escape::encode_text(&display_txt);

                let padded = match align.as_str() {
                    "center" => {
                        let l_pad = total_pad / 2;
                        let r_pad = total_pad - l_pad;
                        format!(" {}{}{} ", " ".repeat(l_pad), esc_txt, " ".repeat(r_pad))
                    }
                    "right" => {
                        format!(" {}{} ", " ".repeat(total_pad), esc_txt)
                    }
                    _ => {
                        format!(" {}{} ", esc_txt, " ".repeat(total_pad))
                    }
                };
                line_cells.push(padded);
            }
            table_lines.push(format!("|{}|", line_cells.join("|")));
            if idx == 0 && has_header && norm_rows.len() > 1 {
                table_lines.push(separator.clone());
            }
        }

        format!("<pre><code>{}</code></pre>", table_lines.join("\n"))
    }
}
