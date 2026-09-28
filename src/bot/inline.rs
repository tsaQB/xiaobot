//! Inline mode: the owner types `@XiaoBot question` in any chat, picks the
//! "Tanya Xiao" result, and the message it posts is edited in place with the
//! answer once it is ready.
//!
//! Two updates make one answer. The `inline_query` is answered at once with a
//! placeholder result (it carries an inline keyboard, which is what makes
//! Telegram report the posted message's `inline_message_id`). The
//! `chosen_inline_result` that follows starts the generation. It requires
//! inline feedback to be enabled in @BotFather (`/setinlinefeedback`).
//!
//! Like guest mode, the answer lands in a chat other people read, so the
//! generation is stateless (see [`guest::generate_stateless_answer`]) and
//! nothing is ever sent to a chat directly: only the inline message is edited.
//! Queries from anyone but the owner are dropped without an answer.

use serde_json::{json, Value};
use tracing::warn;

use crate::ai::service::AIChatService;
use crate::bot::client::TelegramBotClient;
use crate::bot::guest::{
    deliver_inline_answer, generate_stateless_answer, EMPTY_ANSWER_TEXT, NO_PROVIDER_TEXT,
    THINKING_TEXT,
};
use crate::bot::models::{ChosenInlineResult, InlineQuery, InputRichMessage, RichBlock};
use crate::bot::router::ChatRouteScope;
use crate::bot::worker::{record_task_outcome, TaskOutcome, INLINE_SCOPE_THREAD_ID};
use crate::parser::build_full_rich_message;

/// Id of the single result offered for every query.
pub(crate) const INLINE_RESULT_ID: &str = "xiao-inline-answer";
const RESULT_TITLE: &str = "Tanya Xiao";
const ASK_AGAIN_TEXT: &str = "✍️ Tanya Xiao lagi";
const INTERRUPTED_TEXT: &str = "⚠️ Jawaban Xiao terhenti sebelum selesai. Silakan tanya lagi.";
/// Characters of the question shown under the result title.
const DESCRIPTION_CHARS: usize = 100;

fn question_line(query: &str) -> String {
    format!("❓ {query}")
}

/// The one result offered for a query: a placeholder message with a button
/// to ask again (the keyboard is required for the edit that follows).
fn placeholder_result(query: &str) -> Value {
    json!({
        "type": "article",
        "id": INLINE_RESULT_ID,
        "title": RESULT_TITLE,
        "description": crate::util::truncate_chars(query, DESCRIPTION_CHARS),
        "input_message_content": {
            "message_text": format!("{}\n\n{THINKING_TEXT}", question_line(query)),
        },
        "reply_markup": {
            "inline_keyboard": [[{
                "text": ASK_AGAIN_TEXT,
                "switch_inline_query_current_chat": "",
            }]],
        },
    })
}

/// The final message: the question in bold, then the answer. The question is
/// placed as a ready-made block so its text is never read as Markdown.
fn answer_message(query: &str, answer: &str) -> InputRichMessage {
    let mut message = build_full_rich_message(answer, None);
    message.blocks.insert(
        0,
        RichBlock::Paragraph {
            text: json!({"type": "bold", "text": question_line(query)}),
        },
    );
    message
}

pub async fn handle_inline_query(
    bot: &TelegramBotClient,
    route_scope: &ChatRouteScope,
    query: InlineQuery,
) {
    // Hard single-owner invariant: other users get no answer at all.
    if query.from.id != route_scope.owner_user_id {
        return;
    }
    let text = query.query.trim();
    let results = if text.is_empty() {
        json!([])
    } else {
        json!([placeholder_result(text)])
    };
    // A query outlives its answer by seconds only, so a failure is logged
    // and not retried.
    if let Err(error) = bot.answer_inline_query(&query.id, results).await {
        warn!("Inline query could not be answered: {error}");
    }
}

pub async fn handle_chosen_inline_result(
    bot: &TelegramBotClient,
    ai_service: &AIChatService,
    route_scope: &ChatRouteScope,
    chosen: ChosenInlineResult,
) {
    let owner_id = chosen.from.id;
    if owner_id != route_scope.owner_user_id || chosen.result_id != INLINE_RESULT_ID {
        return;
    }
    let Some(inline_message_id) = chosen
        .inline_message_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
    else {
        return;
    };
    if ai_service.is_shutting_down() {
        // The placeholder stays; the answer is written after the restart.
        record_task_outcome(TaskOutcome::Interrupted);
        return;
    }
    let query = chosen.query.trim();
    if !ai_service.has_configured_provider(owner_id).await {
        let notice = build_full_rich_message(NO_PROVIDER_TEXT, None);
        deliver_inline_answer(bot, inline_message_id, &notice).await;
        return;
    }

    let (answer, cancelled) = generate_stateless_answer(
        ai_service,
        owner_id,
        INLINE_SCOPE_THREAD_ID,
        owner_id,
        query,
    )
    .await;
    if cancelled && ai_service.is_shutting_down() {
        // Unlike a guest query, an inline message can still be edited after
        // a restart, so the update is replayed rather than given up.
        record_task_outcome(TaskOutcome::Interrupted);
        return;
    }
    let final_text = if cancelled {
        INTERRUPTED_TEXT
    } else if answer.trim().is_empty() {
        EMPTY_ANSWER_TEXT
    } else {
        answer.trim()
    };
    deliver_inline_answer(bot, inline_message_id, &answer_message(query, final_text)).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot::test_support::{FakeProvider, FakeTelegram};
    use std::collections::HashSet;

    const OWNER: i64 = 42;

    fn scope(bot: &TelegramBotClient) -> ChatRouteScope {
        ChatRouteScope::new(
            OWNER,
            HashSet::new(),
            HashSet::new(),
            Some(900),
            Some("XiaoBot".to_string()),
            bot.clone(),
        )
    }

    fn inline_query(from_id: i64, text: &str) -> InlineQuery {
        serde_json::from_value(json!({
            "id": "iq-1",
            "from": {"id": from_id, "is_bot": false, "first_name": "Owner"},
            "query": text,
            "offset": "",
            "chat_type": "group",
        }))
        .expect("valid inline query")
    }

    fn chosen(from_id: i64, inline_message_id: Option<&str>) -> ChosenInlineResult {
        serde_json::from_value(json!({
            "result_id": INLINE_RESULT_ID,
            "from": {"id": from_id, "is_bot": false, "first_name": "Owner"},
            "inline_message_id": inline_message_id,
            "query": "apa itu Rust?",
        }))
        .expect("valid chosen inline result")
    }

    #[tokio::test]
    async fn owner_query_gets_one_personal_uncached_placeholder() {
        let telegram = FakeTelegram::always_ok().await;
        handle_inline_query(
            &telegram.client,
            &scope(&telegram.client),
            inline_query(OWNER, " apa itu Rust? "),
        )
        .await;

        let requests = telegram.requests();
        assert_eq!(requests.len(), 1);
        let body = &requests[0].json;
        assert_eq!(requests[0].method, "answerInlineQuery");
        assert_eq!(body["inline_query_id"], "iq-1");
        assert_eq!(body["is_personal"], true);
        assert_eq!(body["cache_time"], 0);
        let result = &body["results"][0];
        assert_eq!(result["id"], INLINE_RESULT_ID);
        assert_eq!(result["type"], "article");
        assert!(result["input_message_content"]["message_text"]
            .as_str()
            .is_some_and(|text| text.starts_with("❓ apa itu Rust?")));
        assert_eq!(
            result["reply_markup"]["inline_keyboard"][0][0]["switch_inline_query_current_chat"],
            ""
        );
    }

    #[tokio::test]
    async fn other_users_get_no_inline_answer() {
        let telegram = FakeTelegram::always_ok().await;
        let scope = scope(&telegram.client);
        handle_inline_query(&telegram.client, &scope, inline_query(7, "bocorkan")).await;

        let provider = FakeProvider::streaming("tidak boleh").await;
        let service = AIChatService::isolated_for_tests(provider.config());
        handle_chosen_inline_result(&telegram.client, &service, &scope, chosen(7, Some("im-1")))
            .await;

        assert!(telegram.requests().is_empty());
        assert!(provider.chat_requests().is_empty());
    }

    #[tokio::test]
    async fn chosen_result_is_answered_statelessly_by_editing_the_inline_message() {
        let telegram = FakeTelegram::always_ok().await;
        let provider = FakeProvider::streaming("Rust adalah bahasa pemrograman.").await;
        let service = AIChatService::isolated_for_tests(provider.config());
        handle_chosen_inline_result(
            &telegram.client,
            &service,
            &scope(&telegram.client),
            chosen(OWNER, Some("im-1")),
        )
        .await;

        let requests = telegram.requests();
        assert_eq!(telegram.methods(), ["editMessageText"]);
        let edit = &requests[0].json;
        assert_eq!(edit["inline_message_id"], "im-1");
        assert!(edit.get("chat_id").is_none(), "never addressed to a chat");
        let rich = edit["rich_message"].to_string();
        assert!(rich.contains("❓ apa itu Rust?"), "{rich}");
        assert!(rich.contains("Rust adalah bahasa pemrograman."), "{rich}");

        let chat_requests = provider.chat_requests();
        assert_eq!(chat_requests.len(), 1);
        let messages = chat_requests[0]["messages"]
            .as_array()
            .expect("messages array");
        assert_eq!(messages.len(), 2, "system prompt and the question only");
    }

    #[tokio::test]
    async fn chosen_result_without_an_editable_message_is_ignored() {
        let telegram = FakeTelegram::always_ok().await;
        let provider = FakeProvider::streaming("tidak dipakai").await;
        let service = AIChatService::isolated_for_tests(provider.config());
        handle_chosen_inline_result(
            &telegram.client,
            &service,
            &scope(&telegram.client),
            chosen(OWNER, None),
        )
        .await;
        assert!(telegram.requests().is_empty());
        assert!(provider.chat_requests().is_empty());
    }
}
