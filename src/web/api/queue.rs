//! Durable inbox queues: counters and the quarantined (failed) rows.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::Json;
use serde_json::{json, Value};

use crate::ai::storage::web::{self as store, FailedInboxRow, Inbox};
use crate::web::error::{ApiError, ApiResult};
use crate::web::WebState;

use super::ok;

/// Quarantined rows listed per channel.
const FAILED_LIMIT: usize = 200;

struct RowInfo {
    kind: &'static str,
    chat_id: Option<i64>,
    thread_id: Option<i64>,
    private: bool,
    retryable: bool,
}

/// What a quarantined Telegram update was, read from its payload.
fn telegram_row_info(payload_json: &str) -> RowInfo {
    let payload: Value = serde_json::from_str(payload_json).unwrap_or(Value::Null);
    let (kind, retryable, container) = [
        ("message", "message", true),
        ("edited_message", "edited", true),
        ("guest_message", "guest", true),
        ("inline_query", "inline", false),
        ("chosen_inline_result", "inline_result", true),
        ("callback_query", "callback", true),
        ("stopped_message_generation", "stop", false),
    ]
    .iter()
    .find(|(field, _, _)| payload.get(*field).is_some())
    .map_or(("other", false, Value::Null), |(field, kind, retryable)| {
        (
            *kind,
            *retryable,
            payload.get(*field).cloned().unwrap_or(Value::Null),
        )
    });
    let message = container.get("message").unwrap_or(&container);
    let chat = message.get("chat");
    RowInfo {
        kind,
        chat_id: chat.and_then(|chat| chat.get("id")).and_then(Value::as_i64),
        thread_id: message.get("message_thread_id").and_then(Value::as_i64),
        private: chat
            .and_then(|chat| chat.get("type"))
            .and_then(Value::as_str)
            == Some("private"),
        retryable,
    }
}

fn row_info(channel: Inbox, row: &FailedInboxRow) -> RowInfo {
    match channel {
        Inbox::Telegram => telegram_row_info(&row.payload_json),
        Inbox::WhatsApp => RowInfo {
            kind: "message",
            chat_id: row.chat_id,
            thread_id: None,
            private: row.chat_id.is_some_and(|chat| chat > 0),
            retryable: true,
        },
    }
}

fn channel_of(row: &FailedInboxRow) -> Inbox {
    if row.chat_id.is_some() {
        Inbox::WhatsApp
    } else {
        Inbox::Telegram
    }
}

fn parse_channel(raw: &str) -> Result<Inbox, ApiError> {
    match raw {
        "telegram" => Ok(Inbox::Telegram),
        "whatsapp" => Ok(Inbox::WhatsApp),
        _ => Err(ApiError::not_found()),
    }
}

/// GET /api/queue
pub(crate) async fn state() -> Json<Value> {
    let telegram = store::queue_counts_async(Inbox::Telegram).await;
    let whatsapp = store::queue_counts_async(Inbox::WhatsApp).await;
    let mut rows = store::failed_rows_async(FAILED_LIMIT).await;
    rows.sort_by(|a, b| b.received_at.cmp(&a.received_at));
    let failed: Vec<Value> = rows
        .iter()
        .take(FAILED_LIMIT)
        .map(|row| {
            let channel = channel_of(row);
            let info = row_info(channel, row);
            json!({
                "channel": if channel == Inbox::Telegram { "telegram" } else { "whatsapp" },
                "id": row.id,
                "kind": info.kind,
                "chat_id": info.chat_id.map(|id| id.to_string()),
                "thread_id": info.thread_id,
                "private": info.private,
                "attempts": row.attempts,
                "received_at": row.received_at,
                "error": row.last_error,
                "retryable": info.retryable,
            })
        })
        .collect();
    Json(json!({
        "telegram": telegram,
        "whatsapp": whatsapp,
        "failed": failed,
    }))
}

/// POST /api/queue/:channel/:id/retry
pub(crate) async fn retry(
    State(state): State<Arc<WebState>>,
    Path((channel, id)): Path<(String, String)>,
) -> ApiResult<Value> {
    let channel = parse_channel(&channel)?;
    let row = store::failed_rows_async(1_000)
        .await
        .into_iter()
        .find(|row| row.id == id && channel_of(row) == channel)
        .ok_or_else(ApiError::not_found)?;
    if !row_info(channel, &row).retryable {
        return Err(ApiError::conflict(
            "This update cannot be answered again (it expired).",
            "Update ini tidak bisa dijawab lagi (sudah kedaluwarsa).",
        ));
    }
    match channel {
        Inbox::Telegram => {
            let update_id = id.parse::<i64>().map_err(|_| ApiError::not_found())?;
            let payload = store::retry_telegram_async(update_id)
                .await
                .ok_or_else(ApiError::not_found)?;
            // Hand it to the running workers; without Telegram it waits in
            // the queue and is answered after the next start.
            if let Some(link) = state.telegram() {
                match serde_json::from_str::<crate::bot::models::Update>(&payload) {
                    Ok(update) => {
                        if link.updates.send(update).await.is_err() {
                            tracing::warn!(
                                "Telegram workers stopped; update {update_id} stays queued"
                            );
                        }
                    }
                    Err(error) => {
                        tracing::warn!("Queued update {update_id} could not be decoded: {error}")
                    }
                }
            }
        }
        Inbox::WhatsApp => {
            if !store::retry_whatsapp_async(id).await {
                return Err(ApiError::not_found());
            }
            state.wa.replay();
        }
    }
    Ok(ok())
}

/// DELETE /api/queue/:channel/:id
pub(crate) async fn dismiss(Path((channel, id)): Path<(String, String)>) -> ApiResult<Value> {
    let channel = parse_channel(&channel)?;
    if !store::dismiss_async(channel, id).await {
        return Err(ApiError::not_found());
    }
    Ok(ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn telegram_payloads_are_classified() {
        let message = telegram_row_info(
            r#"{"update_id":1,"message":{"message_id":2,"chat":{"id":42,"type":"private"},"date":0}}"#,
        );
        assert_eq!(message.kind, "message");
        assert_eq!(message.chat_id, Some(42));
        assert!(message.private && message.retryable);

        let topic = telegram_row_info(
            r#"{"update_id":1,"message":{"message_id":2,"message_thread_id":82,"chat":{"id":-100,"type":"supergroup"},"date":0}}"#,
        );
        assert_eq!(
            (topic.chat_id, topic.thread_id, topic.private),
            (Some(-100), Some(82), false)
        );

        let inline = telegram_row_info(r#"{"update_id":1,"inline_query":{"id":"x","query":"q"}}"#);
        assert_eq!(inline.kind, "inline");
        assert!(
            !inline.retryable,
            "an expired inline query cannot be answered"
        );

        let scrubbed =
            telegram_row_info(r#"{"update_id":1,"payload":"redacted_after_completion"}"#);
        assert_eq!(scrubbed.kind, "other");
        assert!(!scrubbed.retryable);
    }
}
