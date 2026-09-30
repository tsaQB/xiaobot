//! Routes of the WebUI API. The contract lives in `webui/src/api/types.ts`.

mod ai;
mod channels;
mod chat;
mod context;
mod memory;
mod overview;
mod queue;
mod search;
mod security;
mod system;

use std::sync::Arc;

use axum::extract::DefaultBodyLimit;
use axum::routing::{delete, get, patch, post, put};
use axum::{Json, Router};
use serde_json::{json, Value};

use super::auth;
use super::WebState;

/// JSON bodies are small; uploads have their own limit.
const MAX_JSON_BODY: usize = 256 * 1024;

pub(crate) fn router(state: Arc<WebState>) -> Router {
    let public = Router::new()
        .route("/api/auth/state", get(auth::auth_state))
        .route("/api/auth/code/send", post(auth::code_send))
        .route("/api/auth/code/verify", post(auth::code_verify))
        .route("/api/auth/password", post(auth::password_login));

    let protected = Router::new()
        .route("/api/auth/logout", post(auth::logout))
        .route("/api/auth/logout-all", post(auth::logout_all))
        .route("/api/overview", get(overview::overview))
        .route("/api/ai", get(ai::state))
        .route("/api/ai/providers", post(ai::create_provider))
        .route("/api/ai/providers/test", post(ai::test_provider))
        .route(
            "/api/ai/providers/{id}",
            put(ai::update_provider).delete(ai::delete_provider),
        )
        .route("/api/ai/providers/{id}/models", post(ai::refresh_models))
        .route("/api/ai/active", post(ai::set_active))
        .route("/api/ai/routes/{role}", put(ai::set_route))
        .route("/api/ai/caps/{role}", put(ai::set_caps))
        .route("/api/ai/probe/{role}", post(ai::probe))
        .route("/api/ai/test/{role}", post(ai::test_role))
        .route("/api/search", get(search::state))
        .route("/api/search/cooldowns/reset", post(search::reset_cooldowns))
        .route("/api/search/test", post(search::test))
        .route("/api/mcp", get(search::mcp_state).put(search::mcp_update))
        .route("/api/mcp/reset", post(search::mcp_reset))
        .route("/api/mcp/test", post(search::mcp_test))
        .route("/api/memory", get(memory::list).delete(memory::clear))
        .route(
            "/api/memory/{key}",
            put(memory::upsert).delete(memory::remove),
        )
        .route("/api/telegram", get(channels::telegram))
        .route("/api/telegram/check", post(channels::telegram_check))
        .route("/api/whatsapp", get(channels::whatsapp))
        .route(
            "/api/whatsapp/pair",
            get(channels::pair_state).post(channels::pair_start),
        )
        .route("/api/whatsapp/pair/cancel", post(channels::pair_cancel))
        .route("/api/whatsapp/unlink", post(channels::unlink))
        .route("/api/queue", get(queue::state))
        .route("/api/queue/{channel}/{id}/retry", post(queue::retry))
        .route("/api/queue/{channel}/{id}", delete(queue::dismiss))
        .route("/api/context", get(context::state))
        .route("/api/context/clear", post(context::clear))
        .route("/api/logs", get(system::logs))
        .route("/api/system", get(system::state))
        .route("/api/settings", put(system::update_settings))
        .route(
            "/api/secrets/{key}",
            put(system::set_secret).delete(system::delete_secret),
        )
        .route("/api/system/restart", post(system::restart))
        .route("/api/system/backup", get(system::backup))
        .route("/api/security", get(security::state))
        .route(
            "/api/security/password",
            put(security::set_password).delete(security::delete_password),
        )
        .route(
            "/api/security/sessions/revoke-others",
            post(security::revoke_others),
        )
        .route("/api/security/sessions/{id}", delete(security::revoke))
        .route("/api/chat/sessions", get(chat::sessions).post(chat::create))
        .route(
            "/api/chat/sessions/{id}",
            patch(chat::rename).delete(chat::remove),
        )
        .route("/api/chat/sessions/{id}/messages", get(chat::messages))
        .route("/api/chat/sessions/{id}/clear", post(chat::clear))
        .route("/api/chat/sessions/{id}/send", post(chat::send))
        .route("/api/chat/sessions/{id}/stop", post(chat::stop))
        .route(
            "/api/chat/uploads",
            post(chat::upload).layer(DefaultBodyLimit::max(
                super::chat::MAX_UPLOAD_BYTES + 64 * 1024,
            )),
        )
        .route("/api/chat/files/{id}", get(chat::file))
        .route_layer(axum::middleware::from_fn(auth::require_session));

    public
        .merge(protected)
        .layer(DefaultBodyLimit::max(MAX_JSON_BODY))
        .with_state(state)
}

pub(crate) fn ok() -> Json<Value> {
    Json(json!({"ok": true}))
}

/// Result of a write that may wait for a restart.
pub(crate) fn write_result(state: &WebState, gateway_restarted: bool) -> Json<Value> {
    Json(json!({
        "ok": true,
        "restart_needed": !state.restart_keys().is_empty(),
        "gateway_restarted": gateway_restarted,
    }))
}

/// Size of a file, 0 when missing.
pub(crate) fn file_size(path: &std::path::Path) -> u64 {
    std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0)
}

/// Total size of the database with its WAL sidecar.
pub(crate) fn database_size() -> u64 {
    let db = crate::ai::storage::session_db_path();
    let wal = std::path::PathBuf::from(format!("{}-wal", db.display()));
    file_size(&db) + file_size(&wal)
}

/// Total size of a directory tree (no symlinks followed).
pub(crate) fn dir_size(path: &std::path::Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| match entry.file_type() {
            Ok(kind) if kind.is_dir() => dir_size(&entry.path()),
            Ok(kind) if kind.is_file() => entry.metadata().map(|meta| meta.len()).unwrap_or(0),
            _ => 0,
        })
        .sum()
}

/// Size of the attachments folder, measured off the async runtime.
pub(crate) async fn attachments_size() -> u64 {
    let root = crate::ai::storage::xiao_data_dir().join("attachments");
    tokio::task::spawn_blocking(move || dir_size(&root))
        .await
        .unwrap_or(0)
}

/// "armbian (aarch64)"
pub(crate) fn host_label() -> String {
    let name = std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok())
        .or_else(|| std::env::var("COMPUTERNAME").ok())
        .unwrap_or_else(|| std::env::consts::OS.to_string());
    format!("{name} ({})", std::env::consts::ARCH)
}

pub(crate) fn system_summary(state: &WebState) -> Value {
    json!({
        "version": env!("CARGO_PKG_VERSION"),
        "pid": std::process::id(),
        "host": host_label(),
        "started_at": state.started_at.to_rfc3339(),
        "uptime_secs": state.uptime_secs(),
    })
}
