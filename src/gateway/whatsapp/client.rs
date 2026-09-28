//! WhatsApp intake, processing and connection lifecycle.
//!
//! Delivery guarantees:
//! - An [`InboundDurabilityHook`] persists every authorized message to the
//!   `whatsapp_inbox` table *before* the SDK acknowledges it, so a crash or a
//!   dropped event can no longer lose a message (at-least-once).
//! - The ordered event callback only classifies and enqueues. Generation runs
//!   in per-chat mailbox workers, so one slow AI reply no longer blocks every
//!   other chat or fills the SDK's bounded event mailbox.
//! - The serialized protobuf message is stored with the row, so media can be
//!   downloaded again when a message is replayed after a restart.

use crate::ai::service::GenerationInput;
use crate::ai::storage::WhatsAppInboxEntry;
use crate::ai::AIChatService;
use crate::bot::worker::{
    execute_with_scoped_retry, ScopedRetryPolicy, ScopedRetryStorage, TaskOutcome,
    GLOBAL_WORKER_PERMITS, SCOPE_IDLE_TIMEOUT,
};
use crate::gateway::whatsapp::delivery::WhatsAppDeliverySink;
use crate::gateway::whatsapp::mapper::{
    is_sender_authorized, jid_phone_number, parse_group_jid_to_i64, resolve_sender_phone,
};
use crate::gateway::whatsapp::WhatsAppConfig;
use crate::gateway::DeliverySink;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex, OnceCell, Semaphore};
use tracing::{debug, info, warn};
use whatsapp_rust::buffa::Message as _;
use whatsapp_rust::pair_code::PairCodeOptions;
use whatsapp_rust::prelude::*;

/// Largest inbound media file downloaded for the model (same cap as Telegram).
const MAX_WHATSAPP_MEDIA_BYTES: u64 = 20 * 1024 * 1024;
/// Maximum characters of a quoted message forwarded as reply context.
const MAX_QUOTED_CONTEXT_CHARS: usize = 1_000;
/// Pending rows loaded per replay page.
const REPLAY_PAGE_SIZE: usize = 200;

/// Why a WhatsApp session ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WhatsAppExit {
    /// The daemon asked the client to stop.
    Shutdown,
    /// The phone removed this linked device; a new pairing is required.
    LoggedOut,
    /// The session ended unexpectedly and may be restarted.
    Failed(String),
}

/// Titik sambung WhatsApp ke mesin percobaan ulang durabel yang sama
/// dengan yang dipakai Telegram.
pub struct WhatsAppInboxStorage {
    pub message_key: String,
}

impl ScopedRetryStorage for WhatsAppInboxStorage {
    async fn claim(&self) -> Option<i64> {
        crate::ai::storage::mark_whatsapp_processing_claim_async(self.message_key.clone()).await
    }
    async fn retry(&self, reason: &'static str) -> bool {
        crate::ai::storage::mark_whatsapp_processing_retry_async(self.message_key.clone(), reason)
            .await
    }
    async fn failed(&self, reason: &'static str) -> bool {
        crate::ai::storage::mark_whatsapp_processing_failed_async(self.message_key.clone(), reason)
            .await
    }
    async fn processed(&self) -> bool {
        crate::ai::storage::mark_whatsapp_processed_async(self.message_key.clone()).await
    }
    async fn release(&self, reason: &'static str) -> bool {
        crate::ai::storage::mark_whatsapp_processing_released_async(
            self.message_key.clone(),
            reason,
        )
        .await
    }
}

/// Who may talk to the bot and where.
#[derive(Clone, Default)]
struct IntakePolicy {
    owner_number: Option<String>,
    /// Group JIDs (or their numeric ids) where every owner message is
    /// answered without a mention, like Telegram dedicated workspaces.
    dedicated_groups: Vec<String>,
}

/// An authorized inbound message ready to be persisted and processed.
#[derive(Clone)]
struct WaJob {
    message_key: String,
    chat_id: i64,
    sender_id: i64,
    chat_jid: Jid,
    /// Original message, needed to download media. `None` only for rows
    /// stored by older versions that did not keep the message bytes.
    message: Option<Arc<wa::Message>>,
    /// User text (text body or media caption) with quoted-reply context.
    text: String,
    has_media: bool,
}

impl WaJob {
    fn inbox_entry(&self) -> WhatsAppInboxEntry {
        use base64::Engine;
        let message_b64 = self.message.as_ref().map(|message| {
            base64::engine::general_purpose::STANDARD.encode(message.encode_to_vec())
        });
        WhatsAppInboxEntry {
            message_key: self.message_key.clone(),
            chat_id: self.chat_id,
            sender_id: self.sender_id,
            payload_json: serde_json::json!({
                "chat_jid": self.chat_jid.to_string(),
                "text": self.text,
                "has_media": self.has_media,
                "message_b64": message_b64,
            })
            .to_string(),
        }
    }

    fn from_record(record: &crate::ai::storage::WhatsAppInboxRecord) -> Option<Self> {
        use base64::Engine;
        let payload: serde_json::Value = serde_json::from_str(&record.payload_json).ok()?;
        let chat_jid = payload
            .get("chat_jid")
            .and_then(serde_json::Value::as_str)
            .and_then(|raw| Jid::from_str(raw).ok())?;
        let message = payload
            .get("message_b64")
            .and_then(serde_json::Value::as_str)
            .and_then(|encoded| {
                base64::engine::general_purpose::STANDARD
                    .decode(encoded)
                    .ok()
            })
            .and_then(|bytes| wa::Message::decode_from_slice(&bytes).ok())
            .map(Arc::new);
        Some(Self {
            message_key: record.message_key.clone(),
            chat_id: record.chat_id,
            sender_id: record.sender_id,
            chat_jid,
            message,
            text: payload
                .get("text")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string(),
            has_media: payload
                .get("has_media")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
        })
    }
}

/// The `ContextInfo` attached to whichever sub-message carries it.
fn context_info(message: &wa::Message) -> Option<&wa::ContextInfo> {
    let base = message.get_base_message();
    base.extended_text_message
        .as_option()
        .and_then(|m| m.context_info.as_option())
        .or_else(|| {
            base.image_message
                .as_option()
                .and_then(|m| m.context_info.as_option())
        })
        .or_else(|| {
            base.video_message
                .as_option()
                .and_then(|m| m.context_info.as_option())
        })
        .or_else(|| {
            base.document_message
                .as_option()
                .and_then(|m| m.context_info.as_option())
        })
        .or_else(|| {
            base.audio_message
                .as_option()
                .and_then(|m| m.context_info.as_option())
        })
}

fn has_supported_media(message: &wa::Message) -> bool {
    let base = message.get_base_message();
    base.image_message.as_option().is_some()
        || base.video_message.as_option().is_some()
        || base.audio_message.as_option().is_some()
        || base.document_message.as_option().is_some()
}

/// Identity users (`Jid.user`) of this linked account, used to recognize
/// mentions of and replies to the bot inside groups.
fn own_identity_users(client: &Client) -> Vec<String> {
    [client.pn(), client.lid()]
        .into_iter()
        .flatten()
        .map(|jid| jid.user.to_string())
        .collect()
}

/// Groups are shared spaces: like Telegram guest groups, the bot answers only
/// when it is mentioned, replied to, addressed with a leading `/`, or when the
/// group is configured as dedicated. Previously every owner message in every
/// group triggered a reply visible to all members.
fn group_message_addresses_bot(
    message: &wa::Message,
    text: &str,
    chat_jid: &Jid,
    chat_id: i64,
    own_users: &[String],
    policy: &IntakePolicy,
) -> bool {
    let chat_jid_text = chat_jid.to_string();
    let dedicated = policy.dedicated_groups.iter().any(|group| {
        let group = group.trim();
        group == chat_jid_text
            || group == chat_jid.user
            || group
                .parse::<i64>()
                .is_ok_and(|id| id == chat_id || -id == chat_id)
    });
    if dedicated || text.trim_start().starts_with('/') {
        return true;
    }
    let Some(ctx) = context_info(message) else {
        return false;
    };
    let is_own = |raw: &str| {
        Jid::from_str(raw)
            .ok()
            .is_some_and(|jid| own_users.iter().any(|user| *user == jid.user))
    };
    ctx.mentioned_jid.iter().any(|raw| is_own(raw))
        || ctx.participant.as_deref().is_some_and(is_own)
}

/// Builds the prompt text: the message body or media caption, prefixed with
/// the text of the message being replied to so the model sees what "this"
/// refers to.
fn inbound_text(message: &wa::Message) -> String {
    let body = message
        .text_content()
        .or_else(|| message.get_caption())
        .unwrap_or_default()
        .trim()
        .to_string();
    let quoted = context_info(message)
        .and_then(|ctx| ctx.quoted_message.as_option())
        .and_then(|quoted| quoted.text_content().or_else(|| quoted.get_caption()))
        .map(str::trim)
        .filter(|quoted| !quoted.is_empty())
        .map(|quoted| crate::util::truncate_chars(quoted, MAX_QUOTED_CONTEXT_CHARS));
    match quoted {
        Some(quoted) if !body.is_empty() => {
            format!("[Membalas pesan: \"{quoted}\"]\n\n{body}")
        }
        _ => body,
    }
}

/// Classifies an inbound message. Returns `None` for anything that must be
/// ignored: own messages, unauthorized senders (silently, without logging
/// their identity), empty messages and group messages not addressed to the bot.
fn classify_inbound(
    message: &Arc<wa::Message>,
    info: &MessageInfo,
    policy: &IntakePolicy,
    own_users: &[String],
) -> Option<WaJob> {
    if info.source.is_from_me {
        return None;
    }
    let sender_id = resolve_sender_phone(&info.source.sender, info.source.sender_alt.as_ref())?;
    // HARDENED SINGLE-OWNER BOUNDARY: silent drop, no identity in logs.
    if !is_sender_authorized(sender_id, policy.owner_number.as_deref()) {
        return None;
    }

    let chat_jid = info.source.chat.clone();
    let is_group = info.source.is_group;
    let chat_id = if is_group {
        parse_group_jid_to_i64(&chat_jid.to_string())
    } else {
        sender_id
    };

    let text = inbound_text(message);
    let has_media = has_supported_media(message);
    if text.is_empty() && !has_media {
        return None;
    }
    if is_group
        && !group_message_addresses_bot(message, &text, &chat_jid, chat_id, own_users, policy)
    {
        return None;
    }

    // Stanza ids are only unique per (chat, sender); key on all three.
    let sender_key = jid_phone_number(&info.source.sender)
        .map(|number| number.to_string())
        .unwrap_or_else(|| info.source.sender.user.to_string());
    Some(WaJob {
        message_key: format!("{chat_jid}:{sender_key}:{}", info.id),
        chat_id,
        sender_id,
        chat_jid,
        message: Some(Arc::clone(message)),
        text,
        has_media,
    })
}

/// Persists authorized messages before the SDK acknowledges them.
struct DurableIntakeHook {
    policy: IntakePolicy,
}

#[async_trait::async_trait]
impl whatsapp_rust::InboundDurabilityHook for DurableIntakeHook {
    async fn on_messages(
        &self,
        client: Arc<Client>,
        batch: &[InboundMessage],
    ) -> anyhow::Result<()> {
        let own_users = own_identity_users(&client);
        let entries: Vec<WhatsAppInboxEntry> = batch
            .iter()
            .filter_map(|inbound| {
                classify_inbound(&inbound.message, &inbound.info, &self.policy, &own_users)
            })
            .map(|job| job.inbox_entry())
            .collect();
        if entries.is_empty() {
            return Ok(());
        }
        match crate::ai::storage::enqueue_whatsapp_messages_async(entries).await {
            Some(_) => Ok(()),
            // Returning an error withholds the ack; the server redelivers.
            None => Err(anyhow::anyhow!("durable WhatsApp inbox write failed")),
        }
    }
}

/// Shared state for message processing.
#[derive(Clone)]
struct WaRuntime {
    ai: Arc<AIChatService>,
    sink: WhatsAppDeliverySink,
    client: Arc<Client>,
    semaphore: Arc<Semaphore>,
    mailboxes: ChatMailboxes,
}

/// Per-chat FIFO queues: messages within one chat run in order, different
/// chats run concurrently (bounded by the global semaphore).
#[derive(Clone, Default)]
struct ChatMailboxes {
    inner: Arc<Mutex<HashMap<i64, mpsc::UnboundedSender<WaJob>>>>,
}

impl ChatMailboxes {
    async fn dispatch(&self, runtime: &WaRuntime, job: WaJob) {
        let mut boxes = self.inner.lock().await;
        let job = match boxes.get(&job.chat_id) {
            Some(sender) => match sender.send(job) {
                Ok(()) => return,
                Err(mpsc::error::SendError(returned)) => returned,
            },
            None => job,
        };
        let chat_id = job.chat_id;
        let (tx, rx) = mpsc::unbounded_channel();
        let _ = tx.send(job);
        boxes.insert(chat_id, tx);
        tokio::spawn(chat_worker(chat_id, rx, runtime.clone()));
    }
}

async fn chat_worker(chat_id: i64, mut rx: mpsc::UnboundedReceiver<WaJob>, runtime: WaRuntime) {
    loop {
        let next = tokio::select! {
            job = rx.recv() => job,
            _ = tokio::time::sleep(SCOPE_IDLE_TIMEOUT) => None,
        };
        match next {
            Some(job) => process_job(&runtime, job).await,
            None => {
                // Same critical section as the Telegram worker: a message that
                // raced in while we decided to exit is still processed.
                let mut boxes = runtime.mailboxes.inner.lock().await;
                match rx.try_recv() {
                    Ok(job) => {
                        drop(boxes);
                        process_job(&runtime, job).await;
                    }
                    Err(_) => {
                        boxes.remove(&chat_id);
                        break;
                    }
                }
            }
        }
    }
}

async fn process_job(runtime: &WaRuntime, job: WaJob) {
    if runtime.ai.is_shutting_down() {
        // Leave the row pending; it is replayed after restart.
        return;
    }
    runtime
        .sink
        .remember_jid(job.chat_id, job.chat_jid.clone())
        .await;
    let storage = WhatsAppInboxStorage {
        message_key: job.message_key.clone(),
    };
    let outcome = execute_with_scoped_retry(
        &runtime.semaphore,
        &storage,
        ScopedRetryPolicy::default(),
        || {
            let runtime = runtime.clone();
            let job = job.clone();
            async move { run_generation(runtime, job).await }
        },
    )
    .await;
    debug!("WhatsApp job selesai dengan hasil {outcome:?}");
}

/// Downloaded media, bounded by [`MAX_WHATSAPP_MEDIA_BYTES`].
#[derive(Default)]
struct DownloadedMedia {
    image: Option<Vec<u8>>,
    audio: Option<Vec<u8>>,
    video: Option<Vec<u8>>,
    video_seconds: Option<u32>,
    document: Option<Vec<u8>>,
    document_name: Option<String>,
    mime_type: Option<String>,
    notice: Option<&'static str>,
}

async fn download_media(client: &Client, message: &wa::Message) -> DownloadedMedia {
    let base = message.get_base_message();
    let too_large = |length: Option<u64>| length.is_some_and(|len| len > MAX_WHATSAPP_MEDIA_BYTES);
    let within_limit =
        |bytes: Vec<u8>| (bytes.len() as u64 <= MAX_WHATSAPP_MEDIA_BYTES).then_some(bytes);
    let mut media = DownloadedMedia::default();

    if let Some(img) = base.image_message.as_option() {
        media.mime_type = img.mimetype.clone();
        if too_large(img.file_length) {
            media.notice = Some("Lampiran melebihi batas 20 MB sehingga tidak diproses.");
        } else {
            media.image = client.download(img).await.ok().and_then(within_limit);
        }
    } else if let Some(video) = base.video_message.as_option() {
        media.mime_type = video.mimetype.clone();
        media.video_seconds = video.seconds;
        if too_large(video.file_length) {
            media.notice = Some("Lampiran melebihi batas 20 MB sehingga tidak diproses.");
        } else {
            media.video = client.download(video).await.ok().and_then(within_limit);
        }
    } else if let Some(doc) = base.document_message.as_option() {
        media.mime_type = doc.mimetype.clone();
        media.document_name = doc.file_name.clone();
        if too_large(doc.file_length) {
            media.notice = Some("Lampiran melebihi batas 20 MB sehingga tidak diproses.");
        } else {
            media.document = client.download(doc).await.ok().and_then(within_limit);
        }
    } else if let Some(audio) = base.audio_message.as_option() {
        media.mime_type = audio.mimetype.clone();
        if too_large(audio.file_length) {
            media.notice = Some("Lampiran melebihi batas 20 MB sehingga tidak diproses.");
        } else {
            media.audio = client.download(audio).await.ok().and_then(within_limit);
        }
    }

    let downloaded = media.image.is_some()
        || media.video.is_some()
        || media.document.is_some()
        || media.audio.is_some();
    if media.notice.is_none() && !downloaded && has_supported_media(message) {
        media.notice = Some("Lampiran gagal diunduh dari server WhatsApp.");
    }
    media
}

/// Menjalankan satu siklus generasi untuk pesan WhatsApp yang sudah terotorisasi.
///
/// Generasi didaftarkan ke `active_generations` supaya pembatalan global saat
/// penutupan aplikasi benar-benar menjangkau WhatsApp, dan dikunci per chat
/// supaya dua generasi tidak saling menimpa riwayat.
async fn run_generation(runtime: WaRuntime, job: WaJob) -> TaskOutcome {
    let ai = Arc::clone(&runtime.ai);
    let sink = runtime.sink.clone();
    let chat_id = job.chat_id;
    let generation_lock = ai.generation_lock(chat_id, 0).await;
    let _generation_guard = generation_lock.lock().await;

    let draft_id = crate::ai::service::next_draft_id();
    let (mut cancel_rx, _generation_guard_handle) = ai.begin_generation(chat_id, draft_id).await;

    let _ = sink.indicate_typing(chat_id).await;

    let mut media = match job.message.as_deref() {
        Some(message) if job.has_media => download_media(&runtime.client, message).await,
        _ if job.has_media => DownloadedMedia {
            notice: Some(
                "Lampiran dari pesan ini tidak dapat dipulihkan setelah bot dimulai ulang.",
            ),
            ..Default::default()
        },
        _ => DownloadedMedia::default(),
    };

    // A media message whose attachment could not be obtained and that has no
    // text is answered with an explanation instead of being dropped silently.
    if let (Some(notice), true) = (media.notice, job.text.trim().is_empty()) {
        let reply = format!("⚠️ {notice} Silakan kirim ulang lampirannya.");
        let delivered = sink.send_text(chat_id, &reply).await.is_ok();
        sink.stop_typing(chat_id).await;
        ai.end_generation(chat_id, draft_id).await;
        return if delivered {
            TaskOutcome::Completed
        } else {
            TaskOutcome::DeliveryFailed("attachment notice could not be delivered")
        };
    }

    // Teks dokumen diekstrak lebih dulu supaya AI menerima isi,
    // bukan sekadar gumpalan biner mentah.
    let doc_text = match media.document.take() {
        Some(bytes) => {
            let name = media
                .document_name
                .clone()
                .unwrap_or_else(|| "document".to_string());
            let mime = media.mime_type.clone().unwrap_or_default();
            crate::document::extract_document(bytes, &mime, &name)
                .await
                .ok()
                .and_then(|extracted| extracted.text)
        }
        None => None,
    };

    let prompt = match media.notice {
        Some(notice) => format!("{}\n\n[Catatan sistem: {notice}]", job.text.trim()),
        None => job.text.trim().to_string(),
    };
    let video_duration = media
        .video_seconds
        .and_then(|seconds| i32::try_from(seconds).ok());
    let gen_input = GenerationInput {
        prompt: &prompt,
        canonical_prompt: None,
        media_to_main: true,
        sink: None,
        image_bytes: media.image.take(),
        document_images: None,
        mime_type: media.mime_type.as_deref(),
        doc_text: doc_text.as_deref(),
        doc_name: media.document_name.as_deref(),
        audio_bytes: media.audio.take(),
        audio_mime: media.mime_type.as_deref(),
        video_bytes: media.video.take(),
        video_mime: media.mime_type.as_deref(),
        video_duration,
        bot: None,
        reply_to_message_id: None,
    };

    let (_thinking, answer, staged_docs, cancelled) = ai
        .generate_response(chat_id, 0, job.sender_id, gen_input, &mut cancel_rx)
        .await;

    let outcome = if cancelled {
        if ai.is_shutting_down() {
            TaskOutcome::Interrupted
        } else {
            TaskOutcome::Completed
        }
    } else {
        deliver_answer(&sink, chat_id, &answer, staged_docs).await
    };

    sink.stop_typing(chat_id).await;
    ai.end_generation(chat_id, draft_id).await;
    outcome
}

/// Sends the text answer and every staged document. Documents used to be
/// dropped on WhatsApp even though the model had announced them.
async fn deliver_answer(
    sink: &WhatsAppDeliverySink,
    chat_id: i64,
    answer: &str,
    staged_docs: Vec<crate::bot::models::StagedDocument>,
) -> TaskOutcome {
    if !answer.trim().is_empty() && sink.send_text(chat_id, answer).await.is_err() {
        return TaskOutcome::DeliveryFailed("WhatsApp text reply could not be delivered");
    }
    let mut failed_documents = 0usize;
    for doc in staged_docs {
        let (_, bytes, mime, filename) = doc.into_raw_tuple();
        if sink
            .send_document(chat_id, &filename, bytes, &mime)
            .await
            .is_err()
        {
            failed_documents += 1;
        }
    }
    if failed_documents > 0 {
        let _ = sink
            .send_text(
                chat_id,
                &format!("⚠️ {failed_documents} berkas gagal dikirim. Silakan minta ulang."),
            )
            .await;
        return TaskOutcome::DeliveryFailed("WhatsApp document could not be delivered");
    }
    TaskOutcome::Completed
}

/// Re-dispatches every pending row, paging through the whole backlog.
async fn replay_pending_messages(runtime: &WaRuntime) {
    let mut cursor: Option<(String, String)> = None;
    let mut replayed = 0usize;
    loop {
        let page =
            crate::ai::storage::pending_whatsapp_messages_async(cursor.clone(), REPLAY_PAGE_SIZE)
                .await;
        let Some(last) = page.last() else {
            break;
        };
        cursor = Some((last.received_at.clone(), last.message_key.clone()));
        for record in page {
            match WaJob::from_record(&record) {
                Some(job) => {
                    replayed += 1;
                    runtime.mailboxes.dispatch(runtime, job).await;
                }
                None => {
                    let _ = crate::ai::storage::mark_whatsapp_processing_failed_async(
                        record.message_key.clone(),
                        "stored payload could not be decoded",
                    )
                    .await;
                }
            }
        }
    }
    if replayed > 0 {
        info!("Memulihkan {replayed} pesan WhatsApp yang tertinggal");
    }
}

/// Watches the SDK's dropped-event counter. Durable rows exist for dropped
/// events (the hook ran first), so a drop triggers a replay sweep instead of
/// silently losing the message.
async fn sweep_if_events_dropped(runtime: &WaRuntime, last_seen: &AtomicU64) {
    let dropped = runtime.client.stats().events_dropped;
    let previous = last_seen.swap(dropped, Ordering::SeqCst);
    if dropped > previous {
        warn!(
            dropped = dropped - previous,
            "Mailbox event WhatsApp penuh; menyapu ulang antrean durabel"
        );
        replay_pending_messages(runtime).await;
    }
}

pub struct WhatsAppClientRunner;

impl WhatsAppClientRunner {
    pub async fn run(
        config: WhatsAppConfig,
        ai_service: Arc<AIChatService>,
        mut shutdown: tokio::sync::watch::Receiver<bool>,
    ) -> WhatsAppExit {
        // Kredensial sesi multi-device ditulis oleh pustaka WhatsApp. Buat
        // berkasnya lebih dulu dengan izin privat supaya tidak ada jeda saat
        // berkas masih berizin bawaan, lalu kunci juga sidecar WAL/SHM.
        crate::ai::storage::ensure_private_file(&config.db_path);
        let db_str = config.db_path.to_string_lossy().to_string();
        let store = match SqliteStore::new(&db_str).await {
            Ok(store) => store,
            Err(error) => return WhatsAppExit::Failed(format!("session store: {error}")),
        };
        crate::ai::storage::harden_file_mode(&config.db_path);
        for suffix in ["-wal", "-shm"] {
            let sidecar = std::path::PathBuf::from(format!("{}{suffix}", config.db_path.display()));
            crate::ai::storage::harden_file_mode(&sidecar);
        }
        info!("WhatsApp SQLite session store loaded");

        let policy = IntakePolicy {
            owner_number: config.owner_number.clone(),
            dedicated_groups: config.dedicated_groups.clone(),
        };
        let runtime_cell: Arc<OnceCell<WaRuntime>> = Arc::new(OnceCell::new());
        let semaphore = Arc::new(Semaphore::new(GLOBAL_WORKER_PERMITS));
        let mailboxes = ChatMailboxes::default();
        let logged_out = Arc::new(AtomicBool::new(false));
        let dropped_seen = Arc::new(AtomicU64::new(0));

        let recovered = crate::ai::storage::recover_whatsapp_processing_async().await;
        if recovered > 0 {
            info!("Mengembalikan {recovered} pesan WhatsApp yang tertinggal ke status antre");
        }

        let make_runtime = {
            let ai = Arc::clone(&ai_service);
            let semaphore = Arc::clone(&semaphore);
            let mailboxes = mailboxes.clone();
            move |client: Arc<Client>| WaRuntime {
                ai: Arc::clone(&ai),
                sink: WhatsAppDeliverySink::new(Arc::clone(&client)),
                client,
                semaphore: Arc::clone(&semaphore),
                mailboxes: mailboxes.clone(),
            }
        };

        let connected_runtime = Arc::clone(&runtime_cell);
        let connected_make = make_runtime.clone();
        let logout_flag = Arc::clone(&logged_out);
        let mut builder = Bot::builder()
            .with_backend(store)
            .with_event_delivery(EventDelivery::Ordered { capacity: 256 })
            .with_inbound_durability_hook(DurableIntakeHook {
                policy: policy.clone(),
            })
            .on_qr_code(|code, timeout| async move {
                println!("\n  \x1b[38;2;16;185;129m╭──────────────────────────────────────────────────────────╮\x1b[0m");
                println!("  \x1b[38;2;16;185;129m│\x1b[0m \x1b[1;37m📲 SCAN QR CODE WHATSAPP (Batas: {}s)\x1b[0m                    \x1b[38;2;16;185;129m│\x1b[0m", timeout.as_secs());
                println!("  \x1b[38;2;16;185;129m╰──────────────────────────────────────────────────────────╯\x1b[0m\n");
                if qr2term::print_qr(&code).is_err() {
                    println!("  Gagal merender QR visual. Kode mentah:\n  {code}\n");
                }
                println!("\n  \x1b[38;5;244mBuka WhatsApp HP > Perangkat Tertaut > Tautkan Perangkat\x1b[0m\n");
            })
            .on_pair_code(|code, timeout| async move {
                // Kode ini harus terlihat oleh pemilik agar bisa diketik di HP;
                // ia dicetak ke terminal, tidak pernah ke log, dan kedaluwarsa cepat.
                println!("\n  \x1b[1;32m🔑 KODE PAIRING WHATSAPP (Batas: {}s): \x1b[1;37m>>> \x1b[1;33m{}\x1b[1;37m <<<\x1b[0m", timeout.as_secs(), code);
                println!("  \x1b[38;5;244mBuka WhatsApp HP > Perangkat Tertaut > Tautkan dengan nomor telepon\x1b[0m\n");
            })
            .on_connected(move |client| {
                let cell = Arc::clone(&connected_runtime);
                let make = connected_make.clone();
                async move {
                    info!("Berhasil terhubung ke jaringan WhatsApp! Gateway siap aktif.");
                    println!("\n  \x1b[1;32m●\x1b[0m \x1b[1;37mWhatsApp Gateway: Terhubung dan Aktif!\x1b[0m\n");
                    let runtime = cell
                        .get_or_init(|| async { make(Arc::clone(&client)) })
                        .await
                        .clone();
                    replay_pending_messages(&runtime).await;
                }
            })
            .on_logged_out(move |_info| {
                let flag = Arc::clone(&logout_flag);
                async move {
                    flag.store(true, Ordering::SeqCst);
                    warn!("Sesi WhatsApp telah dikeluarkan / di-logout dari perangkat utama.");
                    println!("\n  \x1b[31m✖ Sesi WhatsApp telah dikeluarkan / logout.\x1b[0m\n");
                }
            });

        let message_runtime = Arc::clone(&runtime_cell);
        let message_make = make_runtime.clone();
        let message_policy = policy.clone();
        let message_dropped = Arc::clone(&dropped_seen);
        builder = builder.on_message(move |ctx| {
            let cell = Arc::clone(&message_runtime);
            let make = message_make.clone();
            let policy = message_policy.clone();
            let dropped_seen = Arc::clone(&message_dropped);
            async move {
                let own_users = own_identity_users(&ctx.client);
                let Some(job) = classify_inbound(&ctx.message, &ctx.info, &policy, &own_users)
                else {
                    return;
                };
                debug!(
                    "WhatsApp intake diterima (chat={}, panjang={} karakter, media={})",
                    job.chat_id,
                    job.text.chars().count(),
                    job.has_media
                );
                // The durability hook normally inserted the row already; this
                // is an idempotent safety net for paths the hook does not
                // cover (e.g. PDO recoveries).
                if crate::ai::storage::enqueue_whatsapp_messages_async(vec![job.inbox_entry()])
                    .await
                    .is_none()
                {
                    warn!("Pesan WhatsApp tidak dapat dicatat ke antrean durabel");
                }
                let runtime = cell
                    .get_or_init(|| async { make(Arc::clone(&ctx.client)) })
                    .await
                    .clone();
                // Only enqueue here; processing happens in the chat worker so
                // this ordered callback returns immediately.
                runtime.mailboxes.dispatch(&runtime, job).await;
                sweep_if_events_dropped(&runtime, &dropped_seen).await;
            }
        });

        if let Some(phone) = config.phone_login.clone() {
            info!("Mengaktifkan pairing berbasis kode telepon.");
            builder = builder.with_pair_code(PairCodeOptions {
                phone_number: phone,
                ..Default::default()
            });
        }

        let bot = match builder.build().await {
            Ok(bot) => bot,
            Err(error) => return WhatsAppExit::Failed(format!("client build: {error}")),
        };
        info!("Client WhatsApp terinisialisasi. Menghubungkan...");

        let mut handle = bot.spawn();
        let exit = tokio::select! {
            _ = &mut handle => {
                if logged_out.load(Ordering::SeqCst) {
                    WhatsAppExit::LoggedOut
                } else {
                    WhatsAppExit::Failed("connection loop ended".to_string())
                }
            }
            _ = wait_for_shutdown(&mut shutdown) => {
                info!("Menghentikan WhatsApp client secara graceful...");
                // Disconnects and flushes receipts/message secrets before exit.
                handle.shutdown().await;
                WhatsAppExit::Shutdown
            }
        };

        if exit == WhatsAppExit::LoggedOut {
            // The linked-device credentials are dead after a server-side
            // logout; remove them so status reflects reality and the next
            // start asks for a fresh pairing instead of looping.
            if let Err(error) = super::WhatsAppGateway::logout(&config.db_path) {
                warn!("Gagal menghapus sesi WhatsApp yang sudah logout: {error}");
            }
        }
        exit
    }
}

async fn wait_for_shutdown(shutdown: &mut tokio::sync::watch::Receiver<bool>) {
    while !*shutdown.borrow() {
        if shutdown.changed().await.is_err() {
            // Sender gone: treat as shutdown so the client does not linger.
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_message(text: &str) -> Arc<wa::Message> {
        Arc::new(wa::Message::text(text))
    }

    fn info_for(chat: &str, sender: &str, is_group: bool) -> MessageInfo {
        let mut info = MessageInfo::default();
        info.source.chat = Jid::from_str(chat).expect("valid chat jid");
        info.source.sender = Jid::from_str(sender).expect("valid sender jid");
        info.source.is_group = is_group;
        info.id = "ABC123".into();
        info
    }

    fn owner_policy() -> IntakePolicy {
        IntakePolicy {
            owner_number: Some("6281234567890".to_string()),
            dedicated_groups: Vec::new(),
        }
    }

    #[test]
    fn owner_direct_message_is_accepted_and_keyed_by_chat_sender_and_id() {
        let info = info_for(
            "6281234567890@s.whatsapp.net",
            "6281234567890@s.whatsapp.net",
            false,
        );
        let job = classify_inbound(&text_message("halo"), &info, &owner_policy(), &[])
            .expect("owner DM accepted");
        assert_eq!(job.chat_id, 6281234567890);
        assert_eq!(
            job.message_key,
            "6281234567890@s.whatsapp.net:6281234567890:ABC123"
        );
        assert_eq!(job.text, "halo");
    }

    #[test]
    fn stranger_is_dropped_silently() {
        let info = info_for(
            "6289999999999@s.whatsapp.net",
            "6289999999999@s.whatsapp.net",
            false,
        );
        assert!(classify_inbound(&text_message("halo"), &info, &owner_policy(), &[]).is_none());
    }

    #[test]
    fn owner_group_message_needs_mention_reply_or_dedicated_group() {
        let info = info_for(
            "120363028384910293@g.us",
            "6281234567890@s.whatsapp.net",
            true,
        );
        let own = vec!["6280000000001".to_string()];
        assert!(
            classify_inbound(&text_message("ngobrol biasa"), &info, &owner_policy(), &own)
                .is_none(),
            "plain group chatter must not trigger a reply"
        );
        assert!(classify_inbound(
            &text_message("/tanya sesuatu"),
            &info,
            &owner_policy(),
            &own
        )
        .is_some());

        let mut mention = wa::Message::text("halo bot");
        mention.set_context_info(wa::ContextInfo {
            mentioned_jid: vec!["6280000000001@s.whatsapp.net".to_string()],
            ..Default::default()
        });
        assert!(
            classify_inbound(&Arc::new(mention), &info, &owner_policy(), &own).is_some(),
            "a mention of the bot is addressed to it"
        );

        let mut dedicated = owner_policy();
        dedicated.dedicated_groups = vec!["120363028384910293@g.us".to_string()];
        assert!(
            classify_inbound(&text_message("ngobrol biasa"), &info, &dedicated, &own).is_some()
        );
    }

    #[test]
    fn quoted_reply_text_is_forwarded_as_context() {
        let mut reply = wa::Message::text("jelaskan ini");
        reply.set_context_info(wa::ContextInfo {
            quoted_message: MessageField::some(wa::Message::text("E = mc^2")),
            ..Default::default()
        });
        let text = inbound_text(&reply);
        assert!(text.starts_with("[Membalas pesan: \"E = mc^2\"]"), "{text}");
        assert!(text.ends_with("jelaskan ini"));
    }

    #[test]
    fn stored_job_round_trips_with_message_bytes_for_media_replay() {
        let info = info_for(
            "6281234567890@s.whatsapp.net",
            "6281234567890@s.whatsapp.net",
            false,
        );
        let job = classify_inbound(&text_message("halo"), &info, &owner_policy(), &[])
            .expect("owner DM accepted");
        let entry = job.inbox_entry();
        let record = crate::ai::storage::WhatsAppInboxRecord {
            message_key: entry.message_key.clone(),
            chat_id: entry.chat_id,
            sender_id: entry.sender_id,
            payload_json: entry.payload_json.clone(),
            attempts: 0,
            received_at: String::new(),
        };
        let restored = WaJob::from_record(&record).expect("payload decodes");
        assert_eq!(restored.text, "halo");
        assert_eq!(
            restored
                .message
                .as_deref()
                .and_then(|message| message.text_content()),
            Some("halo")
        );
    }
}
