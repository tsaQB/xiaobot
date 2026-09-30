//! Media markup for the WebUI chat.
//!
//! Tools write Telegram-oriented tags (`<img>`, `<audio>`, `<tg-map>`,
//! `<tg-collage>`, `[document: x](attach://y)`). The browser renders plain
//! Markdown without HTML, so the tags become Markdown images and links:
//! pictures stay pictures, audio, video and documents become links, a map
//! becomes a map link, and `attach://` documents are dropped because the
//! WebUI offers those files for download separately. Code is left untouched.

use regex::Regex;
use std::sync::LazyLock;

use crate::parser::markdown::extract_html_attribute;

static RE_CODE_SEGMENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)```[a-zA-Z0-9_-]*\n?.*?```|`[^`\n]+`").expect("valid regex"));
static RE_MEDIA_TAG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?is)<(img|audio|video|tg-photo|tg-video|tg-audio|tg-document|document)\b[^>]*>")
        .expect("valid regex")
});
static RE_GALLERY_OPEN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)<(tg-collage|tg-slideshow)\b[^>]*>").expect("valid regex"));
static RE_MEDIA_CLOSE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)</(?:img|audio|video|tg-photo|tg-video|tg-audio|tg-document|document|tg-collage|tg-slideshow|tg-map)\s*>",
    )
    .expect("valid regex")
});
static RE_MAP_TAG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)<tg-map\b[^>]*>").expect("valid regex"));
static RE_ATTACHED_DOC: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[[^\]\n]*\]\(attach://[^)\s]*\)").expect("valid regex"));
static RE_LABELED_LINK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\[(rekaman|voice|audio|document|dokumen|photo|foto|video):\s*([^\]\n]*)\]\((https?://[^)\s]+)\)",
    )
    .expect("valid regex")
});
static RE_BLANK_RUNS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\n{3,}").expect("valid regex"));

/// Text safe inside `[...]`: no brackets, no line breaks, entities decoded.
fn label(raw: &str) -> String {
    let decoded = html_escape::decode_html_entities(raw);
    decoded
        .chars()
        .map(|ch| match ch {
            '[' | ']' | '\n' | '\r' => ' ',
            other => other,
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// A URL safe inside `(...)`, or `None` when it is not http(s).
fn link_target(raw: &str) -> Option<String> {
    let url = html_escape::decode_html_entities(raw.trim()).into_owned();
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return None;
    }
    Some(
        url.replace(' ', "%20")
            .replace('(', "%28")
            .replace(')', "%29"),
    )
}

fn render_segment(text: &str) -> String {
    let without_attached = RE_ATTACHED_DOC.replace_all(text, "");
    let maps = RE_MAP_TAG.replace_all(&without_attached, |caps: &regex::Captures| {
        let tag = &caps[0];
        let number = |name: &str| {
            extract_html_attribute(tag, name)
                .map(str::trim)
                .filter(|value| value.parse::<f64>().is_ok_and(f64::is_finite))
        };
        match (number("lat"), number("lon")) {
            (Some(lat), Some(lon)) => {
                let title = extract_html_attribute(tag, "title")
                    .map(label)
                    .filter(|title| !title.is_empty())
                    .unwrap_or_else(|| "Lokasi".to_string());
                format!("\n\n[📍 {title}](https://www.google.com/maps?q={lat},{lon})\n\n")
            }
            _ => String::new(),
        }
    });
    let galleries = RE_GALLERY_OPEN.replace_all(&maps, |caps: &regex::Captures| {
        extract_html_attribute(&caps[0], "caption")
            .map(label)
            .filter(|caption| !caption.is_empty())
            .map(|caption| format!("\n\n**{caption}**\n\n"))
            .unwrap_or_else(|| "\n\n".to_string())
    });
    let media = RE_MEDIA_TAG.replace_all(&galleries, |caps: &regex::Captures| {
        let tag = &caps[0];
        let kind = caps[1].to_ascii_lowercase();
        let Some(url) = extract_html_attribute(tag, "src")
            .or_else(|| extract_html_attribute(tag, "url"))
            .and_then(link_target)
        else {
            return String::new();
        };
        let caption = extract_html_attribute(tag, "caption")
            .or_else(|| extract_html_attribute(tag, "title"))
            .map(label)
            .unwrap_or_default();
        if kind.contains("audio") {
            let name = if caption.is_empty() {
                "Audio".to_string()
            } else {
                caption
            };
            format!("\n\n[🎵 {name}]({url})\n\n")
        } else if kind.contains("video") {
            let name = if caption.is_empty() {
                "Video".to_string()
            } else {
                caption
            };
            format!("\n\n[🎬 {name}]({url})\n\n")
        } else if kind.contains("document") {
            let name = if caption.is_empty() {
                "Dokumen".to_string()
            } else {
                caption
            };
            format!("\n\n[📎 {name}]({url})\n\n")
        } else {
            format!("\n\n![{caption}]({url})\n\n")
        }
    });
    let closed = RE_MEDIA_CLOSE.replace_all(&media, "");
    let labeled = RE_LABELED_LINK.replace_all(&closed, |caps: &regex::Captures| {
        let kind = caps[1].to_ascii_lowercase();
        let name = label(&caps[2]);
        let Some(url) = link_target(&caps[3]) else {
            return caps[0].to_string();
        };
        match kind.as_str() {
            "photo" | "foto" => format!("![{name}]({url})"),
            "document" | "dokumen" => format!("[📎 {name}]({url})"),
            "video" => format!("[🎬 {name}]({url})"),
            "audio" => format!("[🎵 {name}]({url})"),
            _ => format!("[🎙️ {name}]({url})"),
        }
    });
    labeled.into_owned()
}

/// Converts tool media markup in an answer to Markdown the WebUI renders.
pub fn render_media_markup_for_web(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut last = 0;
    for found in RE_CODE_SEGMENT.find_iter(text) {
        output.push_str(&render_segment(&text[last..found.start()]));
        output.push_str(found.as_str());
        last = found.end();
    }
    output.push_str(&render_segment(&text[last..]));
    RE_BLANK_RUNS
        .replace_all(output.trim(), "\n\n")
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pictures_become_markdown_images() {
        let out = render_media_markup_for_web(
            "Ini fotonya:\n<img src=\"https://ex.com/a.jpg\" caption=\"Gunung &quot;Rinjani&quot;\"/>\nSelesai.",
        );
        assert!(
            out.contains("![Gunung \"Rinjani\"](https://ex.com/a.jpg)"),
            "{out}"
        );
        assert!(out.starts_with("Ini fotonya:"));
        assert!(out.ends_with("Selesai."));
        assert!(!out.contains('<'));
    }

    #[test]
    fn galleries_audio_maps_and_links_are_readable() {
        let out = render_media_markup_for_web(
            "<tg-collage caption=\"Roma\"><img src=\"https://ex.com/1.jpg\"/><img src=\"https://ex.com/2.jpg\"/></tg-collage>\n\
             <audio src=\"https://ex.com/s.mp3\" title=\"Lagu\"/>\n\
             <tg-map lat=\"-8.41\" lon=\"116.45\" zoom=\"13\" title=\"Rinjani\"/>\n\
             [rekaman: Salam](https://ex.com/v.ogg)\n[document: data.csv](https://ex.com/d.csv)",
        );
        assert!(out.contains("**Roma**"));
        assert!(out.contains("![](https://ex.com/1.jpg)"));
        assert!(out.contains("![](https://ex.com/2.jpg)"));
        assert!(out.contains("[🎵 Lagu](https://ex.com/s.mp3)"));
        assert!(out.contains("[📍 Rinjani](https://www.google.com/maps?q=-8.41,116.45)"));
        assert!(out.contains("[🎙️ Salam](https://ex.com/v.ogg)"));
        assert!(out.contains("[📎 data.csv](https://ex.com/d.csv)"));
        assert!(!out.contains("</tg-collage>"));
        assert!(!out.contains("\n\n\n"));
    }

    #[test]
    fn staged_documents_are_dropped_and_unsafe_media_ignored() {
        let out = render_media_markup_for_web(
            "Berkasnya siap.\n[document: laporan.pdf](attach://doc_1)\n<img src=\"javascript:alert(1)\"/>\n<tg-map lat=\"x\" lon=\"1\"/>",
        );
        assert_eq!(out, "Berkasnya siap.");
    }

    #[test]
    fn code_is_left_untouched() {
        let text = "Contoh:\n```html\n<img src=\"https://ex.com/a.jpg\"/>\n```\nDan `<audio src=\"https://x.y/z.mp3\"/>`.";
        assert_eq!(render_media_markup_for_web(text), text);
    }
}
