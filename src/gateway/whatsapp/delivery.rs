use crate::gateway::DeliverySink;
use crate::parser::whatsapp::{chunk_whatsapp_message, format_for_whatsapp};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::warn;
use whatsapp_rust::client::Client;
use whatsapp_rust::prelude::*;

/// Maximum characters per WhatsApp bubble.
const WHATSAPP_CHUNK_CHARS: usize = 3500;
/// Room reserved for the "(n/m)" part marker appended to multi-part replies.
const PART_MARKER_RESERVE: usize = 16;
/// Delays before the second and third send attempt of one chunk.
const SEND_RETRY_DELAYS: [Duration; 2] = [Duration::from_secs(1), Duration::from_secs(3)];

#[derive(Clone)]
pub struct WhatsAppDeliverySink {
    client: Arc<Client>,
    jid_cache: Arc<RwLock<HashMap<i64, Jid>>>,
}

/// Splits a formatted reply into WhatsApp-sized chunks and labels each part
/// with `(n/m)` when there is more than one, so a reader can tell when a
/// long answer arrived incomplete.
pub fn prepare_whatsapp_chunks(text: &str) -> Vec<String> {
    let formatted = format_for_whatsapp(text);
    let chunks = chunk_whatsapp_message(&formatted, WHATSAPP_CHUNK_CHARS - PART_MARKER_RESERVE);
    let total = chunks.len();
    if total <= 1 {
        return chunks;
    }
    chunks
        .into_iter()
        .enumerate()
        .map(|(index, chunk)| format!("{chunk}\n\n_({}/{total})_", index + 1))
        .collect()
}

impl WhatsAppDeliverySink {
    pub fn new(client: Arc<Client>) -> Self {
        Self {
            client,
            jid_cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Menyimpan asosiasi chat_id i64 ke JID asli WhatsApp.
    ///
    /// Menghindari masalah rekonstruksi lossy pada group JID yang memuat karakter non-digit atau hyphen.
    pub async fn remember_jid(&self, chat_id: i64, jid: Jid) {
        let mut cache = self.jid_cache.write().await;
        cache.insert(chat_id, jid);
    }

    /// Mengonversi chat_id XiaoBot (positif untuk DM, negatif untuk grup)
    /// menjadi JID WhatsApp yang valid, memprioritaskan cache JID asli.
    pub async fn resolve_jid(&self, chat_id: i64) -> Option<Jid> {
        if let Some(cached) = self.jid_cache.read().await.get(&chat_id) {
            return Some(cached.clone());
        }
        Self::id_to_jid(chat_id)
    }

    /// Fallback konversi statis chat_id ke format JID standar.
    pub fn id_to_jid(chat_id: i64) -> Option<Jid> {
        let jid_str = if chat_id > 0 {
            format!("{chat_id}@s.whatsapp.net")
        } else if chat_id < 0 {
            format!("{}@g.us", chat_id.unsigned_abs())
        } else {
            return None;
        };
        Jid::from_str(&jid_str).ok()
    }

    /// Menghentikan indikator mengetik / composing
    pub async fn stop_typing(&self, chat_id: i64) {
        if let Some(jid) = self.resolve_jid(chat_id).await {
            let _ = self.client.chatstate().send_paused(&jid).await;
        }
    }

    /// Mengirim satu pesan dengan percobaan ulang terbatas.
    async fn send_with_retry(&self, jid: &Jid, message: wa::Message) -> Result<(), String> {
        let mut last_error = String::new();
        for attempt in 0..=SEND_RETRY_DELAYS.len() {
            match self.client.send_message(jid, message.clone()).await {
                Ok(_) => return Ok(()),
                Err(error) => {
                    last_error = error.to_string();
                    if let Some(delay) = SEND_RETRY_DELAYS.get(attempt) {
                        warn!(
                            attempt = attempt + 1,
                            "Pengiriman WhatsApp gagal; mencoba lagi"
                        );
                        tokio::time::sleep(*delay).await;
                    }
                }
            }
        }
        Err(last_error)
    }
}

impl DeliverySink for WhatsAppDeliverySink {
    async fn indicate_typing(&self, chat_id: i64) -> Result<(), String> {
        let Some(jid) = self.resolve_jid(chat_id).await else {
            return Err(format!("Invalid chat_id {chat_id} for WhatsApp JID"));
        };
        if self.client.chatstate().send_composing(&jid).await.is_err() {
            warn!("Failed to send WhatsApp composing indicator");
        }
        Ok(())
    }

    async fn send_text(&self, chat_id: i64, text: &str) -> Result<(), String> {
        let Some(jid) = self.resolve_jid(chat_id).await else {
            return Err(format!("Invalid chat_id {chat_id} for WhatsApp JID"));
        };

        let chunks = prepare_whatsapp_chunks(text);
        let total = chunks.len();
        for (index, chunk) in chunks.into_iter().enumerate() {
            if let Err(error) = self.send_with_retry(&jid, wa::Message::text(chunk)).await {
                warn!(
                    part = index + 1,
                    total, "Balasan WhatsApp terhenti di tengah setelah percobaan ulang"
                );
                return Err(format!(
                    "WhatsApp send error on part {}/{total}: {error}",
                    index + 1
                ));
            }
        }
        Ok(())
    }

    async fn send_document(
        &self,
        chat_id: i64,
        filename: &str,
        bytes: Vec<u8>,
        mime_type: &str,
    ) -> Result<(), String> {
        let Some(jid) = self.resolve_jid(chat_id).await else {
            return Err(format!("Invalid chat_id {chat_id} for WhatsApp JID"));
        };
        let upload = self
            .client
            .upload(
                bytes,
                whatsapp_rust::download::MediaType::Document,
                whatsapp_rust::upload::UploadOptions::default(),
            )
            .await
            .map_err(|error| format!("WhatsApp media upload failed: {error}"))?;
        let message = whatsapp_rust::media::document_message(
            upload,
            whatsapp_rust::media::DocumentOptions {
                mimetype: Some(mime_type.to_string()),
                file_name: Some(filename.to_string()),
                title: Some(filename.to_string()),
                ..Default::default()
            },
        );
        self.send_with_retry(&jid, message).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positive_chat_id_maps_to_direct_message_jid() {
        let jid = WhatsAppDeliverySink::id_to_jid(6281234567890).expect("dm jid resolved");
        assert_eq!(jid.to_string(), "6281234567890@s.whatsapp.net");
    }

    #[test]
    fn negative_chat_id_maps_to_group_jid() {
        let jid = WhatsAppDeliverySink::id_to_jid(-120363028384910293).expect("group jid resolved");
        assert_eq!(jid.to_string(), "120363028384910293@g.us");
    }

    #[test]
    fn zero_chat_id_is_rejected() {
        assert!(WhatsAppDeliverySink::id_to_jid(0).is_none());
    }

    #[test]
    fn multi_part_replies_are_numbered_and_bounded() {
        let long = "kata ".repeat(2_000);
        let chunks = prepare_whatsapp_chunks(&long);
        assert!(chunks.len() > 1);
        let total = chunks.len();
        for (index, chunk) in chunks.iter().enumerate() {
            assert!(chunk.chars().count() <= WHATSAPP_CHUNK_CHARS);
            assert!(chunk.ends_with(&format!("_({}/{total})_", index + 1)));
        }
        assert_eq!(prepare_whatsapp_chunks("pendek").len(), 1);
        assert!(!prepare_whatsapp_chunks("pendek")[0].contains("(1/1)"));
    }
}
