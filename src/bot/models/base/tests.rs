use super::*;

#[test]
fn rich_message_requires_exactly_one_representation() {
    let empty = InputRichMessage::default();
    assert!(empty.validate().is_err());

    let mut conflicting = InputRichMessage::new(vec![RichBlock::Paragraph {
        text: Value::String("hello".to_string()),
    }]);
    conflicting.markdown = Some("hello".to_string());
    assert!(conflicting.validate().is_err());

    let valid = InputRichMessage::new(vec![RichBlock::Paragraph {
        text: Value::String("hello".to_string()),
    }]);
    assert!(valid.validate().is_ok());
}

#[test]
fn rich_message_copy_text_button_validates_length_and_action() {
    let button = RichMessageButton::copy("Salin", "teks yang disalin");
    assert!(button.validate().is_ok());

    let value = serde_json::to_value(&button).expect("serialize button succeeds");
    assert_eq!(value["copy_text"]["text"], "teks yang disalin");

    let inline_button = InlineKeyboardButton::copy("Salin Prompt", "prompt text");
    let inline_value =
        serde_json::to_value(&inline_button).expect("serialize inline button succeeds");
    assert_eq!(inline_value["copy_text"]["text"], "prompt text");

    let mut invalid_button = RichMessageButton::copy("Salin", "a".repeat(257));
    assert!(invalid_button.validate().is_err());

    invalid_button.copy_text = Some(CopyTextButton::new(""));
    assert!(invalid_button.validate().is_err());
}

#[test]
fn footer_and_pullquote_serialize_and_validate() {
    let message = InputRichMessage::new(vec![
        RichBlock::PullQuotation {
            text: Value::String("Kutipan penting".to_string()),
            credit: Some(Value::String("Penulis".to_string())),
        },
        RichBlock::Footer {
            text: Value::String("⚡ gpt-4o".to_string()),
        },
    ]);
    assert!(message.validate().is_ok());

    let serialized = serde_json::to_value(&message).expect("serialize message succeeds");
    assert_eq!(serialized["blocks"][0]["type"], "pullquote");
    assert_eq!(serialized["blocks"][0]["text"], "Kutipan penting");
    assert_eq!(serialized["blocks"][1]["type"], "footer");
    assert_eq!(serialized["blocks"][1]["text"], "⚡ gpt-4o");
}

#[test]
fn rich_message_button_requires_exactly_one_action() {
    let mut button = RichMessageButton::callback("Open", "open");
    assert!(button.validate().is_ok());

    button.url = Some("https://example.com".to_string());
    assert!(button.validate().is_err());

    button.callback_data = None;
    assert!(button.validate().is_ok());

    button.url = None;
    assert!(button.validate().is_err());
}

#[test]
fn disabled_inline_button_serializes_as_empty_object() {
    let value = serde_json::to_value(InlineKeyboardButton::disabled("Unavailable"))
        .expect("serialize disabled button succeeds");
    assert_eq!(value["disabled"], serde_json::json!({}));
    assert!(value.get("callback_data").is_none());
}

#[test]
fn bot_api_10_3_stop_update_deserializes() {
    let update: Update = serde_json::from_value(serde_json::json!({
        "update_id": 42,
        "stopped_message_generation": {
            "chat": {"id": 7, "type": "private"},
            "draft_id": 99
        }
    }))
    .expect("deserialize stop update succeeds");
    let stopped = update
        .stopped_message_generation
        .expect("stopped_message_generation present");
    assert_eq!(stopped.chat.id, 7);
    assert_eq!(stopped.draft_id, 99);
}

#[test]
fn rich_message_buttons_follow_10_3_shape() {
    let block = RichBlock::Buttons {
        buttons: vec![RichMessageButton::callback_styled(
            "Retry", "retry", "primary",
        )],
        align: Some("center".to_string()),
    };
    let value = serde_json::to_value(block).expect("serialize button block succeeds");
    assert_eq!(value["type"], "buttons");
    assert_eq!(value["buttons"][0]["callback_data"], "retry");
    assert_eq!(value["buttons"][0]["style"], "primary");
}

#[test]
fn expandable_quote_follows_10_3_shape() {
    let block = RichBlock::ExpandableBlockQuotation {
        text: Value::String("detail".to_string()),
        credit: Some(Value::String("source".to_string())),
    };
    let value = serde_json::to_value(block).expect("serialize quote succeeds");
    assert_eq!(value["type"], "expandable_blockquote");
    assert_eq!(value["text"], "detail");
    assert_eq!(value["credit"], "source");
    assert!(value.get("blocks").is_none());
    assert!(value.get("is_open").is_none());
}

#[test]
fn details_block_uses_summary_and_blocks() {
    let block = RichBlock::Details {
        summary: Value::String("More".to_string()),
        blocks: vec![serde_json::json!({"type": "paragraph", "text": "Body"})],
        is_open: Some(true),
    };
    let value = serde_json::to_value(block).expect("serialize details succeeds");
    assert_eq!(value["summary"], "More");
    assert!(value["blocks"].is_array());
    assert_eq!(value["is_open"], true);
}

#[test]
fn rich_message_text_limit_is_enforced_at_boundary() {
    let at_limit = InputRichMessage::new(vec![RichBlock::Paragraph {
        text: Value::String("x".repeat(RICH_MESSAGE_MAX_TEXT_CHARS)),
    }]);
    assert!(at_limit.validate().is_ok());
    let over = InputRichMessage::new(vec![RichBlock::Paragraph {
        text: Value::String("x".repeat(RICH_MESSAGE_MAX_TEXT_CHARS + 1)),
    }]);
    assert!(over.validate().is_err());
}

#[test]
fn rich_message_block_limit_is_enforced_at_boundary() {
    let paragraph = || RichBlock::Paragraph {
        text: Value::String("x".to_string()),
    };
    let at_limit =
        InputRichMessage::new((0..RICH_MESSAGE_MAX_BLOCKS).map(|_| paragraph()).collect());
    assert!(at_limit.validate().is_ok());
    let over = InputRichMessage::new((0..=RICH_MESSAGE_MAX_BLOCKS).map(|_| paragraph()).collect());
    assert!(over.validate().is_err());
}

#[test]
fn rich_message_media_table_and_button_limits_are_local() {
    let mut message = InputRichMessage::new(vec![RichBlock::Paragraph {
        text: Value::String("ok".to_string()),
    }]);
    message.media = Some(
        (0..RICH_MESSAGE_MAX_MEDIA)
            .map(|i| {
                InputRichMessageMedia::photo(
                    format!("pic_{i}"),
                    format!("https://example.com/pic_{i}.jpg"),
                    None,
                )
                .expect("valid media")
            })
            .collect(),
    );
    assert!(message.validate().is_ok());
    message.media.as_mut().expect("media vector present").push(
        InputRichMessageMedia::photo(
            format!("pic_{RICH_MESSAGE_MAX_MEDIA}"),
            "https://example.com/pic_extra.jpg",
            None,
        )
        .expect("valid media"),
    );
    assert!(message.validate().is_err());

    let table = |columns: usize| {
        InputRichMessage::new(vec![RichBlock::Table {
            cells: vec![(0..columns)
                .map(|_| RichBlockTableCell::text_only("x", false, None))
                .collect()],
            has_header: false,
            is_bordered: true,
            is_striped: false,
            is_compact: true,
            caption: None,
        }])
    };
    assert!(table(RICH_MESSAGE_MAX_TABLE_COLUMNS).validate().is_ok());
    assert!(table(RICH_MESSAGE_MAX_TABLE_COLUMNS + 1)
        .validate()
        .is_err());

    let buttons = |count: usize| {
        InputRichMessage::new(vec![RichBlock::Buttons {
            buttons: (0..count)
                .map(|index| RichMessageButton::callback(format!("b{index}"), format!("c{index}")))
                .collect(),
            align: None,
        }])
    };
    assert!(buttons(RICH_MESSAGE_MAX_BUTTONS_PER_ROW).validate().is_ok());
    assert!(buttons(RICH_MESSAGE_MAX_BUTTONS_PER_ROW + 1)
        .validate()
        .is_err());
}

fn nested_details(depth: usize) -> Value {
    if depth == 0 {
        return serde_json::json!({"type": "paragraph", "text": "leaf"});
    }
    serde_json::json!({
        "type": "details",
        "summary": "nested",
        "blocks": [nested_details(depth - 1)]
    })
}

#[test]
fn rich_media_blocks_serialize_and_validate() {
    let msg = InputRichMessage::new(vec![
        RichBlock::Photo {
            photo: serde_json::json!({"type": "photo", "media": "attach://photo1"}),
            caption: Some(RichBlockCaption::new(Value::String(
                "Pemandangan".to_string(),
            ))),
        },
        RichBlock::Video {
            video: serde_json::json!({"type": "video", "media": "attach://video1"}),
            caption: None,
        },
        RichBlock::Map {
            location: Location {
                latitude: -5.147665,
                longitude: 119.432732,
                horizontal_accuracy: Some(10.0),
            },
            zoom: Some(15),
            width: Some(600),
            height: Some(400),
        },
    ]);
    assert!(msg.validate().is_ok());

    let val = serde_json::to_value(&msg).expect("serialize rich media msg succeeds");
    assert_eq!(val["blocks"][0]["type"], "photo");
    assert_eq!(val["blocks"][0]["caption"]["text"], "Pemandangan");
    assert_eq!(val["blocks"][1]["type"], "video");
    assert_eq!(val["blocks"][2]["type"], "map");
    assert_eq!(val["blocks"][2]["location"]["latitude"], -5.147665);
}

#[test]
fn extended_button_actions_serialize_and_validate() {
    let mut btn = RichMessageButton::callback("Click", "data");
    assert!(btn.validate().is_ok());

    btn.callback_data = None;
    btn.login_url = Some(LoginUrl {
        url: "https://auth.example.com/login".to_string(),
        forward_text: Some("Log in".to_string()),
        bot_username: Some("xiao_bot".to_string()),
        request_write_access: Some(true),
    });
    assert!(btn.validate().is_ok());

    btn.login_url = None;
    btn.switch_inline_query_chosen_chat = Some(SwitchInlineQueryChosenChat {
        query: Some("search query".to_string()),
        allow_user_chats: Some(true),
        allow_bot_chats: Some(false),
        allow_group_chats: Some(true),
        allow_channel_chats: Some(false),
    });
    assert!(btn.validate().is_ok());

    let text_btn = RichTextButton {
        button: btn.clone(),
    };
    let text_val = serde_json::to_value(&text_btn).expect("serialize text_btn succeeds");
    assert!(text_val.get("button").is_some());
}

#[test]
fn rich_message_nesting_limit_is_enforced() {
    // Top-level Details is depth 1, so fifteen nested block levels reaches 16.
    let at_limit = InputRichMessage::new(vec![RichBlock::Details {
        summary: Value::String("root".to_string()),
        blocks: vec![nested_details(RICH_MESSAGE_MAX_NESTING - 2)],
        is_open: None,
    }]);
    assert!(at_limit.validate().is_ok());
    let over = InputRichMessage::new(vec![RichBlock::Details {
        summary: Value::String("root".to_string()),
        blocks: vec![nested_details(RICH_MESSAGE_MAX_NESTING - 1)],
        is_open: None,
    }]);
    assert!(over.validate().is_err());
}

#[test]
fn extract_plain_text_extracts_all_rich_block_types() {
    let msg = InputRichMessage::new(vec![
        RichBlock::Thinking {
            text: Value::String("🧩 Thinking...".to_string()),
        },
        RichBlock::Paragraph {
            text: serde_json::json!([
                {"type": "bold", "text": "Halo"},
                " dunia!"
            ]),
        },
        RichBlock::Preformatted {
            text: "println!(\"hello\");".to_string(),
            language: Some("rust".to_string()),
        },
    ]);
    let plain = msg.extract_plain_text();
    assert!(plain.contains("🧩 Thinking..."));
    assert!(plain.contains("Halo dunia!"));
    assert!(plain.contains("println!(\"hello\");"));
}

#[test]
fn input_rich_message_media_id_validation() {
    // Valid IDs: 1 to 64 chars, ASCII alphanumeric, underscore and hyphen (Bot API 10.3)
    assert!(InputRichMessageMedia::validate_id("a").is_ok());
    assert!(InputRichMessageMedia::validate_id("Z").is_ok());
    assert!(InputRichMessageMedia::validate_id("0").is_ok());
    assert!(InputRichMessageMedia::validate_id("_").is_ok());
    assert!(InputRichMessageMedia::validate_id("photo_1_preview").is_ok());
    assert!(InputRichMessageMedia::validate_id(&"x".repeat(64)).is_ok());

    // Invalid IDs: empty, > 64 chars, or containing invalid characters
    assert!(InputRichMessageMedia::validate_id("").is_err());
    assert!(InputRichMessageMedia::validate_id(&"x".repeat(65)).is_err());
    assert!(InputRichMessageMedia::validate_id("photo 1").is_err());
    assert!(InputRichMessageMedia::validate_id("photo-1").is_ok());
    assert!(InputRichMessageMedia::validate_id("photo.jpg").is_err());
    assert!(InputRichMessageMedia::validate_id("pic@home").is_err());
    assert!(InputRichMessageMedia::validate_id("foto#1").is_err());
}

#[test]
fn input_rich_message_media_constructors() {
    let photo = InputRichMessageMedia::photo(
        "p1",
        "https://example.com/pic.jpg",
        Some("Caption".to_string()),
    );
    assert!(photo.is_ok());
    let photo = photo.expect("valid photo");
    assert_eq!(photo.id, "p1");
    assert_eq!(photo.media.media_url(), "https://example.com/pic.jpg");

    let audio = InputRichMessageMedia::audio(
        "a1",
        "https://example.com/sound.mp3",
        Some("Title".to_string()),
        Some("Artist".to_string()),
        None,
    );
    assert!(audio.is_ok());
    let audio = audio.expect("valid audio");
    assert_eq!(audio.id, "a1");
    assert_eq!(audio.media.media_url(), "https://example.com/sound.mp3");

    let doc = InputRichMessageMedia::document("d1", "https://example.com/file.pdf", None);
    assert!(doc.is_ok());

    let vid = InputRichMessageMedia::video("v1", "https://example.com/vid.mp4", None);
    assert!(vid.is_ok());
}

#[test]
fn input_rich_message_unique_media_ids_enforced() {
    let item1 =
        InputRichMessageMedia::photo("same_id", "https://example.com/1.jpg", None).expect("valid");
    let item2 =
        InputRichMessageMedia::photo("same_id", "https://example.com/2.jpg", None).expect("valid");

    let msg = InputRichMessage::from_html("<p>test</p>", Some(vec![item1, item2]));
    let err = msg
        .validate()
        .expect_err("Duplicate media IDs must be rejected");
    assert!(err.contains("unique"), "Error must mention unique: {err}");
    assert!(
        err.contains("same_id"),
        "Error must mention duplicated ID: {err}"
    );

    // Distinct IDs are valid
    let item3 =
        InputRichMessageMedia::photo("diff_id", "https://example.com/2.jpg", None).expect("valid");
    let item1 =
        InputRichMessageMedia::photo("same_id", "https://example.com/1.jpg", None).expect("valid");
    let valid_msg = InputRichMessage::from_html("<p>test</p>", Some(vec![item1, item3]));
    assert!(valid_msg.validate().is_ok());
}

#[test]
fn input_rich_message_wire_serialization_matches_bot_api_10_2() {
    let photo = InputRichMessageMedia::photo(
        "pic1",
        "https://example.com/summit.jpg",
        Some("Summit view".to_string()),
    )
    .expect("valid");
    let msg = InputRichMessage::from_html(
        "<h3>Rinjani</h3><img src=\"tg://photo?id=pic1\"/>",
        Some(vec![photo]),
    );
    assert!(msg.validate().is_ok());

    let json_val = serde_json::to_value(&msg).expect("serialization succeeds");
    assert_eq!(
        json_val["html"],
        "<h3>Rinjani</h3><img src=\"tg://photo?id=pic1\"/>"
    );
    assert!(json_val["media"].is_array());
    let media_arr = json_val["media"].as_array().expect("media array");
    assert_eq!(media_arr.len(), 1);
    assert_eq!(media_arr[0]["id"], "pic1");
    assert_eq!(media_arr[0]["media"]["type"], "photo");
    assert_eq!(
        media_arr[0]["media"]["media"],
        "https://example.com/summit.jpg"
    );
    assert_eq!(media_arr[0]["media"]["caption"], "Summit view");

    // Roundtrip deserialization
    let deserialized: InputRichMessage =
        serde_json::from_value(json_val).expect("deserialization succeeds");
    assert_eq!(deserialized.html, msg.html);
    let d_media = deserialized.media.expect("deserialized media present");
    assert_eq!(d_media.len(), 1);
    assert_eq!(d_media[0].id, "pic1");
    assert_eq!(
        d_media[0].media.media_url(),
        "https://example.com/summit.jpg"
    );
}

#[test]
fn location_and_tg_map_coordinates_validation() {
    // Valid coordinates
    let valid_coords = vec![
        (0.0, 0.0),
        (-90.0, -180.0),
        (90.0, 180.0),
        (-8.4113, 116.4573), // Rinjani
        (40.7128, -74.0060), // New York
    ];
    for (lat, lon) in valid_coords {
        let loc = Location::new(lat, lon);
        assert!(loc.is_ok(), "Coordinates ({lat}, {lon}) should be valid");
    }

    // Invalid latitude
    assert!(Location::new(90.0001, 0.0).is_err());
    assert!(Location::new(-90.0001, 0.0).is_err());
    assert!(Location::new(f64::NAN, 0.0).is_err());
    assert!(Location::new(f64::INFINITY, 0.0).is_err());

    // Invalid longitude
    assert!(Location::new(0.0, 180.0001).is_err());
    assert!(Location::new(0.0, -180.0001).is_err());
    assert!(Location::new(0.0, f64::NAN).is_err());
    assert!(Location::new(0.0, f64::NEG_INFINITY).is_err());

    // Map RichBlock validation
    let valid_map = InputRichMessage::new(vec![RichBlock::Map {
        location: Location {
            latitude: -8.4113,
            longitude: 116.4573,
            horizontal_accuracy: None,
        },
        zoom: Some(13),
        width: None,
        height: None,
    }]);
    assert!(valid_map.validate().is_ok());

    // Invalid zoom
    let invalid_zoom_zero = InputRichMessage::new(vec![RichBlock::Map {
        location: Location {
            latitude: -8.4113,
            longitude: 116.4573,
            horizontal_accuracy: None,
        },
        zoom: Some(0),
        width: None,
        height: None,
    }]);
    assert!(invalid_zoom_zero.validate().is_err());

    let invalid_zoom_high = InputRichMessage::new(vec![RichBlock::Map {
        location: Location {
            latitude: -8.4113,
            longitude: 116.4573,
            horizontal_accuracy: None,
        },
        zoom: Some(25),
        width: None,
        height: None,
    }]);
    assert!(invalid_zoom_high.validate().is_err());

    // Invalid location inside Map block
    let invalid_lat_map = InputRichMessage::new(vec![RichBlock::Map {
        location: Location {
            latitude: 99.0,
            longitude: 0.0,
            horizontal_accuracy: None,
        },
        zoom: Some(10),
        width: None,
        height: None,
    }]);
    assert!(invalid_lat_map.validate().is_err());
}

#[test]
fn input_rich_message_partial_eq_and_animation_voice() {
    let photo1 = match InputRichMessageMedia::photo("p1", "https://example.com/1.jpg", None) {
        Ok(m) => m,
        Err(e) => panic!("valid photo failed: {e}"),
    };
    let photo2 = match InputRichMessageMedia::photo("p1", "https://example.com/1.jpg", None) {
        Ok(m) => m,
        Err(e) => panic!("valid photo failed: {e}"),
    };
    let photo3 = match InputRichMessageMedia::photo("p2", "https://example.com/2.jpg", None) {
        Ok(m) => m,
        Err(e) => panic!("valid photo failed: {e}"),
    };
    assert_eq!(photo1, photo2);
    assert_ne!(photo1, photo3);

    let anim = match InputRichMessageMedia::animation(
        "anim_1",
        "https://example.com/gif.mp4",
        Some("Animation".to_string()),
    ) {
        Ok(m) => m,
        Err(e) => panic!("valid animation failed: {e}"),
    };
    assert_eq!(anim.id, "anim_1");
    assert_eq!(anim.media.media_url(), "https://example.com/gif.mp4");

    let voice = match InputRichMessageMedia::voice_note(
        "voice_1",
        "https://example.com/voice.ogg",
        None,
        Some(15),
    ) {
        Ok(m) => m,
        Err(e) => panic!("valid voice failed: {e}"),
    };
    assert_eq!(voice.id, "voice_1");
    assert_eq!(voice.media.media_url(), "https://example.com/voice.ogg");

    let msg1 = InputRichMessage::from_html("<p>test</p>", Some(vec![photo1.clone()]));
    let msg2 = InputRichMessage::from_html("<p>test</p>", Some(vec![photo2]));
    let msg3 = InputRichMessage::from_html("<p>diff</p>", Some(vec![photo3]));
    assert_eq!(msg1, msg2);
    assert_ne!(msg1, msg3);
}

#[test]
fn rich_block_map_helpers_and_validation() {
    let map_res = RichBlock::map_coords(-8.4113, 116.4573, Some(14));
    assert!(map_res.is_ok());
    if let Ok(RichBlock::Map { location, zoom, .. }) = map_res {
        assert_eq!(location.latitude, -8.4113);
        assert_eq!(location.longitude, 116.4573);
        assert_eq!(zoom, Some(14));
    } else {
        panic!("Expected RichBlock::Map variant");
    }

    // Exact boundary coordinates
    assert!(RichBlock::map_coords(-90.0, -180.0, Some(1)).is_ok());
    assert!(RichBlock::map_coords(90.0, 180.0, Some(20)).is_ok());

    // Out of boundary coordinates
    assert!(RichBlock::map_coords(-90.001, 0.0, None).is_err());
    assert!(RichBlock::map_coords(90.001, 0.0, None).is_err());
    assert!(RichBlock::map_coords(0.0, -180.001, None).is_err());
    assert!(RichBlock::map_coords(0.0, 180.001, None).is_err());

    // Non-finite coordinates
    assert!(RichBlock::map_coords(f64::NAN, 0.0, None).is_err());
    assert!(RichBlock::map_coords(0.0, f64::INFINITY, None).is_err());
    assert!(RichBlock::map_coords(0.0, f64::NEG_INFINITY, None).is_err());

    // Zoom boundaries
    assert!(RichBlock::map_coords(0.0, 0.0, Some(0)).is_err());
    assert!(RichBlock::map_coords(0.0, 0.0, Some(21)).is_err());
}

#[test]
fn input_rich_message_media_count_boundary_50_limit() {
    let mut items_50 = Vec::new();
    for i in 0..50 {
        let item = match InputRichMessageMedia::photo(
            format!("id_{i}"),
            format!("https://example.com/{i}.jpg"),
            None,
        ) {
            Ok(item) => item,
            Err(e) => panic!("failed to create media item: {e}"),
        };
        items_50.push(item);
    }

    let msg_50 = InputRichMessage::from_html("<p>50 items</p>", Some(items_50.clone()));
    assert!(msg_50.validate().is_ok(), "50 media items must be accepted");

    let mut items_51 = items_50;
    let item_51 = match InputRichMessageMedia::photo("id_50", "https://example.com/50.jpg", None) {
        Ok(item) => item,
        Err(e) => panic!("failed to create media item: {e}"),
    };
    items_51.push(item_51);

    let msg_51 = InputRichMessage::from_html("<p>51 items</p>", Some(items_51));
    assert!(
        msg_51.validate().is_err(),
        "51 media items must be rejected"
    );
}

#[test]
fn input_rich_message_media_rejects_empty_media_url() {
    let empty_photo = InputRichMessageMedia {
        id: "valid_id".to_string(),
        media: InputMedia::photo("", None, None),
    };
    assert!(
        empty_photo.validate().is_err(),
        "Empty media URL must be rejected"
    );
}

#[test]
fn test_staged_document_domain_methods_and_conversions() {
    let doc = StagedDocument::new("doc_0", b"hello world".to_vec(), "text/plain", "hello.txt");
    assert_eq!(doc.attach_key, "doc_0");
    assert_eq!(doc.bytes, b"hello world");
    assert_eq!(doc.mime_type, "text/plain");
    assert_eq!(doc.filename, "hello.txt");
    assert_eq!(doc.attach_uri(), "attach://doc_0");
    assert_eq!(doc.markdown_tag(), "[document: hello.txt](attach://doc_0)");

    let tuple: (String, Vec<u8>, String, String) = doc.clone().into();
    assert_eq!(tuple.0, "doc_0");
    assert_eq!(tuple.1, b"hello world");
    assert_eq!(tuple.2, "text/plain");
    assert_eq!(tuple.3, "hello.txt");

    let from_tuple: StagedDocument = tuple.into();
    assert_eq!(from_tuple, doc);
    assert_eq!(
        from_tuple.into_raw_tuple(),
        (
            "doc_0".to_string(),
            b"hello world".to_vec(),
            "text/plain".to_string(),
            "hello.txt".to_string()
        )
    );
}

#[test]
fn test_rich_block_list_item_checkbox_serialization() {
    let checked_item = RichBlockListItem::checkbox(
        vec![serde_json::json!({"type": "paragraph", "text": "Task selesai"})],
        true,
    );
    let unchecked_item = RichBlockListItem::checkbox(
        vec![serde_json::json!({"type": "paragraph", "text": "Task tertunda"})],
        false,
    );

    let v_checked = serde_json::to_value(&checked_item).expect("serialize checked");
    assert_eq!(v_checked["has_checkbox"], true);
    assert_eq!(v_checked["is_checked"], true);

    let v_unchecked = serde_json::to_value(&unchecked_item).expect("serialize unchecked");
    assert_eq!(v_unchecked["has_checkbox"], true);
    assert!(
        v_unchecked.get("is_checked").is_none(),
        "Bot API `True` flag: unchecked is expressed by omitting it"
    );
}

#[test]
fn test_input_checklist_models_serialization() {
    let tasks = vec![
        InputChecklistTask::new(1, "Task satu"),
        InputChecklistTask::new(2, "Task dua"),
    ];
    let checklist = InputChecklist::new("Daftar Belanja", tasks);

    let value = serde_json::to_value(&checklist).expect("serialize checklist");
    assert_eq!(value["title"], "Daftar Belanja");
    assert_eq!(value["tasks"].as_array().expect("array").len(), 2);
    assert_eq!(value["tasks"][0]["id"], 1);
    assert_eq!(value["tasks"][0]["text"], "Task satu");
    assert_eq!(value["tasks"][1]["id"], 2);
    assert_eq!(value["tasks"][1]["text"], "Task dua");
}

#[test]
fn true_only_flags_are_omitted_instead_of_sent_as_false() {
    let table = |is_bordered, is_striped, is_compact| {
        serde_json::to_value(RichBlock::Table {
            cells: vec![vec![RichBlockTableCell::text_only("x", false, None)]],
            has_header: false,
            is_bordered,
            is_striped,
            is_compact,
            caption: None,
        })
        .expect("serialize table")
    };
    let compact_only = table(false, false, true);
    assert_eq!(compact_only["is_compact"], true);
    assert!(compact_only.get("is_bordered").is_none());
    assert!(compact_only.get("is_striped").is_none());
    assert!(compact_only["cells"][0][0].get("is_header").is_none());

    let plain = table(false, false, false);
    for flag in ["is_bordered", "is_striped", "is_compact"] {
        assert!(plain.get(flag).is_none(), "{flag} must be omitted");
    }
    let parsed: RichBlock = serde_json::from_value(plain).expect("omitted flags parse as false");
    assert!(matches!(
        parsed,
        RichBlock::Table {
            is_bordered: false,
            is_striped: false,
            is_compact: false,
            ..
        }
    ));

    let closed = serde_json::to_value(RichBlock::Details {
        summary: Value::String("Rincian".to_string()),
        blocks: Vec::new(),
        is_open: Some(false),
    })
    .expect("serialize details");
    assert!(closed.get("is_open").is_none());
}
