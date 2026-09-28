#![allow(dead_code)]

#[path = "../src/bot/models.rs"]
pub mod models;
pub mod bot {
    pub use crate::models;
}
#[path = "../src/parser.rs"]
mod parser;
#[path = "../src/ai/stream.rs"]
mod stream;

use models::{
    validate_quiz, InputMedia, InputPollOption, Poll, PollOption, RichBlock, RichMessageButton,
    RichTextButton, Update, QUIZ_MAX_EXPLANATION_CHARS, QUIZ_MAX_EXPLANATION_LINE_BREAKS,
    QUIZ_MAX_OPTIONS, QUIZ_MAX_OPTION_CHARS, QUIZ_MAX_QUESTION_CHARS, QUIZ_MIN_OPTIONS,
};
use stream::SseDecoder;

fn photo_media(id: &str) -> InputMedia {
    InputMedia::Photo {
        media: id.to_string(),
        caption: None,
        parse_mode: None,
        show_caption_above_media: None,
        has_spoiler: None,
    }
}

#[test]
fn voice_note_input_media_uses_bot_api_10_3_discriminator() {
    let media = InputMedia::VoiceNote {
        media: "file-id".to_string(),
        caption: None,
        parse_mode: None,
        duration: None,
    };
    let value = serde_json::to_value(media).expect("voice note should serialize");
    assert_eq!(value["type"], "voice_note");
}

#[test]
fn parsed_voice_note_uses_bot_api_10_3_nested_media_discriminator() {
    let blocks =
        parser::parse_markdown_to_rich_blocks("[voice: Rekaman](https://example.com/sample.ogg)");
    let Some(RichBlock::VoiceNote { voice_note, .. }) = blocks.first() else {
        panic!("expected parsed voice-note block");
    };
    assert_eq!(voice_note["type"], "voice_note");
}

#[test]
fn rich_text_button_includes_required_type_discriminator() {
    let rich_text = RichTextButton {
        button: RichMessageButton::callback("Retry", "retry"),
    };
    let value = serde_json::to_value(rich_text).expect("rich text button should serialize");
    assert_eq!(value["type"], "button");
    assert_eq!(value["button"]["callback_data"], "retry");
}

#[test]
fn malformed_sse_data_is_rejected_instead_of_silently_dropped() {
    let mut decoder = SseDecoder::default();
    let error = decoder
        .push(b"data: {not-json}\n\n")
        .expect_err("malformed SSE JSON must be surfaced as an error");
    assert!(error.contains("invalid JSON"), "unexpected error: {error}");
}

#[test]
fn media_group_requires_two_to_ten_album_compatible_items() {
    assert!(InputMedia::validate_media_group(&[photo_media("a")]).is_err());
    assert!(InputMedia::validate_media_group(&[photo_media("a"), photo_media("b")]).is_ok());
    assert!(InputMedia::validate_media_group(
        &(0..10)
            .map(|index| photo_media(&format!("p{index}")))
            .collect::<Vec<_>>()
    )
    .is_ok());
    assert!(InputMedia::validate_media_group(
        &(0..11)
            .map(|index| photo_media(&format!("p{index}")))
            .collect::<Vec<_>>()
    )
    .is_err());

    let animation = InputMedia::Animation {
        media: "anim".to_string(),
        caption: None,
        parse_mode: None,
        show_caption_above_media: None,
        width: None,
        height: None,
        duration: None,
        has_spoiler: None,
    };
    assert!(InputMedia::validate_media_group(&[photo_media("a"), animation]).is_err());

    let voice_note = InputMedia::VoiceNote {
        media: "voice".to_string(),
        caption: None,
        parse_mode: None,
        duration: None,
    };
    assert!(InputMedia::validate_media_group(&[photo_media("a"), voice_note]).is_err());
}

#[test]
fn media_group_keeps_audio_and_documents_in_homogeneous_albums() {
    let audio = InputMedia::audio("audio", None, None, None, None);
    let document = InputMedia::document("document", None, None);
    let video = InputMedia::video("video", None, None);

    assert!(InputMedia::validate_media_group(&[photo_media("photo"), video]).is_ok());
    assert!(InputMedia::validate_media_group(&[photo_media("photo"), audio.clone()]).is_err());
    assert!(InputMedia::validate_media_group(&[photo_media("photo"), document.clone()]).is_err());
    assert!(InputMedia::validate_media_group(&[audio.clone(), document.clone()]).is_err());
    assert!(InputMedia::validate_media_group(&[audio.clone(), audio]).is_ok());
    assert!(InputMedia::validate_media_group(&[document.clone(), document]).is_ok());
}

#[test]
fn parsed_expandable_blockquote_serializes_with_10_3_discriminator() {
    let blocks = parser::parse_markdown_to_rich_blocks("**> Catatan penting yang dapat dilipat");
    let Some(RichBlock::ExpandableBlockQuotation { .. }) = blocks.first() else {
        panic!("expected expandable blockquote");
    };
    let rich_message = models::InputRichMessage::new(blocks);
    assert!(rich_message.validate().is_ok());
    let value = serde_json::to_value(&rich_message).expect("should serialize rich message");
    assert_eq!(value["blocks"][0]["type"], "expandable_blockquote");
    assert_eq!(
        value["blocks"][0]["text"],
        "Catatan penting yang dapat dilipat"
    );
}

#[test]
fn parsed_rich_inline_spoiler_and_strikethrough_serialize() {
    let blocks = parser::parse_markdown_to_rich_blocks("Hasil: ||jawaban rahasia|| dan ~~coret~~");
    let rich_message = models::InputRichMessage::new(blocks);
    assert!(rich_message.validate().is_ok());
    let value = serde_json::to_value(&rich_message).expect("should serialize rich message");
    let serialized = value.to_string();
    assert!(serialized.contains(r#""type":"spoiler""#));
    assert!(serialized.contains(r#""type":"strikethrough""#));
}

#[test]
fn parsed_collage_and_slideshow_serialize_with_10_3_discriminators() {
    let collage_block = RichBlock::Collage {
        blocks: vec![
            serde_json::json!({"type": "photo", "photo": {"type": "photo", "media": "https://example.com/p1.jpg"}}),
            serde_json::json!({"type": "photo", "photo": {"type": "photo", "media": "https://example.com/p2.jpg"}}),
        ],
        caption: Some(models::RichBlockCaption::new(serde_json::json!(
            "Galeri Foto"
        ))),
    };
    let slideshow_block = RichBlock::Slideshow {
        blocks: vec![
            serde_json::json!({"type": "photo", "photo": {"type": "photo", "media": "https://example.com/s1.jpg"}}),
            serde_json::json!({"type": "photo", "photo": {"type": "photo", "media": "https://example.com/s2.jpg"}}),
        ],
        caption: None,
    };
    let rich_message = models::InputRichMessage::new(vec![collage_block, slideshow_block]);
    assert!(rich_message.validate().is_ok());
    let value =
        serde_json::to_value(&rich_message).expect("should serialize collage and slideshow");
    assert_eq!(value["blocks"][0]["type"], "collage");
    assert_eq!(
        value["blocks"][0]["blocks"]
            .as_array()
            .expect("array of collage blocks")
            .len(),
        2
    );
    assert_eq!(value["blocks"][1]["type"], "slideshow");
    assert_eq!(
        value["blocks"][1]["blocks"]
            .as_array()
            .expect("array of slideshow blocks")
            .len(),
        2
    );
}

#[test]
fn telegram_command_registration_is_pure_zero_slash() {
    let source = include_str!("../src/bot/daemon.rs");
    let cmd_start = source
        .find("// Register Bot Commands")
        .expect("bot command registration block");
    let cmd_end = source[cmd_start..]
        .find("bot.set_my_commands")
        .map(|offset| cmd_start + offset)
        .expect("bot.set_my_commands call");
    let block = &source[cmd_start..cmd_end];
    assert!(block.contains("&empty_commands") || block.contains("&[]") || block.contains("vec![]"));
    assert!(!block.contains("\"start\""));
    assert!(!block.contains("\"menu\""));
    assert!(!block.contains("\"help\""));
    assert!(!block.contains("\"clear\""));
    assert!(!block.contains("\"image\""));
    assert!(!block.contains("\"session\""));
    assert!(!block.contains("\"context\""));
}

#[test]
fn chat_deserializes_is_forum_field() {
    let json_str = r#"{
        "id": -1001234567890,
        "type": "supergroup",
        "title": "Topic Workspace",
        "is_forum": true
    }"#;
    let chat: models::Chat =
        serde_json::from_str(json_str).expect("Chat with is_forum must deserialize");
    assert_eq!(chat.id, -1001234567890);
    assert_eq!(chat.is_forum, Some(true));
}

#[test]
fn chat_member_deserializes_and_validates_admin_status() {
    let admin_json = r#"{
        "status": "administrator",
        "user": {
            "id": 123456,
            "is_bot": true,
            "first_name": "xiao"
        }
    }"#;
    let admin: models::ChatMember =
        serde_json::from_str(admin_json).expect("admin ChatMember must deserialize");
    assert!(admin.is_admin_or_creator());
    assert!(admin.is_administrator());
    assert!(!admin.is_creator());

    let creator_json = r#"{
        "status": "creator",
        "user": {
            "id": 654321,
            "is_bot": false,
            "first_name": "owner"
        }
    }"#;
    let creator: models::ChatMember =
        serde_json::from_str(creator_json).expect("creator ChatMember must deserialize");
    assert!(creator.is_admin_or_creator());
    assert!(creator.is_creator());
    assert!(!creator.is_administrator());

    let member_json = r#"{
        "status": "member",
        "user": {
            "id": 999999,
            "is_bot": false,
            "first_name": "member"
        }
    }"#;
    let member: models::ChatMember =
        serde_json::from_str(member_json).expect("member ChatMember must deserialize");
    assert!(!member.is_admin_or_creator());
}

#[test]
fn input_rich_message_serializes_is_rtl_true_when_set() {
    let mut msg = models::InputRichMessage::new(vec![models::RichBlock::Paragraph {
        text: serde_json::Value::String("مرحبا".to_string()),
    }]);
    msg.is_rtl = Some(true);
    let val = serde_json::to_value(&msg).expect("rich message should serialize");
    assert_eq!(val["is_rtl"], true);
}

#[test]
fn input_rich_message_omits_is_rtl_when_none() {
    let msg = models::InputRichMessage::new(vec![models::RichBlock::Paragraph {
        text: serde_json::Value::String("Hello".to_string()),
    }]);
    let val = serde_json::to_value(&msg).expect("rich message should serialize");
    assert!(val.get("is_rtl").is_none());
}

#[test]
fn parser_emits_is_rtl_and_right_aligned_cells_for_arabic_table_with_hindi_numerals() {
    let table_md = "| الرقم | الاسم |\n| --- | --- |\n| ١ | أحمد |\n| ٢ | فاطمة |";
    let rich_msg = parser::build_full_rich_message(table_md, None);
    assert_eq!(rich_msg.is_rtl, Some(true));

    let Some(models::RichBlock::Table { cells, .. }) = rich_msg.blocks.first() else {
        panic!("expected rich table block");
    };

    assert_eq!(cells[0][0].align.as_deref(), Some("right"));
    assert_eq!(cells[0][1].align.as_deref(), Some("right"));
    assert_eq!(cells[1][0].align.as_deref(), Some("right"));
    assert_eq!(cells[1][1].align.as_deref(), Some("right"));

    let val = serde_json::to_value(&rich_msg).expect("serialization must succeed");
    assert_eq!(val["is_rtl"], true);
    assert_eq!(val["blocks"][0]["type"], "table");
    assert_eq!(val["blocks"][0]["cells"][0][0]["align"], "right");
}

#[test]
fn parser_honors_explicit_column_alignments_in_contract_wire_format() {
    let md = "| A | B | C |\n| :--- | :---: | ---: |\n| 1 | 2 | 3 |";
    let rich_msg = parser::build_full_rich_message(md, None);
    let val = serde_json::to_value(&rich_msg).expect("serialization must succeed");
    assert_eq!(val["blocks"][0]["cells"][0][0]["align"], "left");
    assert_eq!(val["blocks"][0]["cells"][0][1]["align"], "center");
    assert_eq!(val["blocks"][0]["cells"][0][2]["align"], "right");
    assert_eq!(val["blocks"][0]["cells"][1][0]["align"], "left");
    assert_eq!(val["blocks"][0]["cells"][1][1]["align"], "center");
    assert_eq!(val["blocks"][0]["cells"][1][2]["align"], "right");
}

#[test]
fn ephemeral_message_parameters_serializes_replace_callback_query_message() {
    let params = models::EphemeralMessageParameters {
        receiver_user_id: 123456,
        callback_query_id: Some("cq_1".to_string()),
        replace_callback_query_message: Some(true),
    };
    let val = serde_json::to_value(&params).expect("serialization must succeed");
    assert_eq!(val["receiver_user_id"], 123456);
    assert_eq!(val["callback_query_id"], "cq_1");
    assert_eq!(val["replace_callback_query_message"], true);

    let params_without = models::EphemeralMessageParameters {
        receiver_user_id: 123456,
        callback_query_id: None,
        replace_callback_query_message: None,
    };
    let val_without = serde_json::to_value(&params_without).expect("serialization must succeed");
    assert_eq!(val_without["receiver_user_id"], 123456);
    assert!(val_without.get("callback_query_id").is_none());
    assert!(val_without.get("replace_callback_query_message").is_none());
}

#[test]
fn input_poll_option_wire_format_serializes_and_deserializes() {
    let opt_plain = InputPollOption::new("Paris");
    let val_plain = serde_json::to_value(&opt_plain).expect("should serialize plain option");
    assert_eq!(val_plain["text"], "Paris");
    assert!(val_plain.get("text_parse_mode").is_none());
    assert!(val_plain.get("text_entities").is_none());

    let opt_html = InputPollOption::with_parse_mode("<b>Berlin</b>", "HTML");
    let val_html = serde_json::to_value(&opt_html).expect("should serialize html option");
    assert_eq!(val_html["text"], "<b>Berlin</b>");
    assert_eq!(val_html["text_parse_mode"], "HTML");

    let from_str: InputPollOption = "Rome".into();
    assert_eq!(from_str.text, "Rome");

    let from_string: InputPollOption = "Madrid".to_string().into();
    assert_eq!(from_string.text, "Madrid");

    let json_input = r#"{"text": "Tokyo"}"#;
    let opt_de: InputPollOption =
        serde_json::from_str(json_input).expect("should deserialize option");
    assert_eq!(opt_de.text, "Tokyo");
    assert_eq!(opt_de.text_parse_mode, None);

    let json_str_input = r#""Kyoto""#;
    let opt_from_str: InputPollOption =
        serde_json::from_str(json_str_input).expect("should deserialize plain string option");
    assert_eq!(opt_from_str.text, "Kyoto");
    assert_eq!(opt_from_str.text_parse_mode, None);
}

#[test]
fn quiz_character_limits_and_validation_rules() {
    let valid_options = vec![
        InputPollOption::new("Option A"),
        InputPollOption::new("Option B"),
    ];

    // 1. Happy path
    assert!(validate_quiz(
        "What is 2 + 2?",
        &valid_options,
        0,
        Some("Basic arithmetic")
    )
    .is_ok());

    // 2. Question boundaries (1..=300 chars)
    let q_300 = "a".repeat(QUIZ_MAX_QUESTION_CHARS);
    assert!(validate_quiz(&q_300, &valid_options, 0, None).is_ok());

    let q_301 = "a".repeat(QUIZ_MAX_QUESTION_CHARS + 1);
    let err_q = validate_quiz(&q_301, &valid_options, 0, None).expect_err("301 chars must fail");
    assert!(err_q.contains("300"));

    assert!(validate_quiz("", &valid_options, 0, None).is_err());
    assert!(validate_quiz("   ", &valid_options, 0, None).is_err());

    // 3. Option count boundaries (2..=10)
    let one_option = vec![InputPollOption::new("Only one")];
    let err_count_min =
        validate_quiz("Question?", &one_option, 0, None).expect_err("1 option must fail");
    assert!(err_count_min.contains(&QUIZ_MIN_OPTIONS.to_string()));

    let ten_options: Vec<InputPollOption> = (0..QUIZ_MAX_OPTIONS)
        .map(|i| InputPollOption::new(format!("Option {i}")))
        .collect();
    assert!(validate_quiz("Question?", &ten_options, 0, None).is_ok());

    let eleven_options: Vec<InputPollOption> = (0..=QUIZ_MAX_OPTIONS)
        .map(|i| InputPollOption::new(format!("Option {i}")))
        .collect();
    let err_count_max =
        validate_quiz("Question?", &eleven_options, 0, None).expect_err("11 options must fail");
    assert!(err_count_max.contains(&QUIZ_MAX_OPTIONS.to_string()));

    // 4. Option text character limits (1..=100 chars)
    let empty_text_option = vec![InputPollOption::new("Valid"), InputPollOption::new("   ")];
    assert!(validate_quiz("Question?", &empty_text_option, 0, None).is_err());

    let opt_100 = "x".repeat(QUIZ_MAX_OPTION_CHARS);
    let options_100 = vec![InputPollOption::new(opt_100), InputPollOption::new("B")];
    assert!(validate_quiz("Question?", &options_100, 0, None).is_ok());

    let opt_101 = "x".repeat(QUIZ_MAX_OPTION_CHARS + 1);
    let options_101 = vec![InputPollOption::new(opt_101), InputPollOption::new("B")];
    let err_opt_len =
        validate_quiz("Question?", &options_101, 0, None).expect_err("101 char option must fail");
    assert!(err_opt_len.contains(&QUIZ_MAX_OPTION_CHARS.to_string()));

    // 5. Correct option ID boundaries
    assert!(validate_quiz("Question?", &valid_options, -1, None).is_err());
    assert!(validate_quiz("Question?", &valid_options, 0, None).is_ok());
    assert!(validate_quiz("Question?", &valid_options, 1, None).is_ok());
    assert!(validate_quiz("Question?", &valid_options, 2, None).is_err());

    // 6. Explanation limits (<= 200 chars, <= 2 line breaks)
    assert_eq!(QUIZ_MAX_EXPLANATION_LINE_BREAKS, 2);
    let exp_200 = "e".repeat(QUIZ_MAX_EXPLANATION_CHARS);
    assert!(validate_quiz("Question?", &valid_options, 0, Some(&exp_200)).is_ok());

    let exp_201 = "e".repeat(QUIZ_MAX_EXPLANATION_CHARS + 1);
    let err_exp_len = validate_quiz("Question?", &valid_options, 0, Some(&exp_201))
        .expect_err("201 char explanation must fail");
    assert!(err_exp_len.contains(&QUIZ_MAX_EXPLANATION_CHARS.to_string()));

    let exp_2_breaks = "Line 1\nLine 2\nLine 3";
    assert!(validate_quiz("Question?", &valid_options, 0, Some(exp_2_breaks)).is_ok());

    let exp_3_breaks = "Line 1\nLine 2\nLine 3\nLine 4";
    let err_breaks = validate_quiz("Question?", &valid_options, 0, Some(exp_3_breaks))
        .expect_err("3 line breaks must fail");
    assert!(err_breaks.contains("line breaks"));

    let exp_3_cr_breaks = "Line 1\rLine 2\rLine 3\rLine 4";
    let err_cr_breaks = validate_quiz("Question?", &valid_options, 0, Some(exp_3_cr_breaks))
        .expect_err("3 CR line breaks must fail");
    assert!(err_cr_breaks.contains("line breaks"));

    // 7. Duplicate options rejection (Telegram requires options to be unique)
    let dup_options = vec![
        InputPollOption::new("Same Option"),
        InputPollOption::new("Same Option"),
    ];
    let err_dup = validate_quiz("Question?", &dup_options, 0, None)
        .expect_err("Duplicate option texts must fail");
    assert!(err_dup.contains("unique"));
}

#[test]
fn quiz_poll_and_poll_option_deserializes_telegram_wire_format() {
    let wire_json = r#"{
        "id": "5432109876",
        "question": "Berapakah hasil dari 2^10?",
        "options": [
            {"text": "512", "voter_count": 2},
            {"text": "1024", "voter_count": 8},
            {"text": "2048", "voter_count": 1}
        ],
        "total_voter_count": 11,
        "is_closed": false,
        "is_anonymous": false,
        "type": "quiz",
        "allows_multiple_answers": false,
        "correct_option_id": 1,
        "explanation": "2 pangkat 10 adalah 1024."
    }"#;

    let poll: Poll = serde_json::from_str(wire_json).expect("poll must deserialize");
    assert_eq!(poll.id, "5432109876");
    assert_eq!(poll.question, "Berapakah hasil dari 2^10?");
    assert_eq!(poll.poll_type, "quiz");
    assert_eq!(poll.total_voter_count, 11);
    assert!(!poll.is_closed);
    assert!(!poll.is_anonymous);
    assert!(!poll.allows_multiple_answers);
    assert_eq!(poll.correct_option_id, Some(1));
    assert_eq!(
        poll.explanation.as_deref(),
        Some("2 pangkat 10 adalah 1024.")
    );
    assert_eq!(poll.options.len(), 3);
    let opt_0: &PollOption = &poll.options[0];
    assert_eq!(opt_0.text, "512");
    assert_eq!(opt_0.voter_count, 2);
    assert_eq!(poll.options[1].text, "1024");
    assert_eq!(poll.options[1].voter_count, 8);
    assert_eq!(poll.options[2].text, "2048");
    assert_eq!(poll.options[2].voter_count, 1);
}

#[test]
fn update_wire_format_deserializes_poll_update() {
    let wire_update = r#"{
        "update_id": 999111,
        "poll": {
            "id": "poll_wire_1",
            "question": "Quiz in update?",
            "options": [
                {"text": "Yes", "voter_count": 5},
                {"text": "No", "voter_count": 0}
            ],
            "total_voter_count": 5,
            "type": "quiz"
        }
    }"#;
    let update: Update =
        serde_json::from_str(wire_update).expect("update with poll must deserialize");
    assert_eq!(update.update_id, 999111);
    let poll = update.poll.expect("poll must be present");
    assert_eq!(poll.id, "poll_wire_1");
    assert_eq!(poll.question, "Quiz in update?");
    assert_eq!(poll.options.len(), 2);
    // Verified default flags
    assert!(!poll.is_closed);
    assert!(!poll.is_anonymous);
    assert!(!poll.allows_multiple_answers);
}

#[test]
fn update_wire_format_deserializes_guest_edited_and_new_message_kinds() {
    let guest: Update = serde_json::from_str(
        r#"{"update_id": 1, "guest_message": {
            "message_id": 5, "date": 1700000000,
            "chat": {"id": -1009, "type": "supergroup"},
            "from": {"id": 42, "is_bot": false, "first_name": "Owner"},
            "guest_query_id": "AAQ-guest", "text": "@XiaoBot ringkas"}}"#,
    )
    .expect("Bot API 10.0 guest_message must deserialize");
    let guest_message = guest.guest_message.expect("guest message present");
    assert_eq!(guest_message.guest_query_id.as_deref(), Some("AAQ-guest"));
    assert!(guest.message.is_none());

    let edited: Update = serde_json::from_str(
        r#"{"update_id": 2, "edited_message": {
            "message_id": 6, "date": 1700000000, "edit_date": 1700000060,
            "chat": {"id": 42, "type": "private"},
            "from": {"id": 42, "is_bot": false, "first_name": "Owner"},
            "text": "versi baru"}}"#,
    )
    .expect("edited_message must deserialize");
    assert_eq!(
        edited.edited_message.expect("edited present").edit_date,
        Some(1700000060)
    );

    let kinds: Update = serde_json::from_str(
        r#"{"update_id": 3, "message": {
            "message_id": 7, "date": 1700000000,
            "chat": {"id": 42, "type": "private"},
            "photo": [{"file_id": "P", "file_unique_id": "p", "width": 1, "height": 1}],
            "live_photo": {"file_id": "LV", "file_unique_id": "lv", "width": 1,
                           "height": 1, "duration": 2},
            "rich_message": {"blocks": [{"type": "paragraph", "text": "diteruskan"}]}}}"#,
    )
    .expect("Bot API 10.0 live_photo and 10.1 rich_message must deserialize");
    let message = kinds.message.expect("message present");
    assert_eq!(
        message
            .live_photo
            .as_ref()
            .map(|live| live.file_id.as_str()),
        Some("LV")
    );
    assert_eq!(message.rich_message_text().as_deref(), Some("diteruskan"));
}

#[test]
fn test_inline_interleaved_rich_media_placement_order() {
    // Tests Telegram Bot API in-line interleaved rich media placement (matching @richtextdemobot demo)
    // where media elements are interwoven between headings and paragraphs.
    let text = "# Media Demo\n\n\
                Paragraf pembuka observasi satwa liar.\n\n\
                <img src=\"https://example.com/tiger.jpg\" caption=\"Harimau Sumatera\"/>\n\n\
                Paragraf penjelasan lanjutan setelah foto harimau.\n\n\
                <audio src=\"https://example.com/roar.mp3\" title=\"Suara Auman\" performer=\"Satwa\"/>\n\n\
                Paragraf penutup berisi kesimpulan observasi.";

    let blocks = parser::parse_markdown_to_rich_blocks(text);
    assert_eq!(blocks.len(), 6);
    assert!(matches!(blocks[0], RichBlock::SectionHeading { .. }));
    assert!(matches!(blocks[1], RichBlock::Paragraph { .. }));
    assert!(matches!(blocks[2], RichBlock::Photo { .. }));
    assert!(matches!(blocks[3], RichBlock::Paragraph { .. }));
    assert!(matches!(blocks[4], RichBlock::Audio { .. }));
    assert!(matches!(blocks[5], RichBlock::Paragraph { .. }));

    // Verify wire format serialization retains order
    let rich_msg = models::InputRichMessage::new(blocks);
    assert!(rich_msg.validate().is_ok());
    let serialized = serde_json::to_value(&rich_msg).expect("serialize rich message");
    let json_blocks = serialized["blocks"].as_array().expect("blocks array");
    assert_eq!(json_blocks.len(), 6);
    assert_eq!(json_blocks[0]["type"], "heading");
    assert_eq!(json_blocks[1]["type"], "paragraph");
    assert_eq!(json_blocks[2]["type"], "photo");
    assert_eq!(json_blocks[3]["type"], "paragraph");
    assert_eq!(json_blocks[4]["type"], "audio");
    assert_eq!(json_blocks[5]["type"], "paragraph");
}

#[test]
fn update_wire_format_deserializes_replies_checklists_and_inline_mode() {
    let reply: Update = serde_json::from_value(serde_json::json!({
        "update_id": 30,
        "message": {
            "message_id": 9, "date": 1,
            "chat": {"id": 42, "type": "private"},
            "text": "sudah?",
            "quote": {"text": "Telur", "position": 3, "is_manual": true},
            "reply_to_checklist_task_id": 2,
            "reply_to_message": {
                "message_id": 5, "date": 1, "chat": {"id": 42, "type": "private"},
                "checklist": {"title": "Belanja", "tasks": [
                    {"id": 1, "text": "Beras", "completion_date": 1700000000},
                    {"id": 2, "text": "Telur"}
                ]}
            },
            "external_reply": {"origin": {"type": "hidden_user", "date": 1, "sender_user_name": "Anon"}}
        }
    }))
    .expect("reply update should deserialize");
    let message = reply.message.expect("message present");
    let quote = message.quote.expect("quote present");
    assert_eq!(quote.text, "Telur");
    assert_eq!(quote.is_manual, Some(true));
    assert_eq!(message.reply_to_checklist_task_id, Some(2));
    let checklist = message
        .reply_to_message
        .and_then(|replied| replied.checklist)
        .expect("checklist present");
    assert!(checklist.tasks[0].is_done());
    assert!(!checklist.tasks[1].is_done());
    assert!(message.external_reply.is_some());

    let query: Update = serde_json::from_value(serde_json::json!({
        "update_id": 31,
        "inline_query": {"id": "iq", "from": {"id": 42, "is_bot": false, "first_name": "O"},
                         "query": "halo", "offset": "", "chat_type": "sender"}
    }))
    .expect("inline query should deserialize");
    assert_eq!(query.inline_query.expect("inline query").query, "halo");

    let chosen: Update = serde_json::from_value(serde_json::json!({
        "update_id": 32,
        "chosen_inline_result": {"result_id": "r", "from": {"id": 42, "is_bot": false, "first_name": "O"},
                                 "inline_message_id": "im", "query": "halo"}
    }))
    .expect("chosen inline result should deserialize");
    assert_eq!(
        chosen
            .chosen_inline_result
            .and_then(|result| result.inline_message_id)
            .as_deref(),
        Some("im")
    );
}

#[test]
fn extended_rich_text_entities_match_bot_api_shapes() {
    let blocks = parser::parse_markdown_to_rich_blocks(
        "==a== x<sup>2</sup> H<sub>2</sub>O ![22:45](tg://time?unix=1647531900&format=wDT) ![👍](tg://emoji?id=5368324170671202286)",
    );
    let json = serde_json::to_value(&blocks).expect("blocks serialize");
    let text = &json[0]["text"];
    let entity = |kind: &str| {
        text.as_array()
            .and_then(|parts| parts.iter().find(|part| part["type"] == kind))
            .cloned()
            .unwrap_or_default()
    };
    assert_eq!(entity("marked")["text"], "a");
    assert_eq!(entity("superscript")["text"], "2");
    assert_eq!(entity("subscript")["text"], "2");
    let date_time = entity("date_time");
    assert_eq!(date_time["unix_time"], 1_647_531_900_i64);
    assert_eq!(date_time["date_time_format"], "wDT");
    assert_eq!(date_time["text"], "22:45");
    let emoji = entity("custom_emoji");
    assert_eq!(emoji["custom_emoji_id"], "5368324170671202286");
    assert_eq!(emoji["alternative_text"], "👍");
    assert!(
        emoji.get("text").is_none(),
        "custom emoji has no text field"
    );
}

#[test]
fn poll_options_carry_optional_media() {
    let mut option = InputPollOption::new("Paus");
    let plain = serde_json::to_value(&option).expect("option serializes");
    assert!(plain.get("media").is_none());
    option.media = Some(serde_json::json!({"type": "photo", "media": "https://example.com/a.jpg"}));
    let with_media = serde_json::to_value(&option).expect("option serializes");
    assert_eq!(with_media["media"]["type"], "photo");
}

#[test]
fn in_message_navigation_matches_bot_api_shapes() {
    let blocks = parser::parse_markdown_to_rich_blocks(
        "[Ke bagian DNS](#dns)\n\n## DNS\nPenjelasan[^1].\n\n[^1]: Sumber: RFC 1035.",
    );
    let json = serde_json::to_value(&blocks).expect("blocks serialize");
    assert_eq!(json[0]["text"]["type"], "anchor_link");
    assert_eq!(json[0]["text"]["anchor_name"], "bagian-1");
    assert_eq!(
        json[1],
        serde_json::json!({"type": "anchor", "name": "bagian-1"})
    );
    assert_eq!(json[2]["type"], "heading");
    let marker = &json[3]["text"][1];
    assert_eq!(marker["type"], "reference_link");
    assert_eq!(marker["reference_name"], "catatan-1");
    let note = &json[4]["text"][1];
    assert_eq!(note["type"], "reference");
    assert_eq!(note["name"], "catatan-1");
    assert_eq!(note["text"], "Sumber: RFC 1035.");
}
