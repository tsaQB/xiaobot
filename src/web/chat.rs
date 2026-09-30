//! Chat from the WebUI.
//!
//! Web chat sessions are the terminal sessions of `xiao chat`: the same list
//! and the same history scope (`thread_id = cli_session_thread_id(id)` in the
//! owner's chat). A generation runs like a WhatsApp one: under the scope's
//! generation lock, registered with `begin_generation` so Stop and shutdown
//! reach it, with progress streamed to the browser through a
//! [`GenerationProgressSink`]. It keeps running when the browser goes away,
//! so the answer still lands in the history.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use axum::response::sse::Event;
use regex::Regex;
use serde::Serialize;
use serde_json::{json, Value};
use tokio::sync::mpsc::UnboundedSender;
use tracing::warn;

use crate::ai::service::session::cli_session_thread_id;
use crate::ai::service::{GenerationInput, ModelRole};
use crate::bot::models::StagedDocument;
use crate::parser::web::render_media_markup_for_web;
use crate::timeline::{GenerationProgressSink, ProgressActivity};

use super::auth::random_hex;
use super::WebState;

/// Largest single upload.
pub(crate) const MAX_UPLOAD_BYTES: usize = 20 * 1024 * 1024;
/// Uploads not used in a message are dropped after this long.
const UPLOAD_TTL: Duration = Duration::from_secs(30 * 60);
/// Generated files stay downloadable this long.
const FILE_TTL: Duration = Duration::from_secs(2 * 60 * 60);
/// Memory kept for uploads and for generated files, each.
const STORE_BUDGET_BYTES: usize = 200 * 1024 * 1024;

/// Marker the web chat puts before attached document text in the history.
pub(crate) const DOC_MARKER: &str = "[Dokumen Terlampir: ";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum UploadRoute {
    Vision,
    Stt,
    Video,
    Doc,
}

pub(crate) struct Upload {
    pub name: String,
    pub mime: String,
    pub bytes: Vec<u8>,
    at: Instant,
}

impl Upload {
    pub(crate) fn route(&self) -> UploadRoute {
        route_for(&self.mime)
    }
}

pub(crate) fn route_for(mime: &str) -> UploadRoute {
    let mime = mime.to_ascii_lowercase();
    if mime.starts_with("image/") {
        UploadRoute::Vision
    } else if mime.starts_with("audio/") {
        UploadRoute::Stt
    } else if mime.starts_with("video/") {
        UploadRoute::Video
    } else {
        UploadRoute::Doc
    }
}

/// A MIME type for a name when the browser sent none.
pub(crate) fn guess_mime(name: &str) -> &'static str {
    let lower = name.to_ascii_lowercase();
    let ext = lower.rsplit('.').next().unwrap_or_default();
    match ext {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "mp3" => "audio/mpeg",
        "ogg" | "oga" | "opus" => "audio/ogg",
        "wav" => "audio/wav",
        "m4a" => "audio/mp4",
        "flac" => "audio/flac",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "pdf" => "application/pdf",
        "txt" | "md" | "log" | "csv" => "text/plain",
        "json" => "application/json",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "zip" => "application/zip",
        _ => "application/octet-stream",
    }
}

struct StoredFile {
    session_id: usize,
    attach_key: String,
    name: String,
    mime: String,
    bytes: Vec<u8>,
    at: Instant,
}

/// A file shown in the chat: `id` downloads it when it is still stored.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct FileRef {
    pub id: Option<String>,
    pub name: String,
    pub size: Option<usize>,
    pub mime: Option<String>,
}

#[derive(Default)]
pub(crate) struct ChatRuntime {
    uploads: Mutex<HashMap<String, Upload>>,
    files: Mutex<HashMap<String, StoredFile>>,
    /// Session id → draft id of the generation answering it.
    running: Mutex<HashMap<usize, i64>>,
}

/// Drops expired entries, then the oldest ones until `budget` is kept.
fn prune<T>(
    map: &mut HashMap<String, T>,
    ttl: Duration,
    budget: usize,
    at: impl Fn(&T) -> Instant,
    size: impl Fn(&T) -> usize,
) {
    map.retain(|_, entry| at(entry).elapsed() < ttl);
    let mut total: usize = map.values().map(&size).sum();
    while total > budget {
        let Some(oldest) = map
            .iter()
            .min_by_key(|(_, entry)| at(entry))
            .map(|(id, _)| id.clone())
        else {
            break;
        };
        if let Some(entry) = map.remove(&oldest) {
            total = total.saturating_sub(size(&entry));
        }
    }
}

impl ChatRuntime {
    pub(crate) fn add_upload(&self, name: String, mime: String, bytes: Vec<u8>) -> Option<String> {
        let mut uploads = self.uploads.lock().ok()?;
        let id = random_hex(12);
        uploads.insert(
            id.clone(),
            Upload {
                name,
                mime,
                bytes,
                at: Instant::now(),
            },
        );
        prune(
            &mut uploads,
            UPLOAD_TTL,
            STORE_BUDGET_BYTES,
            |upload| upload.at,
            |upload| upload.bytes.len(),
        );
        uploads.contains_key(&id).then_some(id)
    }

    /// Takes the uploads for a message; `Err` names the first missing id.
    pub(crate) fn take_uploads(&self, ids: &[String]) -> Result<Vec<Upload>, String> {
        let Ok(mut uploads) = self.uploads.lock() else {
            return Err(String::new());
        };
        if let Some(missing) = ids.iter().find(|id| !uploads.contains_key(*id)) {
            return Err(missing.clone());
        }
        Ok(ids.iter().filter_map(|id| uploads.remove(id)).collect())
    }

    fn store_file(&self, session_id: usize, document: StagedDocument) -> FileRef {
        let (attach_key, bytes, mime, name) = document.into_raw_tuple();
        let id = random_hex(12);
        let reference = FileRef {
            id: Some(id.clone()),
            name: name.clone(),
            size: Some(bytes.len()),
            mime: Some(mime.clone()),
        };
        if let Ok(mut files) = self.files.lock() {
            files.insert(
                id,
                StoredFile {
                    session_id,
                    attach_key,
                    name,
                    mime,
                    bytes,
                    at: Instant::now(),
                },
            );
            prune(
                &mut files,
                FILE_TTL,
                STORE_BUDGET_BYTES,
                |file| file.at,
                |file| file.bytes.len(),
            );
        }
        reference
    }

    /// Name, MIME type and bytes of a generated file.
    pub(crate) fn file(&self, id: &str) -> Option<(String, String, Vec<u8>)> {
        let files = self.files.lock().ok()?;
        let file = files.get(id)?;
        (file.at.elapsed() < FILE_TTL)
            .then(|| (file.name.clone(), file.mime.clone(), file.bytes.clone()))
    }

    /// The newest stored file a history answer refers to with `attach://`.
    pub(crate) fn find_attached(&self, session_id: usize, attach_key: &str, name: &str) -> FileRef {
        let found = self.files.lock().ok().and_then(|files| {
            files
                .iter()
                .filter(|(_, file)| {
                    file.session_id == session_id
                        && file.attach_key == attach_key
                        && file.name == name
                        && file.at.elapsed() < FILE_TTL
                })
                .max_by_key(|(_, file)| file.at)
                .map(|(id, file)| (id.clone(), file.bytes.len(), file.mime.clone()))
        });
        match found {
            Some((id, size, mime)) => FileRef {
                id: Some(id),
                name: name.to_string(),
                size: Some(size),
                mime: Some(mime),
            },
            None => FileRef {
                id: None,
                name: name.to_string(),
                size: None,
                mime: None,
            },
        }
    }

    pub(crate) fn is_busy(&self, session_id: usize) -> bool {
        self.running
            .lock()
            .is_ok_and(|running| running.contains_key(&session_id))
    }

    fn begin(&self, session_id: usize, draft_id: i64) -> bool {
        let Ok(mut running) = self.running.lock() else {
            return false;
        };
        if running.contains_key(&session_id) {
            return false;
        }
        running.insert(session_id, draft_id);
        true
    }

    fn end(&self, session_id: usize) {
        if let Ok(mut running) = self.running.lock() {
            running.remove(&session_id);
        }
    }

    pub(crate) fn draft_of(&self, session_id: usize) -> Option<i64> {
        self.running
            .lock()
            .ok()
            .and_then(|running| running.get(&session_id).copied())
    }
}

static RE_ATTACHED_DOC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\[(?:document|dokumen):\s*([^\]\n]+)\]\(attach://([^)\s]+)\)")
        .expect("valid regex")
});

/// Documents an answer refers to with `attach://` links: (name, key).
pub(crate) fn attached_documents(text: &str) -> Vec<(String, String)> {
    RE_ATTACHED_DOC
        .captures_iter(text)
        .map(|caps| (caps[1].trim().to_string(), caps[2].to_string()))
        .collect()
}

fn sse(event: &str, data: &Value) -> Event {
    Event::default()
        .event(event)
        .json_data(data)
        .unwrap_or_else(|_| Event::default().event(event).data("{}"))
}

fn activity_name(activity: ProgressActivity) -> &'static str {
    match activity {
        ProgressActivity::Thinking => "thinking",
        ProgressActivity::Looking => "looking",
        ProgressActivity::Reading => "reading",
        ProgressActivity::Searching => "searching",
        ProgressActivity::Fetching => "fetching",
        ProgressActivity::Writing => "writing",
        ProgressActivity::Listening => "listening",
        ProgressActivity::Drawing => "drawing",
        ProgressActivity::Watching => "watching",
        ProgressActivity::Summarizing => "summarizing",
        ProgressActivity::Quiz => "quiz",
    }
}

/// Forwards generation progress to the browser as SSE events.
struct WebSink {
    events: UnboundedSender<Event>,
    failure: Mutex<Option<String>>,
}

impl GenerationProgressSink for WebSink {
    fn on_action(&self, label: &str, activity: Option<ProgressActivity>) {
        let activity = activity_name(activity.unwrap_or(ProgressActivity::Thinking));
        let _ = self.events.send(sse(
            "status",
            &json!({"label": label, "activity": activity}),
        ));
    }

    fn on_partial_answer(&self, text: &str) {
        let partial = render_media_markup_for_web(text);
        let _ = self.events.send(sse("text", &json!({"partial": partial})));
    }

    fn on_failure(&self, error: &str, _force_sync: bool) {
        if let Ok(mut failure) = self.failure.lock() {
            *failure = Some(error.to_string());
        }
    }

    fn on_complete(&self) {}
}

/// Owner chat that holds the terminal/web sessions (0 without an owner).
pub(crate) fn owner_id() -> i64 {
    crate::get_configured_owner_id().unwrap_or(0)
}

/// Starts answering `text` in `session_id`, streaming events to `events`.
/// Returns `false` when the session is already answering.
pub(crate) fn spawn_generation(
    state: Arc<WebState>,
    session_id: usize,
    text: String,
    uploads: Vec<Upload>,
    events: UnboundedSender<Event>,
) -> bool {
    let draft_id = crate::ai::service::next_draft_id();
    if !state.chat.begin(session_id, draft_id) {
        return false;
    }
    tokio::spawn(async move {
        run_generation(&state, session_id, draft_id, text, uploads, events).await;
        state.chat.end(session_id);
    });
    true
}

/// Media and document text prepared from the uploads of one message.
#[derive(Default)]
struct PreparedMedia {
    image: Option<(Vec<u8>, String)>,
    audio: Option<(Vec<u8>, String)>,
    video: Option<(Vec<u8>, String)>,
    pages: Option<Vec<Vec<u8>>>,
    doc_names: Vec<String>,
    doc_text: String,
    notes: Vec<String>,
}

impl PreparedMedia {
    fn has_media(&self) -> bool {
        self.image.is_some() || self.audio.is_some() || self.video.is_some() || self.pages.is_some()
    }
}

async fn prepare_media(uploads: Vec<Upload>) -> PreparedMedia {
    let mut media = PreparedMedia::default();
    for upload in uploads {
        let route = upload.route();
        if route != UploadRoute::Doc && media.has_media() {
            media.notes.push(format!(
                "[Catatan sistem: hanya satu gambar, audio, atau video per pesan yang dibaca; '{}' dilewati.]",
                upload.name
            ));
            continue;
        }
        match route {
            UploadRoute::Vision => media.image = Some((upload.bytes, upload.mime)),
            UploadRoute::Stt => media.audio = Some((upload.bytes, upload.mime)),
            UploadRoute::Video => media.video = Some((upload.bytes, upload.mime)),
            UploadRoute::Doc => {
                match crate::document::extract_document(upload.bytes, &upload.mime, &upload.name)
                    .await
                {
                    Ok(extracted) => {
                        if let Some(text) = extracted.text.filter(|text| !text.trim().is_empty()) {
                            if !media.doc_text.is_empty() {
                                media.doc_text.push_str("\n\n");
                            }
                            media.doc_text.push_str(&format!(
                                "--- {} ---\n{}",
                                upload.name,
                                text.trim()
                            ));
                            media.doc_names.push(upload.name);
                        } else if !extracted.rendered_pages.is_empty() && !media.has_media() {
                            media.pages = Some(extracted.rendered_pages);
                            media.doc_names.push(upload.name);
                        } else {
                            media.notes.push(format!(
                                "[Catatan sistem: berkas '{}' tidak berisi teks yang bisa dibaca.]",
                                upload.name
                            ));
                        }
                        if let Some(warning) = extracted.warning {
                            media.notes.push(format!("[Catatan sistem: {warning}]"));
                        }
                    }
                    Err(error) => media.notes.push(format!(
                        "[Catatan sistem: berkas '{}' tidak dapat dibaca: {error}]",
                        upload.name
                    )),
                }
            }
        }
    }
    media
}

async fn run_generation(
    state: &Arc<WebState>,
    session_id: usize,
    draft_id: i64,
    text: String,
    uploads: Vec<Upload>,
    events: UnboundedSender<Event>,
) {
    let started = Instant::now();
    let ai = Arc::clone(&state.ai);
    let owner = owner_id();
    let thread = cli_session_thread_id(session_id);
    let _ = events.send(sse(
        "status",
        &json!({"label": "Thinking", "activity": "thinking"}),
    ));

    let generation_lock = ai.generation_lock(owner, thread).await;
    let _generation_guard = generation_lock.lock().await;
    let (mut cancel_rx, _registration) = ai.begin_generation(owner, draft_id).await;

    let media = prepare_media(uploads).await;
    let mut prompt = text.trim().to_string();
    for note in &media.notes {
        if !prompt.is_empty() {
            prompt.push_str("\n\n");
        }
        prompt.push_str(note);
    }
    // The user's words come first in the history; the documents follow
    // under a marker the WebUI shows as file chips.
    let canonical = (!media.doc_text.is_empty()).then(|| {
        let head = if prompt.is_empty() {
            String::new()
        } else {
            format!("{prompt}\n\n")
        };
        format!(
            "{head}{DOC_MARKER}{}]\n{}",
            media.doc_names.join(", "),
            media.doc_text
        )
    });
    let doc_name = media.doc_names.join(", ");
    let sink = WebSink {
        events: events.clone(),
        failure: Mutex::new(None),
    };
    let input = GenerationInput {
        prompt: &prompt,
        canonical_prompt: canonical.as_deref(),
        media_to_main: true,
        sink: Some(&sink),
        image_bytes: media.image.as_ref().map(|(bytes, _)| bytes.clone()),
        document_images: media.pages.clone(),
        mime_type: media.image.as_ref().map(|(_, mime)| mime.as_str()),
        doc_text: (!media.doc_text.is_empty()).then_some(media.doc_text.as_str()),
        doc_name: (!doc_name.is_empty()).then_some(doc_name.as_str()),
        audio_bytes: media.audio.as_ref().map(|(bytes, _)| bytes.clone()),
        audio_mime: media.audio.as_ref().map(|(_, mime)| mime.as_str()),
        video_bytes: media.video.as_ref().map(|(bytes, _)| bytes.clone()),
        video_mime: media.video.as_ref().map(|(_, mime)| mime.as_str()),
        video_duration: None,
        bot: None,
        reply_to_message_id: None,
        guest_mode: false,
    };

    let (thinking, answer, staged, cancelled) = ai
        .generate_response(owner, thread, owner, input, &mut cancel_rx)
        .await;
    ai.end_generation(owner, draft_id).await;

    let files: Vec<FileRef> = staged
        .into_iter()
        .map(|document| state.chat.store_file(session_id, document))
        .collect();
    let failure = sink.failure.lock().ok().and_then(|failure| failure.clone());
    let failed = !cancelled && failure.is_some_and(|reason| reason != "Stopped by user");
    let model = ai
        .resolve_model_route(ModelRole::Main)
        .await
        .ok()
        .map(|route| route.model);
    let answer = if answer == "[QUIZ_SENT]" {
        String::new()
    } else {
        render_media_markup_for_web(&answer)
    };
    if events
        .send(sse(
            "done",
            &json!({
                "answer": answer,
                "thinking": thinking.filter(|thinking| !thinking.trim().is_empty()),
                "files": files,
                "model": model,
                "secs": (started.elapsed().as_secs_f64() * 10.0).round() / 10.0,
                "stopped": cancelled,
                "failed": failed,
            }),
        ))
        .is_err()
    {
        warn!("Web chat answer finished after the browser left; it is kept in the history");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uploads_are_routed_by_type() {
        assert_eq!(route_for("image/png"), UploadRoute::Vision);
        assert_eq!(route_for("audio/ogg"), UploadRoute::Stt);
        assert_eq!(route_for("video/mp4"), UploadRoute::Video);
        assert_eq!(route_for("application/pdf"), UploadRoute::Doc);
        assert_eq!(guess_mime("Foto.JPG"), "image/jpeg");
        assert_eq!(guess_mime("noext"), "application/octet-stream");
    }

    #[test]
    fn stores_expire_and_stay_within_budget() {
        let mut map: HashMap<String, (Instant, usize)> = HashMap::new();
        let now = Instant::now();
        map.insert("a".into(), (now, 60));
        map.insert("b".into(), (now + Duration::from_millis(5), 60));
        prune(
            &mut map,
            Duration::from_secs(60),
            100,
            |entry| entry.0,
            |entry| entry.1,
        );
        assert_eq!(map.len(), 1);
        assert!(map.contains_key("b"), "the oldest entry goes first");
    }

    #[test]
    fn attached_documents_are_found() {
        let docs = attached_documents("Siap.\n[document: laporan.pdf](attach://doc_0) dan [dokumen: data.csv](attach://doc_1)");
        assert_eq!(
            docs,
            vec![
                ("laporan.pdf".to_string(), "doc_0".to_string()),
                ("data.csv".to_string(), "doc_1".to_string())
            ]
        );
    }

    #[test]
    fn uploads_are_single_use() {
        let runtime = ChatRuntime::default();
        let id = runtime
            .add_upload("a.txt".into(), "text/plain".into(), b"hi".to_vec())
            .expect("stored");
        assert!(runtime.take_uploads(&["missing".to_string()]).is_err());
        assert_eq!(
            runtime
                .take_uploads(std::slice::from_ref(&id))
                .expect("taken")
                .len(),
            1
        );
        assert!(
            runtime.take_uploads(&[id]).is_err(),
            "an upload is used once"
        );
    }
}
