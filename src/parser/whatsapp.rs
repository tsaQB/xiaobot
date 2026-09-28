use regex::Regex;
use std::sync::LazyLock;

static RE_HEADER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^#{1,6}\s+(.+)$").expect("valid regex"));
static RE_BULLET: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^\s*[\*\-]\s+(.+)$").expect("valid regex"));
static RE_BOLD_ITALIC: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\*\*\*(.+?)\*\*\*|___(.+?)___").expect("valid regex"));
static RE_BOLD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\*\*(.+?)\*\*|__(.+?)__").expect("valid regex"));
static RE_ITALIC_ASTERISK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)\*([^\*\s](?:.*?[^\*\s])?)\*").expect("valid regex"));
static RE_ITALIC_UNDERSCORE: LazyLock<Regex> = LazyLock::new(|| {
    // Only match standalone words with underscores, not snake_case identifiers
    Regex::new(r"(?s)(^|\s)_([^_\s](?:.*?[^_\s])?)_($|[\s\.,!\?:;])").expect("valid regex")
});
static RE_STRIKE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"~~(.+?)~~").expect("valid regex"));
static RE_CODE_SEGMENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)```[a-zA-Z0-9_-]*\n?(.*?)```|`([^`\n]+)`").expect("valid regex")
});
static RE_MEDIA_TAG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?is)<(img|audio|video|tg-photo|tg-video|tg-audio|tg-document|document)\b[^>]*>")
        .expect("valid regex")
});
static RE_GALLERY_OPEN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)<(tg-collage|tg-slideshow)\b[^>]*>").expect("valid regex"));
static RE_MEDIA_CLOSE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)</(?:audio|video|tg-photo|tg-video|tg-audio|tg-document|document|tg-collage|tg-slideshow|tg-map)\s*>",
    )
    .expect("valid regex")
});
static RE_MAP_TAG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)<tg-map\b[^>]*>").expect("valid regex"));
static RE_ATTACHED_DOC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\[(?:document|dokumen):\s*([^\]]+)\]\(attach://[^)\s]+\)").expect("valid regex")
});
static RE_MD_IMAGE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"!\[([^\]]*)\]\((https?://[^)\s]+)\)").expect("valid regex"));
static RE_MD_LINK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[([^\]]+)\]\((https?://[^)\s]+)\)").expect("valid regex"));

/// Converts Telegram-oriented media markup produced by tools (`<img>`,
/// `<audio>`, `<tg-map>`, `<tg-collage>`, `[document: x](attach://y)`) and
/// Markdown links into plain text WhatsApp can show. Documents staged with
/// `attach://` are delivered as separate WhatsApp files, so only their name
/// is kept inline.
pub fn render_media_markup_for_whatsapp(text: &str) -> String {
    use crate::parser::markdown::extract_html_attribute;

    let docs = RE_ATTACHED_DOC.replace_all(text, "📎 $1");
    let maps = RE_MAP_TAG.replace_all(&docs, |caps: &regex::Captures| {
        let tag = &caps[0];
        let lat = extract_html_attribute(tag, "lat").unwrap_or_default();
        let lon = extract_html_attribute(tag, "lon").unwrap_or_default();
        let title = extract_html_attribute(tag, "title").unwrap_or("Lokasi");
        if lat.is_empty() || lon.is_empty() {
            String::new()
        } else {
            format!("\n📍 {title}: https://maps.google.com/?q={lat},{lon}\n")
        }
    });
    let galleries = RE_GALLERY_OPEN.replace_all(&maps, |caps: &regex::Captures| {
        extract_html_attribute(&caps[0], "caption")
            .map(|caption| format!("\n🖼️ {}\n", caption.replace("&quot;", "\"")))
            .unwrap_or_else(|| "\n".to_string())
    });
    let media = RE_MEDIA_TAG.replace_all(&galleries, |caps: &regex::Captures| {
        let tag = &caps[0];
        let kind = caps[1].to_ascii_lowercase();
        let Some(url) = extract_html_attribute(tag, "src")
            .or_else(|| extract_html_attribute(tag, "url"))
            .filter(|url| url.starts_with("http://") || url.starts_with("https://"))
        else {
            return String::new();
        };
        let label = extract_html_attribute(tag, "caption")
            .or_else(|| extract_html_attribute(tag, "title"))
            .map(|label| label.replace("&quot;", "\""));
        let icon = if kind.contains("audio") {
            "🎵"
        } else if kind.contains("video") {
            "🎬"
        } else if kind.contains("document") {
            "📎"
        } else {
            "🖼️"
        };
        match label {
            Some(label) if !label.trim().is_empty() => format!("\n{icon} {label}: {url}\n"),
            _ => format!("\n{icon} {url}\n"),
        }
    });
    let closed = RE_MEDIA_CLOSE.replace_all(&media, "");
    let images = RE_MD_IMAGE.replace_all(&closed, |caps: &regex::Captures| {
        let alt = caps[1].trim();
        if alt.is_empty() {
            format!("🖼️ {}", &caps[2])
        } else {
            format!("🖼️ {alt}: {}", &caps[2])
        }
    });
    let links = RE_MD_LINK.replace_all(&images, |caps: &regex::Captures| {
        let label = caps[1].trim();
        let url = &caps[2];
        if let Some(name) = label
            .strip_prefix("document:")
            .or_else(|| label.strip_prefix("dokumen:"))
        {
            format!("📎 {}: {url}", name.trim())
        } else if label == url {
            url.to_string()
        } else {
            format!("{label} ({url})")
        }
    });
    links.into_owned()
}

/// Mengonversi teks Markdown umum (CommonMark) menjadi format WhatsApp Markdown.
///
/// Aturan formatting WhatsApp:
/// - **Tebal**: `*teks*`
/// - *Miring*: `_teks_`
/// - ***Tebal & Miring***: `*_teks_*`
/// - ~Coret~: `~teks~`
/// - `Inline code`: `\`teks\``
/// - ```Blok kode```: ```` ```teks``` ````
pub fn format_for_whatsapp(input: &str) -> String {
    // 1. Ekstrak blok kode (fenced) dan inline code terlebih dahulu agar konten di dalamnya terlindungi
    let mut segments = Vec::new();
    let mut last_idx = 0;

    for mat in RE_CODE_SEGMENT.find_iter(input) {
        if mat.start() > last_idx {
            segments.push((false, &input[last_idx..mat.start()]));
        }
        segments.push((true, mat.as_str()));
        last_idx = mat.end();
    }
    if last_idx < input.len() {
        segments.push((false, &input[last_idx..]));
    }

    let mut output = String::new();

    for (is_code, text) in segments {
        if is_code {
            output.push_str(text);
        } else {
            // Tag media gaya Telegram (hasil tool) tidak dapat dirender WhatsApp;
            // ubah menjadi tautan/label yang terbaca sebelum format Markdown.
            // WhatsApp has no highlight, super/subscript or date-time entities.
            let flattened = super::markdown::flatten_extended_inline(text);
            let c0 = render_media_markup_for_whatsapp(&flattened);
            // Gunakan Cow dari hasil replace_all untuk menghindari alokasi String yang tidak perlu (Shallow Modules fix)
            let c1 = RE_HEADER.replace_all(&c0, "\x01$1\x01");
            let c2 = RE_BULLET.replace_all(&c1, "• $1");
            let c3 = RE_STRIKE.replace_all(&c2, "~$1~");
            let c4 = RE_BOLD_ITALIC.replace_all(&c3, |caps: &regex::Captures| {
                let m = caps
                    .get(1)
                    .or_else(|| caps.get(2))
                    .map(|v| v.as_str())
                    .unwrap_or("");
                format!("\x01\x02{}\x02\x01", m)
            });
            let c5 = RE_BOLD.replace_all(&c4, |caps: &regex::Captures| {
                let m = caps
                    .get(1)
                    .or_else(|| caps.get(2))
                    .map(|v| v.as_str())
                    .unwrap_or("");
                format!("\x01{}\x01", m)
            });
            let c6 = RE_ITALIC_ASTERISK.replace_all(&c5, "\x02$1\x02");
            let c7 = RE_ITALIC_UNDERSCORE.replace_all(&c6, "$1\x02$2\x02$3");

            // Resolusi placeholder
            let final_str = c7.replace('\x01', "*").replace('\x02', "_");
            output.push_str(&final_str);
        }
    }

    output
}

/// Memecah pesan agar muat pada batas karakter WhatsApp.
///
/// Pemecahan diutamakan pada pergantian baris. Baris tunggal yang
/// melampaui batas dipotong paksa pada batas karakter, karena
/// memotong pada posisi byte akan memanikkan program pada teks
/// non-ASCII seperti emoji dan aksara Arab.
pub fn chunk_whatsapp_message(text: &str, max_len: usize) -> Vec<String> {
    if max_len == 0 {
        return vec![text.to_string()];
    }
    if text.chars().count() <= max_len {
        return vec![text.to_string()];
    }

    let mut chunks = Vec::new();
    let mut current = String::new();
    // Character counts are tracked incrementally; recounting `current` for
    // every piece made long replies cost O(n * max_len).
    let mut current_chars = 0usize;

    for line in text.lines() {
        for piece in split_oversized_line(line, max_len) {
            let piece_chars = piece.chars().count();
            let projected = current_chars + piece_chars + 1;
            if projected > max_len && !current.is_empty() {
                chunks.push(current.trim_end().to_string());
                current = String::new();
                current_chars = 0;
            }
            if !current.is_empty() {
                current.push('\n');
                current_chars += 1;
            }
            current.push_str(&piece);
            current_chars += piece_chars;
        }
    }

    if !current.trim().is_empty() {
        chunks.push(current.trim_end().to_string());
    }

    if chunks.is_empty() {
        vec![text.to_string()]
    } else {
        chunks
    }
}

/// Memotong satu baris yang melampaui batas menjadi beberapa bagian,
/// selalu pada batas karakter agar aman untuk teks multibyte.
fn split_oversized_line(line: &str, max_len: usize) -> Vec<String> {
    if line.chars().count() <= max_len {
        return vec![line.to_string()];
    }

    let mut pieces = Vec::new();
    let mut buffer = String::new();
    let mut buffer_chars = 0usize;
    for ch in line.chars() {
        if buffer_chars >= max_len {
            pieces.push(std::mem::take(&mut buffer));
            buffer_chars = 0;
        }
        buffer.push(ch);
        buffer_chars += 1;
    }
    if !buffer.is_empty() {
        pieces.push(buffer);
    }
    pieces
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_bold_and_headers() {
        let input = "## Judul Utama\nIni **teks tebal** dan ini ~~teks coret~~.";
        let res = format_for_whatsapp(input);
        assert_eq!(res, "*Judul Utama*\nIni *teks tebal* dan ini ~teks coret~.");
    }

    #[test]
    fn test_format_italics_conversion() {
        let input = "Ini *teks miring* dan ini _miring lagi_.";
        let res = format_for_whatsapp(input);
        assert_eq!(res, "Ini _teks miring_ dan ini _miring lagi_.");
    }

    #[test]
    fn test_format_bold_and_italic_combined() {
        let input = "Ini ***teks tebal dan miring***.";
        let res = format_for_whatsapp(input);
        assert_eq!(res, "Ini *_teks tebal dan miring_*.");
    }

    #[test]
    fn test_preserve_code_blocks_and_inline() {
        let input =
            "Contoh kode:\n```rust\nlet x = **tidak_diubah**;\n```\nDan `inline *code*` di sini.";
        let res = format_for_whatsapp(input);
        assert!(res.contains("```rust\nlet x = **tidak_diubah**;\n```"));
        assert!(res.contains("`inline *code*`"));
    }

    #[test]
    fn telegram_media_markup_becomes_readable_whatsapp_text() {
        let input = "Ini fotonya:\n<img src=\"https://ex.com/a.jpg\" caption=\"Gunung\"/>\n\
            <tg-map lat=\"-8.41\" lon=\"116.45\" zoom=\"13\" title=\"Rinjani\"/>\n\
            [document: laporan.pdf](attach://doc_1)\n[Sumber](https://ex.com/src)";
        let output = format_for_whatsapp(input);
        assert!(
            output.contains("🖼️ Gunung: https://ex.com/a.jpg"),
            "{output}"
        );
        assert!(output.contains("📍 Rinjani: https://maps.google.com/?q=-8.41,116.45"));
        assert!(output.contains("📎 laporan.pdf"));
        assert!(!output.contains("attach://"));
        assert!(output.contains("Sumber (https://ex.com/src)"));
        assert!(!output.contains('<'));
    }

    #[test]
    fn media_markup_inside_code_is_left_untouched() {
        let output = format_for_whatsapp("```html\n<img src=\"https://ex.com/a.jpg\"/>\n```");
        assert!(output.contains("<img src=\"https://ex.com/a.jpg\"/>"));
    }

    #[test]
    fn chunking_a_very_long_line_is_linear() {
        let long = "a".repeat(2_000_000);
        let started = std::time::Instant::now();
        let chunks = chunk_whatsapp_message(&long, 3500);
        assert!(chunks.iter().all(|chunk| chunk.chars().count() <= 3500));
        assert_eq!(chunks.iter().map(|c| c.len()).sum::<usize>(), long.len());
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }

    #[test]
    fn test_preserve_snake_case() {
        let input = "Variabel user_session_id tidak boleh miring.";
        let res = format_for_whatsapp(input);
        assert_eq!(res, "Variabel user_session_id tidak boleh miring.");
    }

    #[test]
    fn test_chunking() {
        let long_text = "Baris 1\n".repeat(600);
        let chunks = chunk_whatsapp_message(&long_text, 1000);
        assert!(chunks.len() > 1);
        for chunk in &chunks {
            assert!(chunk.len() <= 1000);
        }
    }

    #[test]
    fn single_oversized_line_is_split_below_limit() {
        let line = "A".repeat(9_000);
        let chunks = chunk_whatsapp_message(&line, 3_500);
        assert!(chunks.len() >= 3, "baris raksasa harus terpecah");
        for chunk in &chunks {
            assert!(chunk.chars().count() <= 3_500);
        }
    }

    #[test]
    fn multibyte_text_is_split_without_panicking() {
        let text = "\u{1F389}".repeat(5_000);
        let chunks = chunk_whatsapp_message(&text, 1_000);
        for chunk in &chunks {
            assert!(chunk.chars().count() <= 1_000);
        }
        let rejoined: String = chunks.concat();
        assert_eq!(rejoined.chars().count(), 5_000, "tidak ada karakter hilang");
    }

    #[test]
    fn zero_limit_degrades_gracefully() {
        let chunks = chunk_whatsapp_message("halo", 0);
        assert_eq!(chunks, vec!["halo".to_string()]);
    }

    #[test]
    fn long_code_block_survives_chunking_without_loss() {
        let body = (0..400)
            .map(|i| format!("let baris_{i} = {i};"))
            .collect::<Vec<_>>()
            .join("\n");
        let text = format!("```rust\n{body}\n```");
        let chunks = chunk_whatsapp_message(&text, 2_000);
        assert!(chunks.len() > 1);
        for chunk in &chunks {
            assert!(chunk.chars().count() <= 2_000);
        }
        assert!(chunks.concat().contains("let baris_399 = 399;"));
    }
}
