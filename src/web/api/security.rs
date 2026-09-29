//! WebUI security page: address, sign-in methods and signed-in devices.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::ai::storage::web as store;
use crate::web::auth::{self, CurrentSession};
use crate::web::error::{ApiError, ApiResult};
use crate::web::settings;
use crate::web::{net, WebState};

use super::ok;

/// GET /api/security
pub(crate) async fn state(
    State(state): State<Arc<WebState>>,
    Extension(current): Extension<CurrentSession>,
) -> Json<Value> {
    let saved_bind = settings::effective("XIAO_WEB_BIND");
    let saved = net::parse_bind(&saved_bind).ok().flatten();
    let port = saved.map_or(state.bind.port(), |addr| addr.port());
    let lan = saved.is_some_and(|addr| !addr.ip().is_loopback());
    let days = match settings::effective("XIAO_WEB_SESSION_DAYS").trim() {
        "1" => 1,
        "30" => 30,
        _ => 7,
    };
    let sessions: Vec<Value> = store::list_web_sessions_async(auth::now_unix())
        .await
        .into_iter()
        .map(|session| {
            let (device, os) = auth::describe_device(&session.user_agent);
            json!({
                "id": session.id,
                "device": device,
                "os": os,
                "ip": session.ip,
                "created_at": auth::unix_to_rfc3339(session.created_at),
                "last_seen": auth::unix_to_rfc3339(session.last_seen),
                "expires_at": auth::unix_to_rfc3339(session.expires_at),
                "current": session.id == current.id,
            })
        })
        .collect();
    Json(json!({
        "bind": state.bind.to_string(),
        "saved_bind": saved_bind,
        "port": port,
        "lan": lan,
        "lan_ip": net::primary_lan_ip().map(|ip| ip.to_string()),
        "allowed_networks": settings::effective("XIAO_WEB_ALLOWED_NETWORKS"),
        "telegram_login": settings::effective_bool("XIAO_WEB_TELEGRAM_LOGIN"),
        "telegram_available": auth::telegram_configured(),
        "password": settings::secret_meta("XIAO_WEB_PASSWORD"),
        "session_days": days,
        "sessions": sessions,
        "env_locks": settings::env_locks([
            "XIAO_WEB_BIND",
            "XIAO_WEB_ALLOWED_NETWORKS",
            "XIAO_WEB_SESSION_DAYS",
            "XIAO_WEB_TELEGRAM_LOGIN",
            "XIAO_WEB_PASSWORD",
        ]),
        "restart_needed": !state.restart_keys().is_empty(),
    }))
}

#[derive(Deserialize)]
pub(crate) struct PasswordSetRequest {
    password: String,
}

/// PUT /api/security/password
pub(crate) async fn set_password(Json(body): Json<PasswordSetRequest>) -> ApiResult<Value> {
    if settings::env_value("XIAO_WEB_PASSWORD").is_some() {
        return Err(ApiError::env_locked("XIAO_WEB_PASSWORD"));
    }
    auth::validate_new_password(&body.password)?;
    let password = body.password;
    let hash = tokio::task::spawn_blocking(move || auth::hash_password(&password))
        .await
        .map_err(ApiError::internal)?
        .map_err(ApiError::internal)?;
    crate::ai::service::save_app_setting("XIAO_WEB_PASSWORD", &hash).map_err(ApiError::internal)?;
    tracing::info!("WebUI backup password changed");
    Ok(ok())
}

/// DELETE /api/security/password
pub(crate) async fn delete_password() -> ApiResult<Value> {
    if settings::env_value("XIAO_WEB_PASSWORD").is_some() {
        return Err(ApiError::env_locked("XIAO_WEB_PASSWORD"));
    }
    if !auth::telegram_login_available() {
        return Err(ApiError::conflict(
            "Without the password nobody could sign in: Telegram code sign-in is off or not set up.",
            "Tanpa kata sandi tidak ada yang bisa masuk: masuk dengan kode Telegram mati atau belum diatur.",
        ));
    }
    crate::ai::service::save_app_setting("XIAO_WEB_PASSWORD", "").map_err(ApiError::internal)?;
    tracing::info!("WebUI backup password removed");
    Ok(ok())
}

/// DELETE /api/security/sessions/:id
pub(crate) async fn revoke(Path(id): Path<String>) -> ApiResult<Value> {
    if !store::delete_web_session_async(id).await {
        return Err(ApiError::not_found());
    }
    Ok(ok())
}

/// POST /api/security/sessions/revoke-others
pub(crate) async fn revoke_others(Extension(current): Extension<CurrentSession>) -> Json<Value> {
    store::delete_web_sessions_except_async(Some(current.id)).await;
    ok()
}
