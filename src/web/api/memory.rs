//! Long-term memory: the owner's profile facts.

use axum::extract::Path;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::ai::service::prompt::{MAX_PROMPT_FACT_CHARS, MAX_PROMPT_MEMORIES};
use crate::ai::storage::web as store;
use crate::web::chat::owner_id;
use crate::web::error::{ApiError, ApiResult};

use super::ok;

const MAX_KEY_CHARS: usize = 64;

/// GET /api/memory
pub(crate) async fn list() -> Json<Value> {
    let memories = store::list_memories_async(owner_id()).await;
    Json(json!({
        "memories": memories,
        "max_prompt": MAX_PROMPT_MEMORIES,
        "max_chars": MAX_PROMPT_FACT_CHARS,
    }))
}

#[derive(Deserialize)]
pub(crate) struct MemoryUpsertRequest {
    fact: String,
}

/// PUT /api/memory/:key
pub(crate) async fn upsert(
    Path(key): Path<String>,
    Json(body): Json<MemoryUpsertRequest>,
) -> ApiResult<Value> {
    let key = key.trim().to_string();
    let fact = body.fact.trim().to_string();
    let key_chars = key.chars().count();
    let fact_chars = fact.chars().count();
    if !(1..=MAX_KEY_CHARS).contains(&key_chars)
        || !(1..=MAX_PROMPT_FACT_CHARS).contains(&fact_chars)
    {
        return Err(ApiError::invalid(
            format!("The key needs 1 to {MAX_KEY_CHARS} characters and the fact 1 to {MAX_PROMPT_FACT_CHARS}."),
            format!("Kunci perlu 1 sampai {MAX_KEY_CHARS} karakter dan fakta 1 sampai {MAX_PROMPT_FACT_CHARS}."),
        ));
    }
    if !crate::ai::storage::save_user_memory_async(owner_id(), key, fact).await {
        return Err(ApiError::internal("memory could not be saved"));
    }
    Ok(ok())
}

/// DELETE /api/memory/:key
pub(crate) async fn remove(Path(key): Path<String>) -> ApiResult<Value> {
    if !crate::ai::storage::delete_user_memory_async(owner_id(), key).await {
        return Err(ApiError::internal("memory could not be deleted"));
    }
    Ok(ok())
}

/// DELETE /api/memory
pub(crate) async fn clear() -> ApiResult<Value> {
    if !crate::ai::storage::clear_user_memories_async(owner_id()).await {
        return Err(ApiError::internal("memories could not be deleted"));
    }
    tracing::info!("Every long-term memory was deleted from the WebUI");
    Ok(ok())
}
