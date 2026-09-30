//! Web search engines, their keys and cooldowns, and the MCP endpoint.

use std::time::{Duration, Instant};

use axum::extract::Path;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::web::error::{ApiError, ApiResult};
use crate::web::settings;

use super::ok;

const DEFAULT_MCP_URL: &str = crate::cli::mcp::DEFAULT_MCP_URL;
const KEY_NAMES: [&str; 3] = ["BRAVE_API_KEY", "TAVILY_API_KEY", "EXA_API_KEY"];

/// One engine of the `web_search` chain, in the order it is tried.
pub(crate) struct EngineInfo {
    pub id: &'static str,
    pub name: &'static str,
    pub keyed: bool,
    /// The owner's switch.
    pub enabled: bool,
    /// Keyless, or its key is set.
    pub available: bool,
    /// "on", "off" (switched off or no key) or "cool" (skipped after a failure).
    pub state: &'static str,
    pub cooldown: Option<Duration>,
}

pub(crate) fn engines() -> Vec<EngineInfo> {
    let (exa_mcp, ddg) = crate::ai::tools::search_cooldowns();
    let disabled = crate::ai::tools::disabled_search_engines();
    let enabled = |id: &str| !disabled.iter().any(|current| current == id);
    let state = |on: bool, cooldown: Option<Duration>| {
        if !on {
            "off"
        } else if cooldown.is_some() {
            "cool"
        } else {
            "on"
        }
    };
    let keyed = |id, name, has_key: bool| EngineInfo {
        id,
        name,
        keyed: true,
        enabled: enabled(id),
        available: has_key,
        state: state(has_key && enabled(id), None),
        cooldown: None,
    };
    let keyless = |id, name, cooldown: Option<Duration>| EngineInfo {
        id,
        name,
        keyed: false,
        enabled: enabled(id),
        available: true,
        state: state(enabled(id), cooldown),
        cooldown: cooldown.filter(|_| enabled(id)),
    };
    vec![
        keyed(
            "brave",
            "Brave",
            crate::ai::tools::get_brave_key().is_some(),
        ),
        keyed(
            "tavily",
            "Tavily",
            crate::ai::tools::get_tavily_key().is_some(),
        ),
        keyed("exa", "Exa API", crate::ai::tools::get_exa_key().is_some()),
        keyless("exa_mcp", "Exa MCP", exa_mcp),
        keyless("ddg", "DuckDuckGo", ddg),
        keyless("wiki", "Wikipedia", None),
    ]
}

/// GET /api/search
pub(crate) async fn state() -> Json<Value> {
    let engines: Vec<Value> = engines()
        .into_iter()
        .map(|engine| {
            json!({
                "id": engine.id,
                "name": engine.name,
                "keyed": engine.keyed,
                "enabled": engine.enabled,
                "available": engine.available,
                "state": engine.state,
                "cooldown_secs": engine.cooldown.map(|left| left.as_secs().max(1)),
            })
        })
        .collect();
    let keys: serde_json::Map<String, Value> = KEY_NAMES
        .iter()
        .map(|key| {
            (
                (*key).to_string(),
                serde_json::to_value(settings::secret_meta(key)).unwrap_or(Value::Null),
            )
        })
        .collect();
    Json(json!({
        "engines": engines,
        "keys": keys,
        "env_locks": settings::env_locks(["BRAVE_API_KEY", "TAVILY_API_KEY", "TAVILY_KEY", "EXA_API_KEY", "EXA_KEY", "XIAO_SEARCH_DISABLED"]),
    }))
}

#[derive(Deserialize)]
pub(crate) struct EngineToggleRequest {
    enabled: bool,
}

/// PUT /api/search/engines/:id: switches one engine on or off.
pub(crate) async fn set_engine(
    Path(id): Path<String>,
    Json(body): Json<EngineToggleRequest>,
) -> ApiResult<Value> {
    if !crate::ai::tools::SEARCH_ENGINE_IDS.contains(&id.as_str()) {
        return Err(ApiError::not_found());
    }
    if settings::env_value("XIAO_SEARCH_DISABLED").is_some() {
        return Err(ApiError::env_locked("XIAO_SEARCH_DISABLED"));
    }
    let value = crate::ai::tools::search_disabled_value(&id, body.enabled);
    crate::ai::service::save_app_setting("XIAO_SEARCH_DISABLED", &value)
        .map_err(ApiError::internal)?;
    tracing::info!(
        "Search engine {id} switched {} from the WebUI",
        if body.enabled { "on" } else { "off" }
    );
    Ok(ok())
}

/// POST /api/search/cooldowns/reset
pub(crate) async fn reset_cooldowns() -> Json<Value> {
    crate::ai::tools::reset_search_cooldowns();
    ok()
}

#[derive(Deserialize)]
pub(crate) struct SearchTestRequest {
    query: String,
    #[serde(default)]
    pictures: bool,
}

/// Pieces of the text `execute_web_search` hands the model.
struct ParsedSearch {
    engine: Option<String>,
    answer: Option<String>,
    hits: Vec<Value>,
    images: Vec<String>,
}

fn parse_search_output(raw: &str) -> ParsedSearch {
    let mut parsed = ParsedSearch {
        engine: None,
        answer: None,
        hits: Vec::new(),
        images: Vec::new(),
    };
    let mut current: Option<(String, String, String)> = None;
    let mut in_images = false;
    let flush = |current: &mut Option<(String, String, String)>, hits: &mut Vec<Value>| {
        if let Some((title, url, summary)) = current.take() {
            hits.push(json!({"title": title, "url": url, "summary": summary}));
        }
    };
    for line in raw.lines() {
        let trimmed = line.trim();
        if parsed.engine.is_none() {
            if let Some(rest) = trimmed.strip_prefix("[Hasil Pencarian ") {
                parsed.engine = rest.split(" untuk ").next().map(str::to_string);
                continue;
            }
        }
        if trimmed.starts_with("🖼️") {
            in_images = true;
            continue;
        }
        if in_images {
            if let Some(url) = trimmed.strip_prefix("- ") {
                if url.starts_with("http") {
                    parsed.images.push(url.to_string());
                    continue;
                }
            }
            in_images = false;
        }
        if let Some(answer) = trimmed.strip_prefix("💡 **Jawaban Ringkas**:") {
            parsed.answer = Some(answer.trim().to_string());
            continue;
        }
        let numbered = trimmed
            .split_once(". **")
            .filter(|(number, _)| {
                !number.is_empty() && number.chars().all(|ch| ch.is_ascii_digit())
            })
            .and_then(|(_, rest)| rest.strip_suffix("**"));
        if let Some(title) = numbered {
            flush(&mut current, &mut parsed.hits);
            current = Some((title.to_string(), String::new(), String::new()));
            continue;
        }
        if let Some((_, url, summary)) = current.as_mut() {
            if let Some(found) = trimmed.strip_prefix("URL:") {
                *url = found.trim().to_string();
            } else if let Some(found) = trimmed.strip_prefix("Ringkasan:") {
                *summary = found.trim().to_string();
            }
        }
    }
    flush(&mut current, &mut parsed.hits);
    parsed
}

/// POST /api/search/test
pub(crate) async fn test(Json(body): Json<SearchTestRequest>) -> ApiResult<Value> {
    let mut query = body.query.trim().to_string();
    if query.is_empty() {
        return Err(ApiError::invalid("Type a query.", "Tulis kata kunci."));
    }
    if body.pictures && !query.to_lowercase().contains("foto") {
        query.push_str(" foto");
    }
    let started = Instant::now();
    let raw = crate::ai::tools::execute_web_search(&query).await;
    let parsed = parse_search_output(&raw);
    Ok(Json(json!({
        "ms": started.elapsed().as_millis(),
        "engine": parsed.engine,
        "answer": parsed.answer,
        "hits": parsed.hits,
        "images": parsed.images,
        "raw": raw,
    })))
}

/// GET /api/mcp
pub(crate) async fn mcp_state() -> Json<Value> {
    let definitions = crate::ai::tools::get_tools_definition();
    let tools: Vec<Value> = definitions
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let function = item.get("function").unwrap_or(item);
                    let name = function.get("name")?.as_str()?;
                    let description = function
                        .get("description")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    Some(json!({
                        "name": name,
                        "description": crate::util::truncate_chars_with_ellipsis(description.trim(), 240),
                        "guest": crate::ai::tools::GUEST_MODE_TOOLS.contains(&name),
                    }))
                })
                .collect()
        })
        .unwrap_or_default();
    Json(json!({
        "url": crate::ai::tools::get_configured_mcp_url(),
        "default_url": DEFAULT_MCP_URL,
        "tools": tools,
        "env_locks": settings::env_locks(["EXA_MCP_URL"]),
    }))
}

async fn checked_mcp_url(raw: &str) -> Result<String, ApiError> {
    let url = raw.trim().to_string();
    crate::bot::url_policy::resolve_download_url(&url)
        .await
        .map_err(|error| {
            ApiError::invalid(
                format!("The SSRF policy refused this address: {error}"),
                format!("Alamat ditolak kebijakan SSRF: {error}"),
            )
        })?;
    Ok(url)
}

#[derive(Deserialize)]
pub(crate) struct McpUpdateRequest {
    url: String,
}

/// PUT /api/mcp
pub(crate) async fn mcp_update(Json(body): Json<McpUpdateRequest>) -> ApiResult<Value> {
    if settings::env_value("EXA_MCP_URL").is_some() {
        return Err(ApiError::env_locked("EXA_MCP_URL"));
    }
    let url = checked_mcp_url(&body.url).await?;
    crate::ai::service::save_app_setting("EXA_MCP_URL", &url).map_err(ApiError::internal)?;
    Ok(ok())
}

/// POST /api/mcp/reset
pub(crate) async fn mcp_reset() -> ApiResult<Value> {
    if settings::env_value("EXA_MCP_URL").is_some() {
        return Err(ApiError::env_locked("EXA_MCP_URL"));
    }
    crate::ai::service::save_app_setting("EXA_MCP_URL", DEFAULT_MCP_URL)
        .map_err(ApiError::internal)?;
    Ok(ok())
}

#[derive(Deserialize)]
pub(crate) struct McpTestRequest {
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    url: Option<String>,
}

/// POST /api/mcp/test
pub(crate) async fn mcp_test(Json(body): Json<McpTestRequest>) -> ApiResult<Value> {
    let url = match body.url.filter(|url| !url.trim().is_empty()) {
        Some(url) => checked_mcp_url(&url).await?,
        None => crate::ai::tools::get_configured_mcp_url(),
    };
    let query = body
        .query
        .map(|query| query.trim().to_string())
        .filter(|query| !query.is_empty())
        .unwrap_or_else(|| "Colosseum history".to_string());
    let host = url::Url::parse(&url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(str::to_string));
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(ApiError::internal)?;
    let started = Instant::now();
    let result = crate::ai::tools::search_exa_mcp(&client, &url, &query).await;
    let ms = started.elapsed().as_millis();
    Ok(Json(match result {
        Ok(text) => json!({
            "ok": true,
            "ms": ms,
            "host": host,
            "snippet": crate::util::truncate_chars_with_ellipsis(text.trim(), 400),
            "error": null,
        }),
        Err(error) => json!({"ok": false, "ms": ms, "host": host, "snippet": null, "error": error}),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_output_is_split_into_parts() {
        let raw = "[Hasil Pencarian Wikipedia untuk \"Colosseum\"]\n\n🖼️ **URL Foto/Gambar Raster Terverifikasi (Dapat Digunakan untuk Tool Multimedia)**:\n- https://ex.com/a.jpg\n- https://ex.com/b.jpg\n\n💡 **Jawaban Ringkas**: An amphitheatre.\n\n1. **Colosseum**\n   URL: https://en.wikipedia.org/wiki/Colosseum\n   Ringkasan: Oval amphitheatre in Rome.\n\n2. **Flavian**\n   URL: https://ex.com/f";
        let parsed = parse_search_output(raw);
        assert_eq!(parsed.engine.as_deref(), Some("Wikipedia"));
        assert_eq!(parsed.images.len(), 2);
        assert_eq!(parsed.answer.as_deref(), Some("An amphitheatre."));
        assert_eq!(parsed.hits.len(), 2);
        assert_eq!(
            parsed.hits[0]["url"],
            "https://en.wikipedia.org/wiki/Colosseum"
        );
        assert_eq!(parsed.hits[1]["summary"], "");
    }
}
