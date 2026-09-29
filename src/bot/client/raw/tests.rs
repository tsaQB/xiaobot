use super::*;
use crate::bot::models::{Location, RichBlockCaption};

#[test]
fn split_text_chunks_preserves_unicode_and_bounds() {
    let client = TelegramBotClient;
    let input = format!("{}\n{}", "😊世界".repeat(900), "x".repeat(5000));
    let chunks = client.split_text_chunks(&input, 3800);

    assert!(chunks.len() >= 2);
    assert!(chunks.iter().all(|chunk| chunk.chars().count() <= 3800));
    assert_eq!(chunks.concat(), input);
}

#[test]
fn oversized_rich_block_fallback_never_splits_html_entities() {
    let client = TelegramBotClient;
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
    let client = TelegramBotClient;
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
    let client = TelegramBotClient;
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
    let client = TelegramBotClient;
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
    let client = TelegramBotClient;
    let blocks = vec![RichBlock::Thinking {
        text: Value::String("🧩 Thinking\n1s •".to_string()),
    }];
    let html = client.render_blocks_to_html(&blocks);
    assert!(!html.contains("<blockquote"));
    assert!(!html.contains("Thinking:"));
    assert!(html.contains("🧩 Thinking\n1s •"));
}

#[tokio::test]
async fn download_budget_bounds_the_whole_transfer() {
    // A transfer that never completes must be abandoned when the budget
    // expires, without depending on a real network endpoint.
    let started = std::time::Instant::now();
    let result: Option<()> = TelegramBotClient::with_download_budget(
        Duration::from_millis(20),
        std::future::pending::<Option<()>>(),
    )
    .await;
    assert!(result.is_none(), "an expired budget yields None");
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[tokio::test]
async fn media_download_refuses_private_targets_before_any_request() {
    // SSRF policy: loopback and private targets are rejected up front.
    for url in [
        "http://127.0.0.1/file.png",
        "http://10.0.0.1/file.png",
        "http://[::1]/file.png",
        "file:///etc/passwd",
    ] {
        assert!(
            TelegramBotClient
                .download_media_bytes_with_budget(url, 1024, Duration::from_secs(2))
                .await
                .is_none(),
            "{url} must be refused"
        );
    }
}

fn collage_item(media: &str) -> Value {
    serde_json::json!({"type": "photo", "photo": {"type": "photo", "media": media}})
}

/// Only the selected remote pictures of a collage become links; uploads and
/// file ids stay in the collage, since neither is an address anyone can open.
#[test]
fn convert_media_to_rich_links_links_only_selected_gallery_items() {
    let client = TelegramBotClient;
    let msg = InputRichMessage::new(vec![RichBlock::Collage {
        blocks: vec![
            collage_item("attach://file_0"),
            collage_item("https://example.com/1.jpg"),
            collage_item("https://example.com/2.jpg"),
            collage_item("AgACAgPHOTOID"),
        ],
        caption: Some(RichBlockCaption::new(Value::String("Album".to_string()))),
    }]);

    let converted = client.convert_media_to_rich_links(&msg, &|url| url.ends_with("/1.jpg"));
    assert_eq!(converted.blocks.len(), 2);
    let RichBlock::Collage { blocks, caption } = &converted.blocks[0] else {
        panic!("the pictures that still load stay a collage");
    };
    assert_eq!(blocks.len(), 3);
    assert!(caption.is_some(), "the collage keeps its caption");
    let links = serde_json::to_string(&converted.blocks[1]).expect("serialize links paragraph");
    assert!(links.contains("🖼️ Lainnya: "), "{links}");
    assert!(links.contains("Foto #2") && links.contains("https://example.com/1.jpg"));
    assert!(
        !links.contains("2.jpg"),
        "unselected pictures are not linked"
    );

    let converted = client.convert_remote_media_to_rich_links(&msg);
    let RichBlock::Collage { blocks, .. } = &converted.blocks[0] else {
        panic!("uploads and file ids stay a collage");
    };
    assert_eq!(blocks.len(), 2);
    let links = serde_json::to_string(&converted.blocks[1]).expect("serialize links paragraph");
    assert!(
        links.contains("Foto #2") && links.contains("Foto #3"),
        "{links}"
    );
    assert!(!links.contains("attach://") && !links.contains("AgACAgPHOTOID"));
}

/// A gallery left with one picture becomes a single photo block.
#[test]
fn convert_media_to_rich_links_turns_lone_gallery_item_into_a_photo() {
    let client = TelegramBotClient;
    let msg = InputRichMessage::new(vec![RichBlock::Slideshow {
        blocks: vec![
            collage_item("attach://file_0"),
            collage_item("https://example.com/1.jpg"),
        ],
        caption: None,
    }]);
    let converted = client.convert_remote_media_to_rich_links(&msg);
    assert_eq!(converted.blocks.len(), 2);
    let RichBlock::Photo { photo, .. } = &converted.blocks[0] else {
        panic!("a lone slide becomes a photo block");
    };
    assert_eq!(photo["media"], "attach://file_0");
    let links = serde_json::to_string(&converted.blocks[1]).expect("serialize links paragraph");
    assert!(links.contains("Slide #2"), "{links}");

    let untouched = client.convert_media_to_rich_links(&msg, &|_| false);
    assert_eq!(
        untouched.blocks, msg.blocks,
        "nothing selected, nothing changed"
    );
}
