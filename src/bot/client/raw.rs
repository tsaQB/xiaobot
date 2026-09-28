//! Transport-independent Telegram helpers shared by [`crate::bot::client`].
//!
//! This module used to contain a second, full implementation of every Bot API
//! call (~2,500 lines) that the public client shadowed method-for-method. Bug
//! fixes landed in one copy but not the other, and several audit findings
//! pointed at code that never ran in production. All network calls now live
//! in `client.rs`; this module keeps only what that client delegates to:
//! the per-task delivery context, bounded SSRF-safe media downloads, rich
//! message rendering/fallback conversion, and text chunking.

use serde_json::{json, Value};
use std::time::Duration;

use super::models::{InputRichMessage, RichBlock};

#[derive(Debug, Clone, Default)]
pub struct TelegramDeliveryContext {
    pub message_thread_id: Option<i64>,
    pub receiver_user_id: Option<i64>,
    pub source_ephemeral_message_id: Option<i64>,
    pub callback_query_id: Option<String>,
    pub replace_callback_query_message: Option<bool>,
}

tokio::task_local! {
    static TELEGRAM_DELIVERY_CONTEXT: TelegramDeliveryContext;
}

/// Stateless helper; every method is pure or creates its own pinned client.
#[derive(Clone, Default)]
pub struct TelegramBotClient;

impl TelegramBotClient {
    pub async fn with_delivery_context<F, T>(context: TelegramDeliveryContext, future: F) -> T
    where
        F: std::future::Future<Output = T>,
    {
        TELEGRAM_DELIVERY_CONTEXT.scope(context, future).await
    }

    pub fn current_delivery_context() -> TelegramDeliveryContext {
        TELEGRAM_DELIVERY_CONTEXT
            .try_with(Clone::clone)
            .unwrap_or_default()
    }

    /// Runs a download under a total time budget; `None` when it expires.
    async fn with_download_budget<F, T>(budget: Duration, download: F) -> Option<T>
    where
        F: std::future::Future<Output = Option<T>>,
    {
        tokio::time::timeout(budget, download).await.ok().flatten()
    }

    /// Downloads remote media for re-upload when Telegram cannot fetch a URL
    /// itself. The whole transfer (all redirect hops plus the body) shares a
    /// 30-second budget, and every hop goes through the SSRF policy.
    pub async fn download_media_bytes(
        &self,
        url: &str,
        max_bytes: usize,
    ) -> Option<(Vec<u8>, String, String)> {
        Self::with_download_budget(
            Duration::from_secs(30),
            Self::download_media_bytes_inner(url, max_bytes),
        )
        .await
    }

    // Dipakai oleh test untuk membuktikan anggaran waktu unduhan dapat dibatasi
    // tanpa menunggu batas produksi 30 detik.
    #[cfg(test)]
    pub(crate) async fn download_media_bytes_with_budget(
        &self,
        url: &str,
        max_bytes: usize,
        budget: Duration,
    ) -> Option<(Vec<u8>, String, String)> {
        Self::with_download_budget(budget, Self::download_media_bytes_inner(url, max_bytes)).await
    }

    async fn download_media_bytes_inner(
        url: &str,
        max_bytes: usize,
    ) -> Option<(Vec<u8>, String, String)> {
        let fetched = super::url_policy::fetch_public_url(
            url.trim(),
            &super::url_policy::PublicFetchOptions {
                timeout: Duration::from_secs(30),
                max_bytes,
                user_agent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
                accept: "*/*",
            },
        )
        .await
        .ok()?;
        let bytes = fetched.bytes;
        let content_type = fetched.content_type;

        let file_name = if content_type.contains("png") || bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            "image.png"
        } else if content_type.contains("jpeg")
            || content_type.contains("jpg")
            || bytes.starts_with(&[0xff, 0xd8, 0xff])
        {
            "image.jpg"
        } else if content_type.contains("gif")
            || bytes.starts_with(b"GIF87a")
            || bytes.starts_with(b"GIF89a")
        {
            "image.gif"
        } else if content_type.contains("webp")
            || (bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP")
        {
            "image.webp"
        } else if content_type.contains("ogg")
            || content_type.contains("opus")
            || bytes.starts_with(b"OggS")
        {
            "audio.ogg"
        } else if content_type.contains("mp3")
            || content_type.contains("mpeg")
            || bytes.starts_with(b"ID3")
            || bytes.starts_with(&[0xff, 0xfb])
        {
            "audio.mp3"
        } else if content_type.contains("mp4") || bytes.windows(4).take(8).any(|w| w == b"ftyp") {
            "video.mp4"
        } else if content_type.contains("pdf") || bytes.starts_with(b"%PDF-") {
            "document.pdf"
        } else {
            "file.bin"
        };

        Some((bytes, content_type, file_name.to_string()))
    }

    /// Replaces remote media blocks with a paragraph containing a link. Links
    /// use the Bot API 10.3 `RichTextUrl` shape (`{"type":"url","text",…,
    /// "url":…}`); the previous `text_link` type does not exist in the rich
    /// message API and made Telegram reject the converted message.
    pub fn convert_remote_media_to_rich_links(
        &self,
        rich_message: &InputRichMessage,
    ) -> InputRichMessage {
        let mut converted = rich_message.clone();
        for block in &mut converted.blocks {
            match block {
                RichBlock::Photo { photo, caption } => {
                    let url = photo
                        .get("media")
                        .and_then(Value::as_str)
                        .or_else(|| photo.as_str())
                        .unwrap_or("");
                    if url.starts_with("http://") || url.starts_with("https://") {
                        let cap = caption
                            .as_ref()
                            .map(|c| self.rich_caption_to_plain(&Some(c.clone())))
                            .filter(|s| !s.is_empty())
                            .unwrap_or_else(|| "Lihat Foto".to_string());
                        *block = RichBlock::Paragraph {
                            text: Value::Array(vec![
                                json!("🖼️ "),
                                json!({
                                    "type": "url",
                                    "text": cap,
                                    "url": url,
                                }),
                            ]),
                        };
                    }
                }
                RichBlock::Video { video, caption } => {
                    let url = video
                        .get("media")
                        .and_then(Value::as_str)
                        .or_else(|| video.as_str())
                        .unwrap_or("");
                    if url.starts_with("http://") || url.starts_with("https://") {
                        let cap = caption
                            .as_ref()
                            .map(|c| self.rich_caption_to_plain(&Some(c.clone())))
                            .filter(|s| !s.is_empty())
                            .unwrap_or_else(|| "Tonton Video".to_string());
                        *block = RichBlock::Paragraph {
                            text: Value::Array(vec![
                                json!("🎬 "),
                                json!({
                                    "type": "url",
                                    "text": cap,
                                    "url": url,
                                }),
                            ]),
                        };
                    }
                }
                RichBlock::Audio { audio, caption } => {
                    let url = audio
                        .get("media")
                        .and_then(Value::as_str)
                        .or_else(|| audio.as_str())
                        .unwrap_or("");
                    if url.starts_with("http://") || url.starts_with("https://") {
                        let cap = caption
                            .as_ref()
                            .map(|c| self.rich_caption_to_plain(&Some(c.clone())))
                            .filter(|s| !s.is_empty())
                            .unwrap_or_else(|| "Putar Audio".to_string());
                        *block = RichBlock::Paragraph {
                            text: Value::Array(vec![
                                json!("🎵 "),
                                json!({
                                    "type": "url",
                                    "text": cap,
                                    "url": url,
                                }),
                            ]),
                        };
                    }
                }
                RichBlock::Animation { animation, caption } => {
                    let url = animation
                        .get("media")
                        .and_then(Value::as_str)
                        .or_else(|| animation.as_str())
                        .unwrap_or("");
                    if url.starts_with("http://") || url.starts_with("https://") {
                        let cap = caption
                            .as_ref()
                            .map(|c| self.rich_caption_to_plain(&Some(c.clone())))
                            .filter(|s| !s.is_empty())
                            .unwrap_or_else(|| "Animasi".to_string());
                        *block = RichBlock::Paragraph {
                            text: Value::Array(vec![
                                json!("🎞️ "),
                                json!({
                                    "type": "url",
                                    "text": cap,
                                    "url": url,
                                }),
                            ]),
                        };
                    }
                }
                RichBlock::Document { document, caption } => {
                    let url = document
                        .get("media")
                        .and_then(Value::as_str)
                        .or_else(|| document.as_str())
                        .unwrap_or("");
                    if url.starts_with("http://") || url.starts_with("https://") {
                        let cap = caption
                            .as_ref()
                            .map(|c| self.rich_caption_to_plain(&Some(c.clone())))
                            .filter(|s| !s.is_empty())
                            .unwrap_or_else(|| "Dokumen".to_string());
                        *block = RichBlock::Paragraph {
                            text: Value::Array(vec![
                                json!("📄 "),
                                json!({
                                    "type": "url",
                                    "text": cap,
                                    "url": url,
                                }),
                            ]),
                        };
                    }
                }
                RichBlock::Collage {
                    blocks: items,
                    caption,
                } => {
                    let cap_str = caption
                        .as_ref()
                        .map(|c| self.rich_caption_to_plain(&Some(c.clone())))
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| "Galeri Foto".to_string());
                    let mut text_parts = vec![json!(format!("🖼️ [{cap_str}]: "))];
                    let mut count = 0;
                    for item in items.iter() {
                        let sub_url = item
                            .get("photo")
                            .and_then(|p| p.get("media"))
                            .and_then(Value::as_str)
                            .or_else(|| item.get("media").and_then(Value::as_str))
                            .unwrap_or("");
                        if !sub_url.is_empty() {
                            if count > 0 {
                                text_parts.push(json!(" • "));
                            }
                            count += 1;
                            text_parts.push(json!({
                                "type": "url",
                                "text": format!("Foto #{count}"),
                                "url": sub_url,
                            }));
                        }
                    }
                    *block = RichBlock::Paragraph {
                        text: Value::Array(text_parts),
                    };
                }
                RichBlock::Slideshow {
                    blocks: items,
                    caption,
                } => {
                    let cap_str = caption
                        .as_ref()
                        .map(|c| self.rich_caption_to_plain(&Some(c.clone())))
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| "Slideshow".to_string());
                    let mut text_parts = vec![json!(format!("🖼️ [{cap_str}]: "))];
                    let mut count = 0;
                    for item in items.iter() {
                        let sub_url = item
                            .get("photo")
                            .and_then(|p| p.get("media"))
                            .and_then(Value::as_str)
                            .or_else(|| item.get("media").and_then(Value::as_str))
                            .unwrap_or("");
                        if !sub_url.is_empty() {
                            if count > 0 {
                                text_parts.push(json!(" • "));
                            }
                            count += 1;
                            text_parts.push(json!({
                                "type": "url",
                                "text": format!("Slide #{count}"),
                                "url": sub_url,
                            }));
                        }
                    }
                    *block = RichBlock::Paragraph {
                        text: Value::Array(text_parts),
                    };
                }
                _ => {}
            }
        }
        converted
    }

    // HTML Rendering Helpers & Chunking
    // ==========================================

    pub fn split_text_chunks(&self, text: &str, max_chunk_chars: usize) -> Vec<String> {
        if text.is_empty() || max_chunk_chars == 0 {
            return Vec::new();
        }

        let chars: Vec<char> = text.chars().collect();
        if chars.len() <= max_chunk_chars {
            return vec![text.to_string()];
        }

        let mut chunks = Vec::new();
        let mut start = 0usize;
        while start < chars.len() {
            let hard_end = (start + max_chunk_chars).min(chars.len());
            let mut end = hard_end;

            if hard_end < chars.len() {
                // Prefer a natural boundary in the latter half of the chunk, but
                // always make progress even for a single huge token/code line.
                let soft_floor = start + (max_chunk_chars / 2);
                for idx in (soft_floor..hard_end).rev() {
                    if chars[idx] == '\n' {
                        end = idx + 1;
                        break;
                    }
                }
                if end == hard_end {
                    for idx in (soft_floor..hard_end).rev() {
                        if chars[idx].is_whitespace() {
                            end = idx + 1;
                            break;
                        }
                    }
                }
            }

            if end <= start {
                end = hard_end.max(start + 1);
            }
            chunks.push(chars[start..end].iter().collect());
            start = end;
        }

        chunks
    }
}

#[path = "raw/render.rs"]
mod render;

#[cfg(test)]
#[path = "raw/tests.rs"]
mod tests;
