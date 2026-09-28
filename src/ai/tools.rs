use std::collections::HashSet;
use std::env;
use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;
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

/// Normalizes a model-supplied path for an entry inside a generated ZIP.
///
/// Parsing is purely textual and identical on every OS. The previous version
/// relied on `std::path`, which on Linux treats `\` as an ordinary character,
/// so `..\..\evil.bat` survived intact and could escape the extraction folder
/// when the archive was later unpacked on Windows.
pub fn sanitize_archive_entry_path(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    let safe_components: Vec<String> = normalized
        .split('/')
        .map(|segment| {
            segment
                .chars()
                .filter(|ch| !ch.is_control())
                .collect::<String>()
                .trim()
                .to_string()
        })
        .filter(|segment| {
            !segment.is_empty()
                && segment != "."
                && segment != ".."
                // Windows drive prefixes such as `C:`.
                && !(segment.len() == 2 && segment.ends_with(':'))
        })
        .map(|segment| segment.replace(':', "_"))
        .collect();

    if safe_components.is_empty() {
        "file.txt".to_string()
    } else {
        safe_components.join("/")
    }
}

/// Tools usable when answering in guest mode (Bot API 10.0): the reply is a
/// single inline message in someone else's chat, which cannot carry newly
/// uploaded files, quizzes or media albums, so only read-only research tools
/// are offered.
pub const GUEST_MODE_TOOLS: &[&str] = &["web_search", "fetch_url"];

/// Tool definitions for a generation: all tools normally, only
/// [`GUEST_MODE_TOOLS`] in guest mode.
pub fn tools_definition_for(guest_mode: bool) -> Value {
    let all = get_tools_definition();
    if !guest_mode {
        return all;
    }
    let filtered: Vec<Value> = all
        .as_array()
        .map(|tools| {
            tools
                .iter()
                .filter(|tool| {
                    tool.pointer("/function/name")
                        .and_then(Value::as_str)
                        .is_some_and(|name| GUEST_MODE_TOOLS.contains(&name))
                })
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    Value::Array(filtered)
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
                "description": "Buat kuis interaktif native Telegram (mode kuis) dengan 2-12 pilihan ganda. Kuis boleh punya lebih dari satu jawaban benar, bergambar (gambar soal dan/atau gambar per pilihan), dan pilihannya boleh diacak. Gunakan parameter preamble jika ingin menyajikan pengantar, konteks bacaan/studi kasus, atau potongan kode panjang sebelum kuis.",
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
                            "description": "Daftar pilihan jawaban (2 sampai 12 opsi, masing-masing 1-100 karakter)"
                        },
                        "correct_option_ids": {
                            "type": "array",
                            "items": {
                                "type": "integer"
                            },
                            "description": "Indeks semua jawaban yang benar (0-based). Isi lebih dari satu indeks jika soal memang punya beberapa jawaban benar; pemain lalu boleh memilih beberapa jawaban."
                        },
                        "correct_option_id": {
                            "type": "integer",
                            "description": "Bentuk lama untuk satu jawaban benar (0-based); utamakan correct_option_ids"
                        },
                        "shuffle_options": {
                            "type": "boolean",
                            "description": "true agar urutan pilihan diacak untuk tiap pemain"
                        },
                        "image_url": {
                            "type": "string",
                            "description": "URL gambar https (.jpg/.png/.webp) untuk kuis bergambar, tampil bersama soal. Cari dulu URL gambar terverifikasi dengan web_search; jangan mengarang URL."
                        },
                        "option_image_urls": {
                            "type": "array",
                            "items": {
                                "type": "string"
                            },
                            "description": "URL gambar https per pilihan, urutannya sama dengan options; isi string kosong untuk pilihan tanpa gambar"
                        },
                        "description": {
                            "type": "string",
                            "description": "Keterangan singkat yang tampil di bawah soal (opsional, maksimal 1024 karakter), misalnya petunjuk atau konteks soal"
                        },
                        "explanation": {
                            "type": "string",
                            "description": "Penjelasan saat jawaban dibuka (opsional, maksimal 200 karakter, maksimal 2 line breaks)"
                        },
                        "explanation_image_url": {
                            "type": "string",
                            "description": "URL gambar https yang menyertai penjelasan jawaban (opsional); cari dulu dengan web_search, jangan mengarang URL"
                        },
                        "allows_revoting": {
                            "type": "boolean",
                            "description": "true agar pemain boleh mengubah jawabannya (bawaan kuis: tidak boleh)"
                        },
                        "open_period": {
                            "type": "integer",
                            "description": "Lama kuis dibuka dalam detik (5 sampai 2628000); setelah itu kuis ditutup otomatis. Jangan diisi bila kuis tidak perlu batas waktu"
                        },
                        "hide_results_until_closes": {
                            "type": "boolean",
                            "description": "true agar hasil pilihan semua pemain baru terlihat setelah kuis ditutup; wajib disertai open_period"
                        },
                        "preamble": {
                            "type": "string",
                            "description": "Pesan pengantar atau materi/studi kasus/kode panjang sebelum kuis (opsional, terformat Markdown)"
                        }
                    },
                    "required": ["question", "options", "correct_option_ids"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "send_live_photo",
                "description": "Kirim live photo (foto diam beserta klip gerak pendek, seperti Live Photo di iPhone) langsung ke obrolan Telegram. Butuh URL video MP4 berdurasi maksimal 10 detik dan maksimal 10 MB, serta URL foto diam JPG/PNG/WEBP. Gunakan hanya URL yang diberikan pengguna atau ditemukan lewat web_search; jangan mengarang URL. Tool ini mengirim langsung, jadi jangan sematkan tag media untuknya.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "video_url": {
                            "type": "string",
                            "description": "URL https video MP4 (maksimal 10 detik, maksimal 10 MB)"
                        },
                        "photo_url": {
                            "type": "string",
                            "description": "URL https foto diam (JPG, PNG, atau WEBP) yang menjadi sampul live photo"
                        },
                        "caption": {
                            "type": "string",
                            "description": "Keterangan singkat (opsional, maksimal 1024 karakter, teks biasa)"
                        }
                    },
                    "required": ["video_url", "photo_url"]
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

/// Fetches an untrusted web page through the shared SSRF-safe fetcher
/// (DNS pinning, per-hop policy checks, bounded body) and returns the HTML
/// together with the final URL after redirects.
pub(crate) async fn fetch_public_html(
    url: &str,
    timeout: Duration,
    max_bytes: usize,
) -> Result<(String, String), String> {
    let fetched = crate::bot::url_policy::fetch_public_url(
        url.trim(),
        &crate::bot::url_policy::PublicFetchOptions {
            timeout,
            max_bytes,
            user_agent: concat!(
                "xiao/",
                env!("CARGO_PKG_VERSION"),
                " (Telegram Bot Assistant)"
            ),
            accept:
                "text/html,application/xhtml+xml,application/xml;q=0.9,text/plain;q=0.8,*/*;q=0.5",
        },
    )
    .await?;
    Ok((
        String::from_utf8_lossy(&fetched.bytes).into_owned(),
        fetched.final_url.to_string(),
    ))
}

pub async fn fetch_web_content(url: &str) -> Result<String, String> {
    let url = url.trim();
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err("URL harus diawali dengan http:// atau https://".to_string());
    }

    let (html, final_url) = fetch_public_html(url, Duration::from_secs(15), MAX_FETCH_HTML_BYTES)
        .await
        .map_err(|error| format!("Gagal mengunduh halaman web: {error}"))?;
    let mut cleaned = clean_html_to_text(&html);

    if cleaned.is_empty() {
        return Err("Halaman web tidak menghasilkan konten teks yang dapat dibaca.".to_string());
    }

    let extracted_images = extract_raster_images_from_html(&html, Some(&final_url));
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

#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize, PartialEq, Eq)]
pub struct CreateQuizArgs {
    pub question: String,
    #[serde(deserialize_with = "deserialize_quiz_options")]
    pub options: Vec<String>,
    #[serde(
        default,
        deserialize_with = "crate::bot::models::deserialize_flexible_opt_i32"
    )]
    pub correct_option_id: Option<i32>,
    /// Every correct answer (Bot API 9.6); several make a quiz in which
    /// players may pick several answers.
    #[serde(default, deserialize_with = "deserialize_flexible_i32_list")]
    pub correct_option_ids: Vec<i32>,
    #[serde(default)]
    pub explanation: Option<String>,
    #[serde(default)]
    pub preamble: Option<String>,
    #[serde(default, deserialize_with = "deserialize_flexible_opt_bool")]
    pub is_anonymous: Option<bool>,
    #[serde(default, deserialize_with = "deserialize_flexible_opt_bool")]
    pub shuffle_options: Option<bool>,
    /// Picture shown with the question.
    #[serde(default)]
    pub image_url: Option<String>,
    /// Pictures for the options, in option order; empty for none.
    #[serde(default, deserialize_with = "deserialize_url_list")]
    pub option_image_urls: Vec<String>,
    /// Text shown under the question (Bot API 9.6 `description`).
    #[serde(default)]
    pub description: Option<String>,
    /// Picture shown with the explanation (Bot API 10.0 `explanation_media`).
    #[serde(default)]
    pub explanation_image_url: Option<String>,
    #[serde(default, deserialize_with = "deserialize_flexible_opt_bool")]
    pub allows_revoting: Option<bool>,
    /// Seconds the quiz stays open before it closes by itself.
    #[serde(
        default,
        deserialize_with = "crate::bot::models::deserialize_flexible_opt_i32"
    )]
    pub open_period: Option<i32>,
    #[serde(default, deserialize_with = "deserialize_flexible_opt_bool")]
    pub hide_results_until_closes: Option<bool>,
}

/// Arguments of the `send_live_photo` tool.
#[derive(Debug, Clone, Default, serde::Deserialize, PartialEq, Eq)]
pub struct SendLivePhotoArgs {
    pub video_url: String,
    pub photo_url: String,
    #[serde(default)]
    pub caption: Option<String>,
}

impl SendLivePhotoArgs {
    pub fn sanitize(&mut self) {
        self.video_url = self.video_url.trim().to_string();
        self.photo_url = self.photo_url.trim().to_string();
        sanitize_multimedia_caption(&mut self.caption);
    }

    pub fn validate(&self) -> Result<(), String> {
        if web_url(&self.video_url).is_none() {
            return Err("video_url harus berupa URL http(s)".to_string());
        }
        if web_url(&self.photo_url).is_none() {
            return Err("photo_url harus berupa URL http(s)".to_string());
        }
        Ok(())
    }
}

/// Bot API limits for `sendPoll`.
const POLL_MAX_DESCRIPTION_CHARS: usize = 1024;
const POLL_MIN_OPEN_PERIOD_SECS: i32 = 5;
const POLL_MAX_OPEN_PERIOD_SECS: i32 = 2_628_000;

/// Accepts `[1, "2"]`, a single number, or `null`; unreadable items are
/// dropped and caught by validation.
fn deserialize_flexible_i32_list<'de, D>(deserializer: D) -> Result<Vec<i32>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(serde::Deserialize)]
    #[serde(untagged)]
    enum Item {
        Num(i64),
        Str(String),
    }
    #[derive(serde::Deserialize)]
    #[serde(untagged)]
    enum List {
        Many(Vec<Item>),
        One(Item),
    }
    let to_i32 = |item: Item| match item {
        Item::Num(number) => i32::try_from(number).ok(),
        Item::Str(text) => text.trim().parse::<i32>().ok(),
    };
    Ok(match Option::<List>::deserialize(deserializer)? {
        None => Vec::new(),
        Some(List::Many(items)) => items.into_iter().filter_map(to_i32).collect(),
        Some(List::One(item)) => to_i32(item).into_iter().collect(),
    })
}

/// A list of URLs where `null` (for the list or an item) means "none".
fn deserialize_url_list<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<Vec<Option<String>>>::deserialize(deserializer)?
        .unwrap_or_default()
        .into_iter()
        .map(Option::unwrap_or_default)
        .collect())
}

/// Only absolute web URLs can be fetched by Telegram for poll media.
fn web_url(url: &str) -> Option<String> {
    let url = url.trim();
    (url.starts_with("https://") || url.starts_with("http://")).then(|| url.to_string())
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
        // Answers must be in range, unique and ascending ("monotonically
        // increasing" in the Bot API); the legacy single id fills in when the
        // list is missing or held only invalid entries.
        let option_count = self.options.len();
        let mut correct_ids: Vec<i32> = self
            .correct_option_ids
            .iter()
            .copied()
            .filter(|id| usize::try_from(*id).is_ok_and(|id| id < option_count))
            .collect();
        if correct_ids.is_empty() {
            correct_ids.extend(self.correct_option_id);
        }
        correct_ids.sort_unstable();
        correct_ids.dedup();
        self.correct_option_ids = correct_ids;

        self.image_url = self.image_url.as_deref().and_then(web_url);
        self.explanation_image_url = self.explanation_image_url.as_deref().and_then(web_url);
        self.option_image_urls.truncate(option_count);
        for url in &mut self.option_image_urls {
            *url = web_url(url).unwrap_or_default();
        }

        self.description = self
            .description
            .as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(|text| crate::util::truncate_chars(text, POLL_MAX_DESCRIPTION_CHARS).to_string());
        // Zero or a negative value means "no time limit", not the shortest one.
        self.open_period = self
            .open_period
            .filter(|secs| *secs > 0)
            .map(|secs| secs.clamp(POLL_MIN_OPEN_PERIOD_SECS, POLL_MAX_OPEN_PERIOD_SECS));
        // Hidden results are revealed when the quiz closes; without a closing
        // time they would stay hidden forever.
        if self.open_period.is_none() {
            self.hide_results_until_closes = None;
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

    /// The correct answers: the list when given, else the legacy single id.
    pub fn correct_ids(&self) -> Vec<i32> {
        if self.correct_option_ids.is_empty() {
            self.correct_option_id.into_iter().collect()
        } else {
            self.correct_option_ids.clone()
        }
    }

    /// Checks the quiz against the Bot API limits and returns its correct
    /// answers.
    pub fn validate(&self) -> Result<Vec<i32>, String> {
        let correct_ids = self.correct_ids();
        if correct_ids.is_empty() {
            return Err("Quiz requires correct_option_ids".to_string());
        }
        let input_options: Vec<crate::bot::models::InputPollOption> = self
            .options
            .iter()
            .map(|opt| crate::bot::models::InputPollOption::new(opt.as_str()))
            .collect();
        for correct_id in &correct_ids {
            crate::bot::models::validate_quiz(
                &self.question,
                &input_options,
                *correct_id,
                self.explanation.as_deref(),
            )?;
        }
        if correct_ids.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err("Quiz correct_option_ids must be unique and ascending".to_string());
        }
        if correct_ids.len() >= self.options.len() {
            return Err("A quiz needs at least one wrong option".to_string());
        }
        Ok(correct_ids)
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
