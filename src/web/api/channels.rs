//! Telegram and WhatsApp pages.

use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::ai::storage::web as store;
use crate::gateway::whatsapp::LinkPhase;
use crate::web::auth::{sha256, telegram_configured};
use crate::web::error::{ApiError, ApiResult};
use crate::web::settings;
use crate::web::wa::{phase_name, PairMode, WaController};
use crate::web::WebState;

use super::ok;

/// getMe answers are cached this long (the bot's settings rarely change).
const BOT_INFO_TTL: Duration = Duration::from_secs(5 * 60);
/// A failed getMe is remembered this long, so pages do not wait on it again.
const BOT_ERROR_TTL: Duration = Duration::from_secs(60);

type BotCache = Option<(Instant, [u8; 32], Result<Value, String>)>;
static BOT_INFO: LazyLock<Mutex<BotCache>> = LazyLock::new(|| Mutex::new(None));

/// Calls a Bot API method directly. The URL holds the token, so it is
/// stripped from every error.
async fn telegram_call(token: &str, method: &str) -> Result<Value, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|error| error.without_url().to_string())?;
    let response = client
        .get(format!("https://api.telegram.org/bot{token}/{method}"))
        .send()
        .await
        .map_err(|error| error.without_url().to_string())?;
    let body: Value = response
        .json()
        .await
        .map_err(|error| error.without_url().to_string())?;
    if body.get("ok").and_then(Value::as_bool) == Some(true) {
        Ok(body.get("result").cloned().unwrap_or(Value::Null))
    } else {
        Err(body
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("Telegram refused the request")
            .to_string())
    }
}

fn bot_info(result: &Value) -> Value {
    json!({
        "id": result.get("id").map(|id| id.to_string()).unwrap_or_default(),
        "username": result.get("username").and_then(Value::as_str).unwrap_or_default(),
        "first_name": result.get("first_name").and_then(Value::as_str).unwrap_or_default(),
        "can_join_groups": result.get("can_join_groups").and_then(Value::as_bool),
        "can_read_all_group_messages": result.get("can_read_all_group_messages").and_then(Value::as_bool),
        "supports_inline_queries": result.get("supports_inline_queries").and_then(Value::as_bool),
        "supports_guest_queries": result.get("supports_guest_queries").and_then(Value::as_bool),
    })
}

/// getMe for the configured token, cached for a few minutes.
pub(crate) async fn cached_bot_info(fresh: bool) -> Result<Value, String> {
    let token = crate::get_configured_token().ok_or_else(|| "BOT_TOKEN is not set".to_string())?;
    let fingerprint = sha256(token.as_bytes());
    if !fresh {
        if let Ok(cache) = BOT_INFO.lock() {
            if let Some((at, key, result)) = cache.as_ref() {
                let ttl = if result.is_ok() {
                    BOT_INFO_TTL
                } else {
                    BOT_ERROR_TTL
                };
                if *key == fingerprint && at.elapsed() < ttl {
                    return result.clone();
                }
            }
        }
    }
    let result = telegram_call(&token, "getMe").await.map(|me| bot_info(&me));
    if let Ok(mut cache) = BOT_INFO.lock() {
        *cache = Some((Instant::now(), fingerprint, result.clone()));
    }
    result
}

/// Checks a new token with getMe; returns the bot's username.
pub(crate) async fn verify_token(token: &str) -> Result<String, String> {
    let result = telegram_call(token, "getMe").await?;
    Ok(result
        .get("username")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string())
}

/// Telegram polling ran within the last 90 seconds.
pub(crate) fn telegram_online(state: &WebState) -> bool {
    state.telegram().is_some()
        && crate::bot::daemon::last_poll_age_secs().is_some_and(|age| age < 90)
}

/// GET /api/telegram
pub(crate) async fn telegram(State(state): State<Arc<WebState>>) -> Json<Value> {
    let (bot, bot_error) = if crate::get_configured_token().is_some() {
        match cached_bot_info(false).await {
            Ok(info) => (Some(info), None),
            Err(error) => (None, Some(error)),
        }
    } else {
        (None, None)
    };
    let (queries, chosen) = crate::bot::daemon::inline_counters();
    let groups: Vec<String> = store::group_chat_ids_async(20)
        .await
        .into_iter()
        .map(|id| id.to_string())
        .collect();
    Json(json!({
        "configured": telegram_configured(),
        "token": settings::secret_meta("BOT_TOKEN"),
        "owner_id": settings::effective("OWNER_USER_ID"),
        "allowed_chat_ids": settings::effective("ALLOWED_CHAT_IDS"),
        "dedicated_chat_ids": settings::effective("DEDICATED_CHAT_IDS"),
        "bot": bot,
        "bot_error": bot_error,
        "online": telegram_online(&state),
        "last_poll_secs": crate::bot::daemon::last_poll_age_secs(),
        "poll_timeout": crate::bot::daemon::POLL_TIMEOUT_SECS,
        "inline_queries_seen": queries,
        "chosen_results_seen": chosen,
        "groups_seen": groups,
        "env_locks": settings::env_locks(["BOT_TOKEN", "OWNER_USER_ID", "ALLOWED_CHAT_IDS", "DEDICATED_CHAT_IDS"]),
        "restart_needed": !state.restart_keys().is_empty(),
    }))
}

/// POST /api/telegram/check
pub(crate) async fn telegram_check() -> ApiResult<Value> {
    let Some(token) = crate::get_configured_token() else {
        return Err(ApiError::not_configured(
            "Set the bot token first.",
            "Isi token bot dulu.",
        ));
    };
    let started = Instant::now();
    let me = cached_bot_info(true).await;
    let ms = started.elapsed().as_millis();
    let webhook = telegram_call(&token, "getWebhookInfo").await.ok();
    Ok(Json(match me {
        Ok(bot) => json!({
            "ok": true,
            "ms": ms,
            "bot": bot,
            "webhook_url": webhook
                .as_ref()
                .and_then(|info| info.get("url"))
                .and_then(Value::as_str)
                .filter(|url| !url.is_empty()),
            "pending_updates": webhook
                .as_ref()
                .and_then(|info| info.get("pending_update_count"))
                .and_then(Value::as_u64),
            "error": null,
        }),
        Err(error) => json!({
            "ok": false,
            "ms": ms,
            "bot": null,
            "webhook_url": null,
            "pending_updates": null,
            "error": error,
        }),
    }))
}

fn pairing_json(state: &WebState) -> Value {
    serde_json::to_value(state.wa.pairing_view()).unwrap_or(Value::Null)
}

/// GET /api/whatsapp
pub(crate) async fn whatsapp(State(state): State<Arc<WebState>>) -> Json<Value> {
    let link = state.wa.state();
    let queue = store::queue_counts_async(store::Inbox::WhatsApp).await;
    let pairing = if link.phase == LinkPhase::Pairing {
        pairing_json(&state)
    } else {
        Value::Null
    };
    Json(json!({
        "enabled": WaController::should_run(),
        "linked": WaController::linked(),
        "phase": phase_name(link.phase),
        "last_error": link.error,
        "owner_number": settings::effective("WHATSAPP_OWNER_NUMBER"),
        "dedicated_groups": settings::effective("WHATSAPP_DEDICATED_GROUPS"),
        "queue": {"pending": queue.pending, "failed": queue.failed},
        "pairing": pairing,
        "env_locks": settings::env_locks(["WHATSAPP_ENABLED", "WHATSAPP_OWNER_NUMBER", "WHATSAPP_DEDICATED_GROUPS"]),
    }))
}

#[derive(Deserialize)]
pub(crate) struct PairRequest {
    mode: PairModeRequest,
    #[serde(default)]
    phone: Option<String>,
}

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum PairModeRequest {
    Qr,
    Code,
}

/// Saves `WHATSAPP_ENABLED` (`None` clears it, so a later link turns the
/// gateway on again), unless the environment decides.
fn save_enabled(enabled: Option<bool>) {
    let value = enabled.map(|flag| flag.to_string()).unwrap_or_default();
    if settings::env_value("WHATSAPP_ENABLED").is_none()
        && crate::ai::service::save_app_setting("WHATSAPP_ENABLED", &value).is_err()
    {
        tracing::warn!("WHATSAPP_ENABLED could not be saved");
    }
}

/// POST /api/whatsapp/pair
pub(crate) async fn pair_start(
    State(state): State<Arc<WebState>>,
    Json(body): Json<PairRequest>,
) -> ApiResult<Value> {
    if WaController::linked() {
        return Err(ApiError::conflict(
            "WhatsApp is already linked. Unlink it first to link another account.",
            "WhatsApp sudah tertaut. Putuskan dulu untuk menautkan akun lain.",
        ));
    }
    let (mode, phone) = match body.mode {
        PairModeRequest::Qr => (PairMode::Qr, None),
        PairModeRequest::Code => {
            let digits: String = body
                .phone
                .unwrap_or_default()
                .chars()
                .filter(char::is_ascii_digit)
                .collect();
            if !(8..=15).contains(&digits.len()) {
                return Err(ApiError::invalid(
                    "The number needs 8 to 15 digits in international format.",
                    "Nomor harus 8 sampai 15 digit dalam format internasional.",
                ));
            }
            (PairMode::Code, Some(digits))
        }
    };
    save_enabled(Some(true));
    state.wa.pair(mode, phone).await;
    // Give WhatsApp a moment to issue the first code, so the page can show
    // it straight away.
    for _ in 0..40 {
        let link = state.wa.state();
        let ready = match mode {
            PairMode::Qr => link.qr.is_some(),
            PairMode::Code => link.code.is_some(),
        };
        if ready || link.error.is_some() || link.phase == LinkPhase::Online {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    Ok(Json(pairing_json(&state)))
}

/// GET /api/whatsapp/pair
pub(crate) async fn pair_state(State(state): State<Arc<WebState>>) -> Json<Value> {
    Json(pairing_json(&state))
}

/// POST /api/whatsapp/pair/cancel
pub(crate) async fn pair_cancel(State(state): State<Arc<WebState>>) -> Json<Value> {
    state.wa.cancel_pair().await;
    ok()
}

/// POST /api/whatsapp/unlink
pub(crate) async fn unlink(State(state): State<Arc<WebState>>) -> ApiResult<Value> {
    state.wa.unlink().await.map_err(ApiError::internal)?;
    save_enabled(None);
    tracing::info!("WhatsApp session unlinked from the WebUI");
    Ok(ok())
}
