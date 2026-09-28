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
    let code_pattern =
        Regex::new(r"(?s)```[a-zA-Z0-9_-]*\n?(.*?)```|`([^`\n]+)`").expect("valid regex");

    for mat in code_pattern.find_iter(input) {
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
            // Gunakan Cow dari hasil replace_all untuk menghindari alokasi String yang tidak perlu (Shallow Modules fix)
            let c1 = RE_HEADER.replace_all(text, "\x01$1\x01");
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

    for line in text.lines() {
        for piece in split_oversized_line(line, max_len) {
            let projected = current.chars().count() + piece.chars().count() + 1;
            if projected > max_len && !current.is_empty() {
                chunks.push(current.trim_end().to_string());
                current = String::new();
            }
            if !current.is_empty() {
                current.push('\n');
            }
            current.push_str(&piece);
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
    for ch in line.chars() {
        if buffer.chars().count() >= max_len {
            pieces.push(std::mem::take(&mut buffer));
        }
        buffer.push(ch);
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
