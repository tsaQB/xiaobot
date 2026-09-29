use futures_util::StreamExt;
use reqwest::multipart::{Form, Part};
use reqwest::Client;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::ops::Deref;
use std::time::Duration;
use tracing::warn;

use super::client_raw as raw;
use super::models::{
    ApiResponse, BotCommand, ChatMember, EphemeralMessageParameters, FileInfo,
    InlineKeyboardMarkup, InputMedia, InputPollOption, InputRichMessage, ReplyParameters,
    RichBlock, StagedDocument, Update, User,
};
use super::transport_policy::{
    fallback_allowed_error, fallback_allowed_response, is_idempotent_method,
    retry_delay_for_http_status, retry_delay_for_idempotent_timeout, retry_delay_from_error,
    retry_delay_from_response, MAX_TELEGRAM_ATTEMPTS,
};

pub use raw::TelegramDeliveryContext;

const MAX_TELEGRAM_DOWNLOAD_BYTES: usize = 20 * 1024 * 1024;
const MAX_TELEGRAM_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
/// Telegram rejects messages longer than 4096 characters. Text longer than
/// the threshold is split into chunks with headroom for HTML entity growth.
pub(crate) const TELEGRAM_TEXT_SPLIT_THRESHOLD_CHARS: usize = 4000;
pub(crate) const TELEGRAM_TEXT_CHUNK_CHARS: usize = 3800;
/// Longest filename sent in a multipart upload.
const MAX_UPLOAD_FILENAME_CHARS: usize = 128;

/// Why a user file could not be downloaded from Telegram.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileDownloadError {
    /// Larger than the 20 MB Bot API download limit.
    TooLarge,
    /// The file id is invalid or the file is no longer available.
    NotFound,
    /// Transient transport failure; retrying may help.
    Network,
}

/// Upload file name and MIME type of an image, from its signature.
fn image_kind(bytes: &[u8]) -> Option<(&'static str, &'static str)> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(("image.png", "image/png"))
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some(("image.jpg", "image/jpeg"))
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some(("image.gif", "image/gif"))
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some(("image.webp", "image/webp"))
    } else {
        None
    }
}

/// A native poll or quiz for `sendPoll`.
#[derive(Debug, Clone, Default)]
pub struct PollRequest<'a> {
    pub question: &'a str,
    pub options: &'a [InputPollOption],
    pub is_anonymous: Option<bool>,
    /// `"quiz"` or `"regular"` (the Bot API default).
    pub poll_type: Option<&'a str>,
    /// Correct answers of a quiz, ascending; several make a quiz whose
    /// players may pick several answers.
    pub correct_option_ids: &'a [i32],
    pub explanation: Option<&'a str>,
    pub explanation_parse_mode: Option<&'a str>,
    /// Show the options in a random order to each player.
    pub shuffle_options: bool,
    /// Media shown with the question (`InputPollMedia`), e.g. a photo.
    pub media: Option<Value>,
    /// Text shown under the question.
    pub description: Option<&'a str>,
    /// Media shown with the quiz explanation (`InputPollMedia`).
    pub explanation_media: Option<Value>,
    /// Let players change their answer.
    pub allows_revoting: bool,
    /// Seconds before the poll closes by itself.
    pub open_period: Option<i32>,
    /// Show the aggregate results only once the poll is closed.
    pub hide_results_until_closes: bool,
}

/// Converts one rendered Telegram-HTML chunk back to readable plain text.
fn html_chunk_to_plain_text(html: &str) -> String {
    static RE_TAG: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"</?[^>]+>").expect("valid static regex"));
    let stripped = RE_TAG.replace_all(html, "");
    html_escape::decode_html_entities(&stripped)
        .trim()
        .to_string()
}

/// Normalizes a filename before it is placed in a multipart
/// `Content-Disposition` header: strips path components, control characters
/// and quotes, avoids hidden/empty names, and bounds the length. Names can
/// come from model-generated documents.
pub(crate) fn sanitize_upload_filename(raw: &str) -> String {
    let base = raw.rsplit(['/', '\\']).next().unwrap_or(raw);
    let cleaned: String = base
        .chars()
        .filter(|ch| {
            !ch.is_control() && !matches!(ch, '"' | '\'' | ';' | '<' | '>' | '|' | '*' | '?' | ':')
        })
        .collect();
    let trimmed = cleaned.trim().trim_start_matches('.').trim();
    if trimmed.is_empty() {
        return "file.bin".to_string();
    }
    crate::util::truncate_chars(trimmed, MAX_UPLOAD_FILENAME_CHARS)
}

/// Remote media downloaded at the same time for one message.
const MEDIA_DOWNLOAD_CONCURRENCY: usize = 4;

/// How one `sendRichMessage` attempt ended.
enum RichAttempt {
    Sent(Value),
    /// Telegram answered 400: a simpler form of the message may still work.
    Rejected(String),
    /// Any other failure. Another attempt could deliver the message twice.
    Failed(String),
}

/// Where a rich message goes; shared by every attempt to deliver it.
#[derive(Clone, Copy)]
struct RichTarget<'a> {
    chat_id: i64,
    reply_markup: Option<&'a Value>,
    receiver_user_id: Option<i64>,
    reply_to_message_id: Option<i64>,
}

/// A rich message whose remote media was downloaded for upload.
struct StagedRichMessage {
    /// The message pointing at the uploads (`attach://file_N`).
    message: InputRichMessage,
    downloads: Vec<StagedDocument>,
    /// Addresses that could not be downloaded, or that returned a web page
    /// where media was expected. They keep their address in `message`.
    unresolved: HashSet<String>,
}

/// Whether a download is a web page (an error or file description page)
/// rather than media.
fn is_web_page(content_type: &str, bytes: &[u8]) -> bool {
    if content_type.to_ascii_lowercase().contains("html") {
        return true;
    }
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(64)])
        .trim_start()
        .to_ascii_lowercase();
    head.starts_with("<!doctype html") || head.starts_with("<html")
}

/// Multipart part for a staged file. A MIME type that does not parse is left
/// out instead of failing the whole request.
fn upload_part(file: &StagedDocument) -> Part {
    let file_name = sanitize_upload_filename(&file.filename);
    match Part::bytes(file.bytes.clone())
        .file_name(file_name.clone())
        .mime_str(&file.mime_type)
    {
        Ok(part) => part,
        Err(_) => Part::bytes(file.bytes.clone()).file_name(file_name),
    }
}

enum HttpResponseOutcome {
    Success(Value),
    Retry { delay: Duration, error: String },
    TerminalError(String),
}

#[derive(Clone)]
pub struct TelegramBotClient {
    inner: raw::TelegramBotClient,
    base_url: String,
    client: Client,
}

impl Deref for TelegramBotClient {
    type Target = raw::TelegramBotClient;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

// Extended Telegram Bot API 10.3 transport methods
#[allow(dead_code)]
impl TelegramBotClient {
    pub fn new(token: impl Into<String>) -> Self {
        let token = token.into().trim().to_string();
        let inner = raw::TelegramBotClient;
        let base_url = format!("https://api.telegram.org/bot{token}");
        let client = Client::builder()
            .timeout(Duration::from_secs(45))
            .build()
            .unwrap_or_else(|_| Client::new());
        Self {
            inner,
            base_url,
            client,
        }
    }

    pub fn with_base_url(_token: impl Into<String>, base_url: impl Into<String>) -> Self {
        let base_url = base_url.into().trim().trim_end_matches('/').to_string();
        let inner = raw::TelegramBotClient;
        let client = Client::builder()
            .timeout(Duration::from_secs(45))
            .build()
            .unwrap_or_else(|_| Client::new());
        Self {
            inner,
            base_url,
            client,
        }
    }

    pub async fn with_delivery_context<F, T>(context: TelegramDeliveryContext, future: F) -> T
    where
        F: std::future::Future<Output = T>,
    {
        raw::TelegramBotClient::with_delivery_context(context, future).await
    }

    pub fn current_delivery_context() -> TelegramDeliveryContext {
        raw::TelegramBotClient::current_delivery_context()
    }

    pub fn raw(&self) -> &raw::TelegramBotClient {
        &self.inner
    }

    fn replace_callback_query_message() -> Option<bool> {
        Self::current_delivery_context().replace_callback_query_message
    }

    fn telegram_api_error(method: &str, response: &Value) -> String {
        let code = response
            .get("error_code")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        let description = response
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("Telegram API request failed");
        let retry_after = response
            .pointer("/parameters/retry_after")
            .and_then(Value::as_i64)
            .map(|seconds| format!(" retry_after={seconds}s"))
            .unwrap_or_default();
        format!("Telegram API error [{method}] code={code}: {description}{retry_after}")
    }

    fn apply_delivery_context(payload: &mut Value, include_ephemeral: bool) {
        let context = Self::current_delivery_context();
        if payload.get("message_thread_id").is_none() {
            if let Some(thread_id) = context.message_thread_id {
                payload["message_thread_id"] = json!(thread_id);
            }
        }
        if include_ephemeral
            && payload.get("ephemeral_message_parameters").is_none()
            && context.receiver_user_id.is_some()
        {
            payload["ephemeral_message_parameters"] =
                serde_json::to_value(EphemeralMessageParameters {
                    receiver_user_id: context.receiver_user_id.unwrap_or_default(),
                    callback_query_id: context.callback_query_id.clone(),
                    replace_callback_query_message: Self::replace_callback_query_message(),
                })
                .unwrap_or(json!({}));
        }
        if include_ephemeral
            && payload.get("reply_parameters").is_none()
            && context.source_ephemeral_message_id.is_some()
        {
            payload["reply_parameters"] = serde_json::to_value(ReplyParameters::ephemeral(
                context.source_ephemeral_message_id.unwrap_or_default(),
            ))
            .unwrap_or(json!({}));
        }
    }

    fn apply_form_delivery_context(
        &self,
        mut form: Form,
        include_ephemeral: bool,
        receiver_override: Option<i64>,
        reply_to_message_id: Option<i64>,
    ) -> Result<Form, String> {
        let delivery = Self::current_delivery_context();
        if let Some(thread_id) = delivery.message_thread_id {
            form = form.text("message_thread_id", thread_id.to_string());
        }
        if include_ephemeral {
            if let Some(receiver_user_id) = receiver_override.or(delivery.receiver_user_id) {
                let ephemeral = serde_json::to_string(&EphemeralMessageParameters {
                    receiver_user_id,
                    callback_query_id: delivery.callback_query_id.clone(),
                    replace_callback_query_message: Self::replace_callback_query_message(),
                })
                .map_err(|error| error.to_string())?;
                form = form.text("ephemeral_message_parameters", ephemeral);
            }
            let reply_parameters = delivery
                .source_ephemeral_message_id
                .map(ReplyParameters::ephemeral)
                .or_else(|| reply_to_message_id.map(ReplyParameters::new));
            if let Some(reply_parameters) = reply_parameters {
                form = form.text(
                    "reply_parameters",
                    serde_json::to_string(&reply_parameters).map_err(|error| error.to_string())?,
                );
            }
        } else if let Some(reply_to_message_id) = reply_to_message_id {
            form = form.text(
                "reply_parameters",
                serde_json::to_string(&ReplyParameters::new(reply_to_message_id))
                    .map_err(|error| error.to_string())?,
            );
        }
        Ok(form)
    }

    async fn read_bounded_json(response: reqwest::Response) -> Result<Value, String> {
        let status = response.status();
        if response
            .content_length()
            .is_some_and(|length| length > MAX_TELEGRAM_RESPONSE_BYTES as u64)
        {
            return Err(format!(
                "HTTP {status}: response exceeded {MAX_TELEGRAM_RESPONSE_BYTES} bytes"
            ));
        }
        let mut stream = response.bytes_stream();
        let mut bytes = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|error| Self::reqwest_error_kind(&error).to_string())?;
            if bytes.len().saturating_add(chunk.len()) > MAX_TELEGRAM_RESPONSE_BYTES {
                return Err(format!(
                    "HTTP {status}: response exceeded {MAX_TELEGRAM_RESPONSE_BYTES} bytes"
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes)
            .map_err(|error| format!("HTTP {status} invalid JSON: {error}"))
    }

    fn reqwest_error_kind(error: &reqwest::Error) -> &'static str {
        if error.is_timeout() {
            "timeout"
        } else if error.is_connect() {
            "connection failure"
        } else if error.is_request() {
            "request failure"
        } else if error.is_body() {
            "body failure"
        } else if error.is_decode() {
            "decode failure"
        } else {
            "transport failure"
        }
    }

    async fn evaluate_response(
        method: &str,
        response: reqwest::Response,
        attempt: usize,
    ) -> HttpResponseOutcome {
        let status = response.status();
        let retry_after_header = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|h| h.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok());

        if status.is_server_error() || status.as_u16() == 429 {
            let retry_suffix = retry_after_header
                .map(|s| format!(" retry_after={s}s"))
                .unwrap_or_default();
            let error_msg = format!(
                "Telegram API error [{method}] code={}: HTTP {status}{retry_suffix}",
                status.as_u16()
            );
            if let Some(delay) =
                retry_delay_for_http_status(status.as_u16(), retry_after_header, attempt)
            {
                return HttpResponseOutcome::Retry {
                    delay,
                    error: error_msg,
                };
            }
            return HttpResponseOutcome::TerminalError(error_msg);
        }

        match Self::read_bounded_json(response).await {
            Ok(value) => {
                if let Some(delay) = retry_delay_from_response(&value, attempt) {
                    let error = Self::telegram_api_error(method, &value);
                    return HttpResponseOutcome::Retry { delay, error };
                }
                HttpResponseOutcome::Success(value)
            }
            Err(error) => {
                if let Some(delay) = retry_delay_from_error(&error, attempt) {
                    HttpResponseOutcome::Retry { delay, error }
                } else {
                    HttpResponseOutcome::TerminalError(error)
                }
            }
        }
    }

    fn evaluate_request_error(
        method: &str,
        error: &reqwest::Error,
        is_multipart: bool,
        attempt: usize,
    ) -> (Option<Duration>, String) {
        let normalized = if is_multipart {
            format!(
                "{method} multipart error: {}",
                Self::reqwest_error_kind(error)
            )
        } else {
            format!(
                "HTTP error for {method}: {}",
                Self::reqwest_error_kind(error)
            )
        };
        let delay = retry_delay_from_error(&normalized, attempt).or_else(|| {
            // A timed-out request may already have been executed by Telegram,
            // so only methods that are safe to repeat are retried; a timed-out
            // send is reported instead of risking a duplicate message.
            (error.is_timeout() && is_idempotent_method(method))
                .then(|| retry_delay_for_idempotent_timeout(attempt))
                .flatten()
        });
        (delay, normalized)
    }

    async fn post_json_raw(&self, method: &str, payload: Value) -> Result<Value, String> {
        let url = format!("{}/{method}", self.base_url);
        let mut last_error = None;
        for attempt in 0..MAX_TELEGRAM_ATTEMPTS {
            match self.client.post(&url).json(&payload).send().await {
                Ok(response) => match Self::evaluate_response(method, response, attempt).await {
                    HttpResponseOutcome::Success(value) => return Ok(value),
                    HttpResponseOutcome::Retry { delay, error } => {
                        warn!(
                            method,
                            attempt = attempt + 1,
                            delay_ms = delay.as_millis(),
                            "Telegram request is transiently rate-limited/unavailable; retrying"
                        );
                        last_error = Some(error);
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                    HttpResponseOutcome::TerminalError(error) => return Err(error),
                },
                Err(error) => {
                    let (delay, normalized) =
                        Self::evaluate_request_error(method, &error, false, attempt);
                    if let Some(delay) = delay {
                        last_error = Some(normalized);
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                    return Err(normalized);
                }
            }
        }
        Err(last_error.unwrap_or_else(|| format!("Telegram request [{method}] exhausted retries")))
    }

    async fn post_json(&self, method: &str, payload: Value) -> Result<Value, String> {
        let response = self.post_json_raw(method, payload).await?;
        if response.get("ok").and_then(Value::as_bool) == Some(true) {
            Ok(response)
        } else {
            Err(Self::telegram_api_error(method, &response))
        }
    }

    async fn post_multipart<F>(&self, method: &str, mut build: F) -> Result<Value, String>
    where
        F: FnMut() -> Result<Form, String>,
    {
        let url = format!("{}/{method}", self.base_url);
        let mut last_error = None;
        for attempt in 0..MAX_TELEGRAM_ATTEMPTS {
            let form = build()?;
            match self.client.post(&url).multipart(form).send().await {
                Ok(response) => match Self::evaluate_response(method, response, attempt).await {
                    HttpResponseOutcome::Success(value) => {
                        if value.get("ok").and_then(Value::as_bool) == Some(true) {
                            return Ok(value);
                        }
                        return Err(Self::telegram_api_error(method, &value));
                    }
                    HttpResponseOutcome::Retry { delay, error } => {
                        warn!(
                            method,
                            attempt = attempt + 1,
                            delay_ms = delay.as_millis(),
                            "Telegram request is transiently rate-limited/unavailable; retrying"
                        );
                        last_error = Some(error);
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                    HttpResponseOutcome::TerminalError(error) => return Err(error),
                },
                Err(error) => {
                    let (delay, normalized) =
                        Self::evaluate_request_error(method, &error, true, attempt);
                    if let Some(delay) = delay {
                        last_error = Some(normalized);
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                    return Err(normalized);
                }
            }
        }
        Err(last_error.unwrap_or_else(|| format!("Telegram request [{method}] exhausted retries")))
    }

    pub async fn get_me(&self) -> Result<ApiResponse<User>, String> {
        let value = self.post_json("getMe", json!({})).await?;
        serde_json::from_value(value).map_err(|error| error.to_string())
    }

    pub async fn get_file(&self, file_id: &str) -> Result<ApiResponse<FileInfo>, String> {
        let value = self
            .post_json("getFile", json!({"file_id": file_id}))
            .await?;
        serde_json::from_value(value).map_err(|error| error.to_string())
    }

    /// Downloads a file sent by the user. The error distinguishes "too big"
    /// (tell the user the limit) from "gone" and transient network failures,
    /// which previously all collapsed into the same `None`.
    pub async fn get_file_bytes(
        &self,
        file_id: &str,
    ) -> Result<(Vec<u8>, String), FileDownloadError> {
        let file_res = self
            .get_file(file_id)
            .await
            .map_err(|_| FileDownloadError::Network)?;
        if !file_res.ok {
            return Err(FileDownloadError::NotFound);
        }
        let info = file_res.result.ok_or(FileDownloadError::NotFound)?;
        if info
            .file_size
            .and_then(|size| usize::try_from(size).ok())
            .is_some_and(|size| size > MAX_TELEGRAM_DOWNLOAD_BYTES)
        {
            return Err(FileDownloadError::TooLarge);
        }
        // Telegram only exposes a path for files up to its 20 MB bot limit.
        let file_path = info.file_path.ok_or(FileDownloadError::TooLarge)?;
        // Derived from the API base URL so a self-hosted Bot API server is
        // used for downloads too, not the public api.telegram.org.
        let url = format!(
            "{}/{}",
            self.base_url.replacen("/bot", "/file/bot", 1),
            file_path
        );
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|_| FileDownloadError::Network)?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(FileDownloadError::NotFound);
        }
        if !response.status().is_success() {
            return Err(FileDownloadError::Network);
        }
        let mut stream = response.bytes_stream();
        let initial_capacity = info
            .file_size
            .and_then(|s| usize::try_from(s).ok())
            .unwrap_or(8192)
            .min(MAX_TELEGRAM_DOWNLOAD_BYTES);
        let mut bytes = Vec::with_capacity(initial_capacity);
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| FileDownloadError::Network)?;
            if bytes.len().saturating_add(chunk.len()) > MAX_TELEGRAM_DOWNLOAD_BYTES {
                return Err(FileDownloadError::TooLarge);
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok((bytes, file_path))
    }

    pub async fn get_updates(
        &self,
        offset: Option<i64>,
        limit: i32,
        timeout: i32,
        allowed_updates: Option<Vec<String>>,
    ) -> Result<ApiResponse<Vec<Update>>, String> {
        let mut payload = json!({"limit": limit, "timeout": timeout});
        if let Some(offset) = offset {
            payload["offset"] = json!(offset);
        }
        if let Some(allowed_updates) = allowed_updates {
            payload["allowed_updates"] = json!(allowed_updates);
        }
        let value = self.post_json("getUpdates", payload).await?;
        serde_json::from_value(value).map_err(|error| error.to_string())
    }

    pub async fn get_chat_member(
        &self,
        chat_id: i64,
        user_id: i64,
    ) -> Result<ApiResponse<ChatMember>, String> {
        let payload = json!({
            "chat_id": chat_id,
            "user_id": user_id,
        });
        let value = self.post_json("getChatMember", payload).await?;
        serde_json::from_value(value).map_err(|error| error.to_string())
    }

    pub async fn send_message(
        &self,
        chat_id: i64,
        text: &str,
        parse_mode: Option<&str>,
        reply_markup: Option<Value>,
        receiver_user_id: Option<i64>,
        reply_to_message_id: Option<i64>,
    ) -> Result<Value, String> {
        let chunks = if text.chars().count() > TELEGRAM_TEXT_SPLIT_THRESHOLD_CHARS {
            self.inner
                .split_text_chunks(text, TELEGRAM_TEXT_CHUNK_CHARS)
        } else {
            vec![text.to_string()]
        };
        let total = chunks.len();
        let mut last = json!({"ok": true});
        for (index, chunk) in chunks.into_iter().enumerate() {
            let mut payload = json!({"chat_id": chat_id, "text": chunk});
            if let Some(parse_mode) = parse_mode {
                payload["parse_mode"] = json!(parse_mode);
            }
            if index + 1 == total {
                if let Some(ref reply_markup) = reply_markup {
                    payload["reply_markup"] = reply_markup.clone();
                }
            }
            if let Some(receiver_user_id) = receiver_user_id {
                payload["ephemeral_message_parameters"] =
                    serde_json::to_value(EphemeralMessageParameters {
                        receiver_user_id,
                        callback_query_id: Self::current_delivery_context().callback_query_id,
                        replace_callback_query_message: Self::replace_callback_query_message(),
                    })
                    .unwrap_or(json!({}));
            }
            if index == 0 {
                if let Some(reply_to_message_id) = reply_to_message_id {
                    payload["reply_parameters"] =
                        serde_json::to_value(ReplyParameters::new(reply_to_message_id))
                            .unwrap_or(json!({}));
                }
            }
            Self::apply_delivery_context(&mut payload, true);
            last = self.post_json("sendMessage", payload).await?;
        }
        Ok(last)
    }

    pub async fn send_poll(
        &self,
        chat_id: i64,
        poll: &PollRequest<'_>,
        reply_to_message_id: Option<i64>,
    ) -> Result<Value, String> {
        // Bot API defaults `type` to "regular"; a quiz is only requested
        // explicitly (it would be rejected without a correct answer anyway).
        let mut payload = json!({
            "chat_id": chat_id,
            "question": poll.question,
            "options": poll.options,
            "type": poll.poll_type.unwrap_or("regular"),
        });
        if let Some(anon) = poll.is_anonymous {
            payload["is_anonymous"] = json!(anon);
        }
        if !poll.correct_option_ids.is_empty() {
            // Bot API 9.6 replaced `correct_option_id` with the array
            // `correct_option_ids`; a quiz with several correct answers must
            // also let players pick several.
            payload["correct_option_ids"] = json!(poll.correct_option_ids);
            if poll.correct_option_ids.len() > 1 {
                payload["allows_multiple_answers"] = json!(true);
            }
        }
        if poll.shuffle_options {
            payload["shuffle_options"] = json!(true);
        }
        if let Some(media) = poll.media.as_ref() {
            payload["media"] = media.clone();
        }
        if let Some(description) = poll.description.map(str::trim).filter(|s| !s.is_empty()) {
            payload["description"] = json!(description);
        }
        if let Some(media) = poll.explanation_media.as_ref() {
            payload["explanation_media"] = media.clone();
        }
        if poll.allows_revoting {
            payload["allows_revoting"] = json!(true);
        }
        if let Some(open_period) = poll.open_period {
            payload["open_period"] = json!(open_period);
        }
        if poll.hide_results_until_closes {
            payload["hide_results_until_closes"] = json!(true);
        }
        if let Some(exp) = poll.explanation.map(str::trim).filter(|s| !s.is_empty()) {
            payload["explanation"] = json!(exp);
            if let Some(pm) = poll.explanation_parse_mode {
                payload["explanation_parse_mode"] = json!(pm);
            }
        }
        if let Some(rep) = reply_to_message_id {
            payload["reply_parameters"] =
                serde_json::to_value(ReplyParameters::new(rep)).unwrap_or(json!({}));
        }
        Self::apply_delivery_context(&mut payload, true);
        self.post_json("sendPoll", payload).await
    }

    pub async fn send_photo_bytes(
        &self,
        chat_id: i64,
        photo_bytes: Vec<u8>,
        caption: Option<&str>,
        parse_mode: Option<&str>,
        reply_markup: Option<Value>,
        reply_to_message_id: Option<i64>,
    ) -> Result<Value, String> {
        let Some((file_name, mime_type)) = image_kind(&photo_bytes) else {
            return Err("sendPhoto rejected bytes with an unsupported image signature".to_string());
        };
        let caption = caption.map(str::to_string);
        let parse_mode = parse_mode.map(str::to_string);
        self.post_multipart("sendPhoto", || {
            let part = Part::bytes(photo_bytes.clone())
                .file_name(file_name)
                .mime_str(mime_type)
                .map_err(|error| error.to_string())?;
            let mut form = Form::new()
                .text("chat_id", chat_id.to_string())
                .part("photo", part);
            if let Some(ref caption) = caption {
                form = form.text("caption", caption.clone());
            }
            if let Some(ref parse_mode) = parse_mode {
                form = form.text("parse_mode", parse_mode.clone());
            }
            if let Some(ref reply_markup) = reply_markup {
                form = form.text("reply_markup", reply_markup.to_string());
            }
            self.apply_form_delivery_context(form, true, None, reply_to_message_id)
        })
        .await
    }

    /// Uploads a live photo (Bot API 10.0 `sendLivePhoto`): a short video and
    /// its still photo. Telegram does not accept live photos by URL, so both
    /// files are always uploaded.
    pub async fn send_live_photo(
        &self,
        chat_id: i64,
        video_bytes: Vec<u8>,
        photo_bytes: Vec<u8>,
        caption: Option<&str>,
        reply_to_message_id: Option<i64>,
    ) -> Result<Value, String> {
        let Some((photo_name, photo_mime)) =
            image_kind(&photo_bytes).filter(|(_, mime)| *mime != "image/gif")
        else {
            return Err("sendLivePhoto needs a JPEG, PNG or WEBP photo".to_string());
        };
        let caption = caption.map(str::to_string);
        self.post_multipart("sendLivePhoto", || {
            let video = Part::bytes(video_bytes.clone())
                .file_name("live.mp4")
                .mime_str("video/mp4")
                .map_err(|error| error.to_string())?;
            let photo = Part::bytes(photo_bytes.clone())
                .file_name(photo_name)
                .mime_str(photo_mime)
                .map_err(|error| error.to_string())?;
            let mut form = Form::new()
                .text("chat_id", chat_id.to_string())
                .part("live_photo", video)
                .part("photo", photo);
            if let Some(ref caption) = caption {
                form = form.text("caption", caption.clone());
            }
            self.apply_form_delivery_context(form, true, None, reply_to_message_id)
        })
        .await
    }

    pub async fn send_photo(
        &self,
        chat_id: i64,
        photo: &str,
        caption: Option<&str>,
        parse_mode: Option<&str>,
        reply_markup: Option<Value>,
        reply_to_message_id: Option<i64>,
    ) -> Result<Value, String> {
        let mut payload = json!({"chat_id": chat_id, "photo": photo});
        Self::add_caption_fields(&mut payload, caption, parse_mode, reply_markup.as_ref());
        if let Some(reply_to_message_id) = reply_to_message_id {
            payload["reply_parameters"] =
                serde_json::to_value(ReplyParameters::new(reply_to_message_id))
                    .unwrap_or(json!({}));
        }
        Self::apply_delivery_context(&mut payload, true);
        match self.post_json("sendPhoto", payload).await {
            Ok(value) => Ok(value),
            Err(error)
                if fallback_allowed_error(&error)
                    && (photo.starts_with("http://") || photo.starts_with("https://")) =>
            {
                let Some((bytes, _, _)) = self
                    .download_media_bytes(photo, MAX_TELEGRAM_DOWNLOAD_BYTES)
                    .await
                else {
                    return Err(error);
                };
                self.send_photo_bytes(
                    chat_id,
                    bytes,
                    caption,
                    parse_mode,
                    reply_markup,
                    reply_to_message_id,
                )
                .await
            }
            Err(error) => Err(error),
        }
    }

    pub async fn send_media_group(
        &self,
        chat_id: i64,
        media: &[InputMedia],
        reply_to_message_id: Option<i64>,
    ) -> Result<Value, String> {
        InputMedia::validate_media_group(media)?;
        let mut payload = json!({
            "chat_id": chat_id,
            "media": serde_json::to_value(media).map_err(|error| error.to_string())?,
        });
        if let Some(reply_to_message_id) = reply_to_message_id {
            payload["reply_parameters"] =
                serde_json::to_value(ReplyParameters::new(reply_to_message_id))
                    .unwrap_or(json!({}));
        }
        Self::apply_delivery_context(&mut payload, false);
        match self.post_json("sendMediaGroup", payload).await {
            Ok(value) => Ok(value),
            Err(error) if fallback_allowed_error(&error) => {
                let mut updated = media.to_vec();
                let mut attachments = Vec::new();
                for (index, item) in updated.iter_mut().enumerate() {
                    let source = Self::media_reference(item).to_string();
                    if source.starts_with("http://") || source.starts_with("https://") {
                        if let Some((bytes, mime, file_name)) = self
                            .download_media_bytes(&source, MAX_TELEGRAM_DOWNLOAD_BYTES)
                            .await
                        {
                            let attach_name = format!("file_{index}");
                            Self::set_media_reference(item, format!("attach://{attach_name}"));
                            attachments.push((attach_name, bytes, mime, file_name));
                        }
                    }
                }
                if attachments.is_empty() {
                    return Err(error);
                }
                self.post_multipart("sendMediaGroup", || {
                    let mut form = Form::new().text("chat_id", chat_id.to_string()).text(
                        "media",
                        serde_json::to_string(&updated).map_err(|error| error.to_string())?,
                    );
                    form =
                        self.apply_form_delivery_context(form, false, None, reply_to_message_id)?;
                    for (attach_name, bytes, mime, file_name) in &attachments {
                        let part = Part::bytes(bytes.clone())
                            .file_name(sanitize_upload_filename(file_name))
                            .mime_str(mime)
                            .map_err(|error| error.to_string())?;
                        form = form.part(attach_name.clone(), part);
                    }
                    Ok(form)
                })
                .await
            }
            Err(error) => Err(error),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn send_audio(
        &self,
        chat_id: i64,
        audio: &str,
        caption: Option<&str>,
        parse_mode: Option<&str>,
        title: Option<&str>,
        performer: Option<&str>,
        duration: Option<i32>,
        reply_markup: Option<Value>,
        reply_to_message_id: Option<i64>,
    ) -> Result<Value, String> {
        let mut extras = Vec::new();
        if let Some(title) = title {
            extras.push(("title", title.to_string()));
        }
        if let Some(performer) = performer {
            extras.push(("performer", performer.to_string()));
        }
        if let Some(duration) = duration {
            extras.push(("duration", duration.to_string()));
        }
        self.send_media_with_url_fallback(
            "sendAudio",
            "audio",
            chat_id,
            audio,
            caption,
            parse_mode,
            reply_markup,
            reply_to_message_id,
            extras,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn send_voice(
        &self,
        chat_id: i64,
        voice: &str,
        caption: Option<&str>,
        parse_mode: Option<&str>,
        duration: Option<i32>,
        reply_markup: Option<Value>,
        reply_to_message_id: Option<i64>,
    ) -> Result<Value, String> {
        let extras = duration
            .map(|duration| vec![("duration", duration.to_string())])
            .unwrap_or_default();
        self.send_media_with_url_fallback(
            "sendVoice",
            "voice",
            chat_id,
            voice,
            caption,
            parse_mode,
            reply_markup,
            reply_to_message_id,
            extras,
        )
        .await
    }

    pub async fn send_video(
        &self,
        chat_id: i64,
        video: &str,
        caption: Option<&str>,
        parse_mode: Option<&str>,
        reply_markup: Option<Value>,
        reply_to_message_id: Option<i64>,
    ) -> Result<Value, String> {
        self.send_media_with_url_fallback(
            "sendVideo",
            "video",
            chat_id,
            video,
            caption,
            parse_mode,
            reply_markup,
            reply_to_message_id,
            Vec::new(),
        )
        .await
    }

    pub async fn send_animation(
        &self,
        chat_id: i64,
        animation: &str,
        caption: Option<&str>,
        parse_mode: Option<&str>,
        reply_markup: Option<Value>,
        reply_to_message_id: Option<i64>,
    ) -> Result<Value, String> {
        self.send_media_with_url_fallback(
            "sendAnimation",
            "animation",
            chat_id,
            animation,
            caption,
            parse_mode,
            reply_markup,
            reply_to_message_id,
            Vec::new(),
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn send_document_bytes(
        &self,
        chat_id: i64,
        filename: &str,
        bytes: Vec<u8>,
        mime_type: Option<&str>,
        caption: Option<&str>,
        parse_mode: Option<&str>,
        reply_markup: Option<Value>,
        reply_to_message_id: Option<i64>,
    ) -> Result<Value, String> {
        // Uses the shared multipart transport: bounded response reading and
        // the same retry policy as every other Telegram call.
        let filename = sanitize_upload_filename(filename);
        let caption = caption.map(str::to_string);
        let parse_mode = parse_mode.map(str::to_string);
        let mime_type = mime_type.map(str::to_string);
        self.post_multipart("sendDocument", || {
            let mut part = Part::bytes(bytes.clone()).file_name(filename.clone());
            if let Some(ref mime) = mime_type {
                part = part
                    .mime_str(mime)
                    .map_err(|e| format!("MIME type tidak valid: {e}"))?;
            }
            let mut form = Form::new()
                .text("chat_id", chat_id.to_string())
                .part("document", part);
            if let Some(ref cap) = caption {
                form = form.text("caption", cap.clone());
            }
            if let Some(ref pm) = parse_mode {
                form = form.text("parse_mode", pm.clone());
            }
            if let Some(ref rm) = reply_markup {
                form = form.text("reply_markup", rm.to_string());
            }
            self.apply_form_delivery_context(form, true, None, reply_to_message_id)
        })
        .await
    }

    pub async fn send_document(
        &self,
        chat_id: i64,
        document: &str,
        caption: Option<&str>,
        parse_mode: Option<&str>,
        reply_markup: Option<Value>,
        reply_to_message_id: Option<i64>,
    ) -> Result<Value, String> {
        self.send_media_with_url_fallback(
            "sendDocument",
            "document",
            chat_id,
            document,
            caption,
            parse_mode,
            reply_markup,
            reply_to_message_id,
            Vec::new(),
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn send_media_with_url_fallback(
        &self,
        method: &str,
        field: &str,
        chat_id: i64,
        media: &str,
        caption: Option<&str>,
        parse_mode: Option<&str>,
        reply_markup: Option<Value>,
        reply_to_message_id: Option<i64>,
        extras: Vec<(&'static str, String)>,
    ) -> Result<Value, String> {
        let mut payload = json!({"chat_id": chat_id});
        payload[field] = json!(media);
        Self::add_caption_fields(&mut payload, caption, parse_mode, reply_markup.as_ref());
        for (key, value) in &extras {
            payload[*key] = json!(value);
        }
        if let Some(reply_to_message_id) = reply_to_message_id {
            payload["reply_parameters"] =
                serde_json::to_value(ReplyParameters::new(reply_to_message_id))
                    .unwrap_or(json!({}));
        }
        Self::apply_delivery_context(&mut payload, true);
        match self.post_json(method, payload).await {
            Ok(value) => Ok(value),
            Err(error)
                if fallback_allowed_error(&error)
                    && (media.starts_with("http://") || media.starts_with("https://")) =>
            {
                let Some((bytes, mime, file_name)) = self
                    .download_media_bytes(media, MAX_TELEGRAM_DOWNLOAD_BYTES)
                    .await
                else {
                    return Err(error);
                };
                let caption = caption.map(str::to_string);
                let parse_mode = parse_mode.map(str::to_string);
                self.post_multipart(method, || {
                    let part = Part::bytes(bytes.clone())
                        .file_name(sanitize_upload_filename(&file_name))
                        .mime_str(&mime)
                        .map_err(|error| error.to_string())?;
                    let mut form = Form::new()
                        .text("chat_id", chat_id.to_string())
                        .part(field.to_string(), part);
                    if let Some(ref caption) = caption {
                        form = form.text("caption", caption.clone());
                    }
                    if let Some(ref parse_mode) = parse_mode {
                        form = form.text("parse_mode", parse_mode.clone());
                    }
                    if let Some(ref reply_markup) = reply_markup {
                        form = form.text("reply_markup", reply_markup.to_string());
                    }
                    for (key, value) in &extras {
                        form = form.text((*key).to_string(), value.clone());
                    }
                    self.apply_form_delivery_context(form, true, None, reply_to_message_id)
                })
                .await
            }
            Err(error) => Err(error),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn send_location(
        &self,
        chat_id: i64,
        latitude: f64,
        longitude: f64,
        horizontal_accuracy: Option<f64>,
        live_period: Option<i32>,
        reply_markup: Option<Value>,
        reply_to_message_id: Option<i64>,
    ) -> Result<Value, String> {
        if !latitude.is_finite() || !(-90.0..=90.0).contains(&latitude) {
            return Err(format!(
                "Invalid latitude: must be finite number between -90.0 and 90.0, found {latitude}"
            ));
        }
        if !longitude.is_finite() || !(-180.0..=180.0).contains(&longitude) {
            return Err(format!(
                "Invalid longitude: must be finite number between -180.0 and 180.0, found {longitude}"
            ));
        }
        let mut payload = json!({
            "chat_id": chat_id,
            "latitude": latitude,
            "longitude": longitude,
        });
        if let Some(horizontal_accuracy) = horizontal_accuracy {
            payload["horizontal_accuracy"] = json!(horizontal_accuracy);
        }
        if let Some(live_period) = live_period {
            payload["live_period"] = json!(live_period);
        }
        if let Some(reply_markup) = reply_markup {
            payload["reply_markup"] = reply_markup;
        }
        if let Some(reply_to_message_id) = reply_to_message_id {
            payload["reply_parameters"] =
                serde_json::to_value(ReplyParameters::new(reply_to_message_id))
                    .unwrap_or(json!({}));
        }
        Self::apply_delivery_context(&mut payload, true);
        self.post_json("sendLocation", payload).await
    }

    pub async fn edit_message_text(
        &self,
        chat_id: Option<i64>,
        message_id: Option<i64>,
        text: &str,
        parse_mode: Option<&str>,
        reply_markup: Option<Value>,
    ) -> Result<Value, String> {
        let delivery = Self::current_delivery_context();
        let mut payload = json!({"text": text});
        let method = if let (Some(chat_id), Some((receiver_user_id, ephemeral_message_id))) = (
            chat_id,
            delivery
                .receiver_user_id
                .zip(delivery.source_ephemeral_message_id),
        ) {
            payload["chat_id"] = json!(chat_id);
            payload["receiver_user_id"] = json!(receiver_user_id);
            payload["ephemeral_message_id"] = json!(ephemeral_message_id);
            "editEphemeralMessageText"
        } else {
            if let Some(chat_id) = chat_id {
                payload["chat_id"] = json!(chat_id);
            }
            if let Some(message_id) = message_id {
                payload["message_id"] = json!(message_id);
            }
            "editMessageText"
        };
        if let Some(parse_mode) = parse_mode {
            payload["parse_mode"] = json!(parse_mode);
        }
        if let Some(reply_markup) = reply_markup {
            payload["reply_markup"] = reply_markup;
        }
        self.post_json(method, payload).await
    }

    pub async fn edit_rich_message(
        &self,
        chat_id: i64,
        message_id: i64,
        rich_message: &InputRichMessage,
        reply_markup: Option<Value>,
    ) -> Result<Value, String> {
        rich_message.validate()?;
        let delivery = Self::current_delivery_context();
        let rich_json = serde_json::to_value(rich_message).map_err(|error| error.to_string())?;
        let (method, mut payload) = if let Some((receiver_user_id, ephemeral_message_id)) = delivery
            .receiver_user_id
            .zip(delivery.source_ephemeral_message_id)
        {
            (
                "editEphemeralMessageText",
                json!({
                    "chat_id": chat_id,
                    "receiver_user_id": receiver_user_id,
                    "ephemeral_message_id": ephemeral_message_id,
                    "rich_message": rich_json,
                }),
            )
        } else {
            (
                "editMessageText",
                json!({"chat_id": chat_id, "message_id": message_id, "rich_message": rich_json}),
            )
        };
        if let Some(ref reply_markup) = reply_markup {
            payload["reply_markup"] = reply_markup.clone();
        }
        let response = self.post_json_raw(method, payload).await?;
        if response.get("ok").and_then(Value::as_bool) == Some(true)
            || response
                .get("description")
                .and_then(Value::as_str)
                .is_some_and(|description| {
                    description
                        .to_ascii_lowercase()
                        .contains("message is not modified")
                })
        {
            return Ok(response);
        }
        if !fallback_allowed_response(&response) {
            return Err(Self::telegram_api_error(method, &response));
        }
        let html = self.inner.render_blocks_to_html(&rich_message.blocks);
        self.edit_message_text(
            Some(chat_id),
            Some(message_id),
            &html,
            Some("HTML"),
            reply_markup,
        )
        .await
    }

    pub async fn edit_ephemeral_message_media(
        &self,
        chat_id: i64,
        receiver_user_id: i64,
        ephemeral_message_id: i64,
        media: &InputMedia,
        reply_markup: Option<Value>,
    ) -> Result<Value, String> {
        let mut payload = json!({
            "chat_id": chat_id,
            "receiver_user_id": receiver_user_id,
            "ephemeral_message_id": ephemeral_message_id,
            "media": serde_json::to_value(media).map_err(|error| error.to_string())?,
        });
        if let Some(reply_markup) = reply_markup {
            payload["reply_markup"] = reply_markup;
        }
        self.post_json("editEphemeralMessageMedia", payload).await
    }

    pub async fn edit_message_media(
        &self,
        chat_id: i64,
        message_id: i64,
        media: InputMedia,
        reply_markup: Option<InlineKeyboardMarkup>,
    ) -> Result<Value, String> {
        let media_json = serde_json::to_value(&media).map_err(|e| e.to_string())?;
        let mut payload = json!({
            "chat_id": chat_id,
            "message_id": message_id,
            "media": media_json,
        });
        if let Some(rm) = reply_markup {
            payload["reply_markup"] = serde_json::to_value(rm).map_err(|e| e.to_string())?;
        }

        match self.post_json("editMessageMedia", payload).await {
            Ok(res) => Ok(res),
            Err(e) if e.to_ascii_lowercase().contains("message is not modified") => Ok(json!({
                "ok": true,
                "result": true,
                "description": "message is not modified"
            })),
            Err(e) => Err(e),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn edit_ephemeral_message_media_bytes(
        &self,
        chat_id: i64,
        receiver_user_id: i64,
        ephemeral_message_id: i64,
        media: &InputMedia,
        bytes: Vec<u8>,
        file_name: &str,
        mime: &str,
        reply_markup: Option<Value>,
    ) -> Result<Value, String> {
        let mut attached_media = media.clone();
        Self::set_media_reference(&mut attached_media, "attach://media".to_string());
        let media_json =
            serde_json::to_string(&attached_media).map_err(|error| error.to_string())?;
        let file_name = file_name.to_string();
        let mime = mime.to_string();
        self.post_multipart("editEphemeralMessageMedia", || {
            let part = Part::bytes(bytes.clone())
                .file_name(sanitize_upload_filename(&file_name))
                .mime_str(&mime)
                .map_err(|error| error.to_string())?;
            let mut form = Form::new()
                .text("chat_id", chat_id.to_string())
                .text("receiver_user_id", receiver_user_id.to_string())
                .text("ephemeral_message_id", ephemeral_message_id.to_string())
                .text("media", media_json.clone())
                .part("media", part);
            if let Some(ref reply_markup) = reply_markup {
                form = form.text("reply_markup", reply_markup.to_string());
            }
            Ok(form)
        })
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn edit_ephemeral_message_caption(
        &self,
        chat_id: i64,
        receiver_user_id: i64,
        ephemeral_message_id: i64,
        caption: Option<&str>,
        parse_mode: Option<&str>,
        show_caption_above_media: Option<bool>,
        reply_markup: Option<Value>,
    ) -> Result<Value, String> {
        let mut payload = json!({
            "chat_id": chat_id,
            "receiver_user_id": receiver_user_id,
            "ephemeral_message_id": ephemeral_message_id,
        });
        if let Some(caption) = caption {
            payload["caption"] = json!(caption);
        }
        if let Some(parse_mode) = parse_mode {
            payload["parse_mode"] = json!(parse_mode);
        }
        if let Some(show_caption_above_media) = show_caption_above_media {
            payload["show_caption_above_media"] = json!(show_caption_above_media);
        }
        if let Some(reply_markup) = reply_markup {
            payload["reply_markup"] = reply_markup;
        }
        self.post_json("editEphemeralMessageCaption", payload).await
    }

    pub async fn edit_ephemeral_message_reply_markup(
        &self,
        chat_id: i64,
        receiver_user_id: i64,
        ephemeral_message_id: i64,
        reply_markup: Option<Value>,
    ) -> Result<Value, String> {
        let mut payload = json!({
            "chat_id": chat_id,
            "receiver_user_id": receiver_user_id,
            "ephemeral_message_id": ephemeral_message_id,
        });
        if let Some(reply_markup) = reply_markup {
            payload["reply_markup"] = reply_markup;
        }
        self.post_json("editEphemeralMessageReplyMarkup", payload)
            .await
    }

    pub async fn send_rich_message_with_media(
        &self,
        chat_id: i64,
        rich_message: &InputRichMessage,
        attached_files: Vec<StagedDocument>,
        reply_markup: Option<Value>,
        receiver_user_id: Option<i64>,
    ) -> Result<Value, String> {
        self.send_rich_message_with_media_params(
            chat_id,
            rich_message,
            attached_files,
            reply_markup,
            receiver_user_id,
            None,
        )
        .await
    }

    /// Sends a rich message together with staged files and whatever remote
    /// media can be downloaded for upload. When Telegram refuses it, the
    /// message is retried in simpler forms rather than lost: first the media
    /// that could not be downloaded becomes links, then all remote media,
    /// and finally the text goes out on its own with the staged files sent
    /// as ordinary documents. Only a 400 leads to another form; any other
    /// failure is returned, since a retry could deliver the message twice.
    pub async fn send_rich_message_with_media_params(
        &self,
        chat_id: i64,
        rich_message: &InputRichMessage,
        attached_files: Vec<StagedDocument>,
        reply_markup: Option<Value>,
        receiver_user_id: Option<i64>,
        reply_to_message_id: Option<i64>,
    ) -> Result<Value, String> {
        if attached_files.is_empty() && !rich_message.has_media() {
            return self
                .send_rich_message(
                    chat_id,
                    rich_message,
                    reply_markup,
                    receiver_user_id,
                    reply_to_message_id,
                )
                .await;
        }
        let target = RichTarget {
            chat_id,
            reply_markup: reply_markup.as_ref(),
            receiver_user_id,
            reply_to_message_id,
        };
        if let Err(error) = rich_message.validate() {
            warn!("Media answer is not a valid rich message ({error}); sending text and files separately.");
            return self
                .send_rich_without_uploads(target, rich_message, attached_files)
                .await;
        }

        let staged = self.stage_remote_media(rich_message, &attached_files).await;
        let uploads: Vec<&StagedDocument> =
            attached_files.iter().chain(&staged.downloads).collect();

        // Downloaded media goes up with the message; Telegram fetches the
        // addresses that could not be downloaded here.
        let mut reason = match self.attempt_rich(target, &staged.message, &uploads).await {
            RichAttempt::Sent(response) => return Ok(response),
            RichAttempt::Failed(error) => return Err(error),
            RichAttempt::Rejected(reason) => reason,
        };

        if !staged.unresolved.is_empty() {
            warn!(
                "Media answer rejected ({reason}); linking {} media that could not be downloaded.",
                staged.unresolved.len()
            );
            let linked = self
                .inner
                .convert_media_to_rich_links(&staged.message, &|url| {
                    staged.unresolved.contains(url)
                });
            reason = match self.attempt_rich(target, &linked, &uploads).await {
                RichAttempt::Sent(response) => return Ok(response),
                RichAttempt::Failed(error) => return Err(error),
                RichAttempt::Rejected(reason) => reason,
            };
        }

        // An upload was refused (for example a file Telegram cannot process):
        // all remote media becomes links, the staged files stay.
        if !staged.downloads.is_empty() {
            warn!("Media answer rejected ({reason}); linking all remote media.");
            let linked = self.inner.convert_remote_media_to_rich_links(rich_message);
            let uploads: Vec<&StagedDocument> = attached_files.iter().collect();
            reason = match self.attempt_rich(target, &linked, &uploads).await {
                RichAttempt::Sent(response) => return Ok(response),
                RichAttempt::Failed(error) => return Err(error),
                RichAttempt::Rejected(reason) => reason,
            };
        }

        warn!("Media answer rejected ({reason}); sending text and files separately.");
        self.send_rich_without_uploads(target, rich_message, attached_files)
            .await
    }

    /// Last resort for a media answer Telegram refused: the text goes out with
    /// links in place of media (degrading to HTML and plain text if needed),
    /// and the staged files follow as ordinary documents.
    async fn send_rich_without_uploads(
        &self,
        target: RichTarget<'_>,
        rich_message: &InputRichMessage,
        attached_files: Vec<StagedDocument>,
    ) -> Result<Value, String> {
        let placeholder = if attached_files.is_empty() {
            "📎 Lampiran tidak dapat ditampilkan."
        } else {
            "📎 Lampiran dikirim terpisah."
        };
        let text = self.media_as_links(rich_message, placeholder);
        let sent = self
            .send_rich_message(
                target.chat_id,
                &text,
                target.reply_markup.cloned(),
                target.receiver_user_id,
                target.reply_to_message_id,
            )
            .await?;
        for file in attached_files {
            if let Err(error) = self
                .send_document_bytes(
                    target.chat_id,
                    &file.filename,
                    file.bytes,
                    Some(&file.mime_type),
                    None,
                    None,
                    None,
                    None,
                )
                .await
            {
                warn!("A staged file could not be sent separately: {error}");
            }
        }
        Ok(sent)
    }

    /// Downloads the remote media of `rich_message` for upload, a few at a
    /// time, and points the message at the uploads. `taken` holds files
    /// whose attach keys are already in use.
    async fn stage_remote_media(
        &self,
        rich_message: &InputRichMessage,
        taken: &[StagedDocument],
    ) -> StagedRichMessage {
        let mut remote: Vec<String> = Vec::new();
        for url in rich_message.collect_media_urls() {
            if raw::is_remote_url(&url) && !remote.contains(&url) {
                remote.push(url);
            }
        }
        // Only a document may legitimately be a web page.
        let documents: HashSet<String> = rich_message
            .blocks
            .iter()
            .filter(|block| matches!(block, RichBlock::Document { .. }))
            .flat_map(RichBlock::get_media_urls)
            .collect();
        let fetched: Vec<_> = futures_util::stream::iter(remote)
            .map(|url| async move {
                let download = self
                    .download_media_bytes(&url, MAX_TELEGRAM_DOWNLOAD_BYTES)
                    .await;
                (url, download)
            })
            .buffered(MEDIA_DOWNLOAD_CONCURRENCY)
            .collect()
            .await;

        let mut staged = StagedRichMessage {
            message: rich_message.clone(),
            downloads: Vec::new(),
            unresolved: HashSet::new(),
        };
        let mut replacements = HashMap::new();
        let mut next_key = 0usize;
        for (url, download) in fetched {
            match download {
                Some((bytes, content_type, file_name))
                    if documents.contains(&url) || !is_web_page(&content_type, &bytes) =>
                {
                    let attach_key = loop {
                        let key = format!("file_{next_key}");
                        next_key += 1;
                        if !taken
                            .iter()
                            .chain(&staged.downloads)
                            .any(|file| file.attach_key == key)
                        {
                            break key;
                        }
                    };
                    replacements.insert(url, format!("attach://{attach_key}"));
                    staged.downloads.push(StagedDocument::new(
                        attach_key,
                        bytes,
                        content_type,
                        file_name,
                    ));
                }
                _ => {
                    staged.unresolved.insert(url);
                }
            }
        }
        if !replacements.is_empty() {
            staged
                .message
                .replace_media_urls(&|url| replacements.get(url).cloned());
        }
        staged
    }

    /// One `sendRichMessage` attempt: multipart with `uploads`, or plain JSON
    /// when there is nothing to upload.
    async fn attempt_rich(
        &self,
        target: RichTarget<'_>,
        rich_message: &InputRichMessage,
        uploads: &[&StagedDocument],
    ) -> RichAttempt {
        if uploads.is_empty() {
            self.attempt_rich_json(target, rich_message).await
        } else {
            self.attempt_rich_multipart(target, rich_message, uploads)
                .await
        }
    }

    async fn attempt_rich_json(
        &self,
        target: RichTarget<'_>,
        rich_message: &InputRichMessage,
    ) -> RichAttempt {
        let rich_json = match serde_json::to_value(rich_message) {
            Ok(value) => value,
            Err(error) => return RichAttempt::Failed(error.to_string()),
        };
        // Any `media` list is already part of `rich_message`.
        let mut payload = json!({
            "chat_id": target.chat_id,
            "rich_message": rich_json,
        });
        if let Some(reply_markup) = target.reply_markup {
            payload["reply_markup"] = reply_markup.clone();
        }
        if let Some(receiver_user_id) = target.receiver_user_id {
            payload["ephemeral_message_parameters"] =
                serde_json::to_value(EphemeralMessageParameters {
                    receiver_user_id,
                    callback_query_id: Self::current_delivery_context().callback_query_id,
                    replace_callback_query_message: None,
                })
                .unwrap_or(json!({}));
        }
        if let Some(reply_to_message_id) = target.reply_to_message_id {
            payload["reply_parameters"] =
                serde_json::to_value(ReplyParameters::new(reply_to_message_id))
                    .unwrap_or(json!({}));
        }
        Self::apply_delivery_context(&mut payload, true);
        Self::rich_attempt(self.post_json_raw("sendRichMessage", payload).await)
    }

    async fn attempt_rich_multipart(
        &self,
        target: RichTarget<'_>,
        rich_message: &InputRichMessage,
        uploads: &[&StagedDocument],
    ) -> RichAttempt {
        // `media` travels inside `rich_message` (InputRichMessage.media);
        // sendRichMessage has no top-level `media` parameter.
        let rich_json = match serde_json::to_string(rich_message) {
            Ok(value) => value,
            Err(error) => return RichAttempt::Failed(error.to_string()),
        };
        let result = self
            .post_multipart("sendRichMessage", || {
                let mut form = Form::new()
                    .text("chat_id", target.chat_id.to_string())
                    .text("rich_message", rich_json.clone());
                if let Some(reply_markup) = target.reply_markup {
                    form = form.text("reply_markup", reply_markup.to_string());
                }
                form = self.apply_form_delivery_context(
                    form,
                    true,
                    target.receiver_user_id,
                    target.reply_to_message_id,
                )?;
                for file in uploads {
                    form = form.part(file.attach_key.clone(), upload_part(file));
                }
                Ok(form)
            })
            .await;
        Self::rich_attempt(result)
    }

    fn rich_attempt(result: Result<Value, String>) -> RichAttempt {
        match result {
            Ok(response) if response.get("ok").and_then(Value::as_bool) == Some(true) => {
                RichAttempt::Sent(response)
            }
            Ok(response) if fallback_allowed_response(&response) => RichAttempt::Rejected(
                response
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_string(),
            ),
            Ok(response) => {
                RichAttempt::Failed(Self::telegram_api_error("sendRichMessage", &response))
            }
            Err(error) if fallback_allowed_error(&error) => RichAttempt::Rejected(error),
            Err(error) => RichAttempt::Failed(error),
        }
    }

    pub async fn delete_message(&self, chat_id: i64, message_id: i64) -> Result<Value, String> {
        let delivery = Self::current_delivery_context();
        if let Some(receiver_user_id) = delivery.receiver_user_id {
            if let Some(source_id) = delivery.source_ephemeral_message_id {
                if message_id == source_id {
                    return self
                        .delete_ephemeral_message(chat_id, receiver_user_id, source_id)
                        .await;
                }
            }
        }
        self.post_json(
            "deleteMessage",
            json!({"chat_id": chat_id, "message_id": message_id}),
        )
        .await
    }

    pub async fn delete_ephemeral_message(
        &self,
        chat_id: i64,
        receiver_user_id: i64,
        ephemeral_message_id: i64,
    ) -> Result<Value, String> {
        self.post_json(
            "deleteEphemeralMessage",
            json!({
                "chat_id": chat_id,
                "receiver_user_id": receiver_user_id,
                "ephemeral_message_id": ephemeral_message_id,
            }),
        )
        .await
    }

    pub async fn answer_callback_query(
        &self,
        callback_query_id: &str,
        text: Option<&str>,
        show_alert: bool,
    ) -> Result<Value, String> {
        let mut payload = json!({
            "callback_query_id": callback_query_id,
            "show_alert": show_alert,
        });
        if let Some(text) = text {
            payload["text"] = json!(text);
        }
        self.post_json("answerCallbackQuery", payload).await
    }

    pub async fn send_chat_action(&self, chat_id: i64, action: &str) -> Result<Value, String> {
        let mut payload = json!({"chat_id": chat_id, "action": action});
        Self::apply_delivery_context(&mut payload, false);
        self.post_json("sendChatAction", payload).await
    }

    pub async fn send_rich_message_draft(
        &self,
        chat_id: i64,
        draft_id: i64,
        rich_message: &InputRichMessage,
        can_stop: bool,
        keep_on_stop: bool,
    ) -> Result<Value, String> {
        // Bot API 10.3: "Direct upload of new files and explicit upload of
        // files by a URL isn't supported" in drafts. Media in the streamed
        // partial answer is shown as links until the final message is sent.
        let rich_message = &self.prepare_draft_message(rich_message);
        rich_message.validate()?;
        let rich_json = serde_json::to_value(rich_message).map_err(|e| e.to_string())?;
        let mut payload = json!({
            "chat_id": chat_id,
            "draft_id": draft_id,
            "rich_message": rich_json,
            "can_stop": can_stop,
            "keep_on_stop": keep_on_stop,
        });
        Self::apply_delivery_context(&mut payload, false);

        let res = self.post_json_raw("sendRichMessageDraft", payload).await?;
        if res.get("ok").and_then(Value::as_bool) == Some(true) {
            return Ok(res);
        }

        if !fallback_allowed_response(&res) {
            return Err(Self::telegram_api_error("sendRichMessageDraft", &res));
        }

        let mut fallback_text = rich_message.extract_plain_text();
        if fallback_text.trim().is_empty() {
            fallback_text = "Thinking...".to_string();
        }
        // The fallback is plain text: sending it with parse_mode=HTML made
        // any `<` or `&` in the answer fail with "can't parse entities".
        self.send_message_draft(
            chat_id,
            draft_id,
            &fallback_text,
            None,
            can_stop,
            keep_on_stop,
        )
        .await
    }

    /// Makes a rich message safe to stream as a draft: remote media become
    /// links and any other media (uploads, `attach://` references, albums)
    /// is replaced by a short placeholder, because drafts accept neither.
    pub(crate) fn prepare_draft_message(
        &self,
        rich_message: &InputRichMessage,
    ) -> InputRichMessage {
        self.media_as_links(rich_message, "📎 Lampiran disiapkan…")
    }

    /// Converts remote media blocks to links and replaces any other media
    /// block with `placeholder`. Used wherever Telegram only accepts
    /// previously uploaded files: drafts and inline (guest-mode) messages.
    pub(crate) fn media_as_links(
        &self,
        rich_message: &InputRichMessage,
        placeholder: &str,
    ) -> InputRichMessage {
        let mut converted = self.inner.convert_remote_media_to_rich_links(rich_message);
        for block in &mut converted.blocks {
            if block.is_media() {
                *block = crate::bot::models::RichBlock::Paragraph {
                    text: Value::String(placeholder.to_string()),
                };
            }
        }
        converted.media = None;
        converted
    }

    /// Bot API 10.0 guest mode: replies to a guest message with one inline
    /// query result and returns the id of the inline message it created.
    pub async fn answer_guest_query(
        &self,
        guest_query_id: &str,
        result: Value,
    ) -> Result<crate::bot::models::SentGuestMessage, String> {
        let response = self
            .post_json(
                "answerGuestQuery",
                json!({"guest_query_id": guest_query_id, "result": result}),
            )
            .await?;
        serde_json::from_value(response.get("result").cloned().unwrap_or(Value::Null))
            .map_err(|error| format!("answerGuestQuery returned an unexpected result: {error}"))
    }

    /// Answers an inline query. Results are cached only for the asking user
    /// and never reused: each one stands for a question still to be answered.
    pub async fn answer_inline_query(
        &self,
        inline_query_id: &str,
        results: Value,
    ) -> Result<(), String> {
        self.post_json(
            "answerInlineQuery",
            json!({
                "inline_query_id": inline_query_id,
                "results": results,
                "cache_time": 0,
                "is_personal": true,
            }),
        )
        .await
        .map(|_| ())
    }

    /// Edits an inline message (such as a guest-mode reply) into a rich
    /// message. Inline messages may only reference previously uploaded files,
    /// so media become links. If Telegram rejects the rich form, the message
    /// falls back to plain text bounded by the 4096-character text limit.
    pub async fn edit_inline_rich_message(
        &self,
        inline_message_id: &str,
        rich_message: &InputRichMessage,
    ) -> Result<(), String> {
        let rich =
            self.media_as_links(rich_message, "📎 Lampiran tidak dapat ditampilkan di sini.");
        let is_done = |response: &Value| {
            response.get("ok").and_then(Value::as_bool) == Some(true)
                || response
                    .get("description")
                    .and_then(Value::as_str)
                    .is_some_and(|d| d.to_ascii_lowercase().contains("message is not modified"))
        };
        if rich.validate().is_ok() {
            let rich_json = serde_json::to_value(&rich).map_err(|error| error.to_string())?;
            let response = self
                .post_json_raw(
                    "editMessageText",
                    json!({"inline_message_id": inline_message_id, "rich_message": rich_json}),
                )
                .await?;
            if is_done(&response) {
                return Ok(());
            }
            if !fallback_allowed_response(&response) {
                return Err(Self::telegram_api_error("editMessageText", &response));
            }
        }

        let mut plain = rich.extract_plain_text();
        if plain.trim().is_empty() {
            plain = "…".to_string();
        }
        if plain.chars().count() > TELEGRAM_TEXT_SPLIT_THRESHOLD_CHARS {
            plain = crate::util::truncate_chars(&plain, TELEGRAM_TEXT_CHUNK_CHARS);
            plain.push_str("\n\n… (jawaban dipotong karena batas panjang pesan)");
        }
        let response = self
            .post_json_raw(
                "editMessageText",
                json!({"inline_message_id": inline_message_id, "text": plain}),
            )
            .await?;
        if is_done(&response) {
            Ok(())
        } else {
            Err(Self::telegram_api_error("editMessageText", &response))
        }
    }

    pub async fn send_message_draft(
        &self,
        chat_id: i64,
        draft_id: i64,
        text: &str,
        parse_mode: Option<&str>,
        can_stop: bool,
        keep_on_stop: bool,
    ) -> Result<Value, String> {
        let mut payload = json!({
            "chat_id": chat_id,
            "draft_id": draft_id,
            "text": text,
            "can_stop": can_stop,
            "keep_on_stop": keep_on_stop,
        });
        if let Some(pm) = parse_mode {
            payload["parse_mode"] = json!(pm);
        }
        Self::apply_delivery_context(&mut payload, false);

        let res = self.post_json_raw("sendMessageDraft", payload).await?;
        if res.get("ok").and_then(Value::as_bool) == Some(true) {
            Ok(res)
        } else {
            Err(Self::telegram_api_error("sendMessageDraft", &res))
        }
    }

    pub async fn send_rich_message(
        &self,
        chat_id: i64,
        rich_message: &InputRichMessage,
        reply_markup: Option<Value>,
        receiver_user_id: Option<i64>,
        reply_to_message_id: Option<i64>,
    ) -> Result<Value, String> {
        let target = RichTarget {
            chat_id,
            reply_markup: reply_markup.as_ref(),
            receiver_user_id,
            reply_to_message_id,
        };
        let validation = rich_message.validate();
        if validation.is_ok() {
            match self.attempt_rich_json(target, rich_message).await {
                RichAttempt::Sent(response) => return Ok(response),
                RichAttempt::Failed(error) => return Err(error),
                RichAttempt::Rejected(reason) => {
                    tracing::info!(
                        "Telegram rejected Rich Message ({reason}); checking multipart resolution."
                    );
                }
            }

            if rich_message.has_media() {
                // Upload what can be downloaded here. Telegram already failed
                // to fetch something, so the addresses that cannot be
                // downloaded either become links straight away.
                let staged = self.stage_remote_media(rich_message, &[]).await;
                if !staged.downloads.is_empty() {
                    let linked;
                    let message = if staged.unresolved.is_empty() {
                        &staged.message
                    } else {
                        linked = self
                            .inner
                            .convert_media_to_rich_links(&staged.message, &|url| {
                                staged.unresolved.contains(url)
                            });
                        &linked
                    };
                    let uploads: Vec<&StagedDocument> = staged.downloads.iter().collect();
                    match self.attempt_rich_multipart(target, message, &uploads).await {
                        RichAttempt::Sent(response) => {
                            tracing::info!("Multipart sendRichMessage succeeded seamlessly.");
                            return Ok(response);
                        }
                        RichAttempt::Failed(error) => return Err(error),
                        RichAttempt::Rejected(reason) => {
                            tracing::warn!(
                                "Multipart sendRichMessage rejected ({reason}); falling back to zero-download link conversion."
                            );
                        }
                    }
                }

                let converted_msg = self.inner.convert_remote_media_to_rich_links(rich_message);
                match self.attempt_rich_json(target, &converted_msg).await {
                    RichAttempt::Sent(response) => {
                        tracing::info!(
                            "Zero-download link conversion sendRichMessage succeeded seamlessly."
                        );
                        return Ok(response);
                    }
                    RichAttempt::Failed(error) => return Err(error),
                    RichAttempt::Rejected(reason) => {
                        tracing::warn!(
                            "Zero-download sendRichMessage retry rejected ({reason}); degrading to safe HTML."
                        );
                    }
                }
            }
        } else if let Err(error) = validation {
            if rich_message.blocks.is_empty() {
                return Err(error);
            }
            tracing::info!("Rich Message validation required degradation: {error}");
        }

        let html_chunks = self
            .inner
            .render_blocks_to_html_chunks(&rich_message.blocks, TELEGRAM_TEXT_CHUNK_CHARS);
        let total = html_chunks.len();
        let mut html_last = json!({ "ok": true });
        let mut delivered_html_chunks = 0usize;
        for (idx, chunk) in html_chunks.iter().enumerate() {
            let is_last = idx + 1 == total;
            let is_first = idx == 0;
            match self
                .send_message(
                    chat_id,
                    chunk,
                    Some("HTML"),
                    if is_last { reply_markup.clone() } else { None },
                    receiver_user_id,
                    if is_first { reply_to_message_id } else { None },
                )
                .await
            {
                Ok(response) => {
                    html_last = response;
                    delivered_html_chunks += 1;
                }
                Err(error) => {
                    tracing::info!(
                        "HTML fallback failed at part {} of {total} ({error}); degrading the rest to semantic plain text.",
                        idx + 1
                    );
                    break;
                }
            }
        }
        if delivered_html_chunks == total {
            return Ok(html_last);
        }

        // Only the undelivered remainder is re-sent as plain text. Re-sending
        // the whole message after a mid-way failure duplicated the parts the
        // user had already received.
        let plain = if delivered_html_chunks == 0 {
            rich_message.extract_plain_text()
        } else {
            html_chunks[delivered_html_chunks..]
                .iter()
                .map(|chunk| html_chunk_to_plain_text(chunk))
                .collect::<Vec<_>>()
                .join("\n\n")
        };
        let plain_chunks = self
            .inner
            .split_text_chunks(&plain, TELEGRAM_TEXT_CHUNK_CHARS);
        let total = plain_chunks.len();
        let mut last = html_last;
        for (idx, chunk) in plain_chunks.into_iter().enumerate() {
            let is_last = idx + 1 == total;
            let is_first = idx == 0 && delivered_html_chunks == 0;
            last = self
                .send_message(
                    chat_id,
                    &chunk,
                    None,
                    if is_last { reply_markup.clone() } else { None },
                    receiver_user_id,
                    if is_first { reply_to_message_id } else { None },
                )
                .await?;
        }
        Ok(last)
    }

    pub async fn set_my_commands(&self, commands: &[BotCommand]) -> Result<Value, String> {
        self.post_json(
            "setMyCommands",
            json!({"commands": serde_json::to_value(commands).unwrap_or(json!([]))}),
        )
        .await
    }

    fn add_caption_fields(
        payload: &mut Value,
        caption: Option<&str>,
        parse_mode: Option<&str>,
        reply_markup: Option<&Value>,
    ) {
        if let Some(caption) = caption {
            payload["caption"] = json!(caption);
        }
        if let Some(parse_mode) = parse_mode {
            payload["parse_mode"] = json!(parse_mode);
        }
        if let Some(reply_markup) = reply_markup {
            payload["reply_markup"] = reply_markup.clone();
        }
    }

    fn media_reference(media: &InputMedia) -> &str {
        match media {
            InputMedia::Photo { media, .. }
            | InputMedia::Video { media, .. }
            | InputMedia::Animation { media, .. }
            | InputMedia::Audio { media, .. }
            | InputMedia::Document { media, .. }
            | InputMedia::VoiceNote { media, .. } => media,
        }
    }

    fn set_media_reference(media: &mut InputMedia, value: String) {
        match media {
            InputMedia::Photo { media, .. }
            | InputMedia::Video { media, .. }
            | InputMedia::Animation { media, .. }
            | InputMedia::Audio { media, .. }
            | InputMedia::Document { media, .. }
            | InputMedia::VoiceNote { media, .. } => *media = value,
        }
    }
}

#[cfg(test)]
#[path = "client/tests.rs"]
mod tests;
