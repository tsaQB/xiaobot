use std::collections::HashSet;
use std::env;
use std::sync::LazyLock;
use std::time::Duration;

use futures_util::StreamExt;
use regex::Regex;
use reqwest::header::{ACCEPT, ACCEPT_LANGUAGE, USER_AGENT};
use serde::Deserialize;
use serde_json::{json, Value};
use url::Url;

static RE_DDG_TITLE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"<a class="result__url"[^>]*href="(?P<url>[^"]+)"[^>]*>"#)
        .expect("valid static regex")
});
static RE_DDG_SNIPPET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"<a class="result__snippet"[^>]*>(?P<snippet>.*?)</a>"#)
        .expect("valid static regex")
});

static RE_HTML_SCRIPT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)<script.*?</script>").expect("valid static regex"));
static RE_HTML_STYLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)<style.*?</style>").expect("valid static regex"));
static RE_HTML_HEAD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)<head.*?</head>").expect("valid static regex"));
static RE_HTML_NOSCRIPT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)<noscript.*?</noscript>").expect("valid static regex"));
static RE_HTML_BLOCK_BREAK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)</(?:p|div|section|article|blockquote|h[1-6]|tr|table|ul|ol)>|<br\s*/?>|<hr\s*/?>",
    )
    .expect("valid static regex")
});
static RE_HTML_LIST_ITEM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)<li\b[^>]*>").expect("valid static regex"));
static RE_HTML_TAGS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<[^>]+>").expect("valid static regex"));
static RE_WHITESPACE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[ \t]+").expect("valid static regex"));

pub fn clean_html_to_text(html: &str) -> String {
    let no_script = RE_HTML_SCRIPT.replace_all(html, "");
    let no_style = RE_HTML_STYLE.replace_all(&no_script, "");
    let no_head = RE_HTML_HEAD.replace_all(&no_style, "");
    let no_noscript = RE_HTML_NOSCRIPT.replace_all(&no_head, "");
    let with_blocks = RE_HTML_BLOCK_BREAK.replace_all(&no_noscript, "\n\n");
    let with_lists = RE_HTML_LIST_ITEM.replace_all(&with_blocks, "\n• ");
    let no_tags = RE_HTML_TAGS.replace_all(&with_lists, " ");
    let decoded = html_escape::decode_html_entities(&no_tags);

    let mut normalized = String::with_capacity(decoded.len());
    for line in decoded.lines() {
        let trimmed_line = RE_WHITESPACE.replace_all(line.trim(), " ");
        if !trimmed_line.is_empty() {
            normalized.push_str(&trimmed_line);
            normalized.push('\n');
        } else if !normalized.ends_with("\n\n") && !normalized.is_empty() {
            normalized.push('\n');
        }
    }
    normalized.trim().to_string()
}

static RE_HTML_IMG_SRC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)<img\b[^>]*?\b(?:src|data-src|data-original|data-lazy-src|data-high-res-src|data-full-url|data-url)=["'](?P<src>[^"']+)["']"#,
    )
    .expect("valid static regex")
});
static RE_HTML_SRCSET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)\bsrcset=["'](?P<srcset>[^"']+)["']"#).expect("valid static regex")
});
static RE_HTML_A_HREF_IMG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)<a\b[^>]*?\bhref=["'](?P<href>[^"']+\.(?:jpg|jpeg|png|webp)(?:\?[^"']*)?)["']"#,
    )
    .expect("valid static regex")
});
static RE_VISUAL_KEYWORDS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(?:foto|foto-foto|gambar|gambar-gambar|potret|pemandangan|citra|lukisan|ilustrasi|wallpaper|bagan|diagram|grafis|logo|logos|logonya|ikon|icon|icons|lambang|simbol|symbol|symbols|emblem|emblems|badge|badges|vektor|vector|vectors|bendera|flag|flags|photo|photos|picture|pictures|pic|pics|image|images|visual|visuals|illustration|wallpaper|png|jpg|jpeg|webp)\b",
    )
    .expect("valid static regex")
});
static RE_LOGO_KEYWORDS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(?:logo|logos|logonya|ikon|icon|icons|lambang|simbol|symbol|symbols|emblem|emblems|badge|badges|crest|coat of arms)\b",
    )
    .expect("valid static regex")
});
static RE_CONVERSATIONAL_PREFIX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^(?:(?:tolong|coba|mohon|bisakah|bisa|silakan|please|can you|could you|i want|i need|aku mau|saya mau)\s+)?(?:(?:berikan|carikan|tampilkan|tunjukkan|perlihatkan|lihatkan|kirimkan|cari|lihat|minta|give|show|find|search|send|get)\b(?:\s+(?:saya|aku|kami|me|us)\b)?)?(?:\s*(?:\d+|satu|dua|tiga|empat|lima|enam|tujuh|delapan|sembilan|sepuluh|beberapa|one|two|three|four|five|six|seven|eight|nine|ten|some|a|an)\b)?(?:\s*(?:buah|lembar|keping|ekor|item|items)\b)?(?:\s*(?:foto-foto|foto|gambar-gambar|gambar|potret|citra|logo|logos|logonya|ikon|icon|icons|photos?|pictures?|images?|pics?)\b)?(?:\s*(?:dari|tentang|mengenai|of|about)\b)?\s*",
    )
    .expect("valid static regex")
});
static RE_CONVERSATIONAL_SUFFIX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\s*[,.]?\s*\b(?:please|ya|dong|tolong|kan)\b\s*$").expect("valid static regex")
});
static RE_ID_MARKERS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(?:yang|dan|di|ke|dari|ini|itu|untuk|pada|adalah|dengan|foto|gambar|gunung|pemandangan|pantai|kota|indonesia|wisata|kuliner|pulau|sejarah|presiden|taman|danau|masjid|candi)\b",
    )
    .expect("valid static regex")
});

#[allow(dead_code)]
pub fn is_visual_search_query(query: &str) -> bool {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return false;
    }
    RE_VISUAL_KEYWORDS.is_match(trimmed)
}

#[allow(dead_code)]
pub fn is_logo_query(query: &str) -> bool {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return false;
    }
    RE_LOGO_KEYWORDS.is_match(trimmed)
}

#[allow(dead_code)]
pub fn extract_core_search_terms(query: &str) -> String {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let cleaned = RE_CONVERSATIONAL_PREFIX.replace(trimmed, "");
    let no_suffix = RE_CONVERSATIONAL_SUFFIX.replace(&cleaned, "");
    let mut res = no_suffix.trim();
    if let Some(stripped) = res
        .strip_prefix(':')
        .or_else(|| res.strip_prefix(','))
        .or_else(|| res.strip_prefix('-'))
    {
        res = stripped.trim();
    }
    if res.is_empty() {
        trimmed.to_string()
    } else {
        res.to_string()
    }
}

#[allow(dead_code)]
pub fn is_likely_indonesian(text: &str) -> bool {
    RE_ID_MARKERS.is_match(text)
}

#[allow(dead_code)]
pub fn is_valid_raster_image_url(url_str: &str) -> bool {
    sanitize_and_validate_raster_url(url_str).is_some()
}

#[allow(dead_code)]
pub fn sanitize_and_validate_raster_url(url_str: &str) -> Option<String> {
    let trimmed = url_str.trim();
    if trimmed.is_empty() {
        return None;
    }

    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("data:")
        || lower.starts_with("javascript:")
        || lower.starts_with("blob:")
        || lower.starts_with("file:")
    {
        return None;
    }

    let full_url_str = if trimmed.starts_with("//") {
        format!("https:{trimmed}")
    } else {
        trimmed.to_string()
    };

    let Ok(mut parsed) = Url::parse(&full_url_str) else {
        return None;
    };

    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return None;
    }

    let host = parsed.host_str()?;
    let host_lower = host.to_ascii_lowercase();

    if host_lower == "localhost"
        || host_lower == "127.0.0.1"
        || host_lower == "::1"
        || host_lower.ends_with(".local")
        || host_lower.ends_with(".internal")
        || host_lower.ends_with(".test")
        || host_lower.ends_with(".example")
        || host_lower.ends_with(".invalid")
    {
        return None;
    }

    if !host_lower.contains('.') && host_lower.parse::<std::net::IpAddr>().is_err() {
        return None;
    }

    const TRACKING_DOMAINS: &[&str] = &[
        "google-analytics.com",
        "googletagmanager.com",
        "doubleclick.net",
        "adnxs.com",
        "scorecardresearch.com",
        "quantserve.com",
        "clarity.ms",
        "pixel.wp.com",
        "stats.wp.com",
        "analytics.twitter.com",
        "bat.bing.com",
    ];
    for &td in TRACKING_DOMAINS {
        if host_lower == td || host_lower.ends_with(&format!(".{td}")) {
            return None;
        }
    }

    let path = parsed.path().to_ascii_lowercase();

    // DuckDuckGo proxy unwrapping: if URL is /iu/?u=<encoded_url>, unwrap and validate target
    if (host_lower == "duckduckgo.com" || host_lower.ends_with(".duckduckgo.com"))
        && path.starts_with("/iu/")
    {
        if let Some(target_u) = parsed
            .query_pairs()
            .find(|(k, _)| k == "u")
            .map(|(_, v)| v.to_string())
        {
            if let Ok(decoded) = urlencoding::decode(&target_u) {
                if let Some(valid_target) = sanitize_and_validate_raster_url(&decoded) {
                    return Some(valid_target);
                }
            }
        }
        return None;
    }

    if (host_lower == "duckduckgo.com" || host_lower.ends_with(".duckduckgo.com"))
        && path.starts_with("/t/")
    {
        return None;
    }

    if path.contains("anomaly-modal") || path.contains("challenge-form") {
        return None;
    }

    const BAD_PATH_KEYWORDS: &[&str] = &[
        "placeholder",
        "dummyimage",
        "placekitten",
        "placehold.it",
        "pixel.gif",
        "1x1.gif",
        "1x1.png",
        "spacer.gif",
        "blank.gif",
        "/beacon",
        "/telemetry",
        "transparent.png",
        "empty.png",
    ];
    for &kw in BAD_PATH_KEYWORDS {
        if path.contains(kw) || host_lower.contains(kw) {
            return None;
        }
    }

    if path.ends_with(".svg") || path.ends_with(".gif") || path.ends_with(".ico") {
        return None;
    }

    let query_pairs: Vec<(String, String)> = parsed
        .query_pairs()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();

    let mut retained_query: Vec<(String, String)> = Vec::new();
    let mut format_hint = None;

    for (k, v) in query_pairs {
        let k_lower = k.to_ascii_lowercase();
        let v_lower = v.to_ascii_lowercase();
        if k_lower.starts_with("utm_")
            || k_lower == "fbclid"
            || k_lower == "gclid"
            || k_lower == "msclkid"
            || k_lower == "ref"
            || k_lower == "ref_src"
            || k_lower == "_ga"
            || k_lower == "_gl"
            || k_lower == "mc_cid"
            || k_lower == "mc_eid"
        {
            continue;
        }
        if (k_lower == "format" || k_lower == "fm" || k_lower == "ext")
            && matches!(v_lower.as_str(), "jpg" | "jpeg" | "png" | "webp")
        {
            format_hint = Some(v_lower);
        }
        retained_query.push((k, v));
    }

    let has_raster_extension = path.ends_with(".jpg")
        || path.ends_with(".jpeg")
        || path.ends_with(".png")
        || path.ends_with(".webp");

    let is_unsplash = host_lower == "images.unsplash.com" || host_lower.ends_with(".unsplash.com");
    let is_wikimedia = (host_lower.contains("wikimedia.org")
        || host_lower.contains("wikipedia.org"))
        && (path.contains(".jpg")
            || path.contains(".jpeg")
            || path.contains(".png")
            || path.contains(".webp"));

    if !has_raster_extension && format_hint.is_none() && !is_unsplash && !is_wikimedia {
        return None;
    }

    if retained_query.is_empty() {
        parsed.set_query(None);
    } else {
        let mut serializer = url::form_urlencoded::Serializer::new(String::new());
        for (k, v) in &retained_query {
            serializer.append_pair(k, v);
        }
        parsed.set_query(Some(&serializer.finish()));
    }

    Some(parsed.to_string())
}

#[allow(dead_code)]
pub fn extract_raster_images_from_html(html: &str, base_url: Option<&str>) -> Vec<String> {
    let mut images = Vec::new();
    let mut seen = HashSet::new();

    let base_parsed = base_url.and_then(|b| Url::parse(b).ok());

    let mut resolve_and_add = |candidate: &str| {
        let full = if candidate.starts_with("http://")
            || candidate.starts_with("https://")
            || candidate.starts_with("//")
        {
            candidate.to_string()
        } else if let Some(ref base) = base_parsed {
            match base.join(candidate) {
                Ok(u) => u.to_string(),
                Err(_) => return,
            }
        } else {
            return;
        };

        if let Some(valid) = sanitize_and_validate_raster_url(&full) {
            if seen.insert(valid.clone()) {
                images.push(valid);
            }
        }
    };

    for cap in RE_HTML_IMG_SRC.captures_iter(html) {
        if let Some(src) = cap.name("src") {
            resolve_and_add(src.as_str());
        }
    }

    for cap in RE_HTML_SRCSET.captures_iter(html) {
        if let Some(srcset) = cap.name("srcset") {
            for entry in srcset.as_str().split(',') {
                let candidate = entry.split_whitespace().next().unwrap_or("");
                if !candidate.is_empty() {
                    resolve_and_add(candidate);
                }
            }
        }
    }

    for cap in RE_HTML_A_HREF_IMG.captures_iter(html) {
        if let Some(href) = cap.name("href") {
            resolve_and_add(href.as_str());
        }
    }

    images
}

#[allow(dead_code)]
pub fn format_verified_images_section(image_urls: &[String]) -> String {
    if image_urls.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "\n\n🖼️ **URL Foto/Gambar Raster Terverifikasi (Dapat Digunakan untuk Tool Multimedia)**:\n",
    );
    for url in image_urls {
        out.push_str(&format!("- {url}\n"));
    }
    out
}

#[allow(dead_code)]
pub fn format_no_images_guidance(query: &str) -> String {
    format!(
        "\n\nℹ️ **Catatan Media**: Tidak ditemukan berkas gambar raster langsung (.jpg, .png, .webp) yang valid dari hasil pencarian untuk \"{query}\". Berikan penjelasan deskriptif yang kaya dan informatif mengenai topik ini kepada pengguna dalam teks Markdown, dan hindari memanggil tool multimedia dengan URL fiktif/rekaan."
    )
}

pub fn sanitize_archive_entry_path(path: &str) -> String {
    use std::path::{Component, Path};

    let mut safe_components = Vec::new();
    for comp in Path::new(path).components() {
        match comp {
            Component::Normal(c) => {
                let part = c.to_string_lossy().trim().to_string();
                if !part.is_empty() {
                    safe_components.push(part);
                }
            }
            Component::CurDir
            | Component::ParentDir
            | Component::RootDir
            | Component::Prefix(_) => {
                // Skip traversal and root indicators
            }
        }
    }

    if safe_components.is_empty() {
        "file.txt".to_string()
    } else {
        safe_components.join("/")
    }
}

pub fn get_tools_definition() -> Value {
    json!([
        {
            "type": "function",
            "function": {
                "name": "web_search",
                "description": "Cari informasi terkini, fakta ensiklopedia, atau gambar/foto dari internet menggunakan mesin pencari. Jika pengguna meminta foto/gambar, mesin pencari akan menyertakan URL raster terverifikasi (.jpg, .jpeg, .png, .webp) yang siap digunakan untuk tool multimedia (send_photo, send_collage, send_slideshow).",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Kata kunci pencarian yang jelas dan spesifik"
                        }
                    },
                    "required": ["query"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "fetch_url",
                "description": "Ambil dan baca konten teks lengkap dari sebuah tautan URL web.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "url": {
                            "type": "string",
                            "description": "Tautan URL web yang ingin dibaca (diawali http:// atau https://)"
                        }
                    },
                    "required": ["url"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "create_quiz",
                "description": "Buat kuis interaktif native Telegram (mode kuis) dengan 2-10 pilihan ganda. Gunakan parameter preamble jika ingin menyajikan pengantar, konteks bacaan/studi kasus, atau potongan kode panjang sebelum kuis.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "question": {
                            "type": "string",
                            "description": "Pertanyaan kuis (maksimal 300 karakter)"
                        },
                        "options": {
                            "type": "array",
                            "items": {
                                "type": "string"
                            },
                            "description": "Daftar pilihan jawaban (2 sampai 10 opsi, masing-masing 1-100 karakter)"
                        },
                        "correct_option_id": {
                            "type": "integer",
                            "description": "Indeks jawaban yang benar (0-based, dimulai dari 0)"
                        },
                        "explanation": {
                            "type": "string",
                            "description": "Penjelasan saat jawaban dibuka (opsional, maksimal 200 karakter, maksimal 2 line breaks)"
                        },
                        "preamble": {
                            "type": "string",
                            "description": "Pesan pengantar atau materi/studi kasus/kode panjang sebelum kuis (opsional, terformat Markdown)"
                        }
                    },
                    "required": ["question", "options", "correct_option_id"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "send_photo",
                "description": "Kirim sebuah foto atau gambar langsung ke obrolan Telegram via URL gambar publik raster (.jpg, .jpeg, .png, .webp).",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "url": {
                            "type": "string",
                            "description": "URL langsung file gambar raster publik (.jpg, .png, .webp). Hindari tautan Wikimedia yang memblokir bot (HTTP 403) dan jangan gunakan SVG."
                        },
                        "caption": {
                            "type": "string",
                            "description": "Keterangan atau teks pengantar untuk foto (opsional, mendukung Markdown standar)"
                        }
                    },
                    "required": ["url"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "send_collage",
                "description": "Kirim album kolase foto (2 sampai 10 foto) ke obrolan Telegram sebagai native sendMediaGroup.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "urls": {
                            "type": "array",
                            "items": {
                                "type": "string"
                            },
                            "description": "Daftar URL gambar langsung (minimal 2, maksimal 10 foto raster)"
                        },
                        "caption": {
                            "type": "string",
                            "description": "Keterangan atau teks pengantar untuk seluruh album kolase foto (opsional)"
                        }
                    },
                    "required": ["urls"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "send_slideshow",
                "description": "Kirim tayangan slide foto interaktif (native carousel <tg-slideshow>) ke obrolan Telegram yang dapat digeser atau dibolak-balik fotonya langsung melalui kontrol navigasi bawaan Telegram pada gambar.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "urls": {
                            "type": "array",
                            "items": {
                                "type": "string"
                            },
                            "description": "Daftar URL gambar langsung untuk setiap slide carousel"
                        },
                        "caption": {
                            "type": "string",
                            "description": "Keterangan atau deskripsi tayangan slide (opsional)"
                        }
                    },
                    "required": ["urls"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "send_audio",
                "description": "Kirim berkas audio musik native ke obrolan Telegram via URL.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "url": {
                            "type": "string",
                            "description": "URL langsung berkas audio (misal .mp3, .m4a)"
                        },
                        "title": {
                            "type": "string",
                            "description": "Judul lagu atau audio (opsional)"
                        },
                        "performer": {
                            "type": "string",
                            "description": "Nama penyanyi atau pencipta audio (opsional)"
                        },
                        "caption": {
                            "type": "string",
                            "description": "Keterangan atau deskripsi audio (opsional)"
                        }
                    },
                    "required": ["url"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "send_voice",
                "description": "Kirim rekaman suara (voice note) dengan visual waveform native ke obrolan Telegram via URL audio (.ogg/.mp3).",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "url": {
                            "type": "string",
                            "description": "URL langsung berkas suara (.ogg atau .mp3)"
                        },
                        "caption": {
                            "type": "string",
                            "description": "Keterangan pesan suara (opsional)"
                        }
                    },
                    "required": ["url"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "send_location",
                "description": "Kirim koordinat lokasi geografis native Telegram berupa pin peta interaktif.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "latitude": {
                            "type": "number",
                            "description": "Garis lintang lokasi geografis (antara -90.0 dan 90.0)"
                        },
                        "longitude": {
                            "type": "number",
                            "description": "Garis bujur lokasi geografis (antara -180.0 dan 180.0)"
                        },
                        "title": {
                            "type": "string",
                            "description": "Nama tempat atau label lokasi (opsional)"
                        }
                    },
                    "required": ["latitude", "longitude"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "send_document",
                "description": "Kirim berkas dokumen umum (.pdf, .zip, .docx, spreadsheet, dll.) ke obrolan Telegram via URL berkas.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "url": {
                            "type": "string",
                            "description": "URL langsung berkas dokumen yang akan dikirim"
                        },
                        "file_name": {
                            "type": "string",
                            "description": "Nama file berkas beserta ekstensinya, misalnya 'laporan.pdf' (opsional)"
                        },
                        "caption": {
                            "type": "string",
                            "description": "Keterangan ringkas dokumen (opsional)"
                        }
                    },
                    "required": ["url"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "create_document",
                "description": "Buat berkas/dokumen teks, dokumen PDF (.pdf), berkas HTML, source code, data CSV/JSON/YAML, vektor SVG, atau arsip ZIP langsung dan kirimkan ke chat Telegram.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "filename": {
                            "type": "string",
                            "description": "Nama file berkas beserta ekstensinya (contoh: laporan.pdf, script.py, index.html, data.csv)"
                        },
                        "content": {
                            "type": "string",
                            "description": "Isi/teks atau konten dokumen yang akan dibuat (untuk berkas teks/code/markdown/HTML, atau representasi konten PDF/SVG)"
                        },
                        "caption": {
                            "type": "string",
                            "description": "Keterangan ringkas dokumen (opsional)"
                        },
                        "as_zip": {
                            "type": "boolean",
                            "description": "True jika berkas harus dikompresi ke dalam ZIP"
                        }
                    },
                    "required": ["filename", "content"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "create_archive",
                "description": "Buat paket arsip ZIP multi-file langsung yang memuat banyak berkas/script/dokumen ke dalam satu berkas .zip dan kirimkan ke chat Telegram.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "filename": {
                            "type": "string",
                            "description": "Nama file arsip zip (contoh: cpa-toolkit.zip, project-bundle.zip)"
                        },
                        "files": {
                            "type": "array",
                            "description": "Daftar berkas yang dimasukkan ke dalam arsip zip",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "filename": {
                                        "type": "string",
                                        "description": "Nama berkas di dalam zip (contoh: cpa.sh, README.md, config.json)"
                                    },
                                    "content": {
                                        "type": "string",
                                        "description": "Konten/isi berkas"
                                    }
                                },
                                "required": ["filename", "content"]
                            }
                        },
                        "caption": {
                            "type": "string",
                            "description": "Keterangan ringkas arsip zip (opsional)"
                        }
                    },
                    "required": ["filename", "files"]
                }
            }
        }
    ])
}

#[path = "tools/search.rs"]
mod search;

#[allow(unused_imports)]
pub(crate) use search::search_exa_mcp;
pub use search::{
    execute_web_search, get_brave_key, get_configured_mcp_url, get_exa_key,
    get_search_engine_status, get_tavily_key,
};

const MAX_FETCH_HTML_BYTES: usize = 2 * 1024 * 1024;
const MAX_FETCH_REDIRECTS: usize = 5;

pub async fn fetch_web_content(url: &str) -> Result<String, String> {
    let mut current_url_str = url.trim().to_string();
    if !current_url_str.starts_with("http://") && !current_url_str.starts_with("https://") {
        return Err("URL harus diawali dengan http:// atau https://".to_string());
    }

    let mut redirect_count = 0;
    let resp = loop {
        let resolved = crate::bot::url_policy::resolve_download_url(&current_url_str).await?;

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .resolve(&resolved.host, resolved.address)
            .build()
            .map_err(|e| format!("Gagal menginisialisasi client HTTP: {e}"))?;

        let response = client
            .get(resolved.url.clone())
            .header(
                USER_AGENT,
                concat!(
                    "xiao/",
                    env!("CARGO_PKG_VERSION"),
                    " (Telegram Bot Assistant)"
                ),
            )
            .header(
                ACCEPT,
                "text/html,application/xhtml+xml,application/xml;q=0.9,text/plain;q=0.8,*/*;q=0.5",
            )
            .header(ACCEPT_LANGUAGE, "id,en-US;q=0.9,en;q=0.8")
            .send()
            .await
            .map_err(|e| format!("Gagal mengunduh halaman web: {e}"))?;

        let status = response.status();
        if status.is_redirection() {
            if redirect_count >= MAX_FETCH_REDIRECTS {
                return Err("Terlalu banyak pengalihan (redirect loop).".to_string());
            }
            redirect_count += 1;
            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|h| h.to_str().ok())
                .ok_or_else(|| "Pengalihan tanpa header Location yang valid".to_string())?;

            let next_url = resolved
                .url
                .join(location)
                .map_err(|e| format!("URL pengalihan tidak valid: {e}"))?;

            current_url_str = next_url.to_string();
            continue;
        }

        if !status.is_success() {
            return Err(format!("Halaman web mengembalikan status HTTP {status}"));
        }

        break response;
    };

    if resp
        .content_length()
        .is_some_and(|length| length > MAX_FETCH_HTML_BYTES as u64)
    {
        return Err("Ukuran konten web melebihi batas aman 2 MiB.".to_string());
    }

    let mut stream = resp.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk_res) = stream.next().await {
        let chunk = chunk_res.map_err(|e| format!("Gagal membaca stream web: {e}"))?;
        if bytes.len().saturating_add(chunk.len()) > MAX_FETCH_HTML_BYTES {
            return Err("Ukuran konten web melebihi batas aman 2 MiB.".to_string());
        }
        bytes.extend_from_slice(&chunk);
    }

    let html = String::from_utf8_lossy(&bytes);
    let mut cleaned = clean_html_to_text(&html);

    if cleaned.is_empty() {
        return Err("Halaman web tidak menghasilkan konten teks yang dapat dibaca.".to_string());
    }

    let extracted_images = extract_raster_images_from_html(&html, Some(&current_url_str));
    if !extracted_images.is_empty() {
        let max_imgs = extracted_images.into_iter().take(6).collect::<Vec<_>>();
        cleaned.push_str(&format_verified_images_section(&max_imgs));
    }

    let max_len = 8000;
    if cleaned.chars().nth(max_len).is_some() {
        let truncated: String = cleaned.chars().take(max_len).collect();
        Ok(format!(
            "{}\n\n[...Konten web dipotong karena melebihi batas panjang teks xiao...]",
            truncated
        ))
    } else {
        Ok(cleaned)
    }
}

static RE_TOOL_PREAMBLE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^(?:baik|tentu|oke|siap|halo|yes|sure|okay|alright|fine)?\s*[,.:!?-]?\s*(?:tunggu|sebentar|biar|mari|tolong|saya|aku|kami|kita|akan|let me|i will|i'll|allow me|searching|looking up|checking)\b.*?\b(?:cari|carikan|mencari|pencarian|cek|mengecek|pengecekan|periksa|memeriksa|lihat|search|searching|check|checking|look up|looking up|fetch|retrieve|find)\b",
    )
    .expect("valid static regex")
});

const DEFINITIVE_PREAMBLE_PREFIXES: &[&str] = &[
    "tunggu sebentar",
    "sebentar ya",
    "sebentar, saya",
    "sebentar saya",
    "sebentar...",
    "tunggu ya",
    "tunggu sebentar ya",
    "saya akan mencari",
    "saya sedang mencari",
    "saya akan carikan",
    "saya carikan",
    "akan saya carikan",
    "akan saya cari",
    "biar saya carikan",
    "biar saya cari",
    "mari saya cari",
    "mari saya carikan",
    "mari kita cari",
    "saya cek dulu",
    "saya cek",
    "biar saya cek",
    "saya periksa dulu",
    "saya periksa",
    "biar saya periksa",
    "let me search",
    "let me check",
    "let me look up",
    "let me find",
    "i will search",
    "i'll search",
    "i will look up",
    "i'll look up",
    "i will check",
    "i'll check",
    "searching for",
    "looking up",
    "checking",
];

pub fn is_tool_calling_preamble(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return false;
    }
    let lower = trimmed.to_lowercase();
    if lower.chars().count() > 140 {
        return false;
    }
    if RE_TOOL_PREAMBLE.is_match(&lower) {
        return true;
    }
    DEFINITIVE_PREAMBLE_PREFIXES
        .iter()
        .any(|prefix| lower.starts_with(prefix))
}

pub fn is_suppressed_tool_preamble_stream(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return true;
    }
    if is_tool_calling_preamble(trimmed) {
        return true;
    }
    let lower = trimmed.to_lowercase();
    DEFINITIVE_PREAMBLE_PREFIXES
        .iter()
        .any(|prefix| prefix.starts_with(&lower))
}

fn deserialize_quiz_options<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(serde::Deserialize)]
    #[serde(untagged)]
    enum OptionItem {
        Str(String),
        Obj { text: String },
    }

    let items = Vec::<OptionItem>::deserialize(deserializer)?;
    Ok(items
        .into_iter()
        .map(|item| match item {
            OptionItem::Str(s) => s,
            OptionItem::Obj { text } => text,
        })
        .collect())
}

fn deserialize_flexible_opt_bool<'de, D>(deserializer: D) -> Result<Option<bool>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(serde::Deserialize)]
    #[serde(untagged)]
    enum BoolHelper {
        Bool(bool),
        Str(String),
        Num(i64),
    }

    match Option::<BoolHelper>::deserialize(deserializer)? {
        None => Ok(None),
        Some(BoolHelper::Bool(b)) => Ok(Some(b)),
        Some(BoolHelper::Str(s)) => {
            let s_clean = s.trim().to_ascii_lowercase();
            if s_clean == "true" || s_clean == "1" || s_clean == "yes" {
                Ok(Some(true))
            } else if s_clean == "false" || s_clean == "0" || s_clean == "no" {
                Ok(Some(false))
            } else {
                Err(serde::de::Error::custom(format!(
                    "invalid boolean value: {s}"
                )))
            }
        }
        Some(BoolHelper::Num(n)) => Ok(Some(n != 0)),
    }
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize, PartialEq, Eq)]
pub struct CreateQuizArgs {
    pub question: String,
    #[serde(deserialize_with = "deserialize_quiz_options")]
    pub options: Vec<String>,
    #[serde(
        default,
        deserialize_with = "crate::bot::models::deserialize_flexible_opt_i32"
    )]
    pub correct_option_id: Option<i32>,
    #[serde(default)]
    pub explanation: Option<String>,
    #[serde(default)]
    pub preamble: Option<String>,
    #[serde(default, deserialize_with = "deserialize_flexible_opt_bool")]
    pub is_anonymous: Option<bool>,
}

impl CreateQuizArgs {
    pub fn sanitize(&mut self) {
        self.question = self.question.trim().to_string();
        if self.question.chars().count() > crate::bot::models::QUIZ_MAX_QUESTION_CHARS {
            self.question = crate::util::truncate_chars(
                &self.question,
                crate::bot::models::QUIZ_MAX_QUESTION_CHARS,
            )
            .to_string();
        }

        if self.options.len() > crate::bot::models::QUIZ_MAX_OPTIONS {
            self.options.truncate(crate::bot::models::QUIZ_MAX_OPTIONS);
        }
        let mut seen = std::collections::HashSet::new();
        let mut dup_counter = 1;
        for opt in &mut self.options {
            let mut trimmed = opt.trim().to_string();
            if trimmed.chars().count() > crate::bot::models::QUIZ_MAX_OPTION_CHARS {
                trimmed = crate::util::truncate_chars(
                    &trimmed,
                    crate::bot::models::QUIZ_MAX_OPTION_CHARS,
                )
                .to_string();
            }
            if seen.contains(&trimmed) {
                let candidate = loop {
                    dup_counter += 1;
                    let suffix = format!(" ({dup_counter})");
                    let max_base_len = crate::bot::models::QUIZ_MAX_OPTION_CHARS
                        .saturating_sub(suffix.chars().count());
                    let base = crate::util::truncate_chars(&trimmed, max_base_len);
                    let cand = format!("{base}{suffix}");
                    if !seen.contains(&cand) {
                        break cand;
                    }
                };
                seen.insert(candidate.clone());
                *opt = candidate;
            } else {
                seen.insert(trimmed.clone());
                *opt = trimmed;
            }
        }

        if let Some(correct_id) = self.correct_option_id {
            if !self.options.is_empty() {
                if correct_id < 0 {
                    self.correct_option_id = Some(0);
                } else if correct_id as usize >= self.options.len() {
                    self.correct_option_id = Some(self.options.len().saturating_sub(1) as i32);
                }
            }
        }

        if let Some(exp) = &mut self.explanation {
            let normalized = exp.replace("\r\n", "\n").replace('\r', "\n");
            let trimmed = normalized.trim().to_string();
            let mut line_break_count = 0;
            let mut sanitized_exp = String::with_capacity(trimmed.len());
            for ch in trimmed.chars() {
                if ch == '\n' {
                    line_break_count += 1;
                    if line_break_count <= crate::bot::models::QUIZ_MAX_EXPLANATION_LINE_BREAKS {
                        sanitized_exp.push(ch);
                    } else {
                        sanitized_exp.push(' ');
                    }
                } else {
                    sanitized_exp.push(ch);
                }
            }
            let sanitized_trimmed = sanitized_exp.trim();
            if sanitized_trimmed.is_empty() {
                self.explanation = None;
            } else {
                let mut final_exp = sanitized_trimmed.to_string();
                if final_exp.chars().count() > crate::bot::models::QUIZ_MAX_EXPLANATION_CHARS {
                    final_exp = crate::util::truncate_chars(
                        &final_exp,
                        crate::bot::models::QUIZ_MAX_EXPLANATION_CHARS,
                    )
                    .to_string();
                }
                let final_trimmed = final_exp.trim().to_string();
                if final_trimmed.is_empty() {
                    self.explanation = None;
                } else {
                    *exp = final_trimmed;
                }
            }
        }

        if let Some(pre) = &mut self.preamble {
            *pre = pre.trim().to_string();
            if pre.is_empty() {
                self.preamble = None;
            } else if pre.chars().count() > crate::bot::models::RICH_MESSAGE_MAX_TEXT_CHARS {
                *pre = crate::util::truncate_chars(
                    pre,
                    crate::bot::models::RICH_MESSAGE_MAX_TEXT_CHARS,
                )
                .to_string();
            }
        }
    }

    pub fn validate(&self) -> Result<i32, String> {
        let correct_id = self
            .correct_option_id
            .ok_or_else(|| "Quiz requires correct_option_id".to_string())?;
        let input_options: Vec<crate::bot::models::InputPollOption> = self
            .options
            .iter()
            .map(|opt| crate::bot::models::InputPollOption::new(opt.as_str()))
            .collect();
        crate::bot::models::validate_quiz(
            &self.question,
            &input_options,
            correct_id,
            self.explanation.as_deref(),
        )?;
        Ok(correct_id)
    }
}

#[allow(dead_code)]
pub const MULTIMEDIA_CAPTION_MAX_CHARS: usize = 1024;
#[allow(dead_code)]
pub const CAPTION_MAX_CHARS: usize = MULTIMEDIA_CAPTION_MAX_CHARS;

#[allow(dead_code)]
pub fn deserialize_flexible_f64<'de, D>(deserializer: D) -> Result<f64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(serde::Deserialize)]
    #[serde(untagged)]
    enum F64Helper {
        Num(f64),
        Str(String),
    }

    let val = match F64Helper::deserialize(deserializer)? {
        F64Helper::Num(n) => n,
        F64Helper::Str(s) => s.trim().parse::<f64>().map_err(serde::de::Error::custom)?,
    };

    if val.is_finite() {
        Ok(val)
    } else {
        Err(serde::de::Error::custom(
            "coordinate must be a finite number",
        ))
    }
}

pub fn sanitize_multimedia_caption(caption: &mut Option<String>) {
    if let Some(c) = caption {
        let trimmed = c.trim().to_string();
        if trimmed.is_empty() {
            *caption = None;
        } else if trimmed.chars().count() > MULTIMEDIA_CAPTION_MAX_CHARS {
            *caption = Some(
                crate::util::truncate_chars(&trimmed, MULTIMEDIA_CAPTION_MAX_CHARS).to_string(),
            );
        } else {
            *caption = Some(trimmed);
        }
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize, PartialEq, Eq)]
pub struct SendPhotoArgs {
    pub url: String,
    #[serde(default)]
    pub caption: Option<String>,
}

#[allow(dead_code)]
impl SendPhotoArgs {
    pub fn sanitize(&mut self) {
        self.url = self.url.trim().to_string();
        sanitize_multimedia_caption(&mut self.caption);
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.url.trim().is_empty() {
            return Err("URL foto tidak boleh kosong".to_string());
        }
        Ok(())
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize, PartialEq, Eq)]
pub struct SendCollageArgs {
    pub urls: Vec<String>,
    #[serde(default)]
    pub caption: Option<String>,
}

#[allow(dead_code)]
impl SendCollageArgs {
    pub fn sanitize(&mut self) {
        self.urls.retain(|u| !u.trim().is_empty());
        for u in &mut self.urls {
            *u = u.trim().to_string();
        }
        if self.urls.len() > 10 {
            self.urls.truncate(10);
        }
        sanitize_multimedia_caption(&mut self.caption);
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.urls.len() < 2 {
            return Err(format!(
                "Kolase foto memerlukan minimal 2 foto (maksimal 10), ditemukan {}",
                self.urls.len()
            ));
        }
        if self.urls.len() > 10 {
            return Err(format!(
                "Kolase foto maksimal 10 foto, ditemukan {}",
                self.urls.len()
            ));
        }
        for (i, url) in self.urls.iter().enumerate() {
            if url.trim().is_empty() {
                return Err(format!("URL foto kolase ke-{} tidak boleh kosong", i + 1));
            }
        }
        Ok(())
    }

    pub fn to_input_media(&self) -> Vec<crate::bot::models::InputMedia> {
        self.urls
            .iter()
            .enumerate()
            .map(|(idx, url)| {
                let caption = if idx == 0 { self.caption.clone() } else { None };
                crate::bot::models::InputMedia::Photo {
                    media: url.clone(),
                    caption,
                    parse_mode: Some("Markdown".to_string()),
                    show_caption_above_media: None,
                    has_spoiler: None,
                }
            })
            .collect()
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize, PartialEq, Eq)]
pub struct SendSlideshowArgs {
    pub urls: Vec<String>,
    #[serde(default)]
    pub caption: Option<String>,
}

#[allow(dead_code)]
impl SendSlideshowArgs {
    pub fn sanitize(&mut self) {
        self.urls.retain(|u| !u.trim().is_empty());
        for u in &mut self.urls {
            *u = u.trim().to_string();
        }
        sanitize_multimedia_caption(&mut self.caption);
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.urls.is_empty() {
            return Err("Slideshow memerlukan minimal 1 URL slide gambar".to_string());
        }
        for (i, url) in self.urls.iter().enumerate() {
            if url.trim().is_empty() {
                return Err(format!("URL slide ke-{} tidak boleh kosong", i + 1));
            }
        }
        Ok(())
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize, PartialEq, Eq)]
pub struct SendAudioArgs {
    pub url: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub performer: Option<String>,
    #[serde(default)]
    pub caption: Option<String>,
}

#[allow(dead_code)]
impl SendAudioArgs {
    pub fn sanitize(&mut self) {
        self.url = self.url.trim().to_string();
        if let Some(title) = &mut self.title {
            let trimmed = title.trim().to_string();
            self.title = if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            };
        }
        if let Some(performer) = &mut self.performer {
            let trimmed = performer.trim().to_string();
            self.performer = if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            };
        }
        sanitize_multimedia_caption(&mut self.caption);
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.url.trim().is_empty() {
            return Err("URL audio tidak boleh kosong".to_string());
        }
        Ok(())
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize, PartialEq, Eq)]
pub struct SendVoiceArgs {
    pub url: String,
    #[serde(default)]
    pub caption: Option<String>,
}

#[allow(dead_code)]
impl SendVoiceArgs {
    pub fn sanitize(&mut self) {
        self.url = self.url.trim().to_string();
        sanitize_multimedia_caption(&mut self.caption);
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.url.trim().is_empty() {
            return Err("URL voice note tidak boleh kosong".to_string());
        }
        Ok(())
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize, PartialEq)]
pub struct SendLocationArgs {
    #[serde(deserialize_with = "deserialize_flexible_f64")]
    pub latitude: f64,
    #[serde(deserialize_with = "deserialize_flexible_f64")]
    pub longitude: f64,
    #[serde(default)]
    pub title: Option<String>,
}

#[allow(dead_code)]
impl SendLocationArgs {
    pub fn sanitize(&mut self) {
        if let Some(title) = &mut self.title {
            let trimmed = title.trim().to_string();
            self.title = if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            };
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if !self.latitude.is_finite() {
            return Err(
                "Garis lintang (latitude) harus berupa angka berhingga (finite)".to_string(),
            );
        }
        if !self.longitude.is_finite() {
            return Err(
                "Garis bujur (longitude) harus berupa angka berhingga (finite)".to_string(),
            );
        }
        if !(-90.0..=90.0).contains(&self.latitude) {
            return Err(format!(
                "Garis lintang (latitude) harus berada dalam rentang -90.0 hingga 90.0, ditemukan {}",
                self.latitude
            ));
        }
        if !(-180.0..=180.0).contains(&self.longitude) {
            return Err(format!(
                "Garis bujur (longitude) harus berada dalam rentang -180.0 hingga 180.0, ditemukan {}",
                self.longitude
            ));
        }
        Ok(())
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize, PartialEq, Eq)]
pub struct SendDocumentArgs {
    pub url: String,
    #[serde(default)]
    pub file_name: Option<String>,
    #[serde(default)]
    pub caption: Option<String>,
}

#[allow(dead_code)]
impl SendDocumentArgs {
    pub fn sanitize(&mut self) {
        self.url = self.url.trim().to_string();
        if let Some(file_name) = &mut self.file_name {
            let trimmed = file_name.trim().to_string();
            self.file_name = if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            };
        }
        sanitize_multimedia_caption(&mut self.caption);
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.url.trim().is_empty() {
            return Err("URL dokumen tidak boleh kosong".to_string());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize, PartialEq, Eq)]
pub struct CreateDocumentArgs {
    pub filename: String,
    pub content: String,
    #[serde(default)]
    pub caption: Option<String>,
    #[serde(default)]
    pub as_zip: bool,
}

#[allow(dead_code)]
impl CreateDocumentArgs {
    pub fn sanitize(&mut self) {
        let mut clean_name = self
            .filename
            .replace("../", "")
            .replace("..\\", "")
            .replace(['/', '\\'], "")
            .trim()
            .to_string();
        if clean_name.is_empty() {
            clean_name = "document.txt".to_string();
        }
        self.filename = clean_name;
        sanitize_multimedia_caption(&mut self.caption);
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.content.is_empty() {
            return Err("Konten dokumen tidak boleh kosong".to_string());
        }
        if self.content.len() > 20 * 1024 * 1024 {
            return Err("Ukuran konten melebihi batas 20MB".to_string());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize, PartialEq, Eq)]
pub struct ArchiveFileEntry {
    pub filename: String,
    pub content: String,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize, PartialEq, Eq)]
pub struct CreateArchiveArgs {
    pub filename: String,
    pub files: Vec<ArchiveFileEntry>,
    #[serde(default)]
    pub caption: Option<String>,
}

#[allow(dead_code)]
impl CreateArchiveArgs {
    pub fn sanitize(&mut self) {
        let mut clean_name = self
            .filename
            .replace("../", "")
            .replace("..\\", "")
            .replace(['/', '\\'], "")
            .trim()
            .to_string();
        if clean_name.is_empty() {
            clean_name = "archive.zip".to_string();
        } else if !clean_name.to_ascii_lowercase().ends_with(".zip") {
            clean_name = format!("{clean_name}.zip");
        }
        self.filename = clean_name;

        for entry in &mut self.files {
            entry.filename = sanitize_archive_entry_path(&entry.filename);
        }

        sanitize_multimedia_caption(&mut self.caption);
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.files.is_empty() {
            return Err("Daftar file dalam arsip tidak boleh kosong".to_string());
        }
        if self.files.len() > 100 {
            return Err("Jumlah file dalam arsip melebihi batas 100 file".to_string());
        }
        let total_size: usize = self.files.iter().map(|f| f.content.len()).sum();
        if total_size > 20 * 1024 * 1024 {
            return Err("Total ukuran konten arsip melebihi batas 20MB".to_string());
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "tools/tests.rs"]
mod tests;
