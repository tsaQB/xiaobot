use futures_util::StreamExt;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::time::Duration;
use tokio::sync::watch;
use tracing::{error, warn};

use crate::attachments::{encode_user_content, persist_attachment, AttachmentRef};
use crate::timeline::{GenerationProgressSink, ProgressActivity};
use crate::util::truncate_chars;

use super::context::{estimate_stored_content_tokens, estimate_text_tokens};
use super::multimodal::{
    media_data_url, native_audio_input_part, resolved_audio_persistence_mime,
    resolved_runtime_media_mime, select_audio_execution_mode, AudioExecutionMode,
    SpecialistObservationInput,
};
use super::tool_round::{
    follow_up_prompt, has_tool_call_markup, is_quiz_done_reply, live_photo_key, quiz_question_key,
    run_research_calls, unexecuted_tools_notice, RoundState, ToolResultBudget, MAX_TOOL_ROUNDS,
};
use super::{provider_url, AIChatService, ActiveGenerations};
use crate::ai::capability::model_metadata_key;
use crate::ai::http::{is_retryable_status, retry_delay, MAX_PROVIDER_ATTEMPTS};
use crate::ai::routing::{GenerationModelSnapshot, ModelRole, ResolvedModelRoute, RouteOrigin};
use crate::ai::storage::{
    get_scoped_summary_async, get_user_memories_async, load_scoped_messages_async,
    save_scoped_turn_async, CapabilityKind, CapabilityState,
};
use crate::ai::stream::{SseDecoder, StreamEvent};

pub(crate) const MAX_STREAM_VISIBLE_BYTES: usize = 8 * 1024 * 1024;
pub(crate) const MAX_STREAM_REASONING_BYTES: usize = 8 * 1024 * 1024;
pub(crate) const MAX_STREAM_WIRE_BYTES: usize = 32 * 1024 * 1024;
/// Answer text of a generation stopped before it produced any content.
pub(crate) const GENERATION_STOPPED_NOTICE: &str = "⏹️ Generasi dihentikan oleh pengguna.";
/// Upper bound on parallel tool calls accepted from one streamed turn.
pub(crate) const MAX_TOOL_CALLS_PER_TURN: usize = 32;
/// Upper bound on a single tool result injected back into the conversation.
pub(crate) const MAX_TOOL_RESULT_CHARS: usize = 12_000;

pub(crate) use crate::bot::models::StagedDocument;

/// (thinking_text, answer_text, staged_documents, cancelled)
pub(crate) type ChatGenerationResult = (Option<String>, String, Vec<StagedDocument>, bool);

/// Draft ids are seeded from the wall clock so they do not restart at the same
/// value after every process restart. A fixed seed let a replayed "stop"
/// update from before a restart cancel an unrelated new generation that
/// happened to receive the same id.
static NEXT_DRAFT_ID: std::sync::LazyLock<std::sync::atomic::AtomicI64> =
    std::sync::LazyLock::new(|| {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis())
            .ok()
            .and_then(|millis| i64::try_from(millis).ok())
            .unwrap_or(100_000)
            .max(100_000);
        std::sync::atomic::AtomicI64::new(seed)
    });

pub fn next_draft_id() -> i64 {
    NEXT_DRAFT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// Hasil dari sebuah pekerjaan yang dijalankan bersama pemantau pembatalan.
pub(crate) enum Raced<T> {
    Completed(T),
    Cancelled,
}

/// Menjalankan `task` sambil memantau sinyal pembatalan.
///
/// Menggantikan pola `tokio::select!` yang sebelumnya diulang di
/// banyak titik sepanjang alur generasi.
pub(crate) async fn race_with_cancel<T, F>(
    cancel_rx: &mut watch::Receiver<bool>,
    task: F,
) -> Raced<T>
where
    F: std::future::Future<Output = T>,
{
    if *cancel_rx.borrow() {
        return Raced::Cancelled;
    }
    let mut task = std::pin::pin!(task);
    loop {
        tokio::select! {
            result = &mut task => return Raced::Completed(result),
            changed = cancel_rx.changed() => {
                if changed.is_err() {
                    return Raced::Completed(task.await);
                }
                if *cancel_rx.borrow() {
                    return Raced::Cancelled;
                }
            }
        }
    }
}

/// Menunggu `delay` sambil memantau sinyal pembatalan.
pub(crate) async fn sleep_or_cancel(
    cancel_rx: &mut watch::Receiver<bool>,
    delay: Duration,
) -> Raced<()> {
    race_with_cancel(cancel_rx, tokio::time::sleep(delay)).await
}

/// Serializes the persisted user turn. Serializing a `serde_json::Value`
/// cannot realistically fail, but if it ever did the plain prompt is stored
/// instead of an empty string that would silently erase the user's message.
fn serialize_user_content(content: &Value, fallback_text: &str) -> String {
    serde_json::to_string(content).unwrap_or_else(|error| {
        warn!("Failed to serialize user turn for history: {error}");
        Value::String(fallback_text.to_string()).to_string()
    })
}

/// Minimum spacing between partial-answer refreshes pushed to a progress sink.
const PARTIAL_REFRESH_INTERVAL: Duration = Duration::from_millis(250);

/// Allowance for per-message role/format overhead added by chat templates.
const PROMPT_FRAMING_TOKENS: usize = 512;
/// Rough token cost of one inline image or rendered document page.
const INLINE_IMAGE_TOKENS: usize = 1_500;

/// Estimates the context cost of media sent inline with the current prompt.
pub(crate) fn inline_media_token_estimate(
    media_to_main: bool,
    document_images: Option<&[Vec<u8>]>,
    has_image: bool,
    has_audio: bool,
    has_video: bool,
) -> usize {
    if !media_to_main {
        return 0;
    }
    let pages = document_images.map_or(0, <[Vec<u8>]>::len);
    let mut tokens = pages.saturating_mul(INLINE_IMAGE_TOKENS);
    if has_image {
        tokens = tokens.saturating_add(INLINE_IMAGE_TOKENS);
    }
    if has_audio {
        tokens = tokens.saturating_add(INLINE_IMAGE_TOKENS);
    }
    if has_video {
        tokens = tokens.saturating_add(INLINE_IMAGE_TOKENS.saturating_mul(4));
    }
    tokens
}

/// Tool output re-enters the prompt on the next turn. Unbounded results (web
/// pages, search dumps) could push the request past the model context even
/// though the history had been trimmed to fit.
pub(crate) fn bound_tool_result(result: &str) -> String {
    if result.chars().nth(MAX_TOOL_RESULT_CHARS).is_none() {
        return result.to_string();
    }
    let mut bounded = truncate_chars(result, MAX_TOOL_RESULT_CHARS);
    bounded.push_str("\n\n[Hasil tool dipotong Xiao agar muat di context window model.]");
    bounded
}

/// Stages a media tag once. Tools stay available after media is staged, so a
/// repeated call must not put the same photo or file in the answer twice.
fn stage_media_tag(staged: &mut Vec<String>, tag: String, ready_message: String) -> String {
    if staged.contains(&tag) {
        return format!(
            "Media ini sudah disiapkan sebelumnya ({tag}); jangan panggil ulang tool yang sama."
        );
    }
    staged.push(tag);
    ready_message
}

/// Stages a generated file once; an identical repeat is reported instead.
fn stage_document(staged: &mut Vec<StagedDocument>, document: StagedDocument) -> Option<String> {
    if let Some(existing) = staged
        .iter()
        .find(|doc| doc.filename == document.filename && doc.bytes == document.bytes)
    {
        return Some(format!(
            "Berkas '{}' dengan isi yang sama sudah disiapkan ({}); jangan buat ulang.",
            existing.filename,
            existing.markdown_tag()
        ));
    }
    staged.push(document);
    None
}

pub(crate) fn push_bounded(target: &mut String, chunk: &str, max_bytes: usize) -> bool {
    if target.len().saturating_add(chunk.len()) > max_bytes {
        return false;
    }
    target.push_str(chunk);
    true
}

pub(crate) fn canonical_persisted_prompt<'a>(
    canonical: Option<&'a str>,
    runtime_prompt: &'a str,
) -> &'a str {
    canonical.unwrap_or(runtime_prompt)
}

/// Fallback output-token ceiling used only when the provider did not report
/// `max_completion_tokens` (that metadata always wins, see the caller).
///
/// Matching works on whole name segments (`anthropic/claude-sonnet-5` ->
/// `anthropic`, `claude`, `sonnet`, `5`) instead of raw substrings, so an
/// unrelated model such as `console-7b` or `solar-pro` no longer inherits a
/// 65k budget that the provider would reject.
pub(crate) fn max_output_tokens_for_model(model: &str) -> usize {
    let lower = model.to_ascii_lowercase();
    let segments: Vec<&str> = lower
        .split(['/', '-', ':', '_', '.', '@', ' '])
        .filter(|segment| !segment.is_empty())
        .collect();
    let has = |name: &str| segments.contains(&name);
    let family_prefix = |prefix: &str| {
        segments.first().is_some_and(|first| *first == prefix)
            || lower
                .split('/')
                .next_back()
                .is_some_and(|base| base == prefix || base.starts_with(&format!("{prefix}-")))
    };

    if has("claude") {
        64_000
    } else if has("gemini")
        || has("o1")
        || has("o3")
        || has("o4")
        || has("gpt")
            && segments
                .iter()
                .any(|segment| matches!(*segment, "4o" | "5"))
        || family_prefix("sol")
        || family_prefix("terra")
        || family_prefix("luna")
    {
        65_536
    } else {
        16_384
    }
}

pub(crate) fn cancelled_chat_result(
    sink: Option<&dyn GenerationProgressSink>,
) -> ChatGenerationResult {
    if let Some(sink) = sink {
        sink.on_failure("Stopped by user", false);
    }
    (
        None,
        GENERATION_STOPPED_NOTICE.to_string(),
        Vec::new(),
        true,
    )
}

#[derive(Default, Clone)]
pub struct PendingToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

fn extract_leaked_tool_calls(raw: &str) -> Vec<PendingToolCall> {
    static RE_TOOL_CALL: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(
            r#"(?is)<(?:tool_call|function_call)\b[^>]*>(.*?)</(?:tool_call|function_call)>"#,
        )
        .expect("valid static regex")
    });

    let mut result = Vec::new();
    for cap in RE_TOOL_CALL.captures_iter(raw) {
        if let Some(inner) = cap.get(1) {
            let s = inner.as_str().trim();
            if let Ok(v) = serde_json::from_str::<Value>(s) {
                if let Some(name) = v.get("name").and_then(Value::as_str) {
                    let args = if let Some(args_str) = v.get("arguments").and_then(Value::as_str) {
                        args_str.to_string()
                    } else if let Some(args_val) = v.get("arguments") {
                        args_val.to_string()
                    } else {
                        String::new()
                    };
                    let id = v
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or("call_leaked")
                        .to_string();
                    result.push(PendingToolCall {
                        id,
                        name: name.to_string(),
                        arguments: args,
                    });
                }
            } else if let Ok(arr) = serde_json::from_str::<Vec<Value>>(s) {
                for v in arr {
                    if let Some(name) = v.get("name").and_then(Value::as_str) {
                        let args =
                            if let Some(args_str) = v.get("arguments").and_then(Value::as_str) {
                                args_str.to_string()
                            } else if let Some(args_val) = v.get("arguments") {
                                args_val.to_string()
                            } else {
                                String::new()
                            };
                        let id = v
                            .get("id")
                            .and_then(Value::as_str)
                            .unwrap_or("call_leaked")
                            .to_string();
                        result.push(PendingToolCall {
                            id,
                            name: name.to_string(),
                            arguments: args,
                        });
                    }
                }
            }
        }
    }
    result
}

pub struct GenerationGuard {
    active_generations: ActiveGenerations,
    chat_id: i64,
    draft_id: i64,
}

impl GenerationGuard {
    pub fn new(active_generations: ActiveGenerations, chat_id: i64, draft_id: i64) -> Self {
        Self {
            active_generations,
            chat_id,
            draft_id,
        }
    }
}

impl Drop for GenerationGuard {
    fn drop(&mut self) {
        let key = (self.chat_id, self.draft_id);
        if let Ok(mut map) = self.active_generations.try_write() {
            map.remove(&key);
        } else if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let active = self.active_generations.clone();
            handle.spawn(async move {
                active.write().await.remove(&key);
            });
        }
    }
}

pub struct GenerationInput<'a> {
    pub prompt: &'a str,
    pub canonical_prompt: Option<&'a str>,
    pub media_to_main: bool,
    pub sink: Option<&'a (dyn GenerationProgressSink + 'a)>,
    pub image_bytes: Option<Vec<u8>>,
    pub document_images: Option<Vec<Vec<u8>>>,
    pub mime_type: Option<&'a str>,
    pub doc_text: Option<&'a str>,
    pub doc_name: Option<&'a str>,
    pub audio_bytes: Option<Vec<u8>>,
    pub audio_mime: Option<&'a str>,
    pub video_bytes: Option<Vec<u8>>,
    pub video_mime: Option<&'a str>,
    pub video_duration: Option<i32>,
    pub bot: Option<crate::bot::client::TelegramBotClient>,
    pub reply_to_message_id: Option<i64>,
    /// Guest mode (Bot API 10.0): answering in someone else's chat. No
    /// memory, summary or history is used or stored, and only read-only tools
    /// are offered, because the reply is visible to everyone in that chat.
    pub guest_mode: bool,
}

impl AIChatService {
    pub async fn generate_response(
        &self,
        chat_id: i64,
        thread_id: i64,
        user_id: i64,
        input: GenerationInput<'_>,
        cancel_rx: &mut watch::Receiver<bool>,
    ) -> ChatGenerationResult {
        let snapshot = self.generation_model_snapshot().await;
        self.generate_response_with_snapshot(
            chat_id, thread_id, user_id, input, &snapshot, cancel_rx,
        )
        .await
    }

    pub(crate) async fn generate_response_with_snapshot(
        &self,
        chat_id: i64,
        thread_id: i64,
        user_id: i64,
        input: GenerationInput<'_>,
        snapshot: &GenerationModelSnapshot,
        cancel_rx: &mut watch::Receiver<bool>,
    ) -> ChatGenerationResult {
        if thread_id > 0
            && crate::bot::client::TelegramBotClient::current_delivery_context()
                .message_thread_id
                .is_none()
        {
            let mut ctx = crate::bot::client::TelegramBotClient::current_delivery_context();
            ctx.message_thread_id = Some(thread_id);
            crate::bot::client::TelegramBotClient::with_delivery_context(
                ctx,
                self.generate_response_with_snapshot_inner(
                    chat_id, thread_id, user_id, input, snapshot, cancel_rx,
                ),
            )
            .await
        } else {
            self.generate_response_with_snapshot_inner(
                chat_id, thread_id, user_id, input, snapshot, cancel_rx,
            )
            .await
        }
    }

    async fn generate_response_with_snapshot_inner(
        &self,
        chat_id: i64,
        thread_id: i64,
        user_id: i64,
        input: GenerationInput<'_>,
        snapshot: &GenerationModelSnapshot,
        cancel_rx: &mut watch::Receiver<bool>,
    ) -> ChatGenerationResult {
        let GenerationInput {
            prompt,
            canonical_prompt,
            media_to_main: _,
            sink,
            image_bytes,
            document_images,
            mime_type,
            doc_text,
            doc_name,
            audio_bytes,
            audio_mime,
            video_bytes,
            video_mime,
            video_duration,
            bot,
            reply_to_message_id,
            guest_mode,
        } = input;

        if *cancel_rx.borrow() {
            return cancelled_chat_result(sink);
        }

        let main = match Self::resolve_model_route_from_snapshot(snapshot, ModelRole::Main) {
            Ok(route) => route,
            Err(error) => {
                return (
                    None,
                    format!("Main Model is unavailable: {error}"),
                    Vec::new(),
                    false,
                )
            }
        };

        let has_vision = image_bytes.is_some()
            || document_images
                .as_ref()
                .is_some_and(|pages| !pages.is_empty());
        let role = if has_vision {
            Some(ModelRole::Vision)
        } else if video_bytes.is_some() {
            Some(ModelRole::Video)
        } else if audio_bytes.is_some() {
            Some(ModelRole::AudioStt)
        } else {
            None
        };

        let Some(role) = role else {
            return self
                .generate_response_on_main(
                    chat_id,
                    thread_id,
                    user_id,
                    &main,
                    snapshot,
                    GenerationInput {
                        prompt,
                        canonical_prompt,
                        media_to_main: true,
                        sink,
                        image_bytes,
                        document_images,
                        mime_type,
                        doc_text,
                        doc_name,
                        audio_bytes,
                        audio_mime,
                        video_bytes,
                        video_mime,
                        video_duration,
                        bot,
                        reply_to_message_id,
                        guest_mode,
                    },
                    cancel_rx,
                )
                .await;
        };

        let specialist = match Self::resolve_model_route_from_snapshot(snapshot, role) {
            Ok(route) => route,
            Err(error) => {
                return (
                    None,
                    format!("{} unavailable: {error}", role.display_name()),
                    Vec::new(),
                    false,
                )
            }
        };

        let same_as_main =
            specialist.provider.id == main.provider.id && specialist.model == main.model;

        if role == ModelRole::AudioStt {
            let inherited_main = same_as_main && specialist.route_origin == RouteOrigin::MainModel;
            let audio_mode = match select_audio_execution_mode(
                &specialist.capability,
                inherited_main,
                audio_mime,
                doc_name,
            ) {
                Ok(mode) => mode,
                Err(error) => return (None, error, Vec::new(), false),
            };

            if audio_mode == AudioExecutionMode::Native {
                return self
                    .generate_response_on_main(
                        chat_id,
                        thread_id,
                        user_id,
                        &main,
                        snapshot,
                        GenerationInput {
                            prompt,
                            canonical_prompt,
                            media_to_main: true,
                            sink,
                            image_bytes,
                            document_images,
                            mime_type,
                            doc_text,
                            doc_name,
                            audio_bytes,
                            audio_mime,
                            video_bytes,
                            video_mime,
                            video_duration,
                            bot,
                            reply_to_message_id,
                            guest_mode,
                        },
                        cancel_rx,
                    )
                    .await;
            }

            let Some(bytes) = audio_bytes.clone() else {
                return (
                    None,
                    "Audio input is missing.".to_string(),
                    Vec::new(),
                    false,
                );
            };
            let transcript_result = match race_with_cancel(
                cancel_rx,
                self.transcribe_audio_resolved(
                    &specialist,
                    bytes,
                    doc_name.unwrap_or("audio"),
                    audio_mime,
                ),
            )
            .await
            {
                Raced::Completed(result) => result,
                Raced::Cancelled => return cancelled_chat_result(sink),
            };
            let transcript = match transcript_result {
                Ok(transcript) => transcript,
                Err(error) => return (None, error, Vec::new(), false),
            };
            let synthesis_prompt = if prompt.trim().is_empty() {
                format!("Transcript from Audio STT specialist:\n\n{transcript}\n\nRespond to the user based on this transcript.")
            } else {
                format!(
                    "User request:\n{prompt}\n\nTranscript from Audio STT specialist:\n{transcript}\n\nAnswer the user request using the transcript as an execution artifact."
                )
            };
            return self
                .generate_response_on_main(
                    chat_id,
                    thread_id,
                    user_id,
                    &main,
                    snapshot,
                    GenerationInput {
                        prompt: &synthesis_prompt,
                        canonical_prompt: Some(canonical_prompt.unwrap_or(prompt)),
                        media_to_main: false,
                        sink,
                        image_bytes,
                        document_images,
                        mime_type,
                        doc_text,
                        doc_name,
                        audio_bytes,
                        audio_mime,
                        video_bytes,
                        video_mime,
                        video_duration,
                        bot,
                        reply_to_message_id,
                        guest_mode,
                    },
                    cancel_rx,
                )
                .await;
        }

        if same_as_main && specialist.route_origin == RouteOrigin::MainModel {
            return self
                .generate_response_on_main(
                    chat_id,
                    thread_id,
                    user_id,
                    &main,
                    snapshot,
                    GenerationInput {
                        prompt,
                        canonical_prompt,
                        media_to_main: true,
                        sink,
                        image_bytes,
                        document_images,
                        mime_type,
                        doc_text,
                        doc_name,
                        audio_bytes,
                        audio_mime,
                        video_bytes,
                        video_mime,
                        video_duration,
                        bot,
                        reply_to_message_id,
                        guest_mode,
                    },
                    cancel_rx,
                )
                .await;
        }

        let observation_result = match race_with_cancel(
            cancel_rx,
            self.run_specialist_observation(
                &specialist,
                SpecialistObservationInput {
                    prompt,
                    image_bytes: image_bytes.as_deref(),
                    document_images: document_images.as_deref(),
                    mime_type,
                    video_bytes: video_bytes.as_deref(),
                    video_mime,
                },
            ),
        )
        .await
        {
            Raced::Completed(result) => result,
            Raced::Cancelled => return cancelled_chat_result(sink),
        };
        let observation = match observation_result {
            Ok(observation) => observation,
            Err(error) => return (None, error, Vec::new(), false),
        };
        let synthesis_prompt = format!(
            "User request:\n{}\n\nBounded {} observation from {} / {}:\n{}\n\nUse the observation as an execution artifact. Do not claim access to media beyond it.",
            if prompt.trim().is_empty() { "Analyze the supplied media." } else { prompt },
            role.display_name(),
            specialist.provider.name,
            specialist.model,
            observation
        );
        self.generate_response_on_main(
            chat_id,
            thread_id,
            user_id,
            &main,
            snapshot,
            GenerationInput {
                prompt: &synthesis_prompt,
                canonical_prompt: Some(canonical_prompt.unwrap_or(prompt)),
                media_to_main: false,
                sink,
                image_bytes,
                document_images,
                mime_type,
                doc_text,
                doc_name,
                audio_bytes,
                audio_mime,
                video_bytes,
                video_mime,
                video_duration,
                bot,
                reply_to_message_id,
                guest_mode,
            },
            cancel_rx,
        )
        .await
    }
}

// Multimodal attachments are stored outside SQLite and referenced from
// the user message. If the append fails, only newly created references
// are cleaned up; pre-existing media stays intact.
#[allow(clippy::too_many_arguments)]
async fn persist_runtime_attachments(
    chat_id: i64,
    thread_id: i64,
    document_images: Option<&[Vec<u8>]>,
    image_bytes: Option<&[u8]>,
    mime_type: Option<&str>,
    audio_bytes: Option<&[u8]>,
    audio_mime: Option<&str>,
    doc_name: Option<&str>,
    video_bytes: Option<&[u8]>,
    video_mime: Option<&str>,
) -> Vec<AttachmentRef> {
    let mut attachment_refs = Vec::new();
    if let Some(pages) = document_images {
        for (index, page) in pages.iter().enumerate() {
            let page_name = format!(
                "{} page {}",
                doc_name.unwrap_or("PDF scan"),
                index.saturating_add(1)
            );
            match persist_attachment(
                chat_id,
                thread_id,
                "document_page",
                "image/png",
                Some(&page_name),
                page,
            )
            .await
            {
                Ok(reference) => attachment_refs.push(reference),
                Err(err) => warn!("Failed to persist rendered PDF page: {err}"),
            }
        }
    } else if let Some(bytes) = image_bytes {
        match resolved_runtime_media_mime(mime_type, "image/", "persisted image") {
            Ok(resolved_mime) => {
                match persist_attachment(chat_id, thread_id, "image", &resolved_mime, None, bytes)
                    .await
                {
                    Ok(reference) => attachment_refs.push(reference),
                    Err(err) => warn!("Failed to persist image attachment: {err}"),
                }
            }
            Err(err) => warn!("Refusing to persist image with false media identity: {err}"),
        }
    } else if let Some(bytes) = audio_bytes {
        let resolved_mime = resolved_audio_persistence_mime(audio_mime, doc_name);
        match persist_attachment(chat_id, thread_id, "audio", &resolved_mime, doc_name, bytes).await
        {
            Ok(reference) => attachment_refs.push(reference),
            Err(err) => warn!("Failed to persist audio attachment: {err}"),
        }
    } else if let Some(bytes) = video_bytes {
        match resolved_runtime_media_mime(video_mime, "video/", "persisted video") {
            Ok(resolved_mime) => {
                match persist_attachment(chat_id, thread_id, "video", &resolved_mime, None, bytes)
                    .await
                {
                    Ok(reference) => attachment_refs.push(reference),
                    Err(err) => warn!("Failed to persist video attachment: {err}"),
                }
            }
            Err(err) => warn!("Refusing to persist video with false media identity: {err}"),
        }
    }
    attachment_refs
}

impl AIChatService {
    #[allow(clippy::too_many_arguments)]
    async fn generate_response_on_main(
        &self,
        chat_id: i64,
        thread_id: i64,
        user_id: i64,
        main_route: &ResolvedModelRoute,
        snapshot: &GenerationModelSnapshot,
        input: GenerationInput<'_>,
        cancel_rx: &mut watch::Receiver<bool>,
    ) -> ChatGenerationResult {
        let GenerationInput {
            prompt,
            canonical_prompt,
            media_to_main,
            sink,
            image_bytes,
            document_images,
            mime_type,
            doc_text,
            doc_name,
            audio_bytes,
            audio_mime,
            video_bytes,
            video_mime,
            video_duration,
            bot,
            reply_to_message_id,
            guest_mode,
        } = input;

        let provider = &main_route.provider;
        let mut model = main_route.model.as_str();

        if crate::ai::service::is_dedicated_image_generation_model(model) {
            let fallback_text_model = provider
                .models
                .iter()
                .find(|m| !crate::ai::service::is_dedicated_image_generation_model(m));
            if let Some(fallback) = fallback_text_model {
                warn!(
                    chat_id,
                    configured_model = %model,
                    fallback_model = %fallback,
                    "Main model is a dedicated image generation model; falling back to conversational text model"
                );
                model = fallback.as_str();
            } else {
                return (
                    None,
                    format!(
                        "⚠️ Model `{model}` adalah model khusus pembuatan gambar (Image Generation), bukan model percakapan teks.\n\nSilakan gunakan perintah `/image [deskripsi]` untuk membuat gambar, atau alihkan model utama ke model percakapan teks (seperti `gemini-3.8-flash-high`) melalui menu `/model`."
                    ),
                    Vec::new(),
                    false,
                );
            }
        }

        let mut clean_prompt = prompt.trim().to_string();
        if let Some(doc) = doc_text {
            let d_name = doc_name.unwrap_or("Dokumen");
            let doc_header = format!("[Dokumen Terlampir: {d_name}]\n{}\n\n", doc.trim());
            clean_prompt = if clean_prompt.is_empty() {
                format!("{doc_header}Baca, analisis, dan jelaskan isi dokumen ini.")
            } else {
                format!("{doc_header}{clean_prompt}")
            };
        } else if document_images
            .as_ref()
            .is_some_and(|pages| !pages.is_empty())
            && clean_prompt.is_empty()
        {
            let d_name = doc_name.unwrap_or("PDF scan");
            clean_prompt = format!(
                "Baca dan analisis halaman hasil render dari dokumen '{d_name}'. Lakukan OCR visual pada teks yang terlihat dan jelaskan isi dokumen secara akurat."
            );
        } else if video_bytes.is_some() && clean_prompt.is_empty() {
            let dur_str = video_duration
                .map(|d| format!(" ({d} detik)"))
                .unwrap_or_default();
            clean_prompt = format!("Tonton dan analisis rekaman video ini{dur_str} secara mendalam. Jelaskan isi visual, alur peristiwa, teks di layar, dan suara di dalamnya.");
        } else if image_bytes.is_some() && clean_prompt.is_empty() {
            clean_prompt = "Jelaskan dan analisis gambar ini secara detail.".to_string();
        } else if audio_bytes.is_some() && clean_prompt.is_empty() {
            clean_prompt = "Dengarkan rekaman suara ini dan jawab pertanyaan atau instruksi di dalamnya secara lengkap.".to_string();
        }
        let canonical_history_prompt = canonical_prompt.map(str::to_string);

        let resolved_capability = self
            .resolved_model_capability(&provider.endpoint, model)
            .await;
        let metadata_max_completion_tokens = self
            .model_metadata
            .read()
            .await
            .get(&model_metadata_key(&provider.endpoint, model))
            .and_then(|metadata| metadata.max_completion_tokens);
        let mut max_output_tokens = max_output_tokens_for_model(model)
            .min(resolved_capability.context_limit.saturating_div(2).max(1));
        if let Some(limit) = metadata_max_completion_tokens.filter(|limit| *limit > 0) {
            max_output_tokens = max_output_tokens.min(limit);
        }
        // Everything that is sent besides history counts against the context
        // window: the system prompt (instructions + memory + summary), the
        // tool schemas, inline media, and a small allowance for message
        // framing. Previously only the output reserve and user prompt were
        // counted, so long memories or summaries could still overflow the
        // window after history had been trimmed.
        // Guest mode answers inside someone else's chat: the owner's private
        // memory and topic summaries must never leak into that reply.
        let system_text = if guest_mode {
            super::prompt::build_guest_system_prompt()
        } else {
            let user_memories = get_user_memories_async(user_id).await;
            let scoped_summary = get_scoped_summary_async(chat_id, thread_id).await;
            super::prompt::build_system_prompt(&user_memories, scoped_summary.as_deref())
        };

        let cap_record = self.capability_record(&provider.endpoint, model).await;
        let supports_tools = cap_record
            .as_ref()
            .map(|r| r.effective_state_for(CapabilityKind::Tools) != CapabilityState::Unsupported)
            .unwrap_or(true);
        let tools_tokens = if supports_tools {
            estimate_text_tokens(&crate::ai::tools::tools_definition_for(guest_mode).to_string())
        } else {
            0
        };
        let media_tokens = inline_media_token_estimate(
            media_to_main,
            document_images.as_deref(),
            image_bytes.is_some(),
            audio_bytes.is_some(),
            video_bytes.is_some(),
        );
        let fixed_overhead = estimate_text_tokens(&system_text)
            .saturating_add(tools_tokens)
            .saturating_add(media_tokens)
            .saturating_add(PROMPT_FRAMING_TOKENS);

        let max_prompt_tokens = resolved_capability
            .context_limit
            .saturating_sub(max_output_tokens)
            .saturating_sub(fixed_overhead)
            .max(1);
        let prompt_tokens = estimate_text_tokens(&clean_prompt);
        if prompt_tokens > max_prompt_tokens {
            // Scale by the observed chars-per-token ratio so dense scripts
            // (CJK, Arabic) are cut to the budget too, not just ASCII.
            let prompt_chars = clean_prompt.chars().count();
            let max_chars = prompt_chars.saturating_mul(max_prompt_tokens) / prompt_tokens.max(1);
            clean_prompt = truncate_chars(&clean_prompt, max_chars);
            clean_prompt.push_str("\n\n[Input dipotong Xiao agar muat di context window model.]");
        }
        let enhanced_prompt = clean_prompt.clone();

        let reserved_tokens = max_output_tokens
            .saturating_add(estimate_text_tokens(&enhanced_prompt))
            .saturating_add(fixed_overhead);
        let history_budget = resolved_capability
            .context_limit
            .saturating_sub(reserved_tokens);

        // Guest replies are stateless: no stored history is read.
        let scoped_messages = if guest_mode {
            Vec::new()
        } else {
            load_scoped_messages_async(chat_id, thread_id, 20).await
        };
        let mut selected_history = Vec::new();
        let mut used_history_tokens = 0usize;
        for message in scoped_messages.iter().rev() {
            let estimated = estimate_stored_content_tokens(&message.content).saturating_add(8);
            if !selected_history.is_empty()
                && used_history_tokens.saturating_add(estimated) > history_budget
            {
                break;
            }
            if estimated > history_budget && selected_history.is_empty() {
                continue;
            }
            used_history_tokens = used_history_tokens.saturating_add(estimated);
            selected_history.push(message.clone());
        }
        selected_history.reverse();

        let mut history = Vec::with_capacity(selected_history.len());
        for message in &selected_history {
            let content = if message.role == "user" {
                self.rehydrate_history_content(chat_id, thread_id, &message.content, snapshot)
                    .await
            } else {
                message.content.clone()
            };
            history.push(json!({ "role": message.role, "content": content }));
        }

        let mut messages = vec![json!({
            "role": "system",
            "content": system_text
        })];
        messages.extend(history);

        if media_to_main {
            if let Some(pages) = document_images.as_ref().filter(|pages| !pages.is_empty()) {
                use base64::Engine;
                let mut content = vec![json!({ "type": "text", "text": enhanced_prompt })];
                for page in pages {
                    let encoded = base64::engine::general_purpose::STANDARD.encode(page);
                    content.push(json!({
                        "type": "image_url",
                        "image_url": {
                            "url": format!("data:image/png;base64,{encoded}"),
                            "detail": "high"
                        }
                    }));
                }
                messages.push(json!({ "role": "user", "content": content }));
            } else if let Some(v_bytes) = video_bytes.as_ref() {
                let data_url = match media_data_url(v_bytes, video_mime, "video/", "video") {
                    Ok(data_url) => data_url,
                    Err(error) => return (None, error, Vec::new(), false),
                };
                messages.push(json!({
                    "role": "user",
                    "content": [
                        { "type": "text", "text": enhanced_prompt },
                        { "type": "image_url", "image_url": { "url": data_url } }
                    ]
                }));
            } else if let Some(i_bytes) = image_bytes.as_ref() {
                let data_url = match media_data_url(i_bytes, mime_type, "image/", "image") {
                    Ok(data_url) => data_url,
                    Err(error) => return (None, error, Vec::new(), false),
                };
                messages.push(json!({
                    "role": "user",
                    "content": [
                        { "type": "text", "text": enhanced_prompt },
                        { "type": "image_url", "image_url": { "url": data_url, "detail": "auto" } }
                    ]
                }));
            } else if let Some(a_bytes) = audio_bytes.as_ref() {
                let audio_part = match native_audio_input_part(a_bytes, audio_mime, doc_name) {
                    Ok(part) => part,
                    Err(error) => {
                        return (
                            None,
                            format!(
                                "Native audio payload was blocked because its format cannot be represented safely: {error}."
                            ),
                        Vec::new(),
                        false,
                        )
                    }
                };
                messages.push(json!({
                    "role": "user",
                    "content": [
                        { "type": "text", "text": enhanced_prompt },
                        audio_part
                    ]
                }));
            } else {
                messages.push(json!({
                    "role": "user",
                    "content": enhanced_prompt
                }));
            }
        } else {
            messages.push(json!({
                "role": "user",
                "content": enhanced_prompt
            }));
        }

        let url = provider_url(&provider.endpoint, "chat/completions");
        let mut payload = json!({
            "model": model,
            "messages": messages,
            "stream": true,
        });
        if metadata_max_completion_tokens.is_some() {
            payload["max_completion_tokens"] = json!(max_output_tokens);
        } else {
            payload["max_tokens"] = json!(max_output_tokens);
        }

        let use_auth = !provider.api_key.is_empty()
            && !["none", "-", "no"]
                .iter()
                .any(|k| provider.api_key.eq_ignore_ascii_case(k));

        let mut accumulated_raw = String::new();
        let mut accumulated_reasoning = String::new();
        let mut cancelled = false;
        let mut stream_bounded = false;
        let mut stream_interrupted = false;
        let mut has_started_answer = false;
        let mut staged_media_tags: Vec<String> = Vec::new();
        let mut staged_documents: Vec<StagedDocument> = Vec::new();
        let mut media_staged = false;
        let mut quizzes_sent = 0usize;
        let mut quiz_history_summary: Option<String> = None;
        let mut sent_quiz_questions: HashSet<String> = HashSet::new();
        let mut sent_live_photos: HashSet<String> = HashSet::new();
        let mut research_calls = 0usize;
        // Tool calls the model made that were never run; the user is told.
        let mut unexecuted_tools: Vec<String> = Vec::new();
        let mut provider_failed_late = false;
        let mut tool_budget = ToolResultBudget::new(
            resolved_capability
                .context_limit
                .saturating_sub(reserved_tokens)
                .saturating_sub(used_history_tokens),
        );

        // Rounds 0..MAX_TOOL_ROUNDS may call tools; the last request has none,
        // so the model always gets a turn to write the answer.
        for turn in 0..=MAX_TOOL_ROUNDS {
            accumulated_raw.clear();
            accumulated_reasoning.clear();
            let mut accumulated_tool_calls: Vec<PendingToolCall> = Vec::new();
            let mut streamed_wire_bytes = 0usize;
            let mut last_partial_at: Option<std::time::Instant> = None;
            let mut stream_done = false;
            has_started_answer = false;

            let can_run_tools = turn < MAX_TOOL_ROUNDS;
            if supports_tools && can_run_tools {
                payload["tools"] = crate::ai::tools::tools_definition_for(guest_mode);
            } else if let Some(obj) = payload.as_object_mut() {
                obj.remove("tools");
            }

            let mut response = None;
            let mut terminal_transport_failure = false;
            for attempt in 0..MAX_PROVIDER_ATTEMPTS {
                let mut req = self
                    .client
                    .post(&url)
                    .header("Content-Type", "application/json")
                    .json(&payload)
                    .timeout(Duration::from_secs(180));
                if use_auth {
                    req = req.header("Authorization", format!("Bearer {}", provider.api_key));
                }

                let send_result = match race_with_cancel(cancel_rx, req.send()).await {
                    Raced::Completed(result) => result,
                    Raced::Cancelled => return cancelled_chat_result(sink),
                };

                match send_result {
                    Ok(resp)
                        if is_retryable_status(resp.status())
                            && attempt + 1 < MAX_PROVIDER_ATTEMPTS =>
                    {
                        let status = resp.status();
                        let delay = retry_delay(resp.headers(), attempt);
                        drop(resp);
                        warn!(
                            "Transient provider status {}; retrying attempt {}/{}",
                            status.as_u16(),
                            attempt + 2,
                            MAX_PROVIDER_ATTEMPTS
                        );
                        if matches!(sleep_or_cancel(cancel_rx, delay).await, Raced::Cancelled) {
                            return cancelled_chat_result(sink);
                        }
                    }
                    Ok(resp) if resp.status().as_u16() == 400 && payload.get("tools").is_some() => {
                        drop(resp);
                        warn!(
                            "Provider rejected tools parameter (HTTP 400); retrying without tools"
                        );
                        if let Some(obj) = payload.as_object_mut() {
                            obj.remove("tools");
                        }
                        continue;
                    }
                    Ok(resp) => {
                        response = Some(resp);
                        break;
                    }
                    Err(e) => {
                        let retryable_transport = e.is_timeout() || e.is_connect();
                        if retryable_transport && attempt + 1 < MAX_PROVIDER_ATTEMPTS {
                            let delay = Duration::from_millis(
                                500_u64.saturating_mul(1_u64 << attempt.min(5)),
                            );
                            warn!(
                                "Transient provider transport failure; retrying attempt {}/{}",
                                attempt + 2,
                                MAX_PROVIDER_ATTEMPTS
                            );
                            if matches!(sleep_or_cancel(cancel_rx, delay).await, Raced::Cancelled) {
                                return cancelled_chat_result(sink);
                            }
                        } else {
                            error!(
                                "Error sending AI completion request: {}",
                                if e.is_timeout() {
                                    "timeout"
                                } else {
                                    "transport failure"
                                }
                            );
                            terminal_transport_failure = true;
                            break;
                        }
                    }
                }
            }

            // Quizzes, media or files from earlier rounds are already done;
            // a later provider failure must not throw them away.
            let has_earlier_output =
                quizzes_sent > 0 || media_staged || !staged_documents.is_empty();
            if has_earlier_output
                && response
                    .as_ref()
                    .is_none_or(|resp| !resp.status().is_success())
            {
                warn!("Provider failed after earlier tool output; delivering what is ready");
                accumulated_raw.clear();
                provider_failed_late = true;
                break;
            }

            let Some(resp) = response else {
                if let Some(s) = sink {
                    s.on_failure("Provider connection failed", true);
                }
                return (
                    None,
                    if terminal_transport_failure {
                        "⚠️ Terjadi kendala saat memproses jawaban AI.".to_string()
                    } else {
                        "⚠️ Provider tidak merespons setelah beberapa percobaan.".to_string()
                    },
                    Vec::new(),
                    false,
                );
            };

            if !resp.status().is_success() {
                let status_code = resp.status().as_u16();
                drop(resp);
                error!("AI endpoint returned status {status_code}");
                if let Some(s) = sink {
                    s.on_failure(&format!("API status {status_code}"), true);
                }
                return (
                    None,
                    format!("⚠️ Gagal menghubungi AI proxy: {status_code}"),
                    Vec::new(),
                    false,
                );
            }

            let stream = resp.bytes_stream();
            tokio::pin!(stream);
            let mut decoder = SseDecoder::default();

            'streaming: while !stream_done {
                if *cancel_rx.borrow() {
                    cancelled = true;
                    break 'streaming;
                }

                let next_item = match race_with_cancel(cancel_rx, stream.next()).await {
                    Raced::Completed(item) => item,
                    Raced::Cancelled => {
                        cancelled = true;
                        None
                    }
                };

                let Some(item) = next_item else {
                    break;
                };
                let bytes = match item {
                    Ok(bytes) => bytes,
                    Err(_) => {
                        stream_interrupted = true;
                        warn!("AI response stream interrupted");
                        break;
                    }
                };
                streamed_wire_bytes = streamed_wire_bytes.saturating_add(bytes.len());
                if streamed_wire_bytes > MAX_STREAM_WIRE_BYTES {
                    stream_bounded = true;
                    stream_interrupted = true;
                    warn!("AI response exceeded XiaoAI's absolute streamed payload limit");
                    break;
                }

                let events = match decoder.push(&bytes) {
                    Ok(events) => events,
                    Err(error) => {
                        stream_interrupted = true;
                        warn!("AI response SSE decode failed: {error}");
                        break;
                    }
                };
                for event in events {
                    match event {
                        StreamEvent::Done => {
                            stream_done = true;
                            break;
                        }
                        StreamEvent::Json(data) => {
                            let Some(delta) = data
                                .get("choices")
                                .and_then(|choices| choices.get(0))
                                .and_then(|choice| choice.get("delta"))
                            else {
                                continue;
                            };

                            if let Some(tool_calls) =
                                delta.get("tool_calls").and_then(Value::as_array)
                            {
                                for tc in tool_calls {
                                    // The index comes straight from the provider. Anything
                                    // outside the small per-turn budget is rejected rather
                                    // than used to size the accumulator, so a hostile or
                                    // buggy `index: 4000000000` cannot exhaust memory.
                                    let Some(index) = tc
                                        .get("index")
                                        .map_or(Some(0), Value::as_u64)
                                        .and_then(|raw| usize::try_from(raw).ok())
                                        .filter(|index| *index < MAX_TOOL_CALLS_PER_TURN)
                                    else {
                                        warn!("Provider sent an out-of-range tool_call index; ignoring it");
                                        continue;
                                    };
                                    while accumulated_tool_calls.len() <= index {
                                        accumulated_tool_calls.push(PendingToolCall::default());
                                    }
                                    if let Some(id) = tc.get("id").and_then(Value::as_str) {
                                        accumulated_tool_calls[index].id.push_str(id);
                                    }
                                    if let Some(func) = tc.get("function") {
                                        if let Some(name) = func.get("name").and_then(Value::as_str)
                                        {
                                            accumulated_tool_calls[index].name.push_str(name);
                                        }
                                        if let Some(args) =
                                            func.get("arguments").and_then(Value::as_str)
                                        {
                                            accumulated_tool_calls[index].arguments.push_str(args);
                                        }
                                    }
                                }
                                if has_started_answer {
                                    has_started_answer = false;
                                    if let Some(s) = sink {
                                        s.on_action("Searching", Some(ProgressActivity::Searching));
                                    }
                                }
                            }

                            let reasoning_chunk = delta
                                .get("reasoning_content")
                                .or_else(|| delta.get("reasoning"))
                                .or_else(|| delta.get("thought"))
                                .or_else(|| delta.get("thinking"))
                                .and_then(Value::as_str);
                            if let Some(reasoning_chunk) = reasoning_chunk {
                                if !push_bounded(
                                    &mut accumulated_reasoning,
                                    reasoning_chunk,
                                    MAX_STREAM_REASONING_BYTES,
                                ) {
                                    stream_bounded = true;
                                    stream_interrupted = true;
                                    warn!("AI reasoning exceeded XiaoAI's absolute output limit");
                                    break 'streaming;
                                }
                            }

                            let content_chunk =
                                delta.get("content").and_then(Value::as_str).unwrap_or("");
                            if content_chunk.is_empty() {
                                continue;
                            }
                            if !push_bounded(
                                &mut accumulated_raw,
                                content_chunk,
                                MAX_STREAM_VISIBLE_BYTES,
                            ) {
                                stream_bounded = true;
                                stream_interrupted = true;
                                warn!("AI answer exceeded XiaoAI's absolute output limit");
                                break 'streaming;
                            }

                            // Re-sanitizing the whole accumulated answer on every
                            // chunk made streaming O(n^2). Partials only matter to a
                            // progress sink, which itself syncs at most every ~1.2s,
                            // so refresh them on a short interval instead.
                            let Some(s) = sink else {
                                continue;
                            };
                            if last_partial_at
                                .is_some_and(|at| at.elapsed() < PARTIAL_REFRESH_INTERVAL)
                            {
                                continue;
                            }
                            last_partial_at = Some(std::time::Instant::now());
                            let visible_partial =
                                crate::parser::markdown::sanitize_leaked_llm_artifacts(
                                    &accumulated_raw,
                                )
                                .trim()
                                .to_string();

                            if !visible_partial.is_empty() && accumulated_tool_calls.is_empty() {
                                let is_tool_preamble = turn == 0
                                    && payload.get("tools").is_some()
                                    && crate::ai::tools::is_suppressed_tool_preamble_stream(
                                        &visible_partial,
                                    );

                                if !is_tool_preamble {
                                    has_started_answer = true;
                                    s.on_partial_answer(&visible_partial);
                                }
                            }
                        }
                    }
                }
            }

            if !stream_done && !cancelled && !stream_interrupted {
                match decoder.finish() {
                    Ok(events) => {
                        for event in events {
                            if matches!(event, StreamEvent::Done) {
                                stream_done = true;
                            }
                        }
                        if !stream_done {
                            stream_interrupted = true;
                        }
                    }
                    Err(error) => {
                        warn!("AI response SSE final decode failed: {error}");
                        stream_interrupted = true;
                    }
                }
            }

            if accumulated_tool_calls.is_empty() && !accumulated_raw.is_empty() {
                let leaked = extract_leaked_tool_calls(&accumulated_raw);
                if !leaked.is_empty() {
                    accumulated_tool_calls = leaked;
                }
            }

            if !can_run_tools && !cancelled {
                // The model still wanted tools after its last tool round (or
                // wrote tool markup Xiao could not parse). Say so instead of
                // silently answering with whatever text preceded the call.
                if !accumulated_tool_calls.is_empty() {
                    unexecuted_tools.extend(
                        accumulated_tool_calls
                            .iter()
                            .map(|call| call.name.trim().to_string()),
                    );
                } else if has_tool_call_markup(&accumulated_raw) {
                    unexecuted_tools.push(String::new());
                }
            } else if accumulated_tool_calls.is_empty()
                && !cancelled
                && has_tool_call_markup(&accumulated_raw)
            {
                unexecuted_tools.push(String::new());
            }

            if can_run_tools && !accumulated_tool_calls.is_empty() && !cancelled {
                // Searches and page reads of this round run concurrently.
                let (mut research_results, research_cancelled) = run_research_calls(
                    &accumulated_tool_calls,
                    &mut research_calls,
                    cancel_rx,
                    sink,
                )
                .await;
                if research_cancelled {
                    cancelled = true;
                    break;
                }
                let mut tool_results = Vec::new();
                for (call_index, tc) in accumulated_tool_calls.iter().enumerate() {
                    let name = tc.name.trim();
                    let tool_id = if tc.id.is_empty() {
                        "call_default".to_string()
                    } else {
                        tc.id.clone()
                    };
                    let result = if guest_mode
                        && !crate::ai::tools::GUEST_MODE_TOOLS.contains(&name)
                    {
                        // Only research tools are offered in guest mode; a model
                        // that calls anything else gets a refusal, never a side
                        // effect in someone else's chat.
                        format!("Tool `{name}` tidak tersedia saat Xiao dipanggil sebagai tamu. Jawab dengan teks saja.")
                    } else if let Some(research) =
                        research_results.get_mut(call_index).and_then(Option::take)
                    {
                        research
                    } else if name == "create_quiz" {
                        if let Some(s) = sink {
                            s.on_action("Quiz", Some(ProgressActivity::Quiz));
                        }
                        let question_key = quiz_question_key(&tc.arguments);
                        if question_key
                            .as_ref()
                            .is_some_and(|key| sent_quiz_questions.contains(key))
                        {
                            "Kuis dengan pertanyaan ini sudah terkirim; jangan kirim ulang."
                                .to_string()
                        } else {
                            let outcome = super::quiz::run_create_quiz(
                                bot.as_ref(),
                                chat_id,
                                reply_to_message_id,
                                &tc.arguments,
                            )
                            .await;
                            if outcome.sent {
                                quizzes_sent += 1;
                                if let Some(key) = question_key {
                                    sent_quiz_questions.insert(key);
                                }
                            }
                            if let Some(summary) = outcome.history_summary {
                                match &mut quiz_history_summary {
                                    Some(existing) => {
                                        existing.push_str("\n\n---\n\n");
                                        existing.push_str(&summary);
                                    }
                                    None => quiz_history_summary = Some(summary),
                                }
                            }
                            outcome.result
                        }
                    } else if name == "send_live_photo" {
                        if let Some(s) = sink {
                            s.on_action("Live photo", Some(ProgressActivity::Drawing));
                        }
                        let photo_key = live_photo_key(&tc.arguments);
                        if photo_key
                            .as_ref()
                            .is_some_and(|key| sent_live_photos.contains(key))
                        {
                            "Live photo ini sudah terkirim; jangan kirim ulang.".to_string()
                        } else {
                            // Downloads and the upload can take a while, so Stop
                            // and shutdown must be able to interrupt them.
                            match race_with_cancel(
                                cancel_rx,
                                super::live_photo::run_send_live_photo(
                                    bot.as_ref(),
                                    chat_id,
                                    reply_to_message_id,
                                    &tc.arguments,
                                ),
                            )
                            .await
                            {
                                Raced::Completed(res) => {
                                    if res.starts_with(super::live_photo::LIVE_PHOTO_SENT) {
                                        if let Some(key) = photo_key {
                                            sent_live_photos.insert(key);
                                        }
                                    }
                                    res
                                }
                                Raced::Cancelled => {
                                    cancelled = true;
                                    "Pengiriman live photo dibatalkan.".to_string()
                                }
                            }
                        }
                    } else if name == "send_photo" {
                        if let Some(s) = sink {
                            s.on_action("Photo", Some(ProgressActivity::Drawing));
                        }
                        match serde_json::from_str::<crate::ai::tools::SendPhotoArgs>(&tc.arguments)
                        {
                            Ok(mut args) => {
                                args.sanitize();
                                match args.validate() {
                                    Ok(()) => {
                                        let tag = if let Some(caption) = &args.caption {
                                            format!(
                                                r#"<img src="{}" caption="{}"/>"#,
                                                args.url,
                                                caption.replace('"', "&quot;")
                                            )
                                        } else {
                                            format!(r#"<img src="{}"/>"#, args.url)
                                        };
                                        let ready = format!("Foto telah disiapkan. Tag media: {tag}\nAnda DAPAT menyematkan tag media ini langsung di tengah-tengah teks penjelasan pada posisi yang paling relevan (misal di bawah heading atau di antara paragraf), atau biarkan Xiao menampilkannya secara otomatis.");
                                        stage_media_tag(&mut staged_media_tags, tag, ready)
                                    }
                                    Err(validation_err) => {
                                        format!("Validasi foto gagal: {validation_err}")
                                    }
                                }
                            }
                            Err(parse_err) => {
                                format!("Format argumen send_photo tidak valid: {parse_err}")
                            }
                        }
                    } else if name == "send_collage" {
                        if let Some(s) = sink {
                            s.on_action("Collage", Some(ProgressActivity::Drawing));
                        }
                        match serde_json::from_str::<crate::ai::tools::SendCollageArgs>(
                            &tc.arguments,
                        ) {
                            Ok(mut args) => {
                                args.sanitize();
                                match args.validate() {
                                    Ok(()) => {
                                        let caption_attr = if let Some(caption) = &args.caption {
                                            format!(
                                                r#" caption="{}""#,
                                                caption.replace('"', "&quot;")
                                            )
                                        } else {
                                            String::new()
                                        };
                                        let img_tags = args
                                            .urls
                                            .iter()
                                            .map(|u| format!(r#"<img src="{u}"/>"#))
                                            .collect::<Vec<_>>()
                                            .join("");
                                        let collage_tag = format!(
                                            r#"<tg-collage{caption_attr}>{img_tags}</tg-collage>"#
                                        );
                                        let ready = format!("Album kolase foto telah disiapkan. Tag media: {collage_tag}\nAnda DAPAT menyematkan tag media ini langsung di tengah-tengah penjelasan pada bagian yang paling sesuai.");
                                        stage_media_tag(&mut staged_media_tags, collage_tag, ready)
                                    }
                                    Err(validation_err) => {
                                        format!("Validasi kolase foto gagal: {validation_err}")
                                    }
                                }
                            }
                            Err(parse_err) => {
                                format!("Format argumen send_collage tidak valid: {parse_err}")
                            }
                        }
                    } else if name == "send_slideshow" {
                        if let Some(s) = sink {
                            s.on_action("Slideshow", Some(ProgressActivity::Drawing));
                        }
                        match serde_json::from_str::<crate::ai::tools::SendSlideshowArgs>(
                            &tc.arguments,
                        ) {
                            Ok(mut args) => {
                                args.sanitize();
                                match args.validate() {
                                    Ok(()) => {
                                        let caption_attr = if let Some(caption) = &args.caption {
                                            format!(
                                                r#" caption="{}""#,
                                                caption.replace('"', "&quot;")
                                            )
                                        } else {
                                            String::new()
                                        };
                                        let img_tags = args
                                            .urls
                                            .iter()
                                            .map(|u| format!(r#"<img src="{u}"/>"#))
                                            .collect::<Vec<_>>()
                                            .join("");
                                        let slideshow_tag = format!(
                                            r#"<tg-slideshow{caption_attr}>{img_tags}</tg-slideshow>"#
                                        );
                                        let ready = format!("Tayangan slide interaktif telah disiapkan. Tag media: {slideshow_tag}\nAnda DAPAT menyematkan tag media ini langsung di tengah-tengah penjelasan pada bagian yang paling sesuai.");
                                        stage_media_tag(
                                            &mut staged_media_tags,
                                            slideshow_tag,
                                            ready,
                                        )
                                    }
                                    Err(validation_err) => {
                                        format!("Validasi tayangan slide gagal: {validation_err}")
                                    }
                                }
                            }
                            Err(parse_err) => {
                                format!("Format argumen send_slideshow tidak valid: {parse_err}")
                            }
                        }
                    } else if name == "send_audio" {
                        if let Some(s) = sink {
                            s.on_action("Audio", Some(ProgressActivity::Listening));
                        }
                        match serde_json::from_str::<crate::ai::tools::SendAudioArgs>(&tc.arguments)
                        {
                            Ok(mut args) => {
                                args.sanitize();
                                match args.validate() {
                                    Ok(()) => {
                                        let mut attrs = Vec::new();
                                        if let Some(title) = &args.title {
                                            attrs.push(format!(
                                                r#"title="{}""#,
                                                title.replace('"', "&quot;")
                                            ));
                                        }
                                        if let Some(performer) = &args.performer {
                                            attrs.push(format!(
                                                r#"performer="{}""#,
                                                performer.replace('"', "&quot;")
                                            ));
                                        }
                                        if let Some(caption) = &args.caption {
                                            attrs.push(format!(
                                                r#"caption="{}""#,
                                                caption.replace('"', "&quot;")
                                            ));
                                        }
                                        let extra = if attrs.is_empty() {
                                            String::new()
                                        } else {
                                            format!(" {}", attrs.join(" "))
                                        };
                                        let audio_tag =
                                            format!(r#"<audio src="{}"{extra}/>"#, args.url);
                                        let ready = format!("Audio telah disiapkan. Tag media: {audio_tag}\nAnda DAPAT menyematkan tag media ini langsung di tengah penjelasan teks pada posisi yang relevan.");
                                        stage_media_tag(&mut staged_media_tags, audio_tag, ready)
                                    }
                                    Err(validation_err) => {
                                        format!("Validasi audio gagal: {validation_err}")
                                    }
                                }
                            }
                            Err(parse_err) => {
                                format!("Format argumen send_audio tidak valid: {parse_err}")
                            }
                        }
                    } else if name == "send_voice" {
                        if let Some(s) = sink {
                            s.on_action("Voice", Some(ProgressActivity::Listening));
                        }
                        match serde_json::from_str::<crate::ai::tools::SendVoiceArgs>(&tc.arguments)
                        {
                            Ok(mut args) => {
                                args.sanitize();
                                match args.validate() {
                                    Ok(()) => {
                                        let title =
                                            args.caption.as_deref().unwrap_or("Pesan Suara");
                                        let voice_tag = format!("[rekaman: {title}]({})", args.url);
                                        let ready = format!("Pesan suara telah disiapkan. Tag media: {voice_tag}\nAnda DAPAT menyematkan tag media ini langsung di tengah penjelasan teks pada posisi yang relevan.");
                                        stage_media_tag(&mut staged_media_tags, voice_tag, ready)
                                    }
                                    Err(validation_err) => {
                                        format!("Validasi pesan suara gagal: {validation_err}")
                                    }
                                }
                            }
                            Err(parse_err) => {
                                format!("Format argumen send_voice tidak valid: {parse_err}")
                            }
                        }
                    } else if name == "send_location" {
                        if let Some(s) = sink {
                            s.on_action("Location", Some(ProgressActivity::Looking));
                        }
                        match serde_json::from_str::<crate::ai::tools::SendLocationArgs>(
                            &tc.arguments,
                        ) {
                            Ok(mut args) => {
                                args.sanitize();
                                match args.validate() {
                                    Ok(()) => {
                                        let title_attr = if let Some(title) = &args.title {
                                            format!(r#" title="{}""#, title.replace('"', "&quot;"))
                                        } else {
                                            String::new()
                                        };
                                        let map_tag = format!(
                                            r#"<tg-map lat="{}" lon="{}" zoom="13"{title_attr}/>"#,
                                            args.latitude, args.longitude
                                        );
                                        let ready = format!("Peta lokasi telah disiapkan. Tag media: {map_tag}\nAnda DAPAT menyematkan tag media ini langsung di tengah penjelasan teks pada posisi yang relevan.");
                                        stage_media_tag(&mut staged_media_tags, map_tag, ready)
                                    }
                                    Err(validation_err) => {
                                        format!("Validasi lokasi gagal: {validation_err}")
                                    }
                                }
                            }
                            Err(parse_err) => {
                                format!("Format argumen send_location tidak valid: {parse_err}")
                            }
                        }
                    } else if name == "send_document" {
                        if let Some(s) = sink {
                            s.on_action("Document", Some(ProgressActivity::Reading));
                        }
                        match serde_json::from_str::<crate::ai::tools::SendDocumentArgs>(
                            &tc.arguments,
                        ) {
                            Ok(mut args) => {
                                args.sanitize();
                                match args.validate() {
                                    Ok(()) => {
                                        let file_name =
                                            args.file_name.as_deref().unwrap_or("Dokumen");
                                        let doc_tag =
                                            format!("[document: {file_name}]({})", args.url);
                                        let ready = format!("Dokumen telah disiapkan. Tag media: {doc_tag}\nAnda DAPAT menyematkan tag media ini langsung di tengah penjelasan teks pada posisi yang relevan.");
                                        stage_media_tag(&mut staged_media_tags, doc_tag, ready)
                                    }
                                    Err(validation_err) => {
                                        format!("Validasi dokumen gagal: {validation_err}")
                                    }
                                }
                            }
                            Err(parse_err) => {
                                format!("Format argumen send_document tidak valid: {parse_err}")
                            }
                        }
                    } else if name == "create_document" {
                        if let Some(s) = sink {
                            s.on_action("Document", Some(ProgressActivity::Reading));
                        }
                        match serde_json::from_str::<crate::ai::tools::CreateDocumentArgs>(
                            &tc.arguments,
                        ) {
                            Ok(mut args) => {
                                args.sanitize();
                                match args.validate() {
                                    Ok(()) => {
                                        let (final_bytes, final_filename, mime_type) =
                                            crate::document::create_document_payload(
                                                &args.filename,
                                                &args.content,
                                                args.as_zip,
                                            );

                                        let attach_key = format!("doc_{}", staged_documents.len());
                                        let staged_doc = StagedDocument::new(
                                            attach_key,
                                            final_bytes,
                                            mime_type,
                                            final_filename,
                                        );
                                        let doc_tag = staged_doc.markdown_tag();
                                        let doc_filename = staged_doc.filename.clone();
                                        stage_document(&mut staged_documents, staged_doc)
                                            .unwrap_or_else(|| format!(
                                                "Dokumen '{doc_filename}' telah berhasil disiapkan di memori. Tag media Telegram: {doc_tag}\nWAJIB sematkan tag media {doc_tag} ini langsung di dalam teks jawaban/penjelasan Anda pada posisi yang paling relevan, dan jelaskan isi dokumen ini kepada pengguna."
                                            ))
                                    }
                                    Err(validation_err) => {
                                        format!("Validasi dokumen gagal: {validation_err}")
                                    }
                                }
                            }
                            Err(parse_err) => {
                                format!("Format argumen create_document tidak valid: {parse_err}")
                            }
                        }
                    } else if name == "create_archive" {
                        match serde_json::from_str::<crate::ai::tools::CreateArchiveArgs>(
                            &tc.arguments,
                        ) {
                            Ok(mut args) => {
                                args.sanitize();
                                match args.validate() {
                                    Ok(()) => {
                                        match crate::document::create_in_memory_multi_file_zip(
                                            &args.files,
                                        ) {
                                            Ok(zip_bytes) => {
                                                let attach_key =
                                                    format!("doc_{}", staged_documents.len());
                                                let staged_doc = StagedDocument::new(
                                                    attach_key,
                                                    zip_bytes,
                                                    "application/zip".to_string(),
                                                    args.filename,
                                                );
                                                let doc_tag = staged_doc.markdown_tag();
                                                let zip_filename = staged_doc.filename.clone();
                                                let file_count = args.files.len();
                                                stage_document(&mut staged_documents, staged_doc)
                                                    .unwrap_or_else(|| format!(
                                                        "Arsip ZIP '{zip_filename}' yang memuat {file_count} file telah berhasil dibuat di memori. Tag media Telegram: {doc_tag}\nWAJIB sematkan tag media {doc_tag} ini langsung di dalam teks jawaban/penjelasan Anda pada posisi yang paling relevan. Jelaskan daftar berkas yang ada di dalamnya secara rapi kepada pengguna."
                                                    ))
                                            }
                                            Err(zip_err) => {
                                                format!("Gagal membuat berkas ZIP arsip: {zip_err}")
                                            }
                                        }
                                    }
                                    Err(validation_err) => {
                                        format!("Validasi arsip gagal: {validation_err}")
                                    }
                                }
                            }
                            Err(parse_err) => {
                                format!("Format argumen create_archive tidak valid: {parse_err}")
                            }
                        }
                    } else {
                        format!("Tool '{name}' tidak didukung.")
                    };
                    tool_results.push((tool_id, name.to_string(), tc.arguments.clone(), result));
                    if cancelled {
                        break;
                    }
                }

                if cancelled {
                    break;
                }

                let tool_calls_json = tool_results
                    .iter()
                    .map(|(id, name, args, _)| {
                        json!({
                            "id": id,
                            "type": "function",
                            "function": {
                                "name": name,
                                "arguments": args
                            }
                        })
                    })
                    .collect::<Vec<_>>();

                messages.push(json!({
                    "role": "assistant",
                    "content": null,
                    "tool_calls": tool_calls_json
                }));

                for (id, _, _, res) in &tool_results {
                    messages.push(json!({
                        "role": "tool",
                        "tool_call_id": id,
                        "content": tool_budget.bound(res)
                    }));
                }

                media_staged |= !staged_media_tags.is_empty()
                    || !staged_documents.is_empty()
                    || !sent_live_photos.is_empty();

                let rounds_left = if supports_tools {
                    MAX_TOOL_ROUNDS.saturating_sub(turn + 1)
                } else {
                    0
                };
                messages.push(json!({
                    "role": "user",
                    "content": follow_up_prompt(&RoundState {
                        rounds_left,
                        guest_mode,
                        media_staged,
                        quizzes_sent,
                    })
                }));
                payload["messages"] = json!(messages);

                if let Some(s) = sink {
                    s.on_action("Summarizing", Some(ProgressActivity::Summarizing));
                }

                continue;
            }

            break;
        }

        // Post-process final output
        let (extracted_thinking, mut answer_text) =
            crate::parser::markdown::extract_thinking_and_answer(&accumulated_raw);
        let thinking_text = if !accumulated_reasoning.is_empty() {
            Some(accumulated_reasoning.trim().to_string())
        } else {
            extracted_thinking
        };
        // After native quizzes the model is asked to reply only with a short
        // confirmation; that reply is not an answer to show.
        if quizzes_sent > 0 && is_quiz_done_reply(&answer_text) {
            answer_text.clear();
        }

        if !staged_documents.is_empty() {
            let mut missing_tags = Vec::new();
            for doc in &staged_documents {
                let tag_needle = doc.attach_uri();
                if !answer_text.contains(&tag_needle) {
                    missing_tags.push(doc.markdown_tag());
                }
            }
            if !missing_tags.is_empty() {
                if answer_text.trim().is_empty() {
                    answer_text = format!(
                        "Berikut adalah dokumen yang Anda minta:\n\n{}",
                        missing_tags.join("\n\n")
                    );
                } else {
                    answer_text.push_str("\n\n");
                    answer_text.push_str(&missing_tags.join("\n\n"));
                }
            }
        }

        if !staged_media_tags.is_empty() {
            if answer_text.trim().is_empty() {
                answer_text = staged_media_tags.join("\n\n");
            } else {
                let mut missing_tags = Vec::new();
                for tag in &staged_media_tags {
                    let is_already_present = if tag.starts_with("<tg-collage") {
                        answer_text.contains("<tg-collage") || answer_text.contains("kolase")
                    } else if tag.starts_with("<tg-slideshow") {
                        answer_text.contains("<tg-slideshow") || answer_text.contains("carousel")
                    } else if let Some(src) = tag
                        .split(r#"src=""#)
                        .nth(1)
                        .and_then(|s| s.split('"').next())
                    {
                        !src.is_empty() && answer_text.contains(src)
                    } else if let Some(url) =
                        tag.split("](").nth(1).and_then(|s| s.split(')').next())
                    {
                        !url.is_empty() && answer_text.contains(url)
                    } else if tag.starts_with("<tg-map") {
                        answer_text.contains("<tg-map")
                            || (tag.contains(r#"lat=""#) && {
                                let lat = tag
                                    .split(r#"lat=""#)
                                    .nth(1)
                                    .and_then(|s| s.split('"').next())
                                    .unwrap_or("");
                                !lat.is_empty() && answer_text.contains(lat)
                            })
                    } else {
                        answer_text.contains(tag)
                    };

                    if !is_already_present {
                        missing_tags.push(tag.as_str());
                    }
                }
                if !missing_tags.is_empty() {
                    let header = missing_tags.join("\n\n");
                    answer_text = format!("{header}\n\n{}", answer_text.trim());
                }
            }
        }

        if !cancelled {
            let mut notices = Vec::new();
            if !unexecuted_tools.is_empty() {
                notices.push(unexecuted_tools_notice(&unexecuted_tools));
            }
            if provider_failed_late {
                notices.push("_⚠️ Provider AI berhenti merespons sebelum jawaban selesai; hasil yang sudah siap tetap dikirim. Balas \"lanjutkan\" untuk meneruskan._".to_string());
            }
            for notice in notices {
                if answer_text.trim().is_empty() {
                    answer_text = notice;
                } else {
                    answer_text.push_str("\n\n");
                    answer_text.push_str(&notice);
                }
            }
        }

        let turn_record = TurnRecord {
            chat_id,
            thread_id,
            user_id,
            document_images: document_images.as_deref(),
            image_bytes: image_bytes.as_deref(),
            mime_type,
            audio_bytes: audio_bytes.as_deref(),
            audio_mime,
            doc_name,
            video_bytes: video_bytes.as_deref(),
            video_mime,
            canonical_prompt: canonical_history_prompt.as_deref(),
            clean_prompt: &clean_prompt,
        };

        if quizzes_sent > 0 && !cancelled && answer_text.trim().is_empty() {
            // The native quizzes are the whole answer.
            let summary = quiz_history_summary
                .take()
                .unwrap_or_else(|| "[Kuis Native Telegram]".to_string());
            self.record_turn(&turn_record, summary).await;
            if let Some(s) = sink {
                s.on_complete();
            }
            return (thinking_text, "[QUIZ_SENT]".to_string(), Vec::new(), false);
        }

        if cancelled {
            if answer_text.trim().is_empty() {
                answer_text = GENERATION_STOPPED_NOTICE.to_string();
            } else {
                answer_text.push_str("\n\n_⏹️ Generasi dihentikan oleh pengguna._");
            }
        } else if stream_bounded {
            if answer_text.trim().is_empty() {
                answer_text =
                    "⚠️ Respons provider melewati batas ukuran aman XiaoAI dan dihentikan."
                        .to_string();
            } else {
                answer_text.push_str(
                    "\n\n_⚠️ Respons dihentikan karena melewati batas ukuran aman XiaoAI._",
                );
            }
        } else if stream_interrupted {
            if answer_text.trim().is_empty() {
                answer_text = "⚠️ Stream provider terputus sebelum jawaban diterima.".to_string();
            } else {
                answer_text
                    .push_str("\n\n_⚠️ Stream provider terputus; jawaban mungkin tidak lengkap._");
            }
        } else if answer_text.trim().is_empty() {
            if !staged_media_tags.is_empty() {
                answer_text = staged_media_tags.join("\n\n");
            } else if !accumulated_reasoning.is_empty() {
                answer_text = "Maaf, Xiao telah memproses permintaan ini namun model tidak menghasilkan teks jawaban. Silakan coba ulangi pertanyaan dengan instruksi yang lebih jelas.".to_string();
            } else {
                answer_text = "Maaf, Xiao tidak dapat menemukan informasi yang diminta saat ini. Silakan coba ulangi pertanyaan dengan lebih spesifik.".to_string();
            }
        }

        if let Some(s) = sink {
            if cancelled {
                s.on_partial_answer(&answer_text);
                s.on_failure("Stopped by user", false);
            } else if stream_interrupted {
                s.on_partial_answer(&answer_text);
                s.on_failure(
                    if stream_bounded {
                        "Provider output exceeded safety limit"
                    } else {
                        "Provider stream interrupted"
                    },
                    true,
                );
            } else {
                if !has_started_answer {
                    s.on_action("Writing", Some(ProgressActivity::Writing));
                }
                s.on_complete();
            }
        }

        // Cancelled/interrupted output is presentation-only. Do not make a
        // partial answer canonical history: retry/follow-up context must only
        // see completed assistant turns.
        // Quizzes that already reached the chat are still recorded.
        if cancelled || stream_interrupted {
            if let Some(summary) = quiz_history_summary.filter(|_| !guest_mode) {
                self.record_turn(&turn_record, summary).await;
            }
            return (thinking_text, answer_text, staged_documents, cancelled);
        }

        // Guest replies are not written to history and do not feed memory
        // curation; the conversation belongs to someone else's chat.
        if guest_mode {
            return (thinking_text, answer_text, staged_documents, cancelled);
        }

        let assistant_content = match quiz_history_summary {
            Some(summary) => format!("{summary}\n\n---\n\n{answer_text}"),
            None => answer_text.clone(),
        };
        self.record_turn(&turn_record, assistant_content).await;

        (thinking_text, answer_text, staged_documents, cancelled)
    }

    /// Stores the exchange (with its attachments) in history and starts
    /// background memory curation for it.
    async fn record_turn(&self, record: &TurnRecord<'_>, assistant_content: String) {
        let attachment_refs = persist_runtime_attachments(
            record.chat_id,
            record.thread_id,
            record.document_images,
            record.image_bytes,
            record.mime_type,
            record.audio_bytes,
            record.audio_mime,
            record.doc_name,
            record.video_bytes,
            record.video_mime,
        )
        .await;
        let user_message_content = encode_user_content(
            canonical_persisted_prompt(record.canonical_prompt, record.clean_prompt),
            attachment_refs,
        );
        let user_content_str = serialize_user_content(&user_message_content, record.clean_prompt);
        if !save_scoped_turn_async(
            record.chat_id,
            record.thread_id,
            record.user_id,
            user_content_str,
            assistant_content.clone(),
        )
        .await
        {
            warn!("Conversation turn was not persisted to history");
        }

        let service = self.clone();
        let prompt = record.clean_prompt.to_string();
        let (user_id, chat_id, thread_id) = (record.user_id, record.chat_id, record.thread_id);
        self.background_tasks.spawn(async move {
            service
                .process_background_memory_turn(
                    user_id,
                    chat_id,
                    thread_id,
                    &prompt,
                    &assistant_content,
                )
                .await;
        });
    }
}

/// What [`AIChatService::record_turn`] stores for one exchange.
struct TurnRecord<'a> {
    chat_id: i64,
    thread_id: i64,
    user_id: i64,
    document_images: Option<&'a [Vec<u8>]>,
    image_bytes: Option<&'a [u8]>,
    mime_type: Option<&'a str>,
    audio_bytes: Option<&'a [u8]>,
    audio_mime: Option<&'a str>,
    doc_name: Option<&'a str>,
    video_bytes: Option<&'a [u8]>,
    video_mime: Option<&'a str>,
    canonical_prompt: Option<&'a str>,
    clean_prompt: &'a str,
}
