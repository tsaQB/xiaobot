//! End-to-end routing tests for edited messages and the message kinds added
//! with Bot API 10.x (stickers, locations), against a fake Telegram server
//! and a fake streaming provider.
//!
//! Like the other service-level tests, these use the process-wide SQLite
//! database, so every test owns a distinct chat id and uses message ids that
//! grow with the clock (the latest-prompt marker never moves backwards).

use super::*;
use crate::bot::test_support::{FakeProvider, FakeTelegram, RecordedRequest};
use serde_json::{json, Value};

fn clock() -> std::time::Duration {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
}

/// Message date in Unix seconds.
fn clock_secs() -> i64 {
    i64::try_from(clock().as_secs()).unwrap_or(1)
}

/// A message id larger than any used by an earlier run of the same test.
fn fresh_message_id() -> i64 {
    i64::try_from(clock().as_micros()).unwrap_or(1)
}

fn private_scope(owner: i64, bot: &TelegramBotClient) -> ChatRouteScope {
    ChatRouteScope::new(
        owner,
        HashSet::new(),
        HashSet::new(),
        Some(900),
        Some("XiaoBot".to_string()),
        bot.clone(),
    )
}

fn message_json(owner: i64, message_id: i64, date: i64, content: Value) -> Value {
    let mut message = json!({
        "message_id": message_id,
        "date": date,
        "chat": {"id": owner, "type": "private"},
        "from": {"id": owner, "is_bot": false, "first_name": "Owner"},
    });
    if let (Some(target), Some(fields)) = (message.as_object_mut(), content.as_object()) {
        target.extend(fields.clone());
    }
    message
}

fn new_message(update_id: i64, message: Value) -> Update {
    serde_json::from_value(json!({"update_id": update_id, "message": message}))
        .expect("valid message update")
}

fn edited_message(update_id: i64, mut message: Value, edit_date: i64) -> Update {
    message["edit_date"] = json!(edit_date);
    serde_json::from_value(json!({"update_id": update_id, "edited_message": message}))
        .expect("valid edited_message update")
}

struct Harness {
    owner: i64,
    telegram: FakeTelegram,
    provider: FakeProvider,
    service: AIChatService,
    scope: ChatRouteScope,
    images: UserLastImagePrompt,
}

impl Harness {
    async fn new(owner: i64, telegram: FakeTelegram, answer: &str) -> Self {
        let provider = FakeProvider::streaming(answer).await;
        let service = AIChatService::isolated_for_tests(provider.config());
        let scope = private_scope(owner, &telegram.client);
        Self {
            owner,
            telegram,
            provider,
            service,
            scope,
            images: UserLastImagePrompt::default(),
        }
    }

    async fn send(&self, update: Update) {
        handle_update(
            &self.telegram.client,
            &self.service,
            &self.images,
            &self.scope,
            update,
        )
        .await;
    }

    fn main_requests(&self) -> Vec<Value> {
        main_requests(&self.provider)
    }

    fn message(&self, message_id: i64, date: i64, content: Value) -> Value {
        message_json(self.owner, message_id, date, content)
    }

    async fn clean_up(&self) {
        crate::ai::storage::clear_scoped_messages_async(self.owner, 0).await;
    }
}

/// Streamed (main-model) chat requests; background curation is not streamed.
fn main_requests(provider: &FakeProvider) -> Vec<Value> {
    provider
        .chat_requests()
        .into_iter()
        .filter(|body| body["stream"] == json!(true))
        .collect()
}

fn last_user_content(body: &Value) -> String {
    body["messages"]
        .as_array()
        .and_then(|messages| messages.iter().rev().find(|m| m["role"] == "user"))
        .map(|message| message["content"].to_string())
        .unwrap_or_default()
}

fn replies_to(requests: &[RecordedRequest], message_id: i64) -> bool {
    requests
        .iter()
        .any(|request| request.json["reply_parameters"]["message_id"] == json!(message_id))
}

#[tokio::test]
async fn an_edited_prompt_is_answered_again_only_while_latest_and_changed() {
    let h = Harness::new(880_001, FakeTelegram::always_ok().await, "Jawaban Xiao").await;
    let now = clock_secs();
    let first = fresh_message_id();
    let text = |t: &str| json!({ "text": t });

    h.send(new_message(1, h.message(first, now, text("halo"))))
        .await;
    assert_eq!(h.main_requests().len(), 1);
    assert!(
        !replies_to(&h.telegram.requests(), first),
        "a normal private answer does not quote the prompt"
    );

    let edited = h.message(first, now, text("halo, cuaca hari ini?"));
    h.send(edited_message(2, edited.clone(), now + 30)).await;
    let requests = h.main_requests();
    assert_eq!(requests.len(), 2, "a real text change is answered again");
    assert!(last_user_content(&requests[1]).contains("cuaca hari ini"));
    assert!(
        replies_to(&h.telegram.requests(), first),
        "the new answer quotes the edited message"
    );

    h.send(edited_message(3, edited, now + 40)).await;
    assert_eq!(
        h.main_requests().len(),
        2,
        "an edit event without a text change (e.g. a reaction) is ignored"
    );

    let late = h.message(first, now, text("teks sangat terlambat"));
    h.send(edited_message(4, late, now + inbound::EDIT_WINDOW_SECS + 1))
        .await;
    assert_eq!(
        h.main_requests().len(),
        2,
        "edits after the window are ignored"
    );

    h.send(new_message(
        5,
        h.message(first + 1, now, text("pesan baru")),
    ))
    .await;
    assert_eq!(h.main_requests().len(), 3);
    let older = h.message(first, now, text("ubah pesan lama"));
    h.send(edited_message(6, older, now + 50)).await;
    assert_eq!(
        h.main_requests().len(),
        3,
        "an edit of an older message is ignored once a newer prompt exists"
    );

    h.clean_up().await;
}

#[tokio::test]
async fn a_live_location_update_never_triggers_a_reply() {
    let h = Harness::new(880_004, FakeTelegram::always_ok().await, "tidak dipakai").await;
    let now = clock_secs();
    let location = json!({"location": {"latitude": -8.4, "longitude": 116.4, "live_period": 900}});

    h.send(edited_message(
        1,
        h.message(fresh_message_id(), now, location),
        now + 5,
    ))
    .await;

    assert!(h.main_requests().is_empty());
    assert!(h.telegram.requests().is_empty());
}

#[tokio::test]
async fn a_shared_venue_reaches_the_model_as_text_context() {
    let h = Harness::new(880_003, FakeTelegram::always_ok().await, "Itu di Lombok.").await;
    let now = clock_secs();
    let venue = json!({
        "location": {"latitude": -8.41, "longitude": 116.45},
        "venue": {
            "location": {"latitude": -8.41, "longitude": 116.45},
            "title": "Segara Anak",
            "address": "Lombok"
        }
    });

    h.send(new_message(1, h.message(fresh_message_id(), now, venue)))
        .await;

    let requests = h.main_requests();
    assert_eq!(requests.len(), 1);
    let content = last_user_content(&requests[0]);
    assert!(
        content.contains("Tempat dibagikan: Segara Anak, Lombok"),
        "{content}"
    );
    assert!(content.contains("maps.google.com"), "{content}");

    h.clean_up().await;
}

#[tokio::test]
async fn a_sticker_is_shown_to_the_model_as_an_image_with_its_emoji() {
    let telegram = FakeTelegram::start(std::sync::Arc::new(|request, _| {
        if request.method == "getFile" {
            (
                200,
                json!({"ok": true, "result": {
                    "file_id": "S", "file_unique_id": "u", "file_size": 4,
                    "file_path": "stickers/file_1.webp"
                }}),
            )
        } else {
            crate::bot::test_support::ok_message(1)
        }
    }))
    .await;
    let h = Harness::new(880_002, telegram, "Lucu sekali!").await;
    let now = clock_secs();
    let sticker = json!({"sticker": {
        "file_id": "S", "file_unique_id": "u", "type": "regular",
        "width": 512, "height": 512, "is_animated": false, "is_video": false,
        "emoji": "😂", "set_name": "LaughPack"
    }});

    h.send(new_message(1, h.message(fresh_message_id(), now, sticker)))
        .await;

    let methods = h.telegram.methods();
    assert!(
        methods.iter().any(|method| method == "getFile"),
        "{methods:?}"
    );
    let requests = h.main_requests();
    assert_eq!(requests.len(), 1);
    let body = requests[0].to_string();
    assert!(body.contains("Stiker 😂"), "{body}");
    assert!(body.contains("data:image/webp;base64,"), "{body}");

    h.clean_up().await;
}

#[tokio::test]
async fn a_reply_carries_the_replied_message_as_quoted_context() {
    let h = Harness::new(880_005, FakeTelegram::always_ok().await, "Siap.").await;
    let now = clock_secs();
    let reply_to_answer = json!({
        "text": "jelaskan poin kedua",
        "reply_to_message": {
            "message_id": 1, "date": now,
            "chat": {"id": h.owner, "type": "private"},
            "from": {"id": 900, "is_bot": true, "first_name": "Xiao", "username": "XiaoBot"},
            "rich_message": {"blocks": [{"type": "paragraph", "text": "Poin kedua: gunakan cache."}]}
        }
    });
    h.send(new_message(
        1,
        h.message(fresh_message_id(), now, reply_to_answer),
    ))
    .await;

    // Quoted text that reads like an image request must not trigger image
    // generation: intent detection only sees the owner's own words.
    let reply_to_own = json!({
        "text": "terjemahkan ke bahasa Inggris",
        "reply_to_message": {
            "message_id": 2, "date": now,
            "chat": {"id": h.owner, "type": "private"},
            "from": {"id": h.owner, "is_bot": false, "first_name": "Owner"},
            "text": "buatkan gambar kucing lucu"
        }
    });
    h.send(new_message(
        2,
        h.message(fresh_message_id(), now, reply_to_own),
    ))
    .await;

    let requests = h.main_requests();
    assert_eq!(requests.len(), 2, "both replies are answered by the model");
    let first = last_user_content(&requests[0]);
    assert!(
        first.contains("Pesan yang dibalas (jawaban Xiao sebelumnya;"),
        "{first}"
    );
    assert!(first.contains("Poin kedua: gunakan cache."), "{first}");
    assert!(first.contains("jelaskan poin kedua"), "{first}");
    let second = last_user_content(&requests[1]);
    assert!(second.contains("(pesan Anda sendiri;"), "{second}");
    assert!(second.contains("buatkan gambar kucing lucu"), "{second}");

    h.clean_up().await;
}

#[tokio::test]
async fn a_reply_under_a_photo_lets_the_model_see_that_photo() {
    let telegram = FakeTelegram::start(std::sync::Arc::new(|request, _| {
        if request.method == "getFile" {
            (
                200,
                json!({"ok": true, "result": {
                    "file_id": "P", "file_unique_id": "p", "file_size": 4,
                    "file_path": "photos/file_7.jpg"
                }}),
            )
        } else {
            crate::bot::test_support::ok_message(1)
        }
    }))
    .await;
    let h = Harness::new(880_006, telegram, "Itu Gunung Rinjani.").await;
    let now = clock_secs();
    let reply = json!({
        "text": "ini gunung apa?",
        "reply_to_message": {
            "message_id": 3, "date": now,
            "chat": {"id": h.owner, "type": "private"},
            "from": {"id": h.owner, "is_bot": false, "first_name": "Owner"},
            "photo": [{"file_id": "P", "file_unique_id": "p", "width": 10, "height": 10}]
        }
    });
    h.send(new_message(1, h.message(fresh_message_id(), now, reply)))
        .await;

    assert!(h
        .telegram
        .methods()
        .iter()
        .any(|method| method == "getFile"));
    let requests = h.main_requests();
    assert_eq!(requests.len(), 1);
    let body = requests[0].to_string();
    assert!(body.contains("data:image/jpeg;base64,"), "{body}");
    assert!(body.contains("[Foto]"), "{body}");
    assert!(body.contains("ini gunung apa?"), "{body}");

    h.clean_up().await;
}
