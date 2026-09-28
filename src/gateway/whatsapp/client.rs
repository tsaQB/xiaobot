use crate::ai::service::GenerationInput;
use crate::ai::AIChatService;
use crate::bot::worker::{execute_with_scoped_retry, ScopedRetryPolicy, ScopedRetryStorage};
use crate::gateway::whatsapp::delivery::WhatsAppDeliverySink;
use crate::gateway::whatsapp::mapper::{
    is_sender_authorized, parse_group_jid_to_i64, resolve_sender_phone,
};
use crate::gateway::whatsapp::WhatsAppConfig;
use crate::gateway::DeliverySink;
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::{OnceCell, Semaphore};
use tracing::{debug, error, info, warn};
use whatsapp_rust::pair_code::PairCodeOptions;
use whatsapp_rust::prelude::*;

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
}

#[derive(Clone)]
struct WhatsAppIntake {
    message_key: String,
    chat_id: i64,
    sender_id: i64,
    text: String,
    image_bytes: Option<Vec<u8>>,
    audio_bytes: Option<Vec<u8>>,
    doc_bytes: Option<Vec<u8>>,
    doc_name: Option<String>,
    mime_type: Option<String>,
}

/// Menjalankan satu siklus generasi untuk pesan WhatsApp yang sudah terotorisasi.
///
/// Generasi didaftarkan ke `active_generations` supaya pembatalan global saat
/// penutupan aplikasi benar-benar menjangkau WhatsApp, dan dikunci per chat
/// supaya dua generasi tidak saling menimpa riwayat.
async fn run_generation(
    ai: Arc<AIChatService>,
    sink: WhatsAppDeliverySink,
    intake: WhatsAppIntake,
) {
    let chat_id = intake.chat_id;
    let generation_lock = ai.generation_lock(chat_id, 0).await;
    let _generation_guard = generation_lock.lock().await;

    let draft_id = crate::ai::service::next_draft_id();
    let (mut cancel_rx, _generation_guard_handle) = ai.begin_generation(chat_id, draft_id).await;

    let _ = sink.indicate_typing(chat_id).await;

    // Teks dokumen diekstrak lebih dulu supaya AI menerima isi,
    // bukan sekadar gumpalan biner mentah.
    let doc_text = match intake.doc_bytes {
        Some(bytes) => {
            let name = intake
                .doc_name
                .clone()
                .unwrap_or_else(|| "document".to_string());
            let mime = intake.mime_type.clone().unwrap_or_default();
            crate::document::extract_document(bytes, &mime, &name)
                .await
                .ok()
                .and_then(|extracted| extracted.text)
        }
        None => None,
    };

    let mime_ref = intake.mime_type.as_deref();
    let doc_name_ref = intake.doc_name.as_deref();
    let doc_text_ref = doc_text.as_deref();
    let audio_mime_ref = intake.mime_type.as_deref();

    let gen_input = GenerationInput {
        prompt: intake.text.trim(),
        canonical_prompt: None,
        media_to_main: true,
        sink: None,
        image_bytes: intake.image_bytes,
        document_images: None,
        mime_type: mime_ref,
        doc_text: doc_text_ref,
        doc_name: doc_name_ref,
        audio_bytes: intake.audio_bytes,
        audio_mime: audio_mime_ref,
        video_bytes: None,
        video_mime: None,
        video_duration: None,
        bot: None,
        reply_to_message_id: None,
    };

    let (_thinking, answer, _staged_docs, cancelled) = ai
        .generate_response(chat_id, 0, intake.sender_id, gen_input, &mut cancel_rx)
        .await;

    if !cancelled && !answer.trim().is_empty() {
        let _ = sink.send_text(chat_id, &answer).await;
    }

    sink.stop_typing(chat_id).await;
    ai.end_generation(chat_id, draft_id).await;
}

/// Memproses pesan melalui mesin percobaan ulang durabel: klaim antrean,
/// isolasi panik, backoff terbatas, lalu karantina bila terus gagal.
async fn process_intake(
    ai: Arc<AIChatService>,
    sink: WhatsAppDeliverySink,
    semaphore: Arc<Semaphore>,
    intake: WhatsAppIntake,
) {
    let storage = WhatsAppInboxStorage {
        message_key: intake.message_key.clone(),
    };
    execute_with_scoped_retry(&semaphore, &storage, ScopedRetryPolicy::default(), || {
        let ai = Arc::clone(&ai);
        let sink = sink.clone();
        let intake = intake.clone();
        async move { run_generation(ai, sink, intake).await }
    })
    .await;
}

/// Mengantre ulang pesan yang tertinggal saat proses sebelumnya berhenti.
///
/// Hanya teks yang dipulihkan: lampiran media tidak disimpan ke basis data,
/// jadi pesan bermedia diputar ulang sebagai teks saja.
async fn replay_pending_messages(
    ai: Arc<AIChatService>,
    sink: WhatsAppDeliverySink,
    semaphore: Arc<Semaphore>,
) {
    let pending = crate::ai::storage::pending_whatsapp_messages_async(200).await;
    if pending.is_empty() {
        return;
    }
    info!(
        "Memulihkan {} pesan WhatsApp yang tertinggal saat proses berhenti",
        pending.len()
    );
    for record in pending {
        let payload: serde_json::Value =
            serde_json::from_str(&record.payload_json).unwrap_or_default();
        let text = payload
            .get("text")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_string();
        if text.trim().is_empty() {
            let _ = crate::ai::storage::mark_whatsapp_processing_claim_async(
                record.message_key.clone(),
            )
            .await;
            let _ = crate::ai::storage::mark_whatsapp_processing_failed_async(
                record.message_key.clone(),
                "media payload cannot be replayed after restart",
            )
            .await;
            continue;
        }
        if let Some(jid) = payload
            .get("chat_jid")
            .and_then(|value| value.as_str())
            .and_then(|raw| Jid::from_str(raw).ok())
        {
            sink.remember_jid(record.chat_id, jid).await;
        }
        process_intake(
            Arc::clone(&ai),
            sink.clone(),
            Arc::clone(&semaphore),
            WhatsAppIntake {
                message_key: record.message_key,
                chat_id: record.chat_id,
                sender_id: record.sender_id,
                text,
                image_bytes: None,
                audio_bytes: None,
                doc_bytes: None,
                doc_name: None,
                mime_type: None,
            },
        )
        .await;
    }
}

pub struct WhatsAppClientRunner;

impl WhatsAppClientRunner {
    pub async fn run(
        config: WhatsAppConfig,
        ai_service: Arc<AIChatService>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let db_str = config.db_path.to_string_lossy().to_string();
        let store = SqliteStore::new(&db_str).await?;
        // Kredensial sesi multi-device ditulis oleh pustaka WhatsApp dengan izin
        // bawaan sistem. Kunci berkasnya beserta sidecar WAL/SHM segera setelah
        // dibuat, setara dengan perlakuan pada basis data utama dan brankas rahasia.
        crate::ai::storage::harden_file_mode(&config.db_path);
        for suffix in ["-wal", "-shm"] {
            let sidecar = std::path::PathBuf::from(format!("{}{suffix}", config.db_path.display()));
            crate::ai::storage::harden_file_mode(&sidecar);
        }
        info!("WhatsApp SQLite session store loaded at: {db_str}");

        // Satu sink dipakai seumur hidup proses supaya cache JID benar-benar berguna.
        let shared_sink: Arc<OnceCell<WhatsAppDeliverySink>> = Arc::new(OnceCell::new());
        // Bounded concurrency pool (maksimal 8 generasi paralel, konsisten dengan worker.rs)
        let concurrency_semaphore = Arc::new(Semaphore::new(8));

        let recovered = crate::ai::storage::recover_whatsapp_processing_async().await;
        if recovered > 0 {
            info!("Mengembalikan {recovered} pesan WhatsApp yang tertinggal ke status antre");
        }

        let connected_ai = Arc::clone(&ai_service);
        let connected_sink = Arc::clone(&shared_sink);
        let connected_semaphore = Arc::clone(&concurrency_semaphore);

        let mut builder = Bot::builder()
            .with_backend(store)
            .with_event_delivery(EventDelivery::Ordered { capacity: 256 })
            .on_qr_code(|code, timeout| async move {
                println!("\n  \x1b[38;2;16;185;129m╭──────────────────────────────────────────────────────────╮\x1b[0m");
                println!("  \x1b[38;2;16;185;129m│\x1b[0m \x1b[1;37m📲 SCAN QR CODE WHATSAPP (Batas: {}s)\x1b[0m                    \x1b[38;2;16;185;129m│\x1b[0m", timeout.as_secs());
                println!("  \x1b[38;2;16;185;129m╰──────────────────────────────────────────────────────────╯\x1b[0m\n");
                if let Err(e) = qr2term::print_qr(&code) {
                    error!("Gagal render QR visual: {e}. Raw code: {code}");
                }
                println!("\n  \x1b[38;5;244mBuka WhatsApp HP > Perangkat Tertaut > Tautkan Perangkat\x1b[0m\n");
            })
            .on_pair_code(|code, timeout| async move {
                println!("\n  \x1b[1;32m🔑 KODE PAIRING WHATSAPP (Batas: {}s): \x1b[1;37m>>> \x1b[1;33m{}\x1b[1;37m <<<\x1b[0m", timeout.as_secs(), code);
                println!("  \x1b[38;5;244mBuka WhatsApp HP > Perangkat Tertaut > Tautkan dengan nomor telepon\x1b[0m\n");
            })
            .on_connected(move |client| {
                let ai = Arc::clone(&connected_ai);
                let sink_cell = Arc::clone(&connected_sink);
                let semaphore = Arc::clone(&connected_semaphore);
                async move {
                    info!("Berhasil terhubung ke jaringan WhatsApp! Gateway siap aktif.");
                    println!("\n  \x1b[1;32m●\x1b[0m \x1b[1;37mWhatsApp Gateway: Terhubung dan Aktif!\x1b[0m\n");
                    let sink = sink_cell
                        .get_or_init(|| async { WhatsAppDeliverySink::new(Arc::clone(&client)) })
                        .await
                        .clone();
                    replay_pending_messages(ai, sink, semaphore).await;
                }
            })
            .on_logged_out(|_info| async {
                warn!("Sesi WhatsApp telah dikeluarkan / di-logout.");
                println!("\n  \x1b[31m✖ Sesi WhatsApp telah dikeluarkan / logout.\x1b[0m\n");
            });

        let owner_number = config.owner_number.clone();
        let ai = Arc::clone(&ai_service);
        let message_sink = Arc::clone(&shared_sink);
        let message_semaphore = Arc::clone(&concurrency_semaphore);

        builder = builder.on_message(move |ctx| {
            let owner_opt = owner_number.clone();
            let ai_ref = Arc::clone(&ai);
            let semaphore = Arc::clone(&message_semaphore);
            let sink_cell = Arc::clone(&message_sink);

            async move {
                if ctx.info.source.is_from_me {
                    return;
                }

                let Some(sender_id) = resolve_sender_phone(
                    &ctx.info.source.sender,
                    ctx.info.source.sender_alt.as_ref(),
                ) else {
                    return;
                };

                // HARDENED SINGLE-OWNER BOUNDARY:
                // Pesan dari nomor yang tidak berwenang diabaikan seketika (silent drop)
                // tanpa mencatat ID pengirim ke log (zero information leakage).
                if !is_sender_authorized(sender_id, owner_opt.as_deref()) {
                    return;
                }

                let chat_jid_str = ctx.info.source.chat.to_string();
                let is_group = ctx.info.source.is_group;
                let chat_id = if is_group {
                    parse_group_jid_to_i64(&chat_jid_str)
                } else {
                    sender_id
                };

                let raw_text = ctx.message.text_content().unwrap_or_default();
                let trimmed = raw_text.trim();

                // Deteksi lampiran media
                let base = ctx.message.get_base_message();
                let has_media = base.image_message.as_option().is_some()
                    || base.audio_message.as_option().is_some()
                    || base.document_message.as_option().is_some();

                if trimmed.is_empty() && !has_media {
                    return;
                }

                debug!(
                    "WhatsApp intake diterima (chat={chat_id}, panjang={} karakter, media={has_media})",
                    trimmed.chars().count()
                );

                let mut image_bytes = None;
                let mut audio_bytes = None;
                let mut doc_bytes = None;
                let mut doc_name = None;
                let mut mime_type = None;

                if let Some(img) = base.image_message.as_option() {
                    image_bytes = ctx.client.download(img).await.ok();
                    mime_type = img.mimetype.clone();
                } else if let Some(doc) = base.document_message.as_option() {
                    doc_bytes = ctx.client.download(doc).await.ok();
                    mime_type = doc.mimetype.clone();
                    doc_name = doc.file_name.clone();
                } else if let Some(aud) = base.audio_message.as_option() {
                    audio_bytes = ctx.client.download(aud).await.ok();
                    mime_type = aud.mimetype.clone();
                }

                // Catat ke antrean durabel sebelum diproses agar pesan selamat
                // dari proses yang mati mendadak.
                let message_key = format!("{chat_jid_str}:{}", ctx.info.id);
                let payload = serde_json::json!({
                    "chat_jid": chat_jid_str,
                    "text": trimmed,
                    "is_group": is_group,
                    "has_media": has_media,
                })
                .to_string();
                let queued = crate::ai::storage::enqueue_whatsapp_message_async(
                    message_key.clone(),
                    chat_id,
                    sender_id,
                    payload,
                )
                .await;
                if queued != Some(true) {
                    // Pesan yang sama sudah pernah diterima; jangan proses dua kali.
                    return;
                }

                let sink = sink_cell
                    .get_or_init(|| async { WhatsAppDeliverySink::new(ctx.client.clone()) })
                    .await
                    .clone();
                sink.remember_jid(chat_id, ctx.info.source.chat.clone())
                    .await;

                process_intake(
                    ai_ref,
                    sink,
                    semaphore,
                    WhatsAppIntake {
                        message_key,
                        chat_id,
                        sender_id,
                        text: trimmed.to_string(),
                        image_bytes,
                        audio_bytes,
                        doc_bytes,
                        doc_name,
                        mime_type,
                    },
                )
                .await;
            }
        });

        if let Some(phone) = config.phone_login {
            info!("Mengaktifkan pairing berbasis kode telepon.");
            builder = builder.with_pair_code(PairCodeOptions {
                phone_number: phone,
                ..Default::default()
            });
        }

        let bot = builder.build().await?;
        info!("Client WhatsApp terinisialisasi. Menghubungkan...");

        let mut handle = bot.spawn();

        tokio::select! {
            res = &mut handle => {
                info!("Loop WhatsApp bot selesai: {:?}", res);
            }
            _ = tokio::signal::ctrl_c() => {
                info!("Sinyal shutdown diterima, menghentikan WhatsApp client...");
                handle.shutdown().await;
            }
        }

        Ok(())
    }
}
