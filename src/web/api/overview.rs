//! Home page summary.

use std::sync::Arc;

use axum::extract::State;
use axum::Json;
use serde_json::{json, Value};

use crate::ai::storage::web::{self as store, Inbox};
use crate::gateway::whatsapp::LinkPhase;
use crate::web::chat::owner_id;
use crate::web::wa::{phase_name, WaController};
use crate::web::WebState;

use super::{attachments_size, database_size, system_summary};

/// Inline queries without a single chosen result before the page warns.
const INLINE_FEEDBACK_HINT_AFTER: u64 = 3;

fn attention(kind: &str, code: &str, to: &str) -> Value {
    json!({"kind": kind, "code": code, "to": to})
}

/// GET /api/overview
pub(crate) async fn overview(State(state): State<Arc<WebState>>) -> Json<Value> {
    let owner = owner_id();
    let restart_needed = !state.restart_keys().is_empty();
    let telegram_link = state.telegram();
    let online = super::channels::telegram_online(&state);
    let bot = if telegram_link.is_some() {
        super::channels::cached_bot_info(false).await.ok()
    } else {
        None
    };
    let flag = |name: &str| {
        bot.as_ref()
            .and_then(|bot| bot.get(name))
            .and_then(Value::as_bool)
    };

    let link = state.wa.state();
    let linked = WaController::linked();
    let providers = state.ai.get_user_providers(owner).await;
    let active = state.ai.get_active_provider(owner).await;
    let engines = super::search::engines();
    let keyed = engines
        .iter()
        .filter(|engine| engine.keyed && engine.state == "on")
        .count();
    let paused: Vec<&str> = engines
        .iter()
        .filter(|engine| engine.state == "cool")
        .map(|engine| engine.name)
        .collect();
    let first = engines
        .iter()
        .find(|engine| engine.state == "on")
        .map(|engine| engine.name);
    let stats = store::storage_stats_async(owner).await;
    let telegram_queue = store::queue_counts_async(Inbox::Telegram).await;
    let whatsapp_queue = store::queue_counts_async(Inbox::WhatsApp).await;
    let failed = telegram_queue.failed + whatsapp_queue.failed;

    let mut items = Vec::new();
    if restart_needed {
        items.push(attention("warn", "restart_needed", "system"));
    }
    if failed > 0 {
        let mut item = attention("err", "queue_failed", "queue");
        item["count"] = json!(failed);
        items.push(item);
    }
    if providers.is_empty() {
        items.push(attention("err", "no_provider", "ai"));
    }
    if !online {
        items.push(attention("warn", "telegram_offline", "telegram"));
    }
    if flag("can_read_all_group_messages") == Some(false) {
        items.push(attention("warn", "privacy_mode", "telegram"));
    }
    let (queries, chosen) = crate::bot::daemon::inline_counters();
    if flag("supports_inline_queries") == Some(true)
        && queries >= INLINE_FEEDBACK_HINT_AFTER
        && chosen == 0
    {
        items.push(attention("warn", "inline_feedback", "telegram"));
    }
    if keyed == 0 {
        items.push(attention("warn", "no_search_key", "search"));
    }
    if !paused.is_empty() {
        let mut item = attention("info", "search_paused", "search");
        item["names"] = json!(paused);
        items.push(item);
    }
    if link.phase == LinkPhase::Retrying {
        items.push(attention("err", "whatsapp_failed", "whatsapp"));
    } else if !linked {
        items.push(attention("info", "whatsapp_unlinked", "whatsapp"));
    }

    Json(json!({
        "system": system_summary(&state),
        "telegram": {
            "configured": crate::web::auth::telegram_configured(),
            "online": online,
            "username": state.bot_username(),
            "owner_id": crate::get_configured_owner_id().map(|id| id.to_string()),
            "last_poll_secs": crate::bot::daemon::last_poll_age_secs(),
        },
        "whatsapp": {
            "enabled": WaController::should_run(),
            "linked": linked,
            "phase": phase_name(link.phase),
        },
        "main_model": {
            "provider": active.as_ref().map(|provider| provider.name.clone()),
            "model": active.as_ref().map(|provider| provider.active_model.clone()),
            "catalogue": active.as_ref().map_or(0, |provider| provider.models.len()),
        },
        "search": {
            "first": first,
            "paused": paused,
            "keyed": keyed,
        },
        "storage": {
            "db_bytes": database_size(),
            "attachments_bytes": attachments_size().await,
            "memories": stats.memories,
            "messages": stats.messages,
            "conversations": stats.conversations,
        },
        "queue": {
            "pending": telegram_queue.pending + whatsapp_queue.pending,
            "failed": failed,
        },
        "attention": items,
        "restart_needed": restart_needed,
    }))
}
