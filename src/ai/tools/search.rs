use std::env;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use super::{
    clean_html_to_text, extract_core_search_terms, extract_raster_images_from_html,
    format_no_images_guidance, format_verified_images_section, is_likely_indonesian, is_logo_query,
    is_visual_search_query, sanitize_and_validate_raster_url,
};
use regex::Regex;
use reqwest::header::{ACCEPT, ACCEPT_LANGUAGE, REFERER, RETRY_AFTER, USER_AGENT};
use reqwest::StatusCode;
use serde_json::{json, Value};
use tracing::{debug, warn};
use url::Url;

/// Upper bound for any search-engine response body. Search APIs return a few
/// kilobytes; the cap only stops a misbehaving or hostile endpoint from
/// ballooning memory.
const MAX_SEARCH_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
/// Upper bound for a result page scraped for images.
const MAX_SCRAPED_PAGE_BYTES: usize = 2 * 1024 * 1024;

/// Results listed per search.
const MAX_SEARCH_HITS: usize = 5;
/// Text kept per result, so several searches with their image lists fit the
/// tool-output budget of one round.
const MAX_HIT_SUMMARY_CHARS: usize = 600;
/// Length of the short answer some engines give.
const MAX_ANSWER_CHARS: usize = 1_000;
/// Image URLs listed per search.
const MAX_SEARCH_IMAGES: usize = 8;
/// A picture search with fewer images than this is topped up from Wikipedia
/// and Wikimedia Commons.
const MIN_VISUAL_IMAGES: usize = 3;

const WIKI_USER_AGENT: &str = concat!(
    "xiao/",
    env!("CARGO_PKG_VERSION"),
    " (Telegram Bot Assistant)"
);
const BROWSER_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";

/// Pause for keyless Exa MCP after it rate-limited us without `Retry-After`.
const EXA_MCP_RATE_LIMIT_COOLDOWN: Duration = Duration::from_secs(10 * 60);
/// Pause for keyless Exa MCP after a connection failure or server error.
const EXA_MCP_FAILURE_COOLDOWN: Duration = Duration::from_secs(2 * 60);
/// Pause for DuckDuckGo after it failed, blocked us or asked for a captcha
/// (it is also blocked by some Indonesian ISPs).
const DUCKDUCKGO_FAILURE_COOLDOWN: Duration = Duration::from_secs(10 * 60);

/// Skips an engine for a while after it rate-limited or blocked us, so later
/// searches do not each wait for a request that is bound to fail.
struct Cooldown {
    until: Mutex<Option<Instant>>,
}

impl Cooldown {
    const fn new() -> Self {
        Self {
            until: Mutex::new(None),
        }
    }

    fn is_active(&self) -> bool {
        self.until
            .lock()
            .map(|until| until.is_some_and(|deadline| Instant::now() < deadline))
            .unwrap_or(false)
    }

    /// Time left in the cooldown, `None` when it is not active.
    fn remaining(&self) -> Option<Duration> {
        let until = self.until.lock().ok()?;
        until
            .and_then(|deadline| deadline.checked_duration_since(Instant::now()))
            .filter(|left| !left.is_zero())
    }

    fn clear(&self) {
        if let Ok(mut until) = self.until.lock() {
            *until = None;
        }
    }

    /// Starts the cooldown, keeping an earlier one that lasts longer.
    fn trip(&self, duration: Duration) {
        let deadline = Instant::now() + duration;
        if let Ok(mut until) = self.until.lock() {
            if until.is_none_or(|current| current < deadline) {
                *until = Some(deadline);
            }
        }
    }
}

static EXA_MCP_COOLDOWN: Cooldown = Cooldown::new();
static DUCKDUCKGO_COOLDOWN: Cooldown = Cooldown::new();

/// Time left before keyless Exa MCP and DuckDuckGo are tried again.
pub fn search_cooldowns() -> (Option<Duration>, Option<Duration>) {
    (
        EXA_MCP_COOLDOWN.remaining(),
        DUCKDUCKGO_COOLDOWN.remaining(),
    )
}

/// Ends both cooldowns so the next search tries every engine again.
pub fn reset_search_cooldowns() {
    EXA_MCP_COOLDOWN.clear();
    DUCKDUCKGO_COOLDOWN.clear();
}

/// Cooldown after an HTTP 429: the server's `Retry-After` seconds when given
/// (kept between one minute and one hour), otherwise ten minutes.
fn rate_limit_cooldown(retry_after: Option<&str>) -> Duration {
    retry_after
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(|seconds| Duration::from_secs(seconds.clamp(60, 3600)))
        .unwrap_or(EXA_MCP_RATE_LIMIT_COOLDOWN)
}

async fn read_bytes_bounded(resp: reqwest::Response) -> Result<Vec<u8>, String> {
    if resp
        .content_length()
        .is_some_and(|length| length > MAX_SEARCH_RESPONSE_BYTES as u64)
    {
        return Err("respons melebihi batas ukuran".to_string());
    }
    let mut stream = resp.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = futures_util::StreamExt::next(&mut stream).await {
        let chunk = chunk.map_err(|e| format!("stream terputus: {e}"))?;
        if bytes.len().saturating_add(chunk.len()) > MAX_SEARCH_RESPONSE_BYTES {
            return Err("respons melebihi batas ukuran".to_string());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

async fn read_text_bounded(resp: reqwest::Response) -> Result<String, String> {
    read_bytes_bounded(resp)
        .await
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
}

async fn read_json_bounded(resp: reqwest::Response) -> Result<Value, String> {
    let bytes = read_bytes_bounded(resp).await?;
    serde_json::from_slice(&bytes).map_err(|e| format!("JSON tidak valid: {e}"))
}

/// The first of `keys` set in the environment, else the first saved by the
/// CLI. An empty value (such as `BRAVE_API_KEY=` in a copied `.env.example`)
/// counts as unset, so it does not hide a key saved with `xiao search`.
fn setting(keys: &[&str]) -> Option<String> {
    let non_empty = |value: String| {
        let value = value.trim().to_string();
        (!value.is_empty()).then_some(value)
    };
    keys.iter()
        .find_map(|key| env::var(key).ok().and_then(non_empty))
        .or_else(|| {
            keys.iter()
                .find_map(|key| crate::ai::service::load_app_setting(key).and_then(non_empty))
        })
}

pub fn get_brave_key() -> Option<String> {
    setting(&["BRAVE_API_KEY"])
}

pub fn get_tavily_key() -> Option<String> {
    setting(&["TAVILY_API_KEY", "TAVILY_KEY"])
}

pub fn get_exa_key() -> Option<String> {
    setting(&["EXA_API_KEY", "EXA_KEY"])
}

pub fn get_search_engine_status() -> (String, String) {
    let exa_key = get_exa_key();
    let tavily_key = get_tavily_key();
    let brave_key = get_brave_key();

    let engine_name = if brave_key.is_some() {
        "Brave Search (API Key Active)".to_string()
    } else if tavily_key.is_some() {
        "Tavily AI (API Key Active)".to_string()
    } else if exa_key.is_some() {
        "Exa AI (REST API Key Active)".to_string()
    } else {
        "Exa MCP (Keyless) \u{2192} DuckDuckGo \u{2192} Wikipedia".to_string()
    };

    let mcp_url = get_configured_mcp_url();
    (engine_name, mcp_url)
}

pub fn get_configured_mcp_url() -> String {
    setting(&["EXA_MCP_URL"]).unwrap_or_else(|| "https://mcp.exa.ai/".to_string())
}

static RE_MARKDOWN_IMAGE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"!\[[^\]]*\]\([^)]*\)").expect("valid static regex"));
static RE_INLINE_TAG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"</?[a-zA-Z][^>]*>").expect("valid static regex"));
static RE_TABLE_RULE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\|(?:\s*:?-{3,}:?\s*\|)+").expect("valid static regex"));
static RE_TEXT_URL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"https?://[^\s<>"'()\[\]]+"#).expect("valid static regex"));

/// Whether text is undecodable binary content (an engine sometimes returns a
/// picture's bytes as page text), judged by its share of replacement and
/// control characters.
fn looks_binary(text: &str) -> bool {
    let mut total = 0usize;
    let mut garbled = 0usize;
    for c in text.chars() {
        total += 1;
        if c == char::REPLACEMENT_CHARACTER || (c.is_control() && !c.is_whitespace()) {
            garbled += 1;
        }
    }
    garbled >= 3 && garbled * 20 > total
}

/// Flattens result text to one line of at most `max_chars` characters:
/// image markup, HTML tags and table rules are dropped and whitespace is
/// collapsed. Binary content becomes an empty string.
fn compact_text(text: &str, max_chars: usize) -> String {
    if looks_binary(text) {
        return String::new();
    }
    let without_images = RE_MARKDOWN_IMAGE.replace_all(text, " ");
    let without_tags = RE_INLINE_TAG.replace_all(&without_images, " ");
    let without_rules = RE_TABLE_RULE.replace_all(&without_tags, " ");
    let decoded = html_escape::decode_html_entities(&without_rules);
    let collapsed = decoded.split_whitespace().collect::<Vec<_>>().join(" ");
    match collapsed.char_indices().nth(max_chars) {
        None => collapsed,
        Some((end, _)) => format!("{}…", collapsed[..end].trim_end()),
    }
}

/// Image links in page text that are page furniture rather than content.
fn is_decorative_image(url: &str, allow_logos: bool) -> bool {
    const DECORATIVE: &[&str] = &[
        "avatar", "gravatar", "favicon", "sprite", "/icons/", "icon-", "/badges/", "emoji",
    ];
    let lower = url.to_ascii_lowercase();
    DECORATIVE.iter().any(|marker| lower.contains(marker))
        || (!allow_logos && lower.contains("logo"))
}

struct SearchHit {
    title: String,
    url: String,
    summary: String,
}

/// What one engine found. Every engine is rendered in the same shape by
/// [`SearchFindings::render`]: images first, then a short answer and the
/// results, each capped so the image list never falls off the end of a long
/// tool result.
struct SearchFindings {
    source: &'static str,
    answer: Option<String>,
    hits: Vec<SearchHit>,
    images: Vec<String>,
}

impl SearchFindings {
    fn new(source: &'static str) -> Self {
        Self {
            source,
            answer: None,
            hits: Vec::new(),
            images: Vec::new(),
        }
    }

    fn set_answer(&mut self, answer: &str) {
        let answer = compact_text(answer, MAX_ANSWER_CHARS);
        if !answer.is_empty() {
            self.answer = Some(answer);
        }
    }

    fn push_hit(&mut self, title: &str, url: &str, summary: &str) {
        if self.hits.len() >= MAX_SEARCH_HITS {
            return;
        }
        let title = compact_text(title, 200);
        self.hits.push(SearchHit {
            title: if title.is_empty() {
                "Tanpa Judul".to_string()
            } else {
                title
            },
            url: url.trim().to_string(),
            summary: compact_text(summary, MAX_HIT_SUMMARY_CHARS),
        });
    }

    /// Adds `url` when it is a usable raster image not listed yet.
    fn push_image(&mut self, url: &str) {
        if self.images.len() >= MAX_SEARCH_IMAGES {
            return;
        }
        if let Some(valid) = sanitize_and_validate_raster_url(url) {
            if !self.images.contains(&valid) {
                self.images.push(valid);
            }
        }
    }

    fn push_images<I, S>(&mut self, urls: I)
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        for url in urls {
            self.push_image(url.as_ref());
        }
    }

    /// Adds the image links written in result text (markdown images or bare
    /// links), skipping avatars, icons and, unless asked for, logos.
    fn push_text_images(&mut self, text: &str, allow_logos: bool) {
        for found in RE_TEXT_URL.find_iter(text) {
            let url = found
                .as_str()
                .trim_end_matches(['.', ',', ';', ':', '!', '?', '*', '_']);
            if !is_decorative_image(url, allow_logos) {
                self.push_image(url);
            }
        }
    }

    fn is_empty(&self) -> bool {
        self.answer.is_none() && self.hits.is_empty() && self.images.is_empty()
    }

    fn render(&self, query: &str, visual: bool) -> String {
        let mut sections = vec![format!(
            "[Hasil Pencarian {} untuk \"{query}\"]",
            self.source
        )];
        if !self.images.is_empty() {
            sections.push(
                format_verified_images_section(&self.images)
                    .trim()
                    .to_string(),
            );
        } else if visual {
            sections.push(format_no_images_guidance(query).trim().to_string());
        }
        if let Some(answer) = &self.answer {
            sections.push(format!("💡 **Jawaban Ringkas**: {answer}"));
        }
        let hits: Vec<String> = self
            .hits
            .iter()
            .enumerate()
            .map(|(index, hit)| format_search_item(index + 1, hit))
            .collect();
        if !hits.is_empty() {
            sections.push(hits.join("\n\n"));
        }
        sections.join("\n\n")
    }
}

fn format_search_item(index: usize, hit: &SearchHit) -> String {
    let mut item = format!("{index}. **{}**", hit.title);
    if !hit.url.is_empty() {
        item.push_str(&format!("\n   URL: {}", hit.url));
    }
    if !hit.summary.is_empty() {
        item.push_str(&format!("\n   Ringkasan: {}", hit.summary));
    }
    item
}

pub async fn execute_web_search(query: &str) -> String {
    let q = query.trim();
    if q.is_empty() {
        return "Query pencarian tidak boleh kosong. Silakan berikan kata kunci atau topik pencarian yang lebih spesifik.".to_string();
    }

    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
    {
        Ok(client) => client,
        Err(err) => {
            warn!("Gagal membangun klien pencarian dengan batas waktu: {err}");
            return "Layanan pencarian tidak tersedia sementara karena kegagalan konfigurasi jaringan.".to_string();
        }
    };

    let visual = is_visual_search_query(q);

    if let Some(mut findings) = primary_findings(&client, q).await {
        if visual && findings.images.len() < MIN_VISUAL_IMAGES {
            match search_wikipedia(&client, q).await {
                Ok(wiki) => findings.push_images(wiki.images),
                Err(e) => debug!("Wikipedia image top-up failed ({e})"),
            }
        }
        return findings.render(q, visual);
    }

    // Every web engine failed or was cooling down: Wikipedia is the last resort.
    match search_wikipedia(&client, q).await {
        Ok(findings) => findings.render(q, visual),
        Err(e) => {
            warn!("Wikipedia search failed ({e})");
            format!(
                "[Informasi Pencarian Web]\nPencarian web daring untuk topik \"{q}\" saat ini tidak dapat diselesaikan karena kendala koneksi atau penyedia pencarian sedang tidak tersedia ({e}).\n\nℹ️ **Panduan Asisten**: Berikan tanggapan deskriptif dan faktual mengenai topik \"{q}\" berdasarkan pengetahuan internal Anda secara lengkap. Jika pengguna meminta gambar atau foto, jelaskan informasi visualnya secara naratif dalam teks Markdown dan hindari memanggil tool multimedia fiktif."
            )
        }
    }
}

/// Results of the first engine that has any, in order of preference: the
/// keyed APIs, keyless Exa MCP, then DuckDuckGo. Engines in cooldown are
/// skipped.
async fn primary_findings(client: &reqwest::Client, q: &str) -> Option<SearchFindings> {
    if let Some(brave_key) = get_brave_key() {
        debug!("Using Brave Search API");
        match search_brave(client, &brave_key, q).await {
            Ok(findings) if !findings.is_empty() => return Some(findings),
            Ok(_) => debug!("Brave returned no results, trying other providers"),
            Err(e) => warn!("Brave search failed ({e}), falling back to other providers"),
        }
    }

    if let Some(tavily_key) = get_tavily_key() {
        debug!("Using Tavily API");
        match search_tavily(client, &tavily_key, q).await {
            Ok(findings) if !findings.is_empty() => return Some(findings),
            Ok(_) => debug!("Tavily returned no results, trying other providers"),
            Err(e) => warn!("Tavily search failed ({e}), falling back to other providers"),
        }
    }

    if let Some(exa_key) = get_exa_key() {
        debug!("Using Exa API");
        match search_exa_api(client, &exa_key, q).await {
            Ok(findings) if !findings.is_empty() => return Some(findings),
            Ok(_) => debug!("Exa API returned no results, trying other providers"),
            Err(e) => warn!("Exa API search failed ({e}), falling back to other providers"),
        }
    }

    if EXA_MCP_COOLDOWN.is_active() {
        debug!("Skipping Exa MCP while it cools down after a failure");
    } else {
        debug!("Trying Exa Keyless MCP");
        match exa_mcp_findings(client, &get_configured_mcp_url(), q).await {
            Ok(findings) if !findings.is_empty() => return Some(findings),
            Ok(_) => debug!("Exa MCP returned no results, trying DuckDuckGo"),
            Err(e) => warn!("Gagal menghubungi Exa MCP ({e}), beralih ke DuckDuckGo..."),
        }
    }

    if DUCKDUCKGO_COOLDOWN.is_active() {
        debug!("Skipping DuckDuckGo while it cools down after a failure");
    } else {
        debug!("Using DuckDuckGo");
        match search_duckduckgo(client, q).await {
            Ok(findings) if !findings.is_empty() => return Some(findings),
            Ok(_) => debug!("DuckDuckGo returned no results"),
            Err(e) => warn!("Koneksi ke DuckDuckGo gagal ({e}); beralih ke Wikipedia. Catatan: Domain DuckDuckGo diblokir oleh beberapa ISP/Kominfo di Indonesia. Disarankan menggunakan TAVILY_API_KEY, EXA_API_KEY, atau BRAVE_API_KEY untuk hasil yang cepat."),
        }
    }

    None
}

async fn brave_get(client: &reqwest::Client, api_key: &str, url: &str) -> Result<Value, String> {
    let resp = client
        .get(url)
        .header("X-Subscription-Token", api_key)
        .header(ACCEPT, "application/json")
        .send()
        .await
        .map_err(|e| format!("Gagal menghubungi Brave Search API: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("Brave Search API returned HTTP {}", resp.status()));
    }
    read_json_bounded(resp)
        .await
        .map_err(|e| format!("Gagal membaca JSON Brave: {e}"))
}

async fn search_brave(
    client: &reqwest::Client,
    api_key: &str,
    query: &str,
) -> Result<SearchFindings, String> {
    let mut findings = SearchFindings::new("Brave");

    if is_visual_search_query(query) {
        let image_search_url = format!(
            "https://api.search.brave.com/res/v1/images/search?q={}&count={MAX_SEARCH_IMAGES}",
            urlencoding::encode(query)
        );
        match brave_get(client, api_key, &image_search_url).await {
            Ok(body) => {
                for item in body
                    .get("results")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    // The original picture first, Brave's thumbnail otherwise.
                    let candidates = [
                        item.pointer("/properties/url").and_then(Value::as_str),
                        item.pointer("/thumbnail/src").and_then(Value::as_str),
                    ];
                    if let Some(url) = candidates
                        .into_iter()
                        .flatten()
                        .find(|url| sanitize_and_validate_raster_url(url).is_some())
                    {
                        findings.push_image(url);
                    }
                }
            }
            Err(e) => debug!("Brave image search failed ({e})"),
        }
    }

    let url = format!(
        "https://api.search.brave.com/res/v1/web/search?q={}&count={MAX_SEARCH_HITS}",
        urlencoding::encode(query)
    );
    let body = brave_get(client, api_key, &url).await?;

    for item in body
        .pointer("/web/results")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(MAX_SEARCH_HITS)
    {
        findings.push_hit(
            item.get("title").and_then(Value::as_str).unwrap_or(""),
            item.get("url").and_then(Value::as_str).unwrap_or(""),
            item.get("description")
                .and_then(Value::as_str)
                .unwrap_or(""),
        );
        if let Some(thumbnail) = item
            .get("thumbnail")
            .and_then(|t| t.get("original").or_else(|| t.get("src")))
            .and_then(Value::as_str)
        {
            findings.push_image(thumbnail);
        }
    }

    for item in body
        .pointer("/pictures/results")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(url) = item
            .pointer("/properties/url")
            .or_else(|| item.pointer("/thumbnail/src"))
            .and_then(Value::as_str)
        {
            findings.push_image(url);
        }
    }

    Ok(findings)
}

async fn search_tavily(
    client: &reqwest::Client,
    api_key: &str,
    query: &str,
) -> Result<SearchFindings, String> {
    let resp = client
        .post("https://api.tavily.com/search")
        .json(&json!({
            "api_key": api_key,
            "query": query,
            "include_answer": true,
            "include_images": true,
            "max_results": MAX_SEARCH_HITS,
            "search_depth": "basic"
        }))
        .send()
        .await
        .map_err(|e| format!("Gagal menghubungi Tavily API: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        return Err(format!("Tavily API returned HTTP {status}"));
    }

    let body: Value = read_json_bounded(resp)
        .await
        .map_err(|e| format!("Gagal membaca JSON Tavily: {e}"))?;

    let mut findings = SearchFindings::new("Tavily");
    if let Some(answer) = body.get("answer").and_then(Value::as_str) {
        findings.set_answer(answer);
    }

    for image in body
        .get("images")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(url) = image
            .as_str()
            .or_else(|| image.get("url").and_then(Value::as_str))
        {
            findings.push_image(url);
        }
    }

    for item in body
        .get("results")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(MAX_SEARCH_HITS)
    {
        findings.push_hit(
            item.get("title").and_then(Value::as_str).unwrap_or(""),
            item.get("url").and_then(Value::as_str).unwrap_or(""),
            item.get("content").and_then(Value::as_str).unwrap_or(""),
        );
        if let Some(image) = item.get("image").and_then(Value::as_str) {
            findings.push_image(image);
        }
    }

    Ok(findings)
}

async fn search_exa_api(
    client: &reqwest::Client,
    api_key: &str,
    query: &str,
) -> Result<SearchFindings, String> {
    let resp = client
        .post("https://api.exa.ai/search")
        .header("x-api-key", api_key)
        .header(ACCEPT, "application/json")
        .json(&json!({
            "query": query,
            "numResults": MAX_SEARCH_HITS,
            "highlights": true
        }))
        .send()
        .await
        .map_err(|e| format!("Gagal menghubungi Exa API: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        return Err(format!("Exa API returned HTTP {status}"));
    }

    let body: Value = read_json_bounded(resp)
        .await
        .map_err(|e| format!("Gagal membaca JSON Exa: {e}"))?;

    let allow_logos = is_logo_query(query);
    let mut findings = SearchFindings::new("Exa AI");
    for item in body
        .get("results")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(MAX_SEARCH_HITS)
    {
        let highlights: Vec<&str> = item
            .get("highlights")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        let text = if highlights.is_empty() {
            item.get("text")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        } else {
            highlights.join(" … ")
        };
        findings.push_hit(
            item.get("title").and_then(Value::as_str).unwrap_or(""),
            item.get("url").and_then(Value::as_str).unwrap_or(""),
            &text,
        );
        if let Some(image) = item.get("image").and_then(Value::as_str) {
            findings.push_image(image);
        }
        findings.push_text_images(&text, allow_logos);
    }

    Ok(findings)
}

/// Keyless Exa search through its MCP endpoint, rendered as tool output.
/// Used by `xiao mcp test`; web searches go through [`exa_mcp_findings`].
pub(crate) async fn search_exa_mcp(
    client: &reqwest::Client,
    mcp_url: &str,
    query: &str,
) -> Result<String, String> {
    let findings = exa_mcp_findings(client, mcp_url, query).await?;
    Ok(findings.render(query, is_visual_search_query(query)))
}

/// Calls the `web_search_exa` tool of an MCP endpoint. A rate limit or a
/// failing endpoint starts [`EXA_MCP_COOLDOWN`], so the next searches go
/// straight to the other engines.
async fn exa_mcp_findings(
    client: &reqwest::Client,
    mcp_url: &str,
    query: &str,
) -> Result<SearchFindings, String> {
    // The endpoint is configurable, so it goes through the SSRF policy and
    // the connection is pinned to the vetted address.
    let resolved = crate::bot::url_policy::resolve_download_url(mcp_url).await?;
    let mcp_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .resolve(&resolved.host, resolved.address)
        .build()
        .map_err(|e| format!("Gagal menginisialisasi client HTTP Exa MCP: {e}"))?;

    let resp = match mcp_client
        .post(resolved.url)
        .header(ACCEPT, "application/json, text/event-stream")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "web_search_exa",
                "arguments": {
                    "query": query,
                    "numResults": MAX_SEARCH_HITS
                }
            }
        }))
        .send()
        .await
    {
        Ok(resp) => resp,
        Err(e) => {
            EXA_MCP_COOLDOWN.trip(EXA_MCP_FAILURE_COOLDOWN);
            return Err(format!("Gagal menghubungi Exa MCP ({e})"));
        }
    };

    let status = resp.status();
    if status == StatusCode::TOO_MANY_REQUESTS {
        let retry_after = resp
            .headers()
            .get(RETRY_AFTER)
            .and_then(|value| value.to_str().ok());
        EXA_MCP_COOLDOWN.trip(rate_limit_cooldown(retry_after));
        return Err("Exa MCP membatasi permintaan (HTTP 429)".to_string());
    }
    if status.is_server_error() {
        EXA_MCP_COOLDOWN.trip(EXA_MCP_FAILURE_COOLDOWN);
    }
    if !status.is_success() {
        return Err(format!("Exa MCP returned HTTP {status}"));
    }

    let raw = read_text_bounded(resp)
        .await
        .map_err(|e| format!("Gagal membaca stream Exa MCP: {e}"))?;
    let text = exa_mcp_reply_text(&raw).inspect_err(|error| {
        let lower = error.to_ascii_lowercase();
        if lower.contains("rate limit") || lower.contains("429") || lower.contains("too many") {
            EXA_MCP_COOLDOWN.trip(EXA_MCP_RATE_LIMIT_COOLDOWN);
        }
    })?;
    if text.trim().is_empty() {
        return Err("Exa MCP tidak mengembalikan konten yang valid.".to_string());
    }

    let allow_logos = is_logo_query(query);
    let mut findings = SearchFindings::new("Exa AI");
    let mut commons_titles: Vec<String> = Vec::new();
    for result in parse_exa_mcp_text(&text) {
        findings.push_hit(&result.title, &result.url, &result.body);
        if let Some(image) = &result.image {
            findings.push_image(image);
        }
        findings.push_text_images(&result.body, allow_logos);
        let linked = std::iter::once(result.url.as_str())
            .chain(RE_TEXT_URL.find_iter(&result.body).map(|m| m.as_str()));
        for title in linked.filter_map(commons_file_title) {
            if !commons_titles.contains(&title) {
                commons_titles.push(title);
            }
        }
    }
    if findings.hits.is_empty() {
        findings.push_hit("Hasil Pencarian", "", &text);
    }
    if !commons_titles.is_empty() && findings.images.len() < MAX_SEARCH_IMAGES {
        let subject = image_subject(query);
        findings.push_images(
            resolve_commons_files(client, &commons_titles, &subject, allow_logos).await,
        );
    }
    Ok(findings)
}

/// The text content of an MCP `tools/call` reply, sent either as JSON or as
/// server-sent events. A JSON-RPC error or a tool error becomes `Err`.
fn exa_mcp_reply_text(raw: &str) -> Result<String, String> {
    let messages: Vec<Value> = match serde_json::from_str::<Value>(raw.trim()) {
        Ok(message) => vec![message],
        Err(_) => raw
            .lines()
            .filter_map(|line| line.strip_prefix("data:"))
            .filter_map(|data| serde_json::from_str(data.trim()).ok())
            .collect(),
    };

    let mut texts = Vec::new();
    for message in &messages {
        if let Some(error) = message.get("error") {
            let detail = error
                .get("message")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| error.to_string());
            return Err(format!("Exa MCP error: {detail}"));
        }
        let Some(result) = message.get("result") else {
            continue;
        };
        let text = result
            .get("content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|item| item.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n\n");
        if result.get("isError").and_then(Value::as_bool) == Some(true) {
            return Err(format!("Exa MCP tool error: {}", compact_text(&text, 300)));
        }
        if !text.trim().is_empty() {
            texts.push(text);
        }
    }
    Ok(texts.join("\n\n"))
}

#[derive(Default)]
struct ExaMcpResult {
    title: String,
    url: String,
    image: Option<String>,
    body: String,
}

/// Splits the text of an Exa MCP reply into results. Results are separated
/// by `---` lines and start with `Title:` / `URL:` headers; the text after
/// `Highlights:` (or `Text:`/`Summary:`) is the body. A `---` inside a body
/// (a markdown rule) is kept with the result it belongs to.
fn parse_exa_mcp_text(text: &str) -> Vec<ExaMcpResult> {
    let normalized = text.replace("\r\n", "\n");
    let mut results: Vec<ExaMcpResult> = Vec::new();
    for piece in normalized.split("\n---\n") {
        let piece = piece.trim();
        if piece.is_empty() {
            continue;
        }
        let starts_result = piece.starts_with("Title:") || piece.starts_with("URL:");
        if let (false, Some(last)) = (starts_result, results.last_mut()) {
            last.body.push('\n');
            last.body.push_str(piece);
            continue;
        }
        results.push(parse_exa_mcp_block(piece));
    }
    results
}

fn parse_exa_mcp_block(block: &str) -> ExaMcpResult {
    let mut result = ExaMcpResult::default();
    let mut in_body = false;
    for line in block.lines() {
        if in_body {
            result.body.push_str(line);
            result.body.push('\n');
            continue;
        }
        let (key, value) = line
            .split_once(':')
            .map(|(key, value)| (key.trim(), value.trim()))
            .unwrap_or(("", line));
        match key {
            "Title" => result.title = value.to_string(),
            "URL" => result.url = value.to_string(),
            "Image" => result.image = Some(value.to_string()),
            "Published" | "Published Date" | "Author" | "Favicon" | "ID" | "Score" => {}
            "Highlights" | "Text" | "Summary" | "Content" => {
                in_body = true;
                if !value.is_empty() {
                    result.body.push_str(value);
                    result.body.push('\n');
                }
            }
            _ => {
                in_body = true;
                result.body.push_str(line);
                result.body.push('\n');
            }
        }
    }
    result
}

/// The `File:` title behind a Wikimedia Commons or Wikipedia file page. Those
/// URLs are HTML pages, not pictures; the picture itself is looked up with
/// [`resolve_commons_files`].
fn commons_file_title(url: &str) -> Option<String> {
    let parsed = Url::parse(url).ok()?;
    let host = parsed.host_str()?.to_ascii_lowercase();
    if !(host.ends_with("commons.wikimedia.org") || host.ends_with(".wikipedia.org")) {
        return None;
    }
    let path = urlencoding::decode(parsed.path()).ok()?;
    let (namespace, name) = path.strip_prefix("/wiki/")?.split_once(':')?;
    if !matches!(
        namespace.to_ascii_lowercase().as_str(),
        "file" | "berkas" | "image"
    ) {
        return None;
    }
    let name = name.replace('_', " ");
    let name = name.trim();
    let lower = name.to_ascii_lowercase();
    let raster = [".jpg", ".jpeg", ".png", ".webp", ".tif", ".tiff"]
        .iter()
        .any(|extension| lower.ends_with(extension));
    (raster && !name.is_empty()).then(|| format!("File:{name}"))
}

async fn wiki_json(client: &reqwest::Client, url: &str) -> Result<Value, String> {
    let resp = client
        .get(url)
        .header(USER_AGENT, WIKI_USER_AGENT)
        .send()
        .await
        .map_err(|e| format!("permintaan gagal: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    read_json_bounded(resp).await
}

/// Raster image URLs of a MediaWiki `prop=imageinfo` reply, in search order,
/// leaving out files whose title `skip` rejects.
fn imageinfo_urls(body: &Value, skip: impl Fn(&str) -> bool) -> Vec<String> {
    let Some(pages) = body.pointer("/query/pages").and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut pages: Vec<&Value> = pages.values().collect();
    pages.sort_by_key(|page| {
        page.get("index")
            .and_then(Value::as_i64)
            .unwrap_or(i64::MAX)
    });
    let mut urls = Vec::new();
    for page in pages {
        let title = page.get("title").and_then(Value::as_str).unwrap_or("");
        if skip(title) {
            continue;
        }
        let Some(info) = page.pointer("/imageinfo/0") else {
            continue;
        };
        let url = ["thumburl", "url"]
            .iter()
            .filter_map(|key| info.get(*key).and_then(Value::as_str))
            .find_map(sanitize_and_validate_raster_url);
        if let Some(url) = url {
            if !urls.contains(&url) {
                urls.push(url);
            }
        }
    }
    urls
}

/// Picture URLs of Wikimedia Commons files named in search results, without
/// maps, drawings and the like (see [`is_unwanted_wiki_file`]).
async fn resolve_commons_files(
    client: &reqwest::Client,
    titles: &[String],
    subject: &str,
    allow_logos: bool,
) -> Vec<String> {
    let titles: Vec<&str> = titles.iter().take(10).map(String::as_str).collect();
    if titles.is_empty() {
        return Vec::new();
    }
    let url = format!(
        "https://commons.wikimedia.org/w/api.php?action=query&titles={}&prop=imageinfo&iiprop=url&iiurlwidth=1000&format=json",
        urlencoding::encode(&titles.join("|"))
    );
    match wiki_json(client, &url).await {
        Ok(body) => imageinfo_urls(&body, |file| {
            is_unwanted_wiki_file(file, subject, allow_logos)
        }),
        Err(e) => {
            debug!("Resolving Commons files failed ({e})");
            Vec::new()
        }
    }
}

/// Wiki files that are maps, flags, symbols, drawings, video stills or
/// scanned documents rather than pictures of the subject. Works on a file
/// title or on a picture URL. Logos, flags, symbols and vector drawings are
/// kept for a logo search, and a word the search itself asks for (a seal, a
/// map) never rules a file out.
fn is_unwanted_wiki_file(title: &str, subject: &str, allow_logos: bool) -> bool {
    const ALWAYS: &[&str] = &[
        "disambig", "locator", "location", "map", "maps", "peta", "stub",
        // Stills of videos and pages of scanned documents.
        "ogv", "webm", "ogg", "pdf", "djvu",
    ];
    const UNLESS_LOGO: &[&str] = &[
        "logo",
        "flag",
        "bendera",
        "icon",
        "ikon",
        "symbol",
        "simbol",
        "diagram",
        "insignia",
        "lambang",
        "seal",
        "emblem",
        "signature",
        "denah",
        "skema",
        "schematic",
        // Vector drawings: maps, plans and charts rather than photographs.
        "svg",
    ];
    const PHRASES_UNLESS_LOGO: &[&str] = &["coat of arms", "cross section", "floor plan"];
    let lower = urlencoding::decode(title)
        .map(|decoded| decoded.into_owned())
        .unwrap_or_else(|_| title.to_string())
        .to_lowercase()
        .replace(['_', '-'], " ");
    let subject = subject.to_lowercase().replace(['_', '-'], " ");
    let asked_for: Vec<&str> = subject
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect();
    if !allow_logos
        && PHRASES_UNLESS_LOGO
            .iter()
            .any(|phrase| lower.contains(phrase) && !subject.contains(phrase))
    {
        return true;
    }
    lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !asked_for.contains(word))
        .any(|word| ALWAYS.contains(&word) || (!allow_logos && UNLESS_LOGO.contains(&word)))
}

static RE_PHOTO_WORDS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(?:foto-foto|foto|gambar-gambar|gambar|potret|photographs?|photos?|pictures?|pics?|images?|wallpapers?|png|jpe?g|webp|hd|4k)\b",
    )
    .expect("valid static regex")
});

/// What a picture search is about: the query without request phrasing and
/// without words like "foto" or "photo", which steer encyclopedia search
/// toward unrelated articles.
fn image_subject(query: &str) -> String {
    let core = extract_core_search_terms(query);
    let stripped = RE_PHOTO_WORDS.replace_all(&core, " ");
    let subject = stripped.split_whitespace().collect::<Vec<_>>().join(" ");
    if subject.is_empty() {
        core
    } else {
        subject
    }
}

const SUBJECT_STOPWORDS: &[&str] = &[
    "the", "and", "for", "with", "from", "about", "dan", "yang", "dari", "untuk", "dengan", "pada",
    "tentang",
];
/// Kinds of places and things that many titles share ("Mount", "Candi"), so
/// they do not tell one subject from another.
const GENERIC_SUBJECT_WORDS: &[&str] = &[
    "mount",
    "mountain",
    "gunung",
    "lake",
    "danau",
    "river",
    "sungai",
    "island",
    "pulau",
    "beach",
    "pantai",
    "temple",
    "candi",
    "pura",
    "tower",
    "menara",
    "bridge",
    "jembatan",
    "palace",
    "istana",
    "castle",
    "benteng",
    "museum",
    "mosque",
    "masjid",
    "church",
    "gereja",
    "cathedral",
    "katedral",
    "park",
    "taman",
    "national",
    "nasional",
    "city",
    "kota",
    "province",
    "provinsi",
    "kingdom",
    "kerajaan",
    "empire",
    "kekaisaran",
    "battle",
    "pertempuran",
    "war",
    "perang",
    "monument",
    "monumen",
    "statue",
    "patung",
    "waterfall",
    "curug",
    "cave",
    "goa",
    "gua",
];

/// Lowercase letters and digits only, so "Colosseum, Rome" and
/// "Colosseum_Rome.jpg" compare alike.
fn squash(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Distinctive word stems (up to five letters) of a title or query: short
/// words, stopwords and generic kinds are skipped, unless nothing else is
/// left.
fn subject_stems(text: &str) -> Vec<String> {
    let words: Vec<String> = text
        .split(|c: char| !c.is_alphanumeric())
        .map(str::to_lowercase)
        .filter(|word| word.chars().count() >= 3 && !SUBJECT_STOPWORDS.contains(&word.as_str()))
        .collect();
    let distinctive: Vec<&String> = words
        .iter()
        .filter(|word| !GENERIC_SUBJECT_WORDS.contains(&word.as_str()))
        .collect();
    let chosen = if distinctive.is_empty() {
        words.iter().collect()
    } else {
        distinctive
    };
    let mut stems: Vec<String> = Vec::new();
    for word in chosen {
        let stem: String = word.chars().take(5).collect();
        if !stems.contains(&stem) {
            stems.push(stem);
        }
    }
    stems
}

/// Whether `haystack` contains `stem`. A full five-letter stem may differ in
/// one letter, so spellings such as "Koloseum" and "Colosseum" match.
fn contains_stem(haystack: &str, stem: &str) -> bool {
    if haystack.contains(stem) {
        return true;
    }
    let stem: Vec<char> = stem.chars().collect();
    if stem.len() < 5 {
        return false;
    }
    let hay: Vec<char> = haystack.chars().collect();
    hay.windows(stem.len()).any(|window| {
        window
            .iter()
            .zip(&stem)
            .filter(|(left, right)| left != right)
            .count()
            <= 1
    })
}

/// Whether an article is about the searched subject: most words of its main
/// title (before any "(" or ",") are in the subject, and the article covers
/// at least half of the subject. "Pantheon, Rome" is not about "Colosseum
/// Rome"; "Colosseum" is.
fn article_matches_subject(article_title: &str, subject: &str) -> bool {
    let main_title = article_title
        .split(['(', ','])
        .next()
        .unwrap_or(article_title);
    let title_stems = subject_stems(main_title);
    let subject_stems = subject_stems(subject);
    if title_stems.is_empty() || subject_stems.is_empty() {
        return false;
    }
    let squashed_subject = squash(subject);
    let squashed_title = squash(main_title);
    let title_in_subject = title_stems
        .iter()
        .filter(|stem| contains_stem(&squashed_subject, stem))
        .count();
    let subject_in_title = subject_stems
        .iter()
        .filter(|stem| contains_stem(&squashed_title, stem))
        .count();
    title_in_subject * 2 > title_stems.len() && subject_in_title * 2 >= subject_stems.len()
}

/// Whether a file in an article's gallery shows the article's subject: its
/// name carries the article's distinctive words (two of them for longer
/// titles). Galleries also hold pictures of related places and people.
fn file_matches_subject(file_title: &str, article_title: &str) -> bool {
    let main_title = article_title
        .split(['(', ','])
        .next()
        .unwrap_or(article_title);
    let stems = subject_stems(main_title);
    if stems.is_empty() {
        return false;
    }
    let squashed_file = squash(file_title);
    let matched = stems
        .iter()
        .filter(|stem| contains_stem(&squashed_file, stem))
        .count();
    matched >= stems.len().min(2)
}

/// Encyclopedia search on Wikipedia (Indonesian or English first, by the
/// query's language), with pictures only from articles about the subject and
/// a Wikimedia Commons top-up for picture searches.
async fn search_wikipedia(client: &reqwest::Client, query: &str) -> Result<SearchFindings, String> {
    let q = query.trim();
    if q.is_empty() {
        return Err("Query pencarian tidak boleh kosong".to_string());
    }

    let visual = is_visual_search_query(q);
    let is_logo = is_logo_query(q);
    let subject = image_subject(q);
    let candidates = if visual {
        [subject.clone(), q.to_string()]
    } else {
        [q.to_string(), extract_core_search_terms(q)]
    };
    let mut attempts: Vec<String> = Vec::new();
    for candidate in candidates {
        let candidate = candidate.trim().to_string();
        if !candidate.is_empty()
            && !attempts
                .iter()
                .any(|attempt| attempt.eq_ignore_ascii_case(&candidate))
        {
            attempts.push(candidate);
        }
    }
    let langs = if is_likely_indonesian(q) {
        ["id", "en"]
    } else {
        ["en", "id"]
    };

    let mut best: Option<SearchFindings> = None;
    let mut last_error = None;
    'search: for lang in langs {
        for attempt in &attempts {
            let findings =
                match wikipedia_attempt(client, lang, attempt, &subject, visual, is_logo).await {
                    Ok(findings) => findings,
                    Err(e) => {
                        warn!("Wikipedia API request failed for {lang} ({e})");
                        last_error = Some(e);
                        continue;
                    }
                };
            if findings.hits.is_empty() {
                continue;
            }
            let has_images = !findings.images.is_empty();
            if best.is_none() || has_images {
                best = Some(findings);
            }
            // A picture search keeps looking until some article has pictures.
            if !visual || has_images {
                break 'search;
            }
        }
    }

    let mut findings = best.unwrap_or_else(|| SearchFindings::new("Wikipedia"));
    if visual && (findings.images.len() < MIN_VISUAL_IMAGES || is_logo) {
        match search_commons_images(client, &subject, is_logo).await {
            Ok(urls) => findings.push_images(urls),
            Err(e) => debug!("Wikimedia Commons search failed ({e})"),
        }
    }

    if findings.is_empty() {
        return Err(last_error.unwrap_or_else(|| {
            format!("Tidak ada hasil ditemukan di ensiklopedia untuk query: \"{q}\"")
        }));
    }
    Ok(findings)
}

async fn wikipedia_attempt(
    client: &reqwest::Client,
    lang: &str,
    attempt: &str,
    subject: &str,
    visual: bool,
    is_logo: bool,
) -> Result<SearchFindings, String> {
    let url = format!(
        "https://{lang}.wikipedia.org/w/api.php?action=query&generator=search&gsrsearch={}&gsrlimit={MAX_SEARCH_HITS}&prop=pageimages|extracts&piprop=original|thumbnail&pithumbsize=1000&exintro=1&explaintext=1&exchars=350&format=json",
        urlencoding::encode(attempt)
    );
    let body = wiki_json(client, &url).await?;

    let mut findings = SearchFindings::new("Wikipedia");
    let Some(pages) = body.pointer("/query/pages").and_then(Value::as_object) else {
        return Ok(findings);
    };
    let mut pages: Vec<&Value> = pages.values().collect();
    pages.sort_by_key(|page| {
        page.get("index")
            .and_then(Value::as_i64)
            .unwrap_or(i64::MAX)
    });

    let mut gallery_article: Option<&str> = None;
    for page in pages.iter().take(MAX_SEARCH_HITS) {
        let title = page.get("title").and_then(Value::as_str).unwrap_or("");
        let page_url = format!(
            "https://{lang}.wikipedia.org/wiki/{}",
            urlencoding::encode(&title.replace(' ', "_"))
        );
        findings.push_hit(
            title,
            &page_url,
            page.get("extract").and_then(Value::as_str).unwrap_or(""),
        );

        // Search results also list related articles; only an article about
        // the subject itself may supply its pictures.
        if !visual || !article_matches_subject(title, subject) {
            continue;
        }
        // The thumbnail is a raster render even when the original is an SVG.
        let lead = [
            page.pointer("/thumbnail/source").and_then(Value::as_str),
            page.pointer("/original/source").and_then(Value::as_str),
        ];
        if let Some(url) = lead.into_iter().flatten().find(|url| {
            sanitize_and_validate_raster_url(url).is_some()
                && !is_unwanted_wiki_file(url, subject, is_logo)
        }) {
            findings.push_image(url);
        }
        gallery_article.get_or_insert(title);
    }

    if let Some(article) = gallery_article {
        match article_gallery(client, lang, article, subject, is_logo).await {
            Ok(urls) => findings.push_images(urls),
            Err(e) => debug!("Wikipedia gallery request failed ({e})"),
        }
    }
    Ok(findings)
}

/// Pictures used in an article that show its subject.
async fn article_gallery(
    client: &reqwest::Client,
    lang: &str,
    article_title: &str,
    subject: &str,
    is_logo: bool,
) -> Result<Vec<String>, String> {
    let url = format!(
        "https://{lang}.wikipedia.org/w/api.php?action=query&titles={}&generator=images&gimlimit=30&prop=imageinfo&iiprop=url&iiurlwidth=1000&format=json",
        urlencoding::encode(article_title)
    );
    let body = wiki_json(client, &url).await?;
    Ok(imageinfo_urls(&body, |file| {
        is_unwanted_wiki_file(file, subject, is_logo) || !file_matches_subject(file, article_title)
    }))
}

/// Picture files on Wikimedia Commons, which needs no key. Photo searches
/// are limited to bitmaps; a logo search also takes drawings, rendered as
/// PNG thumbnails.
async fn search_commons_images(
    client: &reqwest::Client,
    subject: &str,
    is_logo: bool,
) -> Result<Vec<String>, String> {
    let subject = subject.trim();
    if subject.is_empty() {
        return Ok(Vec::new());
    }
    let search = if is_logo {
        subject.to_string()
    } else {
        format!("filetype:bitmap {subject}")
    };
    let url = format!(
        "https://commons.wikimedia.org/w/api.php?action=query&generator=search&gsrsearch={}&gsrnamespace=6&gsrlimit={MAX_SEARCH_IMAGES}&prop=imageinfo&iiprop=url&iiurlwidth=1000&format=json",
        urlencoding::encode(&search)
    );
    let body = wiki_json(client, &url).await?;
    Ok(imageinfo_urls(&body, |file| {
        is_unwanted_wiki_file(file, subject, is_logo)
    }))
}

static RE_DDG_RESULT_BLOCK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"<div\s+class="(?P<class>result\b[^"]*)""#).expect("valid static regex")
});
static RE_DDG_RESULT_LINK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?s)<a\b(?P<attrs>[^>]*\bclass="result__a"[^>]*)>(?P<title>.*?)</a>"#)
        .expect("valid static regex")
});
static RE_DDG_RESULT_SNIPPET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?s)<(?:a|div|td)\b[^>]*\bclass="result__snippet"[^>]*>(?P<snippet>.*?)</(?:a|div|td)>"#,
    )
    .expect("valid static regex")
});
static RE_HREF: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"\bhref="(?P<href>[^"]*)""#).expect("valid static regex"));

struct DuckDuckGoResult {
    title: String,
    url: String,
    snippet: String,
    images: Vec<String>,
}

fn is_duckduckgo_host(host: &str) -> bool {
    host == "duckduckgo.com" || host.ends_with(".duckduckgo.com")
}

/// The destination of a DuckDuckGo result link. Organic links point at
/// `duckduckgo.com/l/?uddg=<target>`; links that stay on DuckDuckGo, such as
/// `y.js` ad redirects, give `None`.
fn unwrap_ddg_url(raw_href: &str) -> Option<String> {
    let decoded = html_escape::decode_html_entities(raw_href.trim());
    let absolute = if decoded.starts_with("//") {
        format!("https:{decoded}")
    } else if decoded.starts_with('/') {
        format!("https://duckduckgo.com{decoded}")
    } else {
        decoded.into_owned()
    };
    let parsed = Url::parse(&absolute).ok()?;
    let target = if is_duckduckgo_host(&parsed.host_str()?.to_ascii_lowercase()) {
        parsed
            .query_pairs()
            .find(|(key, _)| key == "uddg")
            .map(|(_, value)| value.into_owned())?
    } else {
        absolute
    };
    let target_url = Url::parse(&target).ok()?;
    let host = target_url.host_str()?.to_ascii_lowercase();
    (matches!(target_url.scheme(), "http" | "https") && !is_duckduckgo_host(&host))
        .then_some(target)
}

/// Organic results of a DuckDuckGo HTML page, one per `result` block; ad
/// blocks (`result--ad`) are skipped.
fn parse_duckduckgo_html(html: &str) -> Vec<DuckDuckGoResult> {
    let starts: Vec<(usize, bool)> = RE_DDG_RESULT_BLOCK
        .captures_iter(html)
        .filter_map(|captures| {
            let start = captures.get(0)?.start();
            let class = captures.name("class")?.as_str();
            let skip = class.contains("result--ad") || class.contains("result--no-result");
            Some((start, skip))
        })
        .collect();

    let mut results = Vec::new();
    for (index, &(start, skip)) in starts.iter().enumerate() {
        if skip {
            continue;
        }
        let end = starts.get(index + 1).map_or(html.len(), |&(next, _)| next);
        let block = &html[start..end];
        let Some(link) = RE_DDG_RESULT_LINK.captures(block) else {
            continue;
        };
        let Some(url) = link
            .name("attrs")
            .and_then(|attrs| RE_HREF.captures(attrs.as_str()))
            .and_then(|href| href.name("href"))
            .and_then(|href| unwrap_ddg_url(href.as_str()))
        else {
            continue;
        };
        let title = link
            .name("title")
            .map(|title| clean_html_to_text(title.as_str()))
            .unwrap_or_default();
        let snippet = RE_DDG_RESULT_SNIPPET
            .captures(block)
            .and_then(|captures| captures.name("snippet"))
            .map(|snippet| clean_html_to_text(snippet.as_str()))
            .unwrap_or_default();
        results.push(DuckDuckGoResult {
            title,
            url,
            snippet,
            images: extract_raster_images_from_html(block, Some("https://duckduckgo.com")),
        });
    }
    results
}

/// Keyless search on DuckDuckGo's HTML page. A failure, a block or a
/// captcha page starts [`DUCKDUCKGO_COOLDOWN`].
async fn search_duckduckgo(
    client: &reqwest::Client,
    query: &str,
) -> Result<SearchFindings, String> {
    let url = format!(
        "https://html.duckduckgo.com/html/?q={}",
        urlencoding::encode(query)
    );

    let resp = match client
        .get(&url)
        .header(USER_AGENT, BROWSER_USER_AGENT)
        .header(
            ACCEPT,
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        )
        .header(ACCEPT_LANGUAGE, "id,en-US;q=0.9,en;q=0.8")
        .header(REFERER, "https://duckduckgo.com/")
        .send()
        .await
    {
        Ok(resp) => resp,
        Err(e) => {
            DUCKDUCKGO_COOLDOWN.trip(DUCKDUCKGO_FAILURE_COOLDOWN);
            return Err(format!("Gagal mencari di DuckDuckGo: {e}"));
        }
    };

    // DuckDuckGo answers a suspected bot with 202 and a captcha page.
    let status = resp.status();
    if !status.is_success() || status == StatusCode::ACCEPTED {
        DUCKDUCKGO_COOLDOWN.trip(DUCKDUCKGO_FAILURE_COOLDOWN);
        return Err(format!("DuckDuckGo mengembalikan status HTTP {status}"));
    }

    let html = match read_text_bounded(resp).await {
        Ok(html) => html,
        Err(e) => {
            DUCKDUCKGO_COOLDOWN.trip(DUCKDUCKGO_FAILURE_COOLDOWN);
            return Err(format!("Gagal membaca respon DuckDuckGo: {e}"));
        }
    };
    if html.contains("anomaly-modal") || html.contains("challenge-form") {
        DUCKDUCKGO_COOLDOWN.trip(DUCKDUCKGO_FAILURE_COOLDOWN);
        return Err("DuckDuckGo meminta verifikasi anti-bot".to_string());
    }

    let results = parse_duckduckgo_html(&html);
    if results.is_empty() {
        return Err("Tidak ditemukan hasil pencarian".to_string());
    }

    let mut findings = SearchFindings::new("Web");
    for result in results.iter().take(MAX_SEARCH_HITS) {
        findings.push_hit(&result.title, &result.url, &result.snippet);
        findings.push_images(&result.images);
    }

    // No pictures on the result page itself: look at the top result pages.
    if is_visual_search_query(query) && findings.images.is_empty() {
        for result in results.iter().take(2) {
            // Result URLs come from third-party HTML, so they go through the
            // SSRF-safe fetcher: a crafted result pointing at a private or
            // loopback address (or redirecting there) is refused.
            if let Ok((page_html, final_url)) = super::fetch_public_html(
                &result.url,
                Duration::from_secs(4),
                MAX_SCRAPED_PAGE_BYTES,
            )
            .await
            {
                findings.push_images(extract_raster_images_from_html(
                    &page_html,
                    Some(&final_url),
                ));
            }
            if !findings.images.is_empty() {
                break;
            }
        }
    }

    Ok(findings)
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXA_MCP_REPLY: &str = "Title: Colosseum - Wikipedia\nURL: https://en.wikipedia.org/wiki/Colosseum\nPublished: 2024-01-01\nAuthor: N/A\nHighlights:\nThe Colosseum is an elliptical amphitheatre in the centre of Rome.\n![Colosseum at dusk](https://images.unsplash.com/photo-1552832230-c0197dd311b5?fm=jpg&q=60&w=3000)\n| Feature | Value |\n| --- | --- |\n| Built | 70-80 AD |\n\n---\n\nTitle: Great Wall of China.jpeg\nURL: https://commons.wikimedia.org/wiki/File:Great_Wall_of_China.jpeg\nHighlights:\nA photo of the wall.\n![avatar](https://example.org/avatars/user.jpg)\n\n---\n\nextra rule text that belongs to the previous result";

    #[test]
    fn exa_mcp_text_is_split_into_results_with_bodies() {
        let results = parse_exa_mcp_text(EXA_MCP_REPLY);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Colosseum - Wikipedia");
        assert_eq!(results[0].url, "https://en.wikipedia.org/wiki/Colosseum");
        assert!(results[0].body.contains("elliptical amphitheatre"));
        assert!(results[1].body.contains("belongs to the previous result"));
    }

    #[test]
    fn exa_mcp_reply_text_reads_sse_and_reports_errors() {
        let sse = format!(
            "event: message\ndata: {}\n\n",
            json!({"result": {"content": [{"type": "text", "text": "Title: A\nURL: https://a.example.org"}]}})
        );
        let text = exa_mcp_reply_text(&sse).expect("sse text");
        assert!(text.starts_with("Title: A"));

        let error = json!({"error": {"code": -32000, "message": "Rate limit exceeded"}});
        let failure = exa_mcp_reply_text(&error.to_string()).expect_err("json-rpc error");
        assert!(failure.contains("Rate limit exceeded"));

        let tool_error =
            json!({"result": {"isError": true, "content": [{"type": "text", "text": "quota"}]}});
        assert!(exa_mcp_reply_text(&tool_error.to_string()).is_err());
    }

    #[test]
    fn exa_mcp_results_yield_direct_images_and_commons_files() {
        let mut findings = SearchFindings::new("Exa AI");
        let mut commons = Vec::new();
        for result in parse_exa_mcp_text(EXA_MCP_REPLY) {
            findings.push_hit(&result.title, &result.url, &result.body);
            findings.push_text_images(&result.body, false);
            commons.extend(commons_file_title(&result.url));
        }
        assert_eq!(
            findings.images,
            vec!["https://images.unsplash.com/photo-1552832230-c0197dd311b5?fm=jpg&q=60&w=3000"]
        );
        assert_eq!(commons, vec!["File:Great Wall of China.jpeg"]);
        let summary = &findings.hits[0].summary;
        assert!(!summary.contains("!["), "{summary}");
        assert!(!summary.contains("---"), "{summary}");
        assert!(!summary.contains('\n'), "{summary}");
    }

    #[test]
    fn commons_file_titles_need_a_raster_file_page() {
        assert_eq!(
            commons_file_title("https://commons.wikimedia.org/wiki/File:Colosseo_2020.jpg"),
            Some("File:Colosseo 2020.jpg".to_string())
        );
        assert_eq!(
            commons_file_title("https://id.wikipedia.org/wiki/Berkas:Borobudur%20Temple.png"),
            Some("File:Borobudur Temple.png".to_string())
        );
        assert_eq!(
            commons_file_title("https://commons.wikimedia.org/wiki/File:Map.svg"),
            None
        );
        assert_eq!(
            commons_file_title("https://en.wikipedia.org/wiki/Colosseum"),
            None
        );
        assert_eq!(
            commons_file_title("https://example.org/wiki/File:A.jpg"),
            None
        );
    }

    const DDG_HTML: &str = r#"<div class="results">
<div class="result results_links results_links_deep result--ad ">
  <a rel="nofollow" class="result__a" href="https://duckduckgo.com/y.js?ad_domain=shop.example&amp;u3=x">Buy Colosseum tickets</a>
  <a class="result__snippet" href="https://duckduckgo.com/y.js?ad_domain=shop.example">Ad text</a>
</div>
<div class="result results_links results_links_deep web-result ">
  <div class="links_main links_deep result__body">
    <h2 class="result__title"><a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fen.wikipedia.org%2Fwiki%2FColosseum&amp;rut=abc">The <b>Colosseum</b> &amp; Rome</a></h2>
    <a class="result__snippet" href="//duckduckgo.com/l/?uddg=x">An <b>amphitheatre</b> in Rome.</a>
    <img src="https://upload.wikimedia.org/wikipedia/commons/d/de/Colosseo_2020.jpg">
  </div>
</div>
</div>"#;

    #[test]
    fn duckduckgo_ads_are_dropped_and_organic_links_unwrapped() {
        let results = parse_duckduckgo_html(DDG_HTML);
        assert_eq!(results.len(), 1, "only the organic result is kept");
        let result = &results[0];
        assert_eq!(result.url, "https://en.wikipedia.org/wiki/Colosseum");
        assert_eq!(result.title, "The Colosseum & Rome");
        assert_eq!(result.snippet, "An amphitheatre in Rome.");
        assert_eq!(
            result.images,
            vec!["https://upload.wikimedia.org/wikipedia/commons/d/de/Colosseo_2020.jpg"]
        );
    }

    #[test]
    fn duckduckgo_links_that_stay_on_duckduckgo_are_rejected() {
        assert_eq!(
            unwrap_ddg_url("https://duckduckgo.com/y.js?ad_domain=a.example&amp;u3=1"),
            None
        );
        assert_eq!(
            unwrap_ddg_url("/l/?uddg=https%3A%2F%2Fexample.org%2Fa%3Fb%3D1%26c%3D2&amp;rut=x"),
            Some("https://example.org/a?b=1&c=2".to_string())
        );
        assert_eq!(
            unwrap_ddg_url("https://example.org/page"),
            Some("https://example.org/page".to_string())
        );
        assert_eq!(unwrap_ddg_url("javascript:alert(1)"), None);
    }

    #[test]
    fn pictures_come_only_from_articles_about_the_subject() {
        assert!(article_matches_subject("Colosseum", "Colosseum Rome"));
        assert!(!article_matches_subject("Pantheon, Rome", "Colosseum Rome"));
        assert!(article_matches_subject("Koloseum", "Colosseum Roma"));
        assert!(article_matches_subject("Mount Bromo", "Gunung Bromo"));
        assert!(!article_matches_subject(
            "Candi Prambanan",
            "Candi Borobudur"
        ));
        assert!(!article_matches_subject("China", "Great Wall of China"));
        assert!(article_matches_subject(
            "Great Wall of China",
            "Great Wall of China"
        ));

        assert!(file_matches_subject("File:Colosseo 2020.jpg", "Colosseum"));
        assert!(!file_matches_subject(
            "File:046CupolaSPietro.jpg",
            "Colosseum"
        ));
        assert!(file_matches_subject(
            "File:GreatWall 2004 Summer 4.jpg",
            "Great Wall of China"
        ));
        assert!(!file_matches_subject(
            "File:Mutianyu.jpg",
            "Great Wall of China"
        ));
    }

    #[test]
    fn unwanted_wiki_files_are_matched_by_word() {
        let rome = "Colosseum";
        assert!(is_unwanted_wiki_file(
            "File:Italy location map.svg",
            rome,
            false
        ));
        assert!(is_unwanted_wiki_file("File:Flag of Italy.svg", rome, false));
        assert!(!is_unwanted_wiki_file("File:Flag of Italy.svg", rome, true));
        assert!(!is_unwanted_wiki_file(
            "File:Building of the Colosseum.jpg",
            rome,
            false
        ));
        assert!(!is_unwanted_wiki_file("File:Maple leaves.jpg", rome, false));
        assert!(!is_unwanted_wiki_file(
            "File:Mid-autumn festival.jpg",
            rome,
            false
        ));
    }

    #[test]
    fn drawings_video_stills_and_scans_are_not_photos() {
        let temple = "Candi Borobudur";
        assert!(is_unwanted_wiki_file(
            "File:Borobudur Cross Section id.svg",
            temple,
            false
        ));
        assert!(is_unwanted_wiki_file(
            "File:Borobudur, Java, Indonesia, February 2012.ogv",
            temple,
            false
        ));
        assert!(is_unwanted_wiki_file(
            "File:Borobudur guide 1920.pdf",
            temple,
            false
        ));
        assert!(is_unwanted_wiki_file(
            "File:Colosseum floor-plan.jpg",
            "Colosseum",
            false
        ));
        assert!(!is_unwanted_wiki_file(
            "File:Borobudur 2008.JPG",
            temple,
            false
        ));
        // A logo search takes vector drawings.
        assert!(!is_unwanted_wiki_file(
            "File:Python-logo-notext.svg",
            "Python",
            true
        ));
        // Lead images are checked by URL, including thumbnail renders.
        assert!(is_unwanted_wiki_file(
            "https://upload.wikimedia.org/wikipedia/commons/thumb/9/9d/Map_of_the_Great_Wall_of_China.jpg/1000px-Map_of_the_Great_Wall_of_China.jpg",
            "Great Wall of China",
            false
        ));
        assert!(is_unwanted_wiki_file(
            "https://upload.wikimedia.org/wikipedia/commons/thumb/f/f3/Borobudur_Cross_Section_id.svg/1000px-Borobudur_Cross_Section_id.svg.png",
            temple,
            false
        ));
    }

    #[test]
    fn words_the_search_asks_for_are_never_unwanted() {
        assert!(!is_unwanted_wiki_file(
            "File:Harbor seal on a rock.jpg",
            "harbor seal",
            false
        ));
        assert!(is_unwanted_wiki_file(
            "File:Seal of Rome.png",
            "Colosseum",
            false
        ));
        assert!(!is_unwanted_wiki_file(
            "File:Old map of Batavia.jpg",
            "peta Batavia map",
            false
        ));
        assert!(!is_unwanted_wiki_file(
            "File:Colosseum cross section.jpg",
            "Colosseum cross section",
            false
        ));
    }

    #[test]
    fn image_subject_drops_request_and_photo_words() {
        assert_eq!(image_subject("Colosseum Rome photo"), "Colosseum Rome");
        assert_eq!(image_subject("foto Candi Borobudur"), "Candi Borobudur");
        assert_eq!(
            image_subject("Show me 3 photos of Mount Bromo"),
            "Mount Bromo"
        );
        assert_eq!(image_subject("foto"), "foto");
    }

    #[test]
    fn rendered_findings_list_images_before_results() {
        let mut findings = SearchFindings::new("Exa AI");
        findings.push_hit(
            "Colosseum",
            "https://example.org/colosseum",
            &"x".repeat(5_000),
        );
        findings
            .push_image("https://upload.wikimedia.org/wikipedia/commons/d/de/Colosseo_2020.jpg");
        let rendered = findings.render("foto Colosseum", true);

        let images_at = rendered
            .find("🖼️ **URL Foto/Gambar Raster Terverifikasi")
            .expect("image section");
        let result_at = rendered.find("1. **Colosseum**").expect("result");
        assert!(images_at < result_at);
        assert!(rendered.starts_with("[Hasil Pencarian Exa AI untuk \"foto Colosseum\"]"));
        assert!(rendered.chars().count() < 1_200, "summary is capped");

        let empty = SearchFindings::new("Web").render("foto Colosseum", true);
        assert!(empty.contains("ℹ️ **Catatan Media**"));
        assert!(!SearchFindings::new("Web")
            .render("sejarah Roma", false)
            .contains("Catatan Media"));
    }

    #[test]
    fn findings_cap_hits_and_images() {
        let mut findings = SearchFindings::new("Web");
        for index in 0..10 {
            findings.push_hit("t", &format!("https://example.org/{index}"), "s");
            findings.push_image(&format!("https://example.org/{index}.jpg"));
            findings.push_image(&format!("https://example.org/{index}.jpg"));
        }
        assert_eq!(findings.hits.len(), MAX_SEARCH_HITS);
        assert_eq!(findings.images.len(), MAX_SEARCH_IMAGES);
    }

    #[test]
    fn binary_page_text_is_not_used_as_a_summary() {
        let garbage = "A\u{fffd}\u{fffd}=}\u{fffd}ڕ\u{fffd}\u{fffd}6:4>`iZ\u{fffd}\u{fffd}c\u{fffd}\u{fffd}#\u{fffd}p>\u{fffd}#()PR\u{fffd}$CEJ\u{fffd}t\u{fffd}\u{fffd}-\u{fffd}\u{fffd}\u{fffd}o\u{fffd}\u{fffd}ŝ\u{1}\u{2}";
        let mut findings = SearchFindings::new("Exa AI");
        findings.push_hit(
            "Great Wall pictures",
            "https://unsplash.com/s/photos/x",
            garbage,
        );
        assert_eq!(findings.hits[0].summary, "");
        assert!(!findings.render("q", false).contains("Ringkasan"));

        // A stray replacement character in real text is kept.
        assert_eq!(
            compact_text("Caf\u{fffd} near the Colosseum, open daily", 100),
            "Caf\u{fffd} near the Colosseum, open daily"
        );
    }

    #[test]
    fn cooldown_trips_and_keeps_the_later_deadline() {
        let cooldown = Cooldown::new();
        assert!(!cooldown.is_active());
        cooldown.trip(Duration::from_secs(600));
        assert!(cooldown.is_active());
        cooldown.trip(Duration::ZERO);
        assert!(cooldown.is_active(), "a shorter trip must not end it early");
        let left = cooldown.remaining().expect("time left");
        assert!(left > Duration::from_secs(590) && left <= Duration::from_secs(600));
        cooldown.clear();
        assert!(!cooldown.is_active());
        assert_eq!(cooldown.remaining(), None);
    }

    #[test]
    fn rate_limit_cooldown_follows_retry_after_within_bounds() {
        assert_eq!(rate_limit_cooldown(None), EXA_MCP_RATE_LIMIT_COOLDOWN);
        assert_eq!(rate_limit_cooldown(Some("120")), Duration::from_secs(120));
        assert_eq!(rate_limit_cooldown(Some("5")), Duration::from_secs(60));
        assert_eq!(
            rate_limit_cooldown(Some("99999")),
            Duration::from_secs(3600)
        );
        assert_eq!(
            rate_limit_cooldown(Some("Wed, 21 Oct 2026 07:28:00 GMT")),
            EXA_MCP_RATE_LIMIT_COOLDOWN
        );
    }
}
