use std::env;

use super::{
    extract_core_search_terms, extract_raster_images_from_html, format_no_images_guidance,
    format_verified_images_section, is_likely_indonesian, is_logo_query, is_visual_search_query,
    sanitize_and_validate_raster_url, RE_DDG_SNIPPET, RE_DDG_TITLE,
};
use reqwest::header::{ACCEPT, ACCEPT_LANGUAGE, REFERER, USER_AGENT};
use serde_json::{json, Value};
use std::time::Duration;
use tracing::{info, warn};
use url::Url;

pub fn get_brave_key() -> Option<String> {
    env::var("BRAVE_API_KEY")
        .ok()
        .or_else(|| crate::ai::service::load_app_setting("BRAVE_API_KEY"))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

pub fn get_tavily_key() -> Option<String> {
    env::var("TAVILY_API_KEY")
        .or_else(|_| env::var("TAVILY_KEY"))
        .ok()
        .or_else(|| {
            crate::ai::service::load_app_setting("TAVILY_API_KEY")
                .or_else(|| crate::ai::service::load_app_setting("TAVILY_KEY"))
        })
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

pub fn get_exa_key() -> Option<String> {
    env::var("EXA_API_KEY")
        .or_else(|_| env::var("EXA_KEY"))
        .ok()
        .or_else(|| {
            crate::ai::service::load_app_setting("EXA_API_KEY")
                .or_else(|| crate::ai::service::load_app_setting("EXA_KEY"))
        })
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
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
        "Exa MCP (Keyless) \u{2192} DuckDuckGo / Wikipedia".to_string()
    };

    let mcp_url = get_configured_mcp_url();
    (engine_name, mcp_url)
}

pub fn get_configured_mcp_url() -> String {
    env::var("EXA_MCP_URL")
        .ok()
        .or_else(|| crate::ai::service::load_app_setting("EXA_MCP_URL"))
        .filter(|url| !url.trim().is_empty())
        .unwrap_or_else(|| "https://mcp.exa.ai/".to_string())
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

    let is_visual = is_visual_search_query(q);

    // 1. Check Brave Search API
    if let Some(brave_key) = get_brave_key() {
        info!("Using Brave Search API for query: {q}");
        match search_brave(&client, &brave_key, q).await {
            Ok(mut res) => {
                if is_visual && !res.contains("🖼️ **URL Foto/Gambar Raster Terverifikasi") {
                    if let Ok(wiki_res) = search_wikipedia(&client, q).await {
                        if wiki_res.contains("🖼️ **URL Foto/Gambar Raster Terverifikasi") {
                            let clean_res = res
                                .replace(&format_no_images_guidance(q), "")
                                .trim()
                                .to_string();
                            res = clean_res;
                            res.push_str("\n\n---\n\n");
                            res.push_str(&wiki_res);
                        }
                    }
                }
                return res;
            }
            Err(e) => warn!("Brave search failed ({e}), falling back to other providers"),
        }
    }

    // 2. Check Tavily API
    if let Some(tavily_key) = get_tavily_key() {
        info!("Using Tavily API for query: {q}");
        match search_tavily(&client, &tavily_key, q).await {
            Ok(mut res) => {
                if is_visual && !res.contains("🖼️ **URL Foto/Gambar Raster Terverifikasi") {
                    if let Ok(wiki_res) = search_wikipedia(&client, q).await {
                        if wiki_res.contains("🖼️ **URL Foto/Gambar Raster Terverifikasi") {
                            let clean_res = res
                                .replace(&format_no_images_guidance(q), "")
                                .trim()
                                .to_string();
                            res = clean_res;
                            res.push_str("\n\n---\n\n");
                            res.push_str(&wiki_res);
                        }
                    }
                }
                return res;
            }
            Err(e) => warn!("Tavily search failed ({e}), falling back to other providers"),
        }
    }

    // 3. Check Exa REST API
    if let Some(exa_key) = get_exa_key() {
        info!("Using Exa API for query: {q}");
        match search_exa_api(&client, &exa_key, q).await {
            Ok(mut res) => {
                if is_visual && !res.contains("🖼️ **URL Foto/Gambar Raster Terverifikasi") {
                    if let Ok(wiki_res) = search_wikipedia(&client, q).await {
                        if wiki_res.contains("🖼️ **URL Foto/Gambar Raster Terverifikasi") {
                            let clean_res = res
                                .replace(&format_no_images_guidance(q), "")
                                .trim()
                                .to_string();
                            res = clean_res;
                            res.push_str("\n\n---\n\n");
                            res.push_str(&wiki_res);
                        } else if !res.contains("ℹ️ **Catatan Media**") {
                            res.push_str(&format_no_images_guidance(q));
                        }
                    } else if !res.contains("ℹ️ **Catatan Media**") {
                        res.push_str(&format_no_images_guidance(q));
                    }
                }
                return res;
            }
            Err(e) => warn!("Exa API search failed ({e}), falling back to other providers"),
        }
    }

    // 4. Default / Keyless Exa MCP
    let mcp_url = get_configured_mcp_url();
    info!("Trying Exa Keyless MCP for query: {q}");
    match search_exa_mcp(&client, &mcp_url, q).await {
        Ok(mut res) => {
            if is_visual && !res.contains("🖼️ **URL Foto/Gambar Raster Terverifikasi") {
                if let Ok(wiki_res) = search_wikipedia(&client, q).await {
                    if wiki_res.contains("🖼️ **URL Foto/Gambar Raster Terverifikasi") {
                        let clean_res = res
                            .replace(&format_no_images_guidance(q), "")
                            .trim()
                            .to_string();
                        res = clean_res;
                        res.push_str("\n\n---\n\n");
                        res.push_str(&wiki_res);
                    } else if !res.contains("ℹ️ **Catatan Media**") {
                        res.push_str(&format_no_images_guidance(q));
                    }
                } else if !res.contains("ℹ️ **Catatan Media**") {
                    res.push_str(&format_no_images_guidance(q));
                }
            }
            return res;
        }
        Err(e) => {
            warn!("Gagal menghubungi Exa MCP ({e}), beralih ke DuckDuckGo...");
        }
    }

    // 5. DuckDuckGo Search Fallback
    info!("Using DuckDuckGo for query: {q}");
    match search_duckduckgo(&client, q).await {
        Ok(mut res) => {
            if is_visual && !res.contains("🖼️ **URL Foto/Gambar Raster Terverifikasi") {
                if let Ok(wiki_res) = search_wikipedia(&client, q).await {
                    if wiki_res.contains("🖼️ **URL Foto/Gambar Raster Terverifikasi") {
                        let clean_res = res
                            .replace(&format_no_images_guidance(q), "")
                            .trim()
                            .to_string();
                        res = clean_res;
                        res.push_str("\n\n---\n\n");
                        res.push_str(&wiki_res);
                    }
                }
            }
            return res;
        }
        Err(e) => {
            warn!("DuckDuckGo did not return results or was blocked; trying Wikipedia knowledge base: {q}");
            warn!("Koneksi ke DuckDuckGo gagal ({e}). Catatan: Domain DuckDuckGo diblokir oleh beberapa ISP/Kominfo di Indonesia. Disarankan menggunakan TAVILY_API_KEY, EXA_API_KEY, atau BRAVE_API_KEY untuk hasil yang cepat.");
        }
    }

    // 6. Wikipedia Knowledge Base Fallback
    match search_wikipedia(&client, q).await {
        Ok(res) => {
            if res.trim().is_empty() {
                format!(
                    "[Informasi Pencarian Web]\nPencarian daring untuk topik \"{q}\" telah selesai namun tidak menghasilkan data teks.\n\nℹ️ **Panduan Asisten**: Berikan tanggapan deskriptif dan faktual mengenai topik \"{q}\" berdasarkan pengetahuan internal Anda secara lengkap dalam teks Markdown standar."
                )
            } else {
                res
            }
        }
        Err(e) => {
            warn!("Wikipedia search failed ({e})");
            format!(
                "[Informasi Pencarian Web]\nPencarian web daring untuk topik \"{q}\" saat ini tidak dapat diselesaikan karena kendala koneksi atau penyedia pencarian sedang tidak tersedia ({e}).\n\nℹ️ **Panduan Asisten**: Berikan tanggapan deskriptif dan faktual mengenai topik \"{q}\" berdasarkan pengetahuan internal Anda secara lengkap. Jika pengguna meminta gambar atau foto, jelaskan informasi visualnya secara naratif dalam teks Markdown dan hindari memanggil tool multimedia fiktif."
            )
        }
    }
}

#[inline]
fn format_search_item(index: usize, title: &str, url: &str, summary: &str) -> String {
    format!("{index}. **{title}**\n   URL: {url}\n   Ringkasan: {summary}\n\n")
}

async fn search_brave(
    client: &reqwest::Client,
    api_key: &str,
    query: &str,
) -> Result<String, String> {
    let is_visual = is_visual_search_query(query);
    let mut verified_images = Vec::new();

    if is_visual {
        let img_search_url = format!(
            "https://api.search.brave.com/res/v1/images/search?q={}&count=5",
            urlencoding::encode(query)
        );
        if let Ok(resp) = client
            .get(&img_search_url)
            .header("X-Subscription-Token", api_key)
            .header(ACCEPT, "application/json")
            .send()
            .await
        {
            if resp.status().is_success() {
                if let Ok(img_body) = resp.json::<Value>().await {
                    if let Some(results) = img_body.get("results").and_then(Value::as_array) {
                        for item in results.iter().take(5) {
                            let raw_url = item
                                .get("properties")
                                .and_then(|p| p.get("url"))
                                .and_then(Value::as_str)
                                .or_else(|| {
                                    item.get("thumbnail")
                                        .and_then(|t| t.get("src"))
                                        .and_then(Value::as_str)
                                });
                            if let Some(u) = raw_url {
                                if let Some(valid) = sanitize_and_validate_raster_url(u) {
                                    if !verified_images.contains(&valid) {
                                        verified_images.push(valid);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    let url = format!(
        "https://api.search.brave.com/res/v1/web/search?q={}&count=5",
        urlencoding::encode(query)
    );

    let resp = client
        .get(&url)
        .header("X-Subscription-Token", api_key)
        .header(ACCEPT, "application/json")
        .send()
        .await
        .map_err(|e| format!("Gagal menghubungi Brave Search API: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        return Err(format!("Brave Search API returned HTTP {status}"));
    }

    let body: Value = resp
        .json()
        .await
        .map_err(|e| format!("Gagal membaca JSON Brave: {e}"))?;

    let mut out = String::new();
    if let Some(results) = body
        .get("web")
        .and_then(|w| w.get("results"))
        .and_then(Value::as_array)
    {
        for (i, item) in results.iter().take(5).enumerate() {
            let title = item
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("Tanpa Judul");
            let url = item.get("url").and_then(Value::as_str).unwrap_or("");
            let desc = item
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("");
            out.push_str(&format_search_item(i + 1, title, url, desc));

            if let Some(thumb) = item
                .get("thumbnail")
                .and_then(|t| t.get("src").or_else(|| t.get("original")))
                .and_then(Value::as_str)
            {
                if let Some(valid) = sanitize_and_validate_raster_url(thumb) {
                    if !verified_images.contains(&valid) {
                        verified_images.push(valid);
                    }
                }
            }
        }
    }

    if let Some(pics) = body
        .get("pictures")
        .and_then(|p| p.get("results"))
        .and_then(Value::as_array)
    {
        for item in pics.iter().take(5) {
            let raw_url = item
                .get("thumbnail")
                .and_then(|t| t.get("src"))
                .and_then(Value::as_str);
            if let Some(u) = raw_url {
                if let Some(valid) = sanitize_and_validate_raster_url(u) {
                    if !verified_images.contains(&valid) {
                        verified_images.push(valid);
                    }
                }
            }
        }
    }

    if out.trim().is_empty() {
        Ok(format!(
            "Tidak ada hasil ditemukan di Brave untuk query \"{query}\"."
        ))
    } else {
        let mut res = format!("[Hasil Pencarian Brave untuk \"{query}\"]\n\n{out}")
            .trim()
            .to_string();
        if !verified_images.is_empty() {
            res.push_str(&format_verified_images_section(&verified_images));
        } else if is_visual {
            res.push_str(&format_no_images_guidance(query));
        }
        Ok(res)
    }
}

async fn search_tavily(
    client: &reqwest::Client,
    api_key: &str,
    query: &str,
) -> Result<String, String> {
    let is_visual = is_visual_search_query(query);
    let resp = client
        .post("https://api.tavily.com/search")
        .json(&json!({
            "api_key": api_key,
            "query": query,
            "include_answer": true,
            "include_images": true,
            "max_results": 5,
            "search_depth": "basic"
        }))
        .send()
        .await
        .map_err(|e| format!("Gagal menghubungi Tavily API: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        return Err(format!("Tavily API returned HTTP {status}"));
    }

    let body: Value = resp
        .json()
        .await
        .map_err(|e| format!("Gagal membaca JSON Tavily: {e}"))?;

    let mut out = String::new();
    if let Some(answer) = body
        .get("answer")
        .and_then(Value::as_str)
        .filter(|a| !a.is_empty())
    {
        out.push_str(&format!("💡 **Jawaban Ringkas**: {}\n\n", answer));
    }

    let mut verified_images = Vec::new();

    if let Some(images) = body.get("images").and_then(Value::as_array) {
        for img in images {
            let raw_url = if let Some(s) = img.as_str() {
                Some(s)
            } else {
                img.get("url").and_then(Value::as_str)
            };
            if let Some(u) = raw_url {
                if let Some(valid_url) = sanitize_and_validate_raster_url(u) {
                    if !verified_images.contains(&valid_url) {
                        verified_images.push(valid_url);
                    }
                }
            }
        }
    }

    if let Some(results) = body.get("results").and_then(Value::as_array) {
        for (i, item) in results.iter().take(5).enumerate() {
            let title = item
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("Tanpa Judul");
            let url = item.get("url").and_then(Value::as_str).unwrap_or("");
            let content = item.get("content").and_then(Value::as_str).unwrap_or("");
            out.push_str(&format_search_item(i + 1, title, url, content));

            if let Some(img_u) = item.get("image").and_then(Value::as_str) {
                if let Some(valid) = sanitize_and_validate_raster_url(img_u) {
                    if !verified_images.contains(&valid) {
                        verified_images.push(valid);
                    }
                }
            }
        }
    }

    if out.trim().is_empty() {
        Ok(format!(
            "Tidak ada hasil ditemukan di Tavily untuk query \"{query}\"."
        ))
    } else {
        let mut res = format!("[Hasil Pencarian Tavily untuk \"{query}\"]\n\n{out}")
            .trim()
            .to_string();
        if !verified_images.is_empty() {
            res.push_str(&format_verified_images_section(&verified_images));
        } else if is_visual {
            res.push_str(&format_no_images_guidance(query));
        }
        Ok(res)
    }
}

async fn search_exa_api(
    client: &reqwest::Client,
    api_key: &str,
    query: &str,
) -> Result<String, String> {
    let resp = client
        .post("https://api.exa.ai/search")
        .header("x-api-key", api_key)
        .header(ACCEPT, "application/json")
        .json(&json!({
            "query": query,
            "numResults": 5,
            "highlights": true
        }))
        .send()
        .await
        .map_err(|e| format!("Gagal menghubungi Exa API: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        return Err(format!("Exa API returned HTTP {status}"));
    }

    let body: Value = resp
        .json()
        .await
        .map_err(|e| format!("Gagal membaca JSON Exa: {e}"))?;

    let mut out = String::new();
    if let Some(results) = body.get("results").and_then(Value::as_array) {
        for (i, item) in results.iter().take(5).enumerate() {
            let title = item
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("Tanpa Judul");
            let url = item.get("url").and_then(Value::as_str).unwrap_or("");
            let highlight = item
                .get("highlights")
                .and_then(Value::as_array)
                .and_then(|arr| arr.first())
                .and_then(Value::as_str)
                .or_else(|| item.get("text").and_then(Value::as_str))
                .unwrap_or("");
            out.push_str(&format_search_item(i + 1, title, url, highlight));
        }
    }

    if out.trim().is_empty() {
        Ok(format!(
            "Tidak ada hasil ditemukan di Exa untuk query \"{query}\"."
        ))
    } else {
        Ok(
            format!("[Hasil Pencarian Exa AI untuk \"{query}\"]\n\n{out}")
                .trim()
                .to_string(),
        )
    }
}

pub(crate) async fn search_exa_mcp(
    _client: &reqwest::Client,
    mcp_url: &str,
    query: &str,
) -> Result<String, String> {
    let resolved = crate::bot::url_policy::resolve_download_url(mcp_url).await?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .resolve(&resolved.host, resolved.address)
        .build()
        .map_err(|e| format!("Gagal menginisialisasi client HTTP Exa MCP: {e}"))?;

    let resp = client
        .post(resolved.url)
        .header(ACCEPT, "application/json, text/event-stream")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "web_search_exa",
                "arguments": {
                    "query": query
                }
            }
        }))
        .send()
        .await
        .map_err(|e| format!("Gagal menghubungi Exa MCP ({e})"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        return Err(format!("Exa MCP returned HTTP {status}"));
    }

    let text = resp
        .text()
        .await
        .map_err(|e| format!("Gagal membaca stream Exa MCP: {e}"))?;

    let parsed_text = if let Ok(val) = serde_json::from_str::<Value>(&text) {
        if let Some(content) = val
            .get("result")
            .and_then(|r| r.get("content"))
            .and_then(Value::as_array)
        {
            content
                .iter()
                .filter_map(|c| c.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n\n")
        } else {
            String::new()
        }
    } else {
        let mut extracted: Vec<String> = Vec::new();
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("data:") {
                let rest = rest.trim();
                if let Ok(val) = serde_json::from_str::<Value>(rest) {
                    if let Some(c_arr) = val
                        .get("result")
                        .and_then(|r| r.get("content"))
                        .and_then(Value::as_array)
                    {
                        for c in c_arr {
                            if let Some(t) = c.get("text").and_then(Value::as_str) {
                                extracted.push(t.to_string());
                            }
                        }
                    }
                }
            }
        }
        extracted.join("\n\n")
    };

    if parsed_text.trim().is_empty() {
        Err("Exa MCP tidak mengembalikan konten yang valid.".to_string())
    } else {
        Ok(
            format!("[Hasil Pencarian Exa AI untuk \"{query}\"]\n\n{parsed_text}")
                .trim()
                .to_string(),
        )
    }
}

fn unwrap_ddg_url(raw_url: &str) -> String {
    if let Ok(parsed) = Url::parse(raw_url) {
        if let Some(uddg) = parsed
            .query_pairs()
            .find(|(k, _)| k == "uddg")
            .map(|(_, v)| v.to_string())
        {
            if let Ok(decoded) = urlencoding::decode(&uddg) {
                return decoded.to_string();
            }
        }
    } else if let Ok(parsed) = Url::parse(&format!("https://duckduckgo.com{raw_url}")) {
        if let Some(uddg) = parsed
            .query_pairs()
            .find(|(k, _)| k == "uddg")
            .map(|(_, v)| v.to_string())
        {
            if let Ok(decoded) = urlencoding::decode(&uddg) {
                return decoded.to_string();
            }
        }
    }
    raw_url.to_string()
}

async fn search_duckduckgo(client: &reqwest::Client, query: &str) -> Result<String, String> {
    let is_visual = is_visual_search_query(query);
    let url = format!(
        "https://html.duckduckgo.com/html/?q={}",
        urlencoding::encode(query)
    );

    let resp = client
        .get(&url)
        .header(
            USER_AGENT,
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36",
        )
        .header(
            ACCEPT,
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        )
        .header(ACCEPT_LANGUAGE, "id,en-US;q=0.9,en;q=0.8")
        .header(REFERER, "https://duckduckgo.com/")
        .send()
        .await
        .map_err(|e| format!("Gagal mencari di DuckDuckGo: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        return Err(format!("DuckDuckGo mengembalikan status HTTP {status}"));
    }

    let html = resp
        .text()
        .await
        .map_err(|e| format!("Gagal membaca respon DuckDuckGo: {e}"))?;

    let mut out = String::new();
    let urls: Vec<String> = RE_DDG_TITLE
        .captures_iter(&html)
        .take(5)
        .filter_map(|c| c.name("url").map(|m| unwrap_ddg_url(m.as_str().trim())))
        .collect();
    let snippets: Vec<String> = RE_DDG_SNIPPET
        .captures_iter(&html)
        .take(5)
        .filter_map(|c| {
            c.name("snippet").map(|m| {
                let cleaned = m.as_str().replace("<b>", "").replace("</b>", "");
                html_escape::decode_html_entities(&cleaned).to_string()
            })
        })
        .collect();

    for i in 0..urls.len().min(snippets.len()) {
        out.push_str(&format_search_item(
            i + 1,
            "Hasil Pencarian",
            &urls[i],
            &snippets[i],
        ));
    }

    let mut extracted_images =
        extract_raster_images_from_html(&html, Some("https://duckduckgo.com"));

    // If visual query and no images were directly in DDG HTML, scrape top result URLs
    if is_visual && extracted_images.is_empty() {
        for target_url in urls.iter().take(2) {
            if target_url.starts_with("http://") || target_url.starts_with("https://") {
                if let Ok(page_resp) = client
                    .get(target_url)
                    .timeout(Duration::from_secs(4))
                    .header(
                        USER_AGENT,
                        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36",
                    )
                    .header(
                        ACCEPT,
                        "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
                    )
                    .send()
                    .await
                {
                    if page_resp.status().is_success() {
                        if let Ok(page_html) = page_resp.text().await {
                            let scraped = extract_raster_images_from_html(&page_html, Some(target_url));
                            for img in scraped {
                                if !extracted_images.contains(&img) {
                                    extracted_images.push(img);
                                }
                                if extracted_images.len() >= 6 {
                                    break;
                                }
                            }
                        }
                    }
                }
            }
            if !extracted_images.is_empty() {
                break;
            }
        }
    }

    if out.trim().is_empty() {
        Err("Tidak ditemukan hasil pencarian".to_string())
    } else {
        let mut res = format!("[Hasil Pencarian Web untuk \"{query}\"]\n\n{out}")
            .trim()
            .to_string();
        if !extracted_images.is_empty() {
            res.push_str(&format_verified_images_section(&extracted_images));
        } else if is_visual {
            res.push_str(&format_no_images_guidance(query));
        }
        Ok(res)
    }
}

async fn fetch_wikipedia_article_images(
    client: &reqwest::Client,
    lang: &str,
    article_title: &str,
    is_logo_search: bool,
) -> Result<Vec<String>, String> {
    let url = format!(
        "https://{lang}.wikipedia.org/w/api.php?action=query&titles={}&generator=images&gimlimit=12&prop=imageinfo&iiprop=url&iiurlwidth=1000&format=json",
        urlencoding::encode(article_title)
    );

    let resp = client
        .get(&url)
        .header(
            USER_AGENT,
            concat!(
                "xiao/",
                env!("CARGO_PKG_VERSION"),
                " (Telegram Bot Assistant)"
            ),
        )
        .send()
        .await
        .map_err(|e| format!("Wikipedia gallery request failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("Wikipedia gallery status HTTP {}", resp.status()));
    }

    let body: Value = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse Wikipedia gallery JSON: {e}"))?;

    let mut images = Vec::new();
    if let Some(pages_obj) = body
        .get("query")
        .and_then(|q| q.get("pages"))
        .and_then(Value::as_object)
    {
        for page in pages_obj.values() {
            let title = page.get("title").and_then(Value::as_str).unwrap_or("");
            let title_lower = title.to_ascii_lowercase();

            if !is_logo_search {
                if title_lower.contains("logo")
                    || title_lower.contains("flag")
                    || title_lower.contains("icon")
                    || title_lower.contains("symbol")
                    || title_lower.contains("disambig")
                    || title_lower.contains("ui")
                    || title_lower.contains("locator")
                    || title_lower.contains("map")
                    || title_lower.contains("peta")
                    || title_lower.contains("diagram")
                    || title_lower.contains("insignia")
                    || title_lower.contains("coat_of_arms")
                    || title_lower.contains("lambang")
                    || title_lower.contains("stub")
                {
                    continue;
                }
            } else if title_lower.contains("disambig")
                || title_lower.contains("ui")
                || title_lower.contains("locator")
                || title_lower.contains("map")
                || title_lower.contains("peta")
                || title_lower.contains("stub")
            {
                continue;
            }

            if let Some(info_arr) = page.get("imageinfo").and_then(Value::as_array) {
                if let Some(first_info) = info_arr.first() {
                    let candidate_url = first_info
                        .get("thumburl")
                        .and_then(Value::as_str)
                        .and_then(sanitize_and_validate_raster_url)
                        .or_else(|| {
                            first_info
                                .get("url")
                                .and_then(Value::as_str)
                                .and_then(sanitize_and_validate_raster_url)
                        });
                    if let Some(valid_url) = candidate_url {
                        if !images.contains(&valid_url) {
                            images.push(valid_url);
                        }
                    }
                }
            }
        }
    }

    Ok(images)
}

async fn search_wikimedia_commons_files(
    client: &reqwest::Client,
    query: &str,
) -> Result<Vec<String>, String> {
    let q = query.trim();
    if q.is_empty() {
        return Ok(Vec::new());
    }
    let url = format!(
        "https://commons.wikimedia.org/w/api.php?action=query&generator=search&gsrsearch={}&gsrnamespace=6&gsrlimit=6&prop=imageinfo&iiprop=url&iiurlwidth=1000&format=json",
        urlencoding::encode(q)
    );

    let resp = client
        .get(&url)
        .header(
            USER_AGENT,
            concat!(
                "xiao/",
                env!("CARGO_PKG_VERSION"),
                " (Telegram Bot Assistant)"
            ),
        )
        .send()
        .await
        .map_err(|e| format!("Wikimedia Commons request failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("Wikimedia Commons status HTTP {}", resp.status()));
    }

    let body: Value = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse Wikimedia Commons JSON: {e}"))?;

    let mut images = Vec::new();
    if let Some(pages_obj) = body
        .get("query")
        .and_then(|qu| qu.get("pages"))
        .and_then(Value::as_object)
    {
        for page in pages_obj.values() {
            let title = page.get("title").and_then(Value::as_str).unwrap_or("");
            let title_lower = title.to_ascii_lowercase();
            if title_lower.contains("disambig")
                || title_lower.contains("locator")
                || title_lower.contains("map")
                || title_lower.contains("peta")
                || title_lower.contains("stub")
            {
                continue;
            }

            if let Some(info_arr) = page.get("imageinfo").and_then(Value::as_array) {
                if let Some(first_info) = info_arr.first() {
                    let candidate_url = first_info
                        .get("thumburl")
                        .and_then(Value::as_str)
                        .and_then(sanitize_and_validate_raster_url)
                        .or_else(|| {
                            first_info
                                .get("url")
                                .and_then(Value::as_str)
                                .and_then(sanitize_and_validate_raster_url)
                        });
                    if let Some(valid_url) = candidate_url {
                        if !images.contains(&valid_url) {
                            images.push(valid_url);
                        }
                    }
                }
            }
        }
    }

    Ok(images)
}

async fn search_wikipedia(client: &reqwest::Client, query: &str) -> Result<String, String> {
    let q = query.trim();
    if q.is_empty() {
        return Err("Query pencarian tidak boleh kosong".to_string());
    }

    let langs = if is_likely_indonesian(q) {
        vec!["id", "en"]
    } else {
        vec!["en", "id"]
    };

    let core_terms = extract_core_search_terms(q);
    let is_visual = is_visual_search_query(q);
    let is_logo = is_logo_query(q);

    let search_attempts =
        if is_visual && !core_terms.is_empty() && core_terms.to_lowercase() != q.to_lowercase() {
            vec![core_terms.as_str(), q]
        } else if !core_terms.is_empty() && core_terms.to_lowercase() != q.to_lowercase() {
            vec![q, core_terms.as_str()]
        } else {
            vec![q]
        };

    for lang in &langs {
        for attempt in &search_attempts {
            let url = format!(
                "https://{lang}.wikipedia.org/w/api.php?action=query&generator=search&gsrsearch={}&gsrlimit=5&prop=pageimages|extracts&piprop=original|thumbnail&pithumbsize=1000&exintro=1&explaintext=1&exchars=350&format=json",
                urlencoding::encode(attempt)
            );

            let resp = match client
                .get(&url)
                .header(
                    USER_AGENT,
                    concat!(
                        "xiao/",
                        env!("CARGO_PKG_VERSION"),
                        " (Telegram Bot Assistant)"
                    ),
                )
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    warn!("Wikipedia API connection failed for {lang} ({e})");
                    continue;
                }
            };

            if !resp.status().is_success() {
                continue;
            }

            let body: Value = match resp.json().await {
                Ok(b) => b,
                Err(e) => {
                    warn!("Failed to parse Wikipedia JSON for {lang}: {e}");
                    continue;
                }
            };

            let Some(pages_obj) = body
                .get("query")
                .and_then(|qu| qu.get("pages"))
                .and_then(Value::as_object)
            else {
                continue;
            };

            if pages_obj.is_empty() {
                continue;
            }

            let mut page_list: Vec<&Value> = pages_obj.values().collect();
            page_list.sort_by_key(|p| p.get("index").and_then(Value::as_i64).unwrap_or(999));

            let mut out = String::new();
            let mut verified_images = Vec::new();

            for (i, page) in page_list.iter().enumerate().take(5) {
                let title = page
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or("Tanpa Judul");
                let extract = page.get("extract").and_then(Value::as_str).unwrap_or("");
                let clean_extract = extract.trim();
                let page_url = format!(
                    "https://{lang}.wikipedia.org/wiki/{}",
                    urlencoding::encode(title)
                );

                out.push_str(&format_search_item(i + 1, title, &page_url, clean_extract));

                // Prefer thumbnail (raster render, e.g. 1000px PNG) first if original is SVG or non-raster
                let candidate_url = page
                    .get("thumbnail")
                    .and_then(|t| t.get("source"))
                    .and_then(Value::as_str)
                    .and_then(sanitize_and_validate_raster_url)
                    .or_else(|| {
                        page.get("original")
                            .and_then(|o| o.get("source"))
                            .and_then(Value::as_str)
                            .and_then(sanitize_and_validate_raster_url)
                    });
                if let Some(valid_url) = candidate_url {
                    if !verified_images.contains(&valid_url) {
                        verified_images.push(valid_url);
                    }
                }
            }

            if is_visual {
                let is_logo_search = is_logo || is_logo_query(attempt);
                for page in page_list.iter().take(3) {
                    if let Some(title) = page.get("title").and_then(Value::as_str) {
                        if let Ok(gallery_images) =
                            fetch_wikipedia_article_images(client, lang, title, is_logo_search)
                                .await
                        {
                            for img in gallery_images {
                                if !verified_images.contains(&img) {
                                    verified_images.push(img);
                                }
                                if verified_images.len() >= 8 {
                                    break;
                                }
                            }
                        }
                    }
                    if verified_images.len() >= 8 {
                        break;
                    }
                }

                if verified_images.len() < 3 || is_logo_search {
                    if let Ok(commons_images) =
                        search_wikimedia_commons_files(client, attempt).await
                    {
                        for img in commons_images {
                            if !verified_images.contains(&img) {
                                verified_images.push(img);
                            }
                            if verified_images.len() >= 8 {
                                break;
                            }
                        }
                    }
                }
            }

            if !out.trim().is_empty() {
                // If this is a visual search and we haven't found images yet, try the next search attempt if available
                if is_visual
                    && verified_images.is_empty()
                    && *attempt != search_attempts.last().copied().unwrap_or("")
                {
                    continue;
                }

                let mut res = format!("[Hasil Informasi Ensiklopedia Web untuk \"{q}\"]\n\n{out}")
                    .trim()
                    .to_string();
                if !verified_images.is_empty() {
                    res.push_str(&format_verified_images_section(&verified_images));
                } else if is_visual {
                    res.push_str(&format_no_images_guidance(q));
                }
                return Ok(res);
            }
        }
    }

    if is_visual {
        if let Ok(commons_images) = search_wikimedia_commons_files(client, q).await {
            if !commons_images.is_empty() {
                let mut res = format!("[Hasil Informasi Berkas Media untuk \"{q}\"]\n\nDitemukan berkas media resmi terverifikasi untuk topik tersebut.\n");
                res.push_str(&format_verified_images_section(&commons_images));
                return Ok(res);
            }
        }
    }

    Err(format!(
        "Tidak ada hasil ditemukan di ensiklopedia untuk query: \"{q}\""
    ))
}
