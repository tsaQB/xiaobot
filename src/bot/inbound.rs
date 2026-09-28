//! Telegram message kinds beyond plain text and the classic media types:
//! stickers, shared locations and venues, live photos (Bot API 10.0),
//! forwarded rich messages (Bot API 10.1) and edited messages.
//!
//! Everything here turns such a message into something the existing chat
//! pipeline already understands: extra prompt text or an image/video payload.

use crate::ai::service::{AIChatService, ModelRole, RouteOrigin};
use crate::ai::storage::{CapabilityKind, CapabilityState};
use crate::bot::models::{Message, Sticker, RICH_MESSAGE_MAX_TEXT_CHARS};

/// An edited message is answered again only when the edit happens within this
/// many seconds of the original message.
pub const EDIT_WINDOW_SECS: i64 = 600;
/// Safety cap on text taken from a forwarded rich message or a replied-to
/// message. Telegram already limits a rich message to
/// `RICH_MESSAGE_MAX_TEXT_CHARS` (32,768) characters; the readable rendering
/// adds link targets and structure markers on top, so the cap is twice that:
/// in practice a message Telegram accepted is never cut, and the cap only
/// guards against malformed input.
pub const MAX_QUOTED_CHARS: usize = 2 * RICH_MESSAGE_MAX_TEXT_CHARS;
const TRUNCATED_NOTE: &str = "\n… (teks dipotong karena terlalu panjang)";

/// Applies [`MAX_QUOTED_CHARS`], saying so when anything was cut.
fn bounded_quote(text: &str) -> String {
    if text.chars().count() <= MAX_QUOTED_CHARS {
        return text.to_string();
    }
    let mut bounded = crate::util::truncate_chars(text, MAX_QUOTED_CHARS);
    bounded.push_str(TRUNCATED_NOTE);
    bounded
}

/// The user-visible text of a message: its text, or the media caption.
pub fn message_text(message: &Message) -> &str {
    message
        .text
        .as_deref()
        .or(message.caption.as_deref())
        .unwrap_or("")
        .trim()
}

/// Stable fingerprint of a message's text, used to tell a real text edit
/// apart from edits of fields Xiao does not use. Only the hash is stored.
pub fn prompt_content_hash(message: &Message) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in message_text(message).bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// Whether an edited message falls inside the answering window.
pub fn edit_within_window(message: &Message) -> bool {
    message
        .edit_date
        .is_some_and(|edited| edited >= message.date && edited - message.date <= EDIT_WINDOW_SECS)
}

fn maps_link(latitude: f64, longitude: f64) -> String {
    format!("https://maps.google.com/?q={latitude:.6},{longitude:.6}")
}

/// Plain text of a forwarded rich message, bounded. Only bots can author rich
/// messages, so one reaching Xiao was forwarded. Should Telegram ever add a
/// plain `text` fallback, that text is already the prompt and is not repeated.
pub fn forwarded_rich_text(message: &Message) -> Option<String> {
    if message.text.is_some() {
        return None;
    }
    message.rich_message_text().map(|text| bounded_quote(&text))
}

/// Prompt text describing content that carries no text of its own: a venue
/// or location, a sticker, or a forwarded rich message. `None` when the
/// message has none of these.
pub fn describe_extra_content(message: &Message) -> Option<String> {
    let mut parts = Vec::new();

    if let Some(venue) = message.venue.as_ref() {
        let address = venue.address.trim();
        let place = if address.is_empty() {
            venue.title.trim().to_string()
        } else {
            format!("{}, {address}", venue.title.trim())
        };
        parts.push(format!(
            "[Tempat dibagikan: {place} — koordinat {:.6}, {:.6} · {}]",
            venue.location.latitude,
            venue.location.longitude,
            maps_link(venue.location.latitude, venue.location.longitude)
        ));
    } else if let Some(location) = message.location.as_ref() {
        parts.push(format!(
            "[Lokasi dibagikan: {:.6}, {:.6} · {}]",
            location.latitude,
            location.longitude,
            maps_link(location.latitude, location.longitude)
        ));
    }

    if let Some(sticker) = message.sticker.as_ref() {
        parts.push(describe_sticker(sticker));
    }

    if let Some(text) = forwarded_rich_text(message) {
        parts.push(format!("[Pesan rich yang diteruskan]\n{text}"));
    }

    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

fn describe_sticker(sticker: &Sticker) -> String {
    let emoji = sticker
        .emoji
        .as_deref()
        .map(str::trim)
        .filter(|emoji| !emoji.is_empty())
        .unwrap_or("tanpa emoji");
    let kind = if sticker.is_video {
        " (stiker video)"
    } else if sticker.is_animated {
        " (stiker animasi)"
    } else {
        ""
    };
    match sticker
        .set_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(set) => format!("[Stiker {emoji}{kind} dari set \"{set}\"]"),
        None => format!("[Stiker {emoji}{kind}]"),
    }
}

/// The file to show a vision model for a sticker: the sticker itself when it
/// is a regular WEBP image, otherwise its thumbnail (animated TGS and video
/// WEBM stickers cannot be read as images).
pub fn sticker_image_file_id(sticker: &Sticker) -> Option<&str> {
    if sticker.is_static_image() {
        Some(sticker.file_id.as_str())
    } else {
        sticker
            .thumbnail
            .as_ref()
            .map(|thumbnail| thumbnail.file_id.as_str())
    }
}

/// MIME type of a downloaded Telegram image, from its file path.
pub fn image_mime_from_path(path: &str) -> String {
    match path
        .rsplit('.')
        .next()
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("webp") => "image/webp".to_string(),
        Some("png") => "image/png".to_string(),
        Some("gif") => "image/gif".to_string(),
        _ => "image/jpeg".to_string(),
    }
}

/// Whether the Video route can take the video part of a live photo: either
/// a dedicated Video model is configured, or the inherited main model has
/// verified video-input support. Otherwise only the still photo is used.
pub async fn live_photo_video_supported(ai_service: &AIChatService) -> bool {
    let snapshot = ai_service.generation_model_snapshot().await;
    AIChatService::resolve_model_route_from_snapshot(&snapshot, ModelRole::Video).is_ok_and(
        |route| {
            route.route_origin == RouteOrigin::Specific
                || route
                    .capability
                    .effective_state_for(CapabilityKind::VideoInput)
                    == CapabilityState::Supported
        },
    )
}

/// Text of the message a user replied to, for use as context (guest mode).
pub fn replied_message_context(message: &Message) -> Option<String> {
    let replied = message.reply_to_message.as_deref()?;
    let mut parts = Vec::new();
    let text = message_text(replied);
    if !text.is_empty() {
        parts.push(text.to_string());
    }
    if let Some(extra) = describe_extra_content(replied) {
        parts.push(extra);
    }
    if parts.is_empty() {
        return None;
    }
    Some(bounded_quote(&parts.join("\n\n")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(json: &str) -> Message {
        serde_json::from_str(json).expect("valid Telegram message JSON")
    }

    #[test]
    fn location_and_venue_become_prompt_context() {
        let location = message(
            r#"{"message_id":1,"date":1,"chat":{"id":5,"type":"private"},
                "location":{"latitude":-8.41,"longitude":116.45,"live_period":60}}"#,
        );
        let text = describe_extra_content(&location).expect("location described");
        assert!(
            text.contains("Lokasi dibagikan: -8.410000, 116.450000"),
            "{text}"
        );
        assert!(text.contains("https://maps.google.com/?q=-8.410000,116.450000"));

        let venue = message(
            r#"{"message_id":2,"date":1,"chat":{"id":5,"type":"private"},
                "location":{"latitude":-8.41,"longitude":116.45},
                "venue":{"location":{"latitude":-8.41,"longitude":116.45},
                         "title":"Segara Anak","address":"Lombok"}}"#,
        );
        let text = describe_extra_content(&venue).expect("venue described");
        assert!(
            text.starts_with("[Tempat dibagikan: Segara Anak, Lombok"),
            "{text}"
        );
        assert!(
            !text.contains("Lokasi dibagikan"),
            "venue replaces the plain location"
        );
    }

    #[test]
    fn stickers_use_the_image_or_its_thumbnail() {
        let regular = message(
            r#"{"message_id":3,"date":1,"chat":{"id":5,"type":"private"},
                "sticker":{"file_id":"S","file_unique_id":"u","type":"regular","width":512,
                           "height":512,"is_animated":false,"is_video":false,
                           "emoji":"😂","set_name":"LaughPack"}}"#,
        );
        let sticker = regular.sticker.as_ref().expect("sticker parsed");
        assert_eq!(sticker_image_file_id(sticker), Some("S"));
        assert_eq!(
            describe_extra_content(&regular).as_deref(),
            Some("[Stiker 😂 dari set \"LaughPack\"]")
        );

        let animated = message(
            r#"{"message_id":4,"date":1,"chat":{"id":5,"type":"private"},
                "sticker":{"file_id":"A","file_unique_id":"u","type":"regular","width":512,
                           "height":512,"is_animated":true,"is_video":false,"emoji":"🔥",
                           "thumbnail":{"file_id":"T","file_unique_id":"t","width":128,"height":128}}}"#,
        );
        let sticker = animated.sticker.as_ref().expect("sticker parsed");
        assert_eq!(sticker_image_file_id(sticker), Some("T"));
        assert!(describe_extra_content(&animated)
            .expect("described")
            .contains("stiker animasi"));
    }

    #[test]
    fn forwarded_rich_message_text_is_extracted() {
        let forwarded = message(
            r#"{"message_id":5,"date":1,"chat":{"id":5,"type":"private"},
                "rich_message":{"blocks":[
                    {"type":"heading","text":"Ringkasan","size":2},
                    {"type":"paragraph","text":["Poin ",{"type":"bold","text":"penting"}]}
                ]}}"#,
        );
        let text = describe_extra_content(&forwarded).expect("rich text described");
        assert!(text.starts_with("[Pesan rich yang diteruskan]"));
        assert!(text.contains("## Ringkasan"));
        assert!(text.contains("Poin penting"));
    }

    #[test]
    fn a_maximum_size_forwarded_rich_message_is_never_cut() {
        // The longest text Telegram accepts, split over many list items so the
        // rendering adds its own markers on top.
        let item_text = "a".repeat(RICH_MESSAGE_MAX_TEXT_CHARS / 400);
        let items: Vec<serde_json::Value> = (0..400)
            .map(|_| {
                serde_json::json!({"label": "•",
                    "blocks": [{"type": "paragraph", "text": item_text}]})
            })
            .collect();
        let forwarded: Message = serde_json::from_value(serde_json::json!({
            "message_id": 9, "date": 1, "chat": {"id": 5, "type": "private"},
            "rich_message": {"blocks": [{"type": "list", "items": items}]}
        }))
        .expect("valid rich message");

        let text = forwarded_rich_text(&forwarded).expect("rich text present");
        assert_eq!(
            text.matches(item_text.as_str()).count(),
            400,
            "nothing was cut"
        );
        assert!(!text.contains("dipotong"));
    }

    #[test]
    fn oversized_quotes_are_cut_with_a_visible_note() {
        let huge = "b".repeat(MAX_QUOTED_CHARS + 10);
        let bounded = bounded_quote(&huge);
        assert!(bounded.ends_with("(teks dipotong karena terlalu panjang)"));
        assert_eq!(
            bounded.chars().count(),
            MAX_QUOTED_CHARS + TRUNCATED_NOTE.chars().count()
        );
        assert_eq!(bounded_quote("pendek"), "pendek");
    }

    #[test]
    fn live_photo_keeps_backward_compatible_photo() {
        let live = message(
            r#"{"message_id":6,"date":1,"chat":{"id":5,"type":"private"},
                "photo":[{"file_id":"P","file_unique_id":"p","width":10,"height":10}],
                "live_photo":{"file_id":"V","file_unique_id":"v","width":10,"height":10,
                              "duration":3,"mime_type":"video/mp4"}}"#,
        );
        assert!(live.photo.is_some());
        let live_photo = live.live_photo.as_ref().expect("live photo parsed");
        assert_eq!(live_photo.file_id, "V");
        assert_eq!(live_photo.duration, 3);
    }

    #[test]
    fn edit_window_and_text_fingerprint() {
        let edited = message(
            r#"{"message_id":7,"date":1000,"edit_date":1300,"chat":{"id":5,"type":"private"},
                "text":"halo"}"#,
        );
        assert!(edit_within_window(&edited));
        let late = message(
            r#"{"message_id":7,"date":1000,"edit_date":1601,"chat":{"id":5,"type":"private"},
                "text":"halo"}"#,
        );
        assert!(!edit_within_window(&late));
        let changed = message(
            r#"{"message_id":7,"date":1000,"edit_date":1300,"chat":{"id":5,"type":"private"},
                "text":"halo lagi"}"#,
        );
        assert_ne!(prompt_content_hash(&edited), prompt_content_hash(&changed));
        assert_eq!(prompt_content_hash(&edited), prompt_content_hash(&late));
    }

    #[test]
    fn replied_message_becomes_bounded_context() {
        let reply = message(
            r#"{"message_id":8,"date":1,"chat":{"id":-100,"type":"supergroup"},
                "text":"@XiaoBot jelaskan",
                "reply_to_message":{"message_id":2,"date":1,"chat":{"id":-100,"type":"supergroup"},
                                    "text":"E = mc^2"}}"#,
        );
        assert_eq!(replied_message_context(&reply).as_deref(), Some("E = mc^2"));
    }
}
