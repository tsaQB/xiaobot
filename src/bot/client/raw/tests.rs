use super::*;
use crate::bot::models::{Location, RichBlockCaption};

#[test]
fn split_text_chunks_preserves_unicode_and_bounds() {
    let client = TelegramBotClient::new("test-token");
    let input = format!("{}\n{}", "😊世界".repeat(900), "x".repeat(5000));
    let chunks = client.split_text_chunks(&input, 3800);

    assert!(chunks.len() >= 2);
    assert!(chunks.iter().all(|chunk| chunk.chars().count() <= 3800));
    assert_eq!(chunks.concat(), input);
}

#[test]
fn oversized_rich_block_fallback_never_splits_html_entities() {
    let client = TelegramBotClient::new("test-token");
    let blocks = vec![RichBlock::Paragraph {
        text: Value::String("<&😊>".repeat(2000)),
    }];
    let chunks = client.render_blocks_to_html_chunks(&blocks, 256);

    assert!(chunks.len() > 1);
    assert!(chunks.iter().all(|chunk| chunk.chars().count() <= 256));
    assert!(chunks.iter().all(|chunk| {
        let amp = chunk.matches('&').count();
        amp == chunk.matches("&lt;").count()
            + chunk.matches("&gt;").count()
            + chunk.matches("&amp;").count()
            + chunk.matches("&quot;").count()
            + chunk.matches("&#x27;").count()
    }));
}

#[test]
fn rich_media_blocks_render_to_plain_and_html_fallback() {
    let client = TelegramBotClient::new("test-token");
    let blocks = vec![
        RichBlock::Photo {
            photo: serde_json::json!({"type": "photo", "media": "photo_1"}),
            caption: Some(RichBlockCaption::new(Value::String(
                "Foto sunset".to_string(),
            ))),
        },
        RichBlock::Video {
            video: serde_json::json!({"type": "video", "media": "video_1"}),
            caption: Some(RichBlockCaption::new(Value::String(
                "Video clip".to_string(),
            ))),
        },
        RichBlock::Audio {
            audio: serde_json::json!({"type": "audio", "media": "audio_1"}),
            caption: None,
        },
        RichBlock::VoiceNote {
            voice_note: serde_json::json!({"type": "voice", "media": "voice_1"}),
            caption: None,
        },
        RichBlock::Animation {
            animation: serde_json::json!({"type": "animation", "media": "anim_1"}),
            caption: None,
        },
        RichBlock::Map {
            location: Location {
                latitude: -5.14,
                longitude: 119.43,
                horizontal_accuracy: None,
            },
            zoom: Some(12),
            width: None,
            height: None,
        },
    ];
    let plain = client
        .render_blocks_to_plain_chunks(&blocks, 4000)
        .join("\n");
    assert!(plain.contains("[Photo] Foto sunset"));
    assert!(plain.contains("[Video] Video clip"));
    assert!(plain.contains("[Audio]"));
    assert!(plain.contains("[Voice Note]"));
    assert!(plain.contains("[Animation]"));
    assert!(plain.contains("[Map: lat=-5.14, lon=119.43 zoom=12]"));

    let html = client.render_blocks_to_html(&blocks);
    assert!(html.contains("🖼️ <b><a href=\"photo_1\">Foto sunset</a></b>"));
    assert!(html.contains("🎥 <b><a href=\"video_1\">Video clip</a></b>"));
    assert!(html.contains("🎵 <b><a href=\"audio_1\">Dengarkan Audio</a></b>"));
    assert!(html.contains("🎤 <b><a href=\"voice_1\">Pesan Suara</a></b>"));
    assert!(html.contains("🎞️ <b><a href=\"anim_1\">Lihat Animasi</a></b>"));
    assert!(html.contains("📍 <b><a href=\"https://www.google.com/maps?q=-5.14,119.43\">Lokasi Peta (-5.14, 119.43?z=12)</a></b>"));
}

#[test]
fn semantic_plain_fallback_is_rendered_from_ast_not_raw_markdown() {
    let client = TelegramBotClient::new("test-token");
    let source = "### Heading\n\n**bold** and `code`\n\n---\n\n[link](https://example.com)";
    let blocks = crate::parser::parse_markdown_to_rich_blocks(source);
    let plain = client
        .render_blocks_to_plain_chunks(&blocks, 4000)
        .join("\n");
    assert!(plain.contains("Heading"));
    assert!(plain.contains("bold"));
    assert!(plain.contains("code"));
    assert!(plain.contains("link"));
    assert!(!plain.contains("###"));
    assert!(!plain.contains("**"));
    assert!(!plain.contains('`'));
    assert!(!plain.contains("]("));
}

#[test]
fn convert_remote_media_to_rich_links_transforms_remote_blocks_without_local_download() {
    let client = TelegramBotClient::new("test-token");
    let blocks = vec![
        RichBlock::Photo {
            photo: serde_json::json!({"type": "photo", "media": "https://example.com/cat.jpg"}),
            caption: Some(RichBlockCaption::new(Value::String(
                "Kucing Manis".to_string(),
            ))),
        },
        RichBlock::Video {
            video: serde_json::json!({"type": "video", "media": "https://example.com/movie.mp4"}),
            caption: None,
        },
        RichBlock::Collage {
            blocks: vec![
                serde_json::json!({"type": "photo", "photo": {"type": "photo", "media": "https://example.com/p1.jpg"}}),
                serde_json::json!({"type": "photo", "photo": {"type": "photo", "media": "https://example.com/p2.jpg"}}),
            ],
            caption: Some(RichBlockCaption::new(Value::String("Dua Foto".to_string()))),
        },
        RichBlock::Paragraph {
            text: Value::String("Teks biasa tetap utuh".to_string()),
        },
    ];
    let msg = InputRichMessage::new(blocks);
    let converted = client.convert_remote_media_to_rich_links(&msg);
    assert_eq!(converted.blocks.len(), 4);
    assert!(matches!(converted.blocks[0], RichBlock::Paragraph { .. }));
    assert!(matches!(converted.blocks[1], RichBlock::Paragraph { .. }));
    assert!(matches!(converted.blocks[2], RichBlock::Paragraph { .. }));
    assert!(matches!(converted.blocks[3], RichBlock::Paragraph { .. }));

    let s0 = serde_json::to_string(&converted.blocks[0]).expect("serialize block 0 succeeds");
    assert!(
        s0.contains("🖼️")
            && s0.contains("Kucing Manis")
            && s0.contains("https://example.com/cat.jpg")
    );

    let s1 = serde_json::to_string(&converted.blocks[1]).expect("serialize block 1 succeeds");
    assert!(
        s1.contains("🎬")
            && s1.contains("Tonton Video")
            && s1.contains("https://example.com/movie.mp4")
    );

    let s2 = serde_json::to_string(&converted.blocks[2]).expect("serialize block 2 succeeds");
    assert!(s2.contains("🖼️ [Dua Foto]:") && s2.contains("Foto #1") && s2.contains("Foto #2"));
}

#[test]
fn thinking_block_renders_as_clean_text_without_quote_or_duplicate_header() {
    let client = TelegramBotClient::new("test-token");
    let blocks = vec![RichBlock::Thinking {
        text: Value::String("🧩 Thinking\n1s •".to_string()),
    }];
    let html = client.render_blocks_to_html(&blocks);
    assert!(!html.contains("<blockquote"));
    assert!(!html.contains("Thinking:"));
    assert!(html.contains("🧩 Thinking\n1s •"));
}

#[tokio::test]
async fn download_media_bytes_stops_within_bounded_time_when_budget_expires() {
    let client = TelegramBotClient::new("test-token");
    let start = std::time::Instant::now();
    let res = client
        .download_media_bytes_with_budget(
            "https://1.1.1.1/slow-download.bin",
            1024,
            Duration::from_millis(15),
        )
        .await;
    let elapsed = start.elapsed();
    assert!(res.is_none(), "timed-out download must return None");
    assert!(
        elapsed < Duration::from_secs(2),
        "download must abort within bounded time, took {:?}",
        elapsed
    );
}

#[test]
fn download_media_bytes_wraps_in_total_30s_timeout_budget() {
    let source = include_str!("../raw.rs");
    let start = source
        .find("pub async fn download_media_bytes")
        .expect("download_media_bytes must exist");
    let tail = &source[start..];
    let end = tail
        .find("pub async fn send_photo(")
        .expect("send_photo must follow");
    let body = &tail[..end];
    assert!(
        body.contains("tokio::time::timeout(Duration::from_secs(30)"),
        "download_media_bytes must wrap redirect loop and streaming in a 30s total budget"
    );
    assert!(
        body.contains(".flatten()"),
        "elapsed timeout must return None via .ok().flatten()"
    );
}

#[test]
fn edit_message_media_serializes_with_markup_correctly() {
    let media = InputMedia::photo(
        "https://example.com/slide1.jpg",
        Some("Slide 1".to_string()),
        Some("HTML".to_string()),
    );
    let button = crate::bot::models::InlineKeyboardButton::callback("Next", "carousel:id:1:next");
    let markup = InlineKeyboardMarkup::new(vec![vec![button]]);

    let media_json = match serde_json::to_value(&media) {
        Ok(v) => v,
        Err(e) => panic!("serialization failed: {e}"),
    };
    let markup_json = match serde_json::to_value(&markup) {
        Ok(v) => v,
        Err(e) => panic!("serialization failed: {e}"),
    };

    let payload = json!({
        "chat_id": 12345_i64,
        "message_id": 67890_i64,
        "media": media_json,
        "reply_markup": markup_json,
    });

    assert_eq!(payload["chat_id"], 12345_i64);
    assert_eq!(payload["message_id"], 67890_i64);
    assert_eq!(payload["media"]["type"], "photo");
    assert_eq!(payload["media"]["media"], "https://example.com/slide1.jpg");
    assert_eq!(payload["media"]["caption"], "Slide 1");
    assert_eq!(payload["media"]["parse_mode"], "HTML");
    assert_eq!(
        payload["reply_markup"]["inline_keyboard"][0][0]["text"],
        "Next"
    );
    assert_eq!(
        payload["reply_markup"]["inline_keyboard"][0][0]["callback_data"],
        "carousel:id:1:next"
    );
}

#[test]
fn edit_message_media_source_has_not_modified_error_handling() {
    let source = include_str!("../raw.rs");
    assert!(
        source.contains("pub async fn edit_message_media("),
        "raw.rs must expose edit_message_media"
    );
    assert!(
        source.contains("\"editMessageMedia\""),
        "raw.rs must use editMessageMedia endpoint"
    );
    assert!(
        source.contains("message is not modified"),
        "raw.rs must handle message is not modified cleanly"
    );
}
