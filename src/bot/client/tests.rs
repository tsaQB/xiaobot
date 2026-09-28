//! Behavioural tests for the Telegram client against an in-process fake Bot
//! API server. They replace source-text assertions and tautological tests
//! that only checked JSON literals written by the test itself.

use super::*;
use crate::bot::models::{InputPollOption, InputRichMessage, RichBlock};
use crate::bot::test_support::{api_error, ok_message, FakeTelegram};
use std::sync::Arc;

fn quiz_options() -> Vec<InputPollOption> {
    vec![InputPollOption::new("Satu"), InputPollOption::new("Dua")]
}

#[tokio::test]
async fn quiz_payload_uses_correct_option_ids_and_delivery_context() {
    let fake = FakeTelegram::always_ok().await;
    let context = TelegramDeliveryContext {
        message_thread_id: Some(77),
        ..TelegramDeliveryContext::default()
    };
    TelegramBotClient::with_delivery_context(
        context,
        fake.client.send_poll(
            5,
            "Berapa 1+1?",
            &quiz_options(),
            Some(false),
            Some("quiz"),
            Some(1),
            Some("Karena 1+1=2"),
            None,
            Some(999),
        ),
    )
    .await
    .expect("sendPoll succeeds");

    let request = &fake.requests()[0];
    assert_eq!(request.method, "sendPoll");
    assert_eq!(request.json["type"], "quiz");
    assert_eq!(request.json["correct_option_ids"], json!([1]));
    assert!(
        request.json.get("correct_option_id").is_none(),
        "Bot API 9.6 removed the singular parameter"
    );
    assert_eq!(request.json["message_thread_id"], 77);
    assert_eq!(request.json["reply_parameters"]["message_id"], 999);
    assert_eq!(request.json["options"][1]["text"], "Dua");
}

#[tokio::test]
async fn poll_type_defaults_to_regular() {
    let fake = FakeTelegram::always_ok().await;
    fake.client
        .send_poll(
            5,
            "Pilih",
            &quiz_options(),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .expect("sendPoll succeeds");
    assert_eq!(fake.requests()[0].json["type"], "regular");
}

#[tokio::test]
async fn callback_and_ephemeral_delete_payloads() {
    let fake = FakeTelegram::always_ok().await;
    fake.client
        .answer_callback_query("cbq_1", Some("Siap"), false)
        .await
        .expect("answerCallbackQuery succeeds");
    fake.client
        .delete_ephemeral_message(5, 6, 7)
        .await
        .expect("deleteEphemeralMessage succeeds");
    let requests = fake.requests();
    assert_eq!(requests[0].method, "answerCallbackQuery");
    assert_eq!(requests[0].json["callback_query_id"], "cbq_1");
    assert_eq!(requests[0].json["text"], "Siap");
    assert_eq!(requests[1].method, "deleteEphemeralMessage");
    assert_eq!(requests[1].json["receiver_user_id"], 6);
    assert_eq!(requests[1].json["ephemeral_message_id"], 7);
}

#[tokio::test]
async fn rate_limit_honors_retry_after_then_succeeds() {
    let fake = FakeTelegram::start(Arc::new(|_, index| {
        if index == 0 {
            (
                200,
                json!({
                    "ok": false,
                    "error_code": 429,
                    "description": "Too Many Requests: retry after 1",
                    "parameters": {"retry_after": 1}
                }),
            )
        } else {
            ok_message(10)
        }
    }))
    .await;
    let started = std::time::Instant::now();
    fake.client
        .send_message(5, "halo", None, None, None, None)
        .await
        .expect("request succeeds after the advertised wait");
    assert_eq!(fake.methods(), vec!["sendMessage", "sendMessage"]);
    assert!(
        started.elapsed() >= Duration::from_millis(900),
        "retry_after must be honored"
    );
}

#[tokio::test]
async fn permanent_sends_never_carry_a_draft_id() {
    let fake = FakeTelegram::always_ok().await;
    fake.client
        .send_message(5, "halo", None, None, None, None)
        .await
        .expect("sendMessage succeeds");
    let rich = InputRichMessage::new(vec![RichBlock::Paragraph {
        text: Value::String("halo".to_string()),
    }]);
    fake.client
        .send_rich_message(5, &rich, None, None, None)
        .await
        .expect("sendRichMessage succeeds");
    for request in fake.requests() {
        assert!(
            request.json.get("draft_id").is_none(),
            "{} must not include draft_id",
            request.method
        );
    }
}

/// When the HTML fallback fails part-way, only the undelivered remainder is
/// re-sent as plain text; the parts already delivered are not duplicated.
#[tokio::test]
async fn html_fallback_failure_resends_only_the_remainder() {
    let fake = FakeTelegram::start(Arc::new(|request, _| {
        match request.method.as_str() {
            // Rich message rejected: forces the HTML fallback.
            "sendRichMessage" => api_error(400, "Bad Request: unsupported block"),
            "sendMessage" if request.json.get("parse_mode") == Some(&json!("HTML")) => {
                let text = request.json["text"].as_str().unwrap_or_default();
                if text.contains("BBBB") {
                    api_error(400, "Bad Request: can't parse entities")
                } else {
                    ok_message(1)
                }
            }
            _ => ok_message(2),
        }
    }))
    .await;
    let rich = InputRichMessage::new(vec![
        RichBlock::Paragraph {
            text: Value::String("A".repeat(3_000)),
        },
        RichBlock::Paragraph {
            text: Value::String("B".repeat(3_000)),
        },
    ]);
    fake.client
        .send_rich_message(5, &rich, None, None, None)
        .await
        .expect("message is delivered through the fallback chain");

    let plain_sends: Vec<String> = fake
        .requests()
        .into_iter()
        .filter(|request| {
            request.method == "sendMessage" && request.json.get("parse_mode").is_none()
        })
        .map(|request| {
            request.json["text"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    assert!(
        !plain_sends.is_empty(),
        "the remainder is sent as plain text"
    );
    let combined = plain_sends.join("");
    assert!(combined.contains("BBBB"));
    assert!(
        !combined.contains("AAAA"),
        "the already delivered first part must not be sent again"
    );
}

/// Bot API 10.3 forbids uploads and URL media in drafts; the draft carries
/// links/placeholders instead and is always plain JSON (never multipart).
#[tokio::test]
async fn drafts_never_upload_or_reference_media() {
    let fake = FakeTelegram::always_ok().await;
    let rich = InputRichMessage::new(vec![
        RichBlock::Paragraph {
            text: Value::String("Sedang menyiapkan foto".to_string()),
        },
        RichBlock::Photo {
            photo: json!({"type": "photo", "media": "https://example.com/a.jpg"}),
            caption: None,
        },
        RichBlock::Document {
            document: json!({"type": "document", "media": "attach://file_0"}),
            caption: None,
        },
    ]);
    fake.client
        .send_rich_message_draft(5, 42, &rich, true, false)
        .await
        .expect("draft is accepted");
    let request = &fake.requests()[0];
    assert_eq!(request.method, "sendRichMessageDraft");
    assert!(request.json.is_object(), "draft must be a JSON request");
    let body = request.json.to_string();
    assert!(!body.contains("attach://"));
    assert!(!body.contains("\"type\":\"photo\""));
    assert!(!body.contains("\"type\":\"document\""));
    assert!(
        body.contains("https://example.com/a.jpg"),
        "remote media becomes a link"
    );
}

#[tokio::test]
async fn draft_plain_text_fallback_does_not_use_html_parse_mode() {
    let fake = FakeTelegram::start(Arc::new(|request, _| {
        if request.method == "sendRichMessageDraft" {
            api_error(400, "Bad Request: rich drafts unavailable")
        } else {
            (200, json!({"ok": true, "result": true}))
        }
    }))
    .await;
    let rich = InputRichMessage::new(vec![RichBlock::Paragraph {
        text: Value::String("a < b & c".to_string()),
    }]);
    fake.client
        .send_rich_message_draft(5, 43, &rich, true, false)
        .await
        .expect("plain draft fallback succeeds");
    let fallback = fake
        .requests()
        .into_iter()
        .find(|request| request.method == "sendMessageDraft")
        .expect("fallback draft sent");
    assert!(fallback.json.get("parse_mode").is_none());
}

#[tokio::test]
async fn oversized_telegram_file_reports_too_large() {
    let fake = FakeTelegram::start(Arc::new(|_, _| {
        (
            200,
            json!({"ok": true, "result": {
                "file_id": "f", "file_unique_id": "u", "file_size": 30 * 1024 * 1024
            }}),
        )
    }))
    .await;
    assert_eq!(
        fake.client.get_file_bytes("f").await.err(),
        Some(FileDownloadError::TooLarge)
    );
}

#[tokio::test]
async fn file_downloads_use_the_configured_bot_api_server() {
    let fake = FakeTelegram::start(Arc::new(|request, _| {
        if request.method == "getFile" {
            (
                200,
                json!({"ok": true, "result": {
                    "file_id": "f", "file_unique_id": "u", "file_size": 2,
                    "file_path": "documents/report.txt"
                }}),
            )
        } else {
            (200, json!("ok"))
        }
    }))
    .await;
    let (bytes, path) = fake
        .client
        .get_file_bytes("f")
        .await
        .expect("download succeeds from the configured server");
    assert_eq!(path, "documents/report.txt");
    assert!(!bytes.is_empty());
    assert_eq!(fake.methods(), vec!["getFile", "report.txt"]);
}

#[tokio::test]
async fn uploaded_document_names_are_sanitized() {
    let fake = FakeTelegram::always_ok().await;
    fake.client
        .send_document_bytes(
            5,
            "../../etc/\"evil\".bat",
            b"data".to_vec(),
            Some("text/plain"),
            None,
            None,
            None,
            None,
        )
        .await
        .expect("sendDocument succeeds");
    let body = &fake.requests()[0].raw_body;
    assert!(body.contains("filename=\"evil.bat\""), "{body}");
    assert!(!body.contains(".."));
}

#[test]
fn upload_filename_sanitizer_handles_edge_cases() {
    assert_eq!(sanitize_upload_filename("..\\..\\x.txt"), "x.txt");
    assert_eq!(sanitize_upload_filename(".bashrc"), "bashrc");
    assert_eq!(sanitize_upload_filename("\u{0}\n"), "file.bin");
    assert_eq!(
        sanitize_upload_filename(&"a".repeat(300)).chars().count(),
        MAX_UPLOAD_FILENAME_CHARS
    );
}
