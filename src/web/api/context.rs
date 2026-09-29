//! Context page: what fills the context window of each conversation.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::State;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::ai::service::context::{estimate_stored_content_tokens, estimate_text_tokens};
use crate::ai::service::prompt::{build_system_prompt, BASE_SYSTEM_PROMPT};
use crate::ai::service::session::cli_session_thread_id;
use crate::ai::storage::web as store;
use crate::web::chat::owner_id;
use crate::web::error::{ApiError, ApiResult};
use crate::web::settings;
use crate::web::WebState;

use super::ok;

/// Conversations listed, most recent first.
const SCOPE_LIMIT: usize = 100;
/// Messages read per conversation for the history estimate.
const HISTORY_SAMPLE: usize = 200;

fn scope_kind(owner: i64, chat: i64, thread: i64) -> &'static str {
    if thread < 0 && chat == owner {
        "cli"
    } else if thread > 0 {
        "topic"
    } else if chat == owner && thread == 0 {
        "private"
    } else if chat < 0 {
        "group"
    } else {
        "chat"
    }
}

/// GET /api/context
pub(crate) async fn state(State(state): State<Arc<WebState>>) -> Json<Value> {
    let owner = owner_id();
    let memories = crate::ai::storage::get_user_memories_async(owner).await;
    let base = estimate_text_tokens(BASE_SYSTEM_PROMPT);
    let system = base + estimate_text_tokens(&crate::ai::tools::get_tools_definition().to_string());
    let memory = estimate_text_tokens(&build_system_prompt(&memories, None)).saturating_sub(base);
    let provider = state.ai.get_active_provider(owner).await;
    let model = state.ai.get_user_model(owner).await;
    let endpoint = provider
        .map(|provider| provider.endpoint)
        .unwrap_or_default();
    let budget = state
        .ai
        .resolved_model_capability(&endpoint, &model)
        .await
        .context_limit;
    let sessions: HashMap<usize, String> = state
        .ai
        .get_sessions(owner)
        .await
        .into_iter()
        .map(|session| (session.id, session.name))
        .collect();

    let mut rows = store::list_scopes_async(SCOPE_LIMIT).await;
    // Chat sessions without messages are listed too, like in the mockup.
    for id in sessions.keys() {
        let thread = cli_session_thread_id(*id);
        if !rows
            .iter()
            .any(|row| row.chat_id == owner && row.thread_id == thread)
        {
            rows.push(store::ScopeRow {
                chat_id: owner,
                thread_id: thread,
                messages: 0,
                last_at: None,
            });
        }
    }

    let mut scopes = Vec::with_capacity(rows.len());
    for row in rows {
        let kind = scope_kind(owner, row.chat_id, row.thread_id);
        let session_id = (kind == "cli")
            .then(|| usize::try_from(row.thread_id.unsigned_abs()).ok())
            .flatten();
        let summary = store::summary_async(row.chat_id, row.thread_id).await;
        let history: usize = if row.messages == 0 {
            0
        } else {
            crate::ai::storage::load_scoped_messages_async(
                row.chat_id,
                row.thread_id,
                HISTORY_SAMPLE,
            )
            .await
            .iter()
            .map(|message| estimate_stored_content_tokens(&message.content))
            .sum()
        };
        let summary_tokens = summary
            .as_ref()
            .map_or(0, |(text, _)| estimate_text_tokens(text));
        scopes.push(json!({
            "chat": row.chat_id.to_string(),
            "thread": row.thread_id,
            "kind": kind,
            "name": session_id.and_then(|id| sessions.get(&id).cloned()),
            "session_id": session_id,
            "messages": row.messages,
            "summary": summary.as_ref().map(|(text, _)| text.clone()),
            "summary_updated_at": summary.map(|(_, at)| at),
            "last_at": row.last_at,
            "tokens": {
                "system": system,
                "memory": memory,
                "summary": summary_tokens,
                "history": history,
                "budget": budget,
            },
        }));
    }
    Json(json!({
        "scopes": scopes,
        "retention": settings::effective("XIAO_HISTORY_RETENTION").trim().parse::<u64>().unwrap_or(0),
        "model": Some(model).filter(|model| !model.is_empty()),
        "env_locks": settings::env_locks(["XIAO_HISTORY_RETENTION"]),
    }))
}

#[derive(Deserialize)]
pub(crate) struct ContextClearRequest {
    chat: String,
    thread: i64,
    op: String,
}

/// POST /api/context/clear
pub(crate) async fn clear(
    State(state): State<Arc<WebState>>,
    Json(body): Json<ContextClearRequest>,
) -> ApiResult<Value> {
    let chat = body
        .chat
        .trim()
        .parse::<i64>()
        .map_err(|_| ApiError::invalid("Unknown conversation.", "Percakapan tidak dikenal."))?;
    let owner = owner_id();
    if chat == owner && body.thread < 0 {
        let session = usize::try_from(body.thread.unsigned_abs()).unwrap_or_default();
        if state.chat.is_busy(session) {
            return Err(ApiError::busy(
                "This session is still answering.",
                "Sesi ini masih menjawab.",
            ));
        }
    }
    match body.op.as_str() {
        "history" => {
            if !state.ai.clear_scoped_history(chat, body.thread).await {
                return Err(ApiError::internal("history could not be cleared"));
            }
        }
        "summary" => {
            if !store::delete_summary_async(chat, body.thread).await {
                return Err(ApiError::internal("summary could not be deleted"));
            }
        }
        _ => {
            return Err(ApiError::invalid(
                "op must be history or summary",
                "op harus history atau summary",
            ))
        }
    }
    Ok(ok())
}
