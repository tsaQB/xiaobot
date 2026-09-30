//! System page: daemon details, every setting with its source, settings
//! and secret writes, restart, database backup and the log ring.

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderValue};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::web::auth;
use crate::web::error::{ApiError, ApiResult};
use crate::web::settings::{self, Effect, SecretMeta, ValueSource};
use crate::web::WebState;

use super::{attachments_size, database_size, ok, system_summary, write_result};

/// Unit name when running under systemd (from the cgroup path).
fn service_label() -> Option<String> {
    std::env::var_os("INVOCATION_ID")?;
    let unit = std::fs::read_to_string("/proc/self/cgroup")
        .ok()
        .and_then(|cgroups| {
            cgroups
                .lines()
                .flat_map(|line| line.rsplit('/'))
                .find(|part| part.ends_with(".service"))
                .map(str::to_string)
        });
    Some(match unit {
        Some(unit) => format!("{unit} (systemd)"),
        None => "systemd".to_string(),
    })
}

fn row(key: &str, source: ValueSource, value: String, effect: Effect) -> Value {
    json!({"key": key, "source": source, "value": value, "effect": effect})
}

fn secret_row(key: &str, effect: Effect) -> Value {
    let meta = settings::secret_meta(key);
    let source = match meta.location {
        "environment" => ValueSource::Environment,
        "vault" => ValueSource::Vault,
        _ => ValueSource::None,
    };
    let value = if !meta.set {
        String::new()
    } else if key == "XIAO_WEB_PASSWORD" {
        "(hash)".to_string()
    } else if meta.tail.is_empty() {
        "••••".to_string()
    } else {
        format!("••••{}", meta.tail)
    };
    let effect = if source == ValueSource::Environment {
        Effect::Locked
    } else {
        effect
    };
    row(key, source, value, effect)
}

async fn settings_rows(state: &WebState) -> Vec<Value> {
    let mut rows = Vec::new();
    for (key, _, effect) in settings::SECRETS {
        rows.push(secret_row(key, *effect));
    }
    for spec in settings::SETTINGS {
        let source = settings::source_of(spec.key, false);
        let effect = if source == ValueSource::Environment {
            Effect::Locked
        } else {
            spec.effect
        };
        rows.push(row(spec.key, source, settings::effective(spec.key), effect));
    }
    let mcp_source = settings::source_of("EXA_MCP_URL", false);
    rows.push(row(
        "EXA_MCP_URL",
        if mcp_source == ValueSource::None {
            ValueSource::Default
        } else {
            mcp_source
        },
        crate::ai::tools::get_configured_mcp_url(),
        if mcp_source == ValueSource::Environment {
            Effect::Locked
        } else {
            Effect::Live
        },
    ));
    let main = state
        .ai
        .resolve_model_route(crate::ai::routing::ModelRole::Main)
        .await;
    rows.push(row(
        "provider / model",
        ValueSource::Database,
        main.map(|route| format!("{} ({})", route.model, route.provider.name))
            .unwrap_or_default(),
        Effect::Live,
    ));
    let rust_log = std::env::var("RUST_LOG")
        .ok()
        .filter(|value| !value.trim().is_empty());
    rows.push(row(
        "RUST_LOG",
        if rust_log.is_some() {
            ValueSource::Environment
        } else {
            ValueSource::Default
        },
        rust_log.unwrap_or_else(|| "info".to_string()),
        Effect::Readonly,
    ));
    rows.push(row(
        "XIAO_DATA_DIR",
        if std::env::var("XIAO_DATA_DIR").is_ok_and(|dir| !dir.trim().is_empty()) {
            ValueSource::Environment
        } else {
            ValueSource::Default
        },
        crate::ai::storage::xiao_data_dir().display().to_string(),
        Effect::Readonly,
    ));
    rows
}

/// GET /api/system
pub(crate) async fn state(State(state): State<Arc<WebState>>) -> Json<Value> {
    let mut system = system_summary(&state);
    let config = crate::get_config_path();
    let extra = json!({
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "data_dir": crate::ai::storage::xiao_data_dir().display().to_string(),
        "config_file": config.exists().then(|| config.display().to_string()),
        "db_bytes": database_size(),
        "attachments_bytes": attachments_size().await,
        "service": service_label(),
        "rust_log": std::env::var("RUST_LOG").ok().filter(|value| !value.trim().is_empty()).unwrap_or_else(|| "info".to_string()),
    });
    if let (Some(target), Some(fields)) = (system.as_object_mut(), extra.as_object()) {
        target.extend(fields.clone());
    }
    let restart_keys = state.restart_keys();
    Json(json!({
        "system": system,
        "settings": settings_rows(&state).await,
        "env_locks": settings::all_env_locks(),
        "restart_needed": !restart_keys.is_empty(),
        "restart_keys": restart_keys,
    }))
}

/// PUT /api/settings
pub(crate) async fn update_settings(
    State(state): State<Arc<WebState>>,
    Json(body): Json<HashMap<String, String>>,
) -> ApiResult<Value> {
    if body.is_empty() {
        return Ok(write_result(&state, false));
    }
    // Validate everything first, so a bad field changes nothing.
    let mut changes = Vec::new();
    for (key, raw) in &body {
        let Some(spec) = settings::find_spec(key) else {
            return Err(ApiError::invalid(
                format!("{key} cannot be changed here"),
                format!("{key} tidak bisa diubah di sini"),
            ));
        };
        if settings::env_value(key).is_some() {
            return Err(ApiError::env_locked(key));
        }
        changes.push((spec, settings::normalize(key, raw)?));
    }
    let turns_off_code_login = changes
        .iter()
        .any(|(spec, value)| spec.key == "XIAO_WEB_TELEGRAM_LOGIN" && value == "false");
    if turns_off_code_login && !auth::password_is_set() {
        return Err(ApiError::conflict(
            "Set a backup password before turning off sign-in with Telegram codes.",
            "Buat kata sandi cadangan dulu sebelum mematikan masuk dengan kode Telegram.",
        ));
    }
    let mut gateway = false;
    for (spec, value) in &changes {
        crate::ai::service::save_app_setting(spec.key, value).map_err(ApiError::internal)?;
        gateway |= spec.effect == Effect::Gateway;
    }
    let keys: Vec<&str> = changes.iter().map(|(spec, _)| spec.key).collect();
    tracing::info!("Settings changed from the WebUI: {}", keys.join(", "));
    let restarted = gateway && state.wa.apply_settings().await;
    Ok(write_result(&state, restarted))
}

#[derive(Deserialize)]
pub(crate) struct SecretRequest {
    value: String,
}

fn provider_id(key: &str) -> Option<&str> {
    key.strip_prefix("provider:").filter(|id| !id.is_empty())
}

/// Stores a provider key (empty = keyless) and reloads the providers.
async fn save_provider_key(
    state: &WebState,
    id: &str,
    value: String,
) -> Result<SecretMeta, ApiError> {
    let id = id.to_string();
    let meta = tokio::task::spawn_blocking(move || {
        let mut store = crate::ai::storage::load_provider_store();
        let provider = store
            .providers
            .iter_mut()
            .find(|provider| provider.id == id)
            .ok_or_else(ApiError::not_found)?;
        provider.api_key = if value.trim().is_empty() {
            "none".to_string()
        } else {
            value.trim().to_string()
        };
        let meta = SecretMeta::stored(&provider.api_key, true);
        crate::ai::storage::save_provider_store(&store).map_err(ApiError::internal)?;
        Ok::<_, ApiError>(meta)
    })
    .await
    .map_err(ApiError::internal)??;
    if !state.ai.reload_provider_store().await {
        return Err(ApiError::internal("provider store could not be reloaded"));
    }
    Ok(meta)
}

/// PUT /api/secrets/:key
pub(crate) async fn set_secret(
    State(state): State<Arc<WebState>>,
    Path(key): Path<String>,
    Json(body): Json<SecretRequest>,
) -> ApiResult<Value> {
    let value = body.value.trim().to_string();
    if value.is_empty() {
        return Err(ApiError::invalid("Enter a value.", "Isi nilainya dulu."));
    }
    if let Some(id) = provider_id(&key) {
        let meta = save_provider_key(&state, id, value).await?;
        return Ok(secret_result(&state, meta, None));
    }
    let Some(aliases) = settings::secret_aliases(&key) else {
        return Err(ApiError::not_found());
    };
    if let Some(locked) = aliases
        .iter()
        .find(|alias| settings::env_value(alias).is_some())
    {
        return Err(ApiError::env_locked(locked));
    }
    let mut detail = None;
    let stored = match key.as_str() {
        "BOT_TOKEN" => {
            let username = super::channels::verify_token(&value)
                .await
                .map_err(|error| {
                    ApiError::invalid(
                        format!("Telegram refused the token: {error}"),
                        format!("Token ditolak Telegram: {error}"),
                    )
                })?;
            detail = Some(format!("@{username}"));
            value
        }
        "XIAO_WEB_PASSWORD" => {
            auth::validate_new_password(&body.value)?;
            let password = body.value.clone();
            tokio::task::spawn_blocking(move || auth::hash_password(&password))
                .await
                .map_err(ApiError::internal)?
                .map_err(ApiError::internal)?
        }
        _ => value,
    };
    crate::ai::service::save_app_setting(&key, &stored).map_err(ApiError::internal)?;
    // Legacy names would otherwise keep answering after a replacement.
    for alias in aliases.iter().filter(|alias| **alias != key) {
        let _ = crate::ai::service::save_app_setting(alias, "");
    }
    tracing::info!("Secret {key} changed from the WebUI");
    Ok(secret_result(&state, settings::secret_meta(&key), detail))
}

fn secret_result(state: &WebState, meta: SecretMeta, detail: Option<String>) -> Json<Value> {
    Json(json!({
        "ok": true,
        "restart_needed": !state.restart_keys().is_empty(),
        "secret": meta,
        "detail": detail,
    }))
}

/// DELETE /api/secrets/:key
pub(crate) async fn delete_secret(
    State(state): State<Arc<WebState>>,
    Path(key): Path<String>,
) -> ApiResult<Value> {
    if let Some(id) = provider_id(&key) {
        save_provider_key(&state, id, String::new()).await?;
        return Ok(write_result(&state, false));
    }
    let Some(aliases) = settings::secret_aliases(&key) else {
        return Err(ApiError::not_found());
    };
    if let Some(locked) = aliases
        .iter()
        .find(|alias| settings::env_value(alias).is_some())
    {
        return Err(ApiError::env_locked(locked));
    }
    if key == "XIAO_WEB_PASSWORD" && !auth::telegram_login_available() {
        return Err(ApiError::conflict(
            "Without the password nobody could sign in: Telegram code sign-in is off or not set up.",
            "Tanpa kata sandi tidak ada yang bisa masuk: masuk dengan kode Telegram mati atau belum diatur.",
        ));
    }
    for alias in aliases {
        crate::ai::service::save_app_setting(alias, "").map_err(ApiError::internal)?;
    }
    tracing::info!("Secret {key} removed from the WebUI");
    Ok(write_result(&state, false))
}

/// POST /api/system/restart
pub(crate) async fn restart(State(state): State<Arc<WebState>>) -> Json<Value> {
    state.request_restart();
    ok()
}

/// GET /api/system/backup
pub(crate) async fn backup() -> Result<Response, ApiError> {
    let target =
        crate::ai::storage::xiao_data_dir().join(format!(".backup-{}.db", auth::random_hex(8)));
    let result = crate::ai::storage::web::backup_database_async(target.clone()).await;
    let bytes = match result {
        Ok(()) => tokio::fs::read(&target).await.map_err(ApiError::internal),
        Err(error) => Err(ApiError::internal(error)),
    };
    let _ = tokio::fs::remove_file(&target).await;
    let bytes = bytes?;
    let name = format!(
        "xiaoai-backup-{}.db",
        chrono::Local::now().format("%Y-%m-%d")
    );
    let mut response = Body::from(bytes).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/vnd.sqlite3"),
    );
    if let Ok(value) = HeaderValue::from_str(&format!("attachment; filename=\"{name}\"")) {
        headers.insert(header::CONTENT_DISPOSITION, value);
    }
    Ok(response)
}

#[derive(Deserialize)]
pub(crate) struct LogsQuery {
    #[serde(default)]
    after: u64,
}

/// GET /api/logs?after=seq
pub(crate) async fn logs(Query(query): Query<LogsQuery>) -> Json<Value> {
    let (lines, last) = crate::web::logs::lines_after(query.after);
    Json(json!({
        "lines": lines,
        "last": last,
        "filter": std::env::var("RUST_LOG").ok().filter(|value| !value.trim().is_empty()).unwrap_or_else(|| "info".to_string()),
    }))
}
