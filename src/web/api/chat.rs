//! Chat page: the `xiao chat` sessions, their history, uploads and answers
//! streamed as server-sent events.

use std::convert::Infallible;
use std::sync::Arc;

use axum::body::{Body, Bytes};
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, HeaderValue};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::ai::service::session::cli_session_thread_id;
use crate::ai::storage::web as store;
use crate::ai::storage::ChatMessage;
use crate::attachments::decode_user_content;
use crate::parser::web::render_media_markup_for_web;
use crate::web::chat::{
    attached_documents, guess_mime, owner_id, route_for, spawn_generation, FileRef, DOC_MARKER,
    MAX_UPLOAD_BYTES,
};
use crate::web::error::{ApiError, ApiResult};
use crate::web::WebState;

use super::ok;

/// Newest messages shown when a session opens.
const MESSAGE_LIMIT: usize = 200;
const MAX_SESSION_NAME_CHARS: usize = 60;

async fn session_exists(state: &WebState, id: usize) -> bool {
    state
        .ai
        .get_sessions(owner_id())
        .await
        .iter()
        .any(|session| session.id == id)
}

fn busy_error() -> ApiError {
    ApiError::busy(
        "This session is still answering. Stop it or wait.",
        "Sesi ini masih menjawab. Hentikan atau tunggu.",
    )
}

/// GET /api/chat/sessions
pub(crate) async fn sessions(State(state): State<Arc<WebState>>) -> Json<Value> {
    let owner = owner_id();
    let mut sessions = state.ai.get_sessions(owner).await;
    sessions.sort_by_key(|session| std::cmp::Reverse(session.id));
    let stats = store::cli_scope_stats_async(owner).await;
    let list: Vec<Value> = sessions
        .iter()
        .map(|session| {
            let (messages, last_at) = stats
                .get(&cli_session_thread_id(session.id))
                .cloned()
                .unwrap_or((0, None));
            json!({
                "id": session.id,
                "name": session.name,
                "created_at": session.created_at,
                "messages": messages,
                "last_at": last_at,
                "busy": state.chat.is_busy(session.id),
            })
        })
        .collect();
    let model = state
        .ai
        .resolve_model_route(crate::ai::routing::ModelRole::Main)
        .await
        .ok()
        .map(|route| route.model);
    Json(json!({
        "sessions": list,
        "active": state.ai.get_active_session_id(owner).await,
        "model": model,
    }))
}

#[derive(Deserialize)]
pub(crate) struct CreateRequest {
    #[serde(default)]
    name: Option<String>,
}

/// POST /api/chat/sessions
pub(crate) async fn create(
    State(state): State<Arc<WebState>>,
    Json(body): Json<CreateRequest>,
) -> ApiResult<Value> {
    let session = state
        .ai
        .create_new_session(owner_id(), body.name.as_deref())
        .await
        .ok_or_else(|| ApiError::internal("chat session could not be created"))?;
    Ok(Json(json!({
        "id": session.id,
        "name": session.name,
        "created_at": session.created_at,
        "messages": 0,
        "last_at": null,
        "busy": false,
    })))
}

#[derive(Deserialize)]
pub(crate) struct RenameRequest {
    name: String,
}

/// PATCH /api/chat/sessions/:id
pub(crate) async fn rename(
    State(state): State<Arc<WebState>>,
    Path(id): Path<usize>,
    Json(body): Json<RenameRequest>,
) -> ApiResult<Value> {
    let name = body.name.trim().to_string();
    if name.is_empty() || name.chars().count() > MAX_SESSION_NAME_CHARS {
        return Err(ApiError::invalid(
            format!("The name needs 1 to {MAX_SESSION_NAME_CHARS} characters."),
            format!("Nama perlu 1 sampai {MAX_SESSION_NAME_CHARS} karakter."),
        ));
    }
    let owner = owner_id();
    if !store::rename_session_async(owner, id, name.clone()).await {
        return Err(ApiError::not_found());
    }
    if let Some(list) = state.ai.user_sessions.write().await.get_mut(&owner) {
        if let Some(session) = list.iter_mut().find(|session| session.id == id) {
            session.name = name;
        }
    }
    Ok(ok())
}

/// DELETE /api/chat/sessions/:id
pub(crate) async fn remove(
    State(state): State<Arc<WebState>>,
    Path(id): Path<usize>,
) -> ApiResult<Value> {
    if state.chat.is_busy(id) {
        return Err(busy_error());
    }
    if !state.ai.remove_session_by_id(owner_id(), id).await {
        return Err(ApiError::not_found());
    }
    Ok(ok())
}

/// The user's words and the attachment chips of a stored user message.
fn user_view(content: &Value) -> (String, Vec<FileRef>) {
    let (mut text, mut files) = match decode_user_content(content) {
        Some(persisted) => (
            persisted.text,
            persisted
                .attachments
                .into_iter()
                .map(|attachment| FileRef {
                    id: None,
                    name: attachment.name.unwrap_or(attachment.kind),
                    size: None,
                    mime: Some(attachment.mime_type),
                })
                .collect(),
        ),
        None => (
            content
                .as_str()
                .map_or_else(|| content.to_string(), str::to_string),
            Vec::new(),
        ),
    };
    let split = text
        .find(&format!("\n\n{DOC_MARKER}"))
        .map(|at| (at, at + 2))
        .or_else(|| text.starts_with(DOC_MARKER).then_some((0, 0)));
    if let Some((cut, marker)) = split {
        let names = text[marker + DOC_MARKER.len()..]
            .split(']')
            .next()
            .unwrap_or_default()
            .to_string();
        files.extend(
            names
                .split(", ")
                .filter(|name| !name.is_empty())
                .map(|name| FileRef {
                    id: None,
                    name: name.to_string(),
                    size: None,
                    mime: None,
                }),
        );
        text.truncate(cut);
    }
    (text, files)
}

fn message_view(state: &WebState, session_id: usize, message: &ChatMessage) -> Value {
    if message.role == "user" {
        let (text, files) = user_view(&message.content);
        return json!({"role": "user", "text": text, "files": files, "at": null});
    }
    let raw = message
        .content
        .as_str()
        .map_or_else(|| message.content.to_string(), str::to_string);
    let files: Vec<FileRef> = attached_documents(&raw)
        .into_iter()
        .map(|(name, key)| state.chat.find_attached(session_id, &key, &name))
        .collect();
    json!({
        "role": "assistant",
        "text": render_media_markup_for_web(&raw),
        "files": files,
        "at": null,
    })
}

/// GET /api/chat/sessions/:id/messages
pub(crate) async fn messages(
    State(state): State<Arc<WebState>>,
    Path(id): Path<usize>,
) -> ApiResult<Value> {
    if !session_exists(&state, id).await {
        return Err(ApiError::not_found());
    }
    let stored = crate::ai::storage::load_scoped_messages_async(
        owner_id(),
        cli_session_thread_id(id),
        MESSAGE_LIMIT,
    )
    .await;
    let messages: Vec<Value> = stored
        .iter()
        .filter(|message| message.role == "user" || message.role == "assistant")
        .map(|message| message_view(&state, id, message))
        .collect();
    Ok(Json(json!({
        "messages": messages,
        "busy": state.chat.is_busy(id),
    })))
}

/// POST /api/chat/sessions/:id/clear
pub(crate) async fn clear(
    State(state): State<Arc<WebState>>,
    Path(id): Path<usize>,
) -> ApiResult<Value> {
    if state.chat.is_busy(id) {
        return Err(busy_error());
    }
    if !session_exists(&state, id).await {
        return Err(ApiError::not_found());
    }
    if !state
        .ai
        .clear_scoped_history(owner_id(), cli_session_thread_id(id))
        .await
    {
        return Err(ApiError::internal("chat history could not be cleared"));
    }
    Ok(ok())
}

/// POST /api/chat/uploads (raw body, `X-File-Name`, `Content-Type`)
pub(crate) async fn upload(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Value> {
    if body.is_empty() {
        return Err(ApiError::invalid("The file is empty.", "Berkasnya kosong."));
    }
    if body.len() > MAX_UPLOAD_BYTES {
        return Err(ApiError::invalid(
            "Files can be at most 20 MB.",
            "Berkas paling besar 20 MB.",
        ));
    }
    let raw_name = headers
        .get("x-file-name")
        .and_then(|value| value.to_str().ok())
        .map(|value| {
            urlencoding::decode(value)
                .map(|name| name.into_owned())
                .unwrap_or_else(|_| value.to_string())
        })
        .unwrap_or_default();
    let base = raw_name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .chars()
        .filter(|ch| !ch.is_control())
        .collect::<String>();
    let name = match base.trim() {
        "" => "file".to_string(),
        trimmed => crate::util::truncate_chars(trimmed, 120),
    };
    let declared = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|value| {
            value
                .split(';')
                .next()
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase()
        })
        .filter(|value| !value.is_empty() && value != "application/octet-stream");
    let mime = declared.unwrap_or_else(|| guess_mime(&name).to_string());
    let size = body.len();
    let id = state
        .chat
        .add_upload(name.clone(), mime.clone(), body.to_vec())
        .ok_or_else(|| ApiError::internal("upload could not be stored"))?;
    Ok(Json(json!({
        "id": id,
        "name": name,
        "size": size,
        "mime": mime,
        "route": route_for(&mime),
    })))
}

#[derive(Deserialize)]
pub(crate) struct SendRequest {
    #[serde(default)]
    text: String,
    #[serde(default)]
    uploads: Vec<String>,
}

/// POST /api/chat/sessions/:id/send → `text/event-stream`
pub(crate) async fn send(
    State(state): State<Arc<WebState>>,
    Path(id): Path<usize>,
    Json(body): Json<SendRequest>,
) -> Result<Response, ApiError> {
    if body.text.trim().is_empty() && body.uploads.is_empty() {
        return Err(ApiError::invalid(
            "Write a message first.",
            "Tulis pesan dulu.",
        ));
    }
    if !session_exists(&state, id).await {
        return Err(ApiError::not_found());
    }
    if state.chat.is_busy(id) {
        return Err(busy_error());
    }
    let uploads = state.chat.take_uploads(&body.uploads).map_err(|_| {
        ApiError::invalid(
            "An attachment expired. Attach it again.",
            "Lampiran sudah kedaluwarsa. Lampirkan lagi.",
        )
    })?;
    let (events, receiver) = tokio::sync::mpsc::unbounded_channel::<Event>();
    if !spawn_generation(Arc::clone(&state), id, body.text, uploads, events) {
        return Err(busy_error());
    }
    let stream = futures_util::stream::unfold(receiver, |mut receiver| async move {
        receiver
            .recv()
            .await
            .map(|event| (Ok::<Event, Infallible>(event), receiver))
    });
    let mut response = Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response();
    response
        .headers_mut()
        .insert("x-accel-buffering", HeaderValue::from_static("no"));
    Ok(response)
}

/// POST /api/chat/sessions/:id/stop
pub(crate) async fn stop(State(state): State<Arc<WebState>>, Path(id): Path<usize>) -> Json<Value> {
    if let Some(draft_id) = state.chat.draft_of(id) {
        state.ai.cancel_generation(owner_id(), draft_id).await;
    }
    ok()
}

/// GET /api/chat/files/:id
pub(crate) async fn file(
    State(state): State<Arc<WebState>>,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let (name, mime, bytes) = state.chat.file(&id).ok_or_else(ApiError::not_found)?;
    let mut response = Body::from(bytes).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&mime)
            .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
    );
    let safe_name: String = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_graphic() || ch == ' ' {
                ch
            } else {
                '_'
            }
        })
        .filter(|ch| *ch != '"' && *ch != '\\')
        .collect();
    let disposition = format!(
        "attachment; filename=\"{safe_name}\"; filename*=UTF-8''{}",
        urlencoding::encode(&name)
    );
    if let Ok(value) = HeaderValue::from_str(&disposition) {
        headers.insert(header::CONTENT_DISPOSITION, value);
    }
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_markers_become_file_chips() {
        let (text, files) = user_view(&Value::String(format!(
            "Ringkas ini\n\n{DOC_MARKER}a.pdf, b.txt]\n--- a.pdf ---\nisi"
        )));
        assert_eq!(text, "Ringkas ini");
        let names: Vec<&str> = files.iter().map(|file| file.name.as_str()).collect();
        assert_eq!(names, vec!["a.pdf", "b.txt"]);

        let (text, files) = user_view(&Value::String(format!("{DOC_MARKER}c.csv]\nisi")));
        assert_eq!(text, "");
        assert_eq!(files.len(), 1);

        let (text, files) = user_view(&Value::String("Halo".to_string()));
        assert_eq!((text.as_str(), files.len()), ("Halo", 0));
    }
}
