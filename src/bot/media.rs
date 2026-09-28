//! Downloading and reading the file a Telegram message carries: photos (and
//! the motion clip of a live photo), stickers, voice notes, audio, video,
//! video notes and documents. Shared by ordinary chats and guest mode, so the
//! caller decides what to tell the user when something cannot be read.

use tracing::info;

use crate::ai::AIChatService;
use crate::bot::client::{FileDownloadError, TelegramBotClient};
use crate::bot::inbound;
use crate::bot::models::Message;
use crate::bot::router::{
    classify_telegram_document_media, ClassifiedTelegramDocument, TelegramDocumentMediaKind,
};
use crate::document;
use crate::util::escape_html;

/// Why a document could not be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DocumentProblem {
    /// A supported format whose content could not be extracted.
    Unreadable { name: String, error: String },
    /// A format Xiao does not read.
    Unsupported { name: String },
}

impl DocumentProblem {
    /// Notice for the chat (Telegram HTML).
    pub(crate) fn notice_html(&self) -> String {
        match self {
            DocumentProblem::Unreadable { name, error } => format!(
                "⚠️ <b>Dokumen tidak dapat diproses.</b>\n\n<code>{}</code>\n{}",
                escape_html(name),
                escape_html(error)
            ),
            DocumentProblem::Unsupported { name } => format!(
                "⚠️ <b>Format dokumen belum didukung.</b>\n\n<code>{}</code> tidak akan dipaksa dibaca sebagai teks biner. Xiao mendukung dokumen teks/kode, PDF, DOCX, XLSX, serta arsip ZIP, TAR/TAR.GZ, dan 7Z.",
                escape_html(name)
            ),
        }
    }
}

/// The readable content of a message's file.
#[derive(Debug, Default)]
pub(crate) struct MessageMedia {
    pub image_bytes: Option<Vec<u8>>,
    pub document_images: Option<Vec<Vec<u8>>>,
    pub mime_type: Option<String>,
    pub doc_text: Option<String>,
    pub doc_name: Option<String>,
    pub audio_bytes: Option<Vec<u8>>,
    pub audio_mime: Option<String>,
    pub audio_duration: i32,
    pub video_bytes: Option<Vec<u8>>,
    pub video_mime: Option<String>,
    pub video_duration: i32,
    /// Why the last download failed, for the "could not download" notice.
    pub download_error: Option<FileDownloadError>,
    pub problem: Option<DocumentProblem>,
}

impl MessageMedia {
    /// Whether anything usable was read.
    pub(crate) fn has_content(&self) -> bool {
        self.image_bytes.is_some()
            || self.audio_bytes.is_some()
            || self.video_bytes.is_some()
            || self.doc_text.is_some()
            || self
                .document_images
                .as_ref()
                .is_some_and(|pages| !pages.is_empty())
    }
}

/// Downloads and reads the file `message` carries. Nothing is sent to the
/// chat: failures are reported through `download_error` and `problem`.
pub(crate) async fn load_message_media(
    bot: &TelegramBotClient,
    ai_service: &AIChatService,
    message: &Message,
) -> MessageMedia {
    let mut media = MessageMedia::default();

    if let Some(voice) = message.voice.as_ref() {
        media.audio_duration = voice.duration;
        media.audio_mime = voice.mime_type.clone();
        match bot.get_file_bytes(&voice.file_id).await {
            Ok((data, path)) => {
                media.audio_bytes = Some(data);
                media.doc_name = path.split('/').next_back().map(str::to_string);
            }
            Err(error) => media.download_error = Some(error),
        }
    } else if let Some(audio) = message.audio.as_ref() {
        media.audio_duration = audio.duration;
        media.audio_mime = audio.mime_type.clone();
        match bot.get_file_bytes(&audio.file_id).await {
            Ok((data, path)) => {
                media.audio_bytes = Some(data);
                media.doc_name = audio
                    .file_name
                    .clone()
                    .or_else(|| path.split('/').next_back().map(str::to_string));
            }
            Err(error) => media.download_error = Some(error),
        }
    } else if let Some(video) = message.video.as_ref() {
        media.video_duration = video.duration;
        match bot.get_file_bytes(&video.file_id).await {
            Ok((data, path)) => {
                media.video_bytes = Some(data);
                let ext = path.split('.').next_back().unwrap_or("mp4");
                media.video_mime = video
                    .mime_type
                    .clone()
                    .or_else(|| Some(format!("video/{ext}")));
            }
            Err(error) => media.download_error = Some(error),
        }
    } else if let Some(note) = message.video_note.as_ref() {
        media.video_duration = note.duration;
        match bot.get_file_bytes(&note.file_id).await {
            Ok((data, _)) => {
                media.video_bytes = Some(data);
                media.video_mime = Some("video/mp4".to_string());
            }
            Err(error) => media.download_error = Some(error),
        }
    } else if message.photo.is_some() || message.live_photo.is_some() {
        // A live photo's motion clip goes to the Video route when one can
        // take it; otherwise (or if that download fails) the still photo is
        // used like any other photo.
        let live_clip = match message.live_photo.as_ref() {
            Some(live) if inbound::live_photo_video_supported(ai_service).await => bot
                .get_file_bytes(&live.file_id)
                .await
                .ok()
                .map(|(data, _)| (data, live)),
            _ => None,
        };
        let still = message.photo.as_ref().or_else(|| {
            message
                .live_photo
                .as_ref()
                .and_then(|live| live.photo.as_ref())
        });
        if let Some((data, live)) = live_clip {
            media.video_bytes = Some(data);
            media.video_mime = Some(
                live.mime_type
                    .clone()
                    .unwrap_or_else(|| "video/mp4".to_string()),
            );
            media.video_duration = live.duration;
        } else if let Some(largest) = still.and_then(|photos| photos.last()) {
            match bot.get_file_bytes(&largest.file_id).await {
                Ok((data, path)) => {
                    media.image_bytes = Some(data);
                    let ext = path.split('.').next_back().unwrap_or("jpeg");
                    media.mime_type = Some(if ext == "jpg" {
                        "image/jpeg".to_string()
                    } else {
                        format!("image/{ext}")
                    });
                }
                Err(error) => media.download_error = Some(error),
            }
        }
    } else if let Some(file_id) = message
        .sticker
        .as_ref()
        .and_then(inbound::sticker_image_file_id)
    {
        // Best effort: without the picture the emoji description remains,
        // so a failed download is not reported.
        if let Ok((data, path)) = bot.get_file_bytes(file_id).await {
            media.image_bytes = Some(data);
            media.mime_type = Some(inbound::image_mime_from_path(&path));
        }
    } else if let Some(doc) = message.document.as_ref() {
        let d_mime = doc.mime_type.clone().unwrap_or_default();
        let d_name = doc
            .file_name
            .clone()
            .unwrap_or_else(|| "dokumen".to_string());
        match bot.get_file_bytes(&doc.file_id).await {
            Err(error) => media.download_error = Some(error),
            Ok((data, path)) => {
                let ClassifiedTelegramDocument {
                    kind,
                    mime_type: resolved_mime,
                } = classify_telegram_document_media(&d_mime, &d_name, &path);
                match kind {
                    TelegramDocumentMediaKind::Image => {
                        media.image_bytes = Some(data);
                        media.mime_type = resolved_mime;
                    }
                    TelegramDocumentMediaKind::Audio => {
                        media.audio_bytes = Some(data);
                        media.audio_mime = resolved_mime;
                        media.doc_name = Some(d_name);
                    }
                    TelegramDocumentMediaKind::Video => {
                        media.video_bytes = Some(data);
                        media.video_mime = resolved_mime;
                    }
                    TelegramDocumentMediaKind::Other
                        if document::is_extractable_document(&d_mime, &d_name) =>
                    {
                        match document::extract_document(data, &d_mime, &d_name).await {
                            Ok(extracted) => {
                                media.doc_text = extracted.text;
                                if !extracted.rendered_pages.is_empty() {
                                    media.document_images = Some(extracted.rendered_pages);
                                }
                                media.doc_name = Some(d_name);
                                if let Some(warning) = extracted.warning {
                                    info!("{warning}");
                                }
                            }
                            Err(error) => {
                                media.problem = Some(DocumentProblem::Unreadable {
                                    name: d_name,
                                    error,
                                });
                            }
                        }
                    }
                    TelegramDocumentMediaKind::Other => {
                        media.problem = Some(DocumentProblem::Unsupported { name: d_name });
                    }
                }
            }
        }
    }
    media
}
