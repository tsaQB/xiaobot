//! The `create_quiz` tool: a native Telegram quiz, optionally preceded by a
//! Markdown preamble, with one or several correct answers, pictures for the
//! question and the options, and shuffled options (Bot API 9.6 / 10.1).

use serde_json::{json, Value};
use tracing::warn;

use crate::ai::tools::CreateQuizArgs;
use crate::bot::client::{PollRequest, TelegramBotClient};
use crate::bot::models::InputPollOption;
use crate::bot::transport_policy::fallback_allowed_error;

/// What one `create_quiz` call produced.
pub(super) struct QuizOutcome {
    /// Tool result returned to the model.
    pub result: String,
    /// Whether a native quiz reached the chat.
    pub sent: bool,
    /// Text form of the sent quiz for the conversation history.
    pub history_summary: Option<String>,
}

impl QuizOutcome {
    fn failed(result: String) -> Self {
        Self {
            result,
            sent: false,
            history_summary: None,
        }
    }
}

fn photo(url: &str) -> Value {
    json!({"type": "photo", "media": url})
}

fn is_correct(correct_ids: &[i32], index: usize) -> bool {
    correct_ids
        .iter()
        .any(|&id| usize::try_from(id).is_ok_and(|id| id == index))
}

/// The quiz as history text, marking the correct answers.
fn history_summary(args: &CreateQuizArgs, correct_ids: &[i32]) -> String {
    let mut summary = String::new();
    if let Some(preamble) = &args.preamble {
        summary.push_str(preamble);
        summary.push_str("\n\n");
    }
    summary.push_str(&format!("📊 **Kuis**: {}\n", args.question));
    if let Some(description) = &args.description {
        summary.push_str(&format!("{description}\n"));
    }
    for (index, option) in args.options.iter().enumerate() {
        let mark = if is_correct(correct_ids, index) {
            " (Benar)"
        } else {
            ""
        };
        summary.push_str(&format!("{}. {option}{mark}\n", index + 1));
    }
    if let Some(explanation) = &args.explanation {
        summary.push_str(&format!("\n💡 Penjelasan: {explanation}\n"));
    }
    summary
}

/// The quiz as a plain answer, for channels without native quizzes.
fn text_quiz(args: &CreateQuizArgs, correct_ids: &[i32]) -> String {
    let mut output = String::new();
    if let Some(preamble) = &args.preamble {
        output.push_str(preamble);
        output.push_str("\n\n");
    }
    output.push_str(&format!("📊 **Kuis**: {}\n\n", args.question));
    if let Some(description) = &args.description {
        output.push_str(&format!("{description}\n\n"));
    }
    for (index, option) in args.options.iter().enumerate() {
        let marker = if is_correct(correct_ids, index) {
            "✅"
        } else {
            "⚪"
        };
        output.push_str(&format!("{marker} {}. {option}\n", index + 1));
    }
    if let Some(explanation) = &args.explanation {
        output.push_str(&format!("\n💡 Penjelasan: {explanation}\n"));
    }
    output
}

/// Sends the preamble; returns its message id so the quiz can reply to it.
async fn send_preamble(
    bot: &TelegramBotClient,
    chat_id: i64,
    preamble: &str,
    reply_to_message_id: Option<i64>,
) -> Result<i64, String> {
    let rich_preamble = crate::parser::build_full_rich_message(preamble, None);
    let response = bot
        .send_rich_message(chat_id, &rich_preamble, None, None, reply_to_message_id)
        .await
        .map_err(|err| format!("Gagal mengirim pesan pengantar kuis ke Telegram: {err}"))?;
    response
        .get("message_id")
        .and_then(Value::as_i64)
        .or_else(|| {
            response
                .get("result")
                .and_then(|result| result.get("message_id"))
                .and_then(Value::as_i64)
        })
        .ok_or_else(|| {
            format!("Gagal mendapatkan ID pesan pengantar kuis dari Telegram: {response}")
        })
}

/// Runs one `create_quiz` tool call. Without a Telegram handle (CLI,
/// WhatsApp) the quiz is returned as text for the model to present.
pub(super) async fn run_create_quiz(
    bot: Option<&TelegramBotClient>,
    chat_id: i64,
    reply_to_message_id: Option<i64>,
    arguments: &str,
) -> QuizOutcome {
    let mut args = match serde_json::from_str::<CreateQuizArgs>(arguments) {
        Ok(args) => args,
        Err(err) => return QuizOutcome::failed(format!("Format argumen kuis tidak valid: {err}")),
    };
    args.sanitize();
    let correct_ids = match args.validate() {
        Ok(ids) => ids,
        Err(err) => return QuizOutcome::failed(format!("Validasi kuis gagal: {err}")),
    };
    let Some(bot) = bot else {
        return QuizOutcome::failed(text_quiz(&args, &correct_ids));
    };

    let preamble_id = match args.preamble.as_deref() {
        Some(preamble) => match send_preamble(bot, chat_id, preamble, reply_to_message_id).await {
            Ok(id) => Some(id),
            Err(err) => return QuizOutcome::failed(err),
        },
        None => None,
    };

    let mut options: Vec<InputPollOption> = args
        .options
        .iter()
        .map(|option| InputPollOption::new(option.as_str()))
        .collect();
    for (option, url) in options.iter_mut().zip(&args.option_image_urls) {
        if !url.is_empty() {
            option.media = Some(photo(url));
        }
    }
    let has_pictures = args.image_url.is_some()
        || args.explanation_image_url.is_some()
        || options.iter().any(|o| o.media.is_some());
    let mut poll = PollRequest {
        question: &args.question,
        options: &options,
        is_anonymous: Some(args.is_anonymous.unwrap_or(false)),
        poll_type: Some("quiz"),
        correct_option_ids: &correct_ids,
        explanation: args.explanation.as_deref(),
        explanation_parse_mode: None,
        shuffle_options: args.shuffle_options.unwrap_or(false),
        media: args.image_url.as_deref().map(photo),
        description: args.description.as_deref(),
        explanation_media: args.explanation_image_url.as_deref().map(photo),
        allows_revoting: args.allows_revoting.unwrap_or(false),
        open_period: args.open_period,
        hide_results_until_closes: args.hide_results_until_closes.unwrap_or(false),
    };
    let poll_reply_to = preamble_id.or(reply_to_message_id);

    let mut first = bot.send_poll(chat_id, &poll, poll_reply_to).await;
    let mut pictures_dropped = false;
    if let Err(err) = &first {
        // Telegram fetches poll pictures itself; an unreachable or
        // unsupported URL fails the whole quiz, so retry once without them.
        if has_pictures && fallback_allowed_error(err) {
            warn!("Quiz pictures were rejected; sending the quiz without them: {err}");
            let plain_options: Vec<InputPollOption> = args
                .options
                .iter()
                .map(|option| InputPollOption::new(option.as_str()))
                .collect();
            poll.options = &plain_options;
            poll.media = None;
            poll.explanation_media = None;
            first = bot.send_poll(chat_id, &poll, poll_reply_to).await;
            pictures_dropped = true;
        }
    }

    match first {
        Ok(_) => QuizOutcome {
            result: if pictures_dropped {
                "Kuis native Telegram berhasil dikirim ke obrolan, tetapi tanpa gambar karena Telegram tidak dapat memuat URL gambarnya.".to_string()
            } else {
                "Kuis native Telegram berhasil dikirim ke obrolan.".to_string()
            },
            sent: true,
            history_summary: Some(history_summary(&args, &correct_ids)),
        },
        Err(err) => {
            if let Some(id) = preamble_id {
                let _ = bot.delete_message(chat_id, id).await;
            }
            QuizOutcome::failed(format!("Gagal mengirim kuis native ke Telegram: {err}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot::test_support::{api_error, ok_message, FakeTelegram};
    use std::sync::Arc;

    const PICTURE_QUIZ: &str = r#"{
        "question": "Mana yang mamalia?",
        "options": ["Paus", "Hiu", "Kelelawar"],
        "correct_option_ids": [2, "0", 9],
        "shuffle_options": "true",
        "image_url": " https://example.com/laut.jpg ",
        "option_image_urls": ["https://example.com/paus.jpg", null, "bukan-url", "https://x/y.jpg"]
    }"#;

    #[test]
    fn several_correct_answers_are_normalized_and_validated() {
        let mut args: CreateQuizArgs =
            serde_json::from_str(PICTURE_QUIZ).expect("flexible quiz arguments");
        args.sanitize();
        assert_eq!(args.correct_option_ids, vec![0, 2], "in range, sorted");
        assert_eq!(
            args.image_url.as_deref(),
            Some("https://example.com/laut.jpg")
        );
        assert_eq!(
            args.option_image_urls,
            vec!["https://example.com/paus.jpg", "", ""],
            "one entry per option, web URLs only"
        );
        assert_eq!(args.validate(), Ok(vec![0, 2]));

        let mut all_correct = args.clone();
        all_correct.correct_option_ids = vec![0, 1, 2];
        assert!(
            all_correct.validate().is_err(),
            "a quiz needs a wrong option"
        );

        let legacy: CreateQuizArgs = serde_json::from_str(
            r#"{"question": "1+1?", "options": ["1", "2"], "correct_option_id": 1}"#,
        )
        .expect("legacy single answer");
        assert_eq!(legacy.validate(), Ok(vec![1]));
    }

    #[tokio::test]
    async fn picture_quiz_with_several_answers_reaches_telegram() {
        let fake = FakeTelegram::always_ok().await;
        let outcome = run_create_quiz(Some(&fake.client), 5, None, PICTURE_QUIZ).await;
        assert!(outcome.sent, "{}", outcome.result);

        let request = &fake.requests()[0].json;
        assert_eq!(request["type"], "quiz");
        assert_eq!(request["correct_option_ids"], json!([0, 2]));
        assert_eq!(request["allows_multiple_answers"], true);
        assert_eq!(request["shuffle_options"], true);
        assert_eq!(
            request["media"],
            json!({"type": "photo", "media": "https://example.com/laut.jpg"})
        );
        assert_eq!(
            request["options"][0]["media"]["media"],
            "https://example.com/paus.jpg"
        );
        assert!(request["options"][1].get("media").is_none());
        let summary = outcome.history_summary.expect("history summary");
        assert!(summary.contains("1. Paus (Benar)") && summary.contains("3. Kelelawar (Benar)"));
    }

    #[tokio::test]
    async fn description_explanation_picture_revoting_and_hidden_results_are_sent() {
        let fake = FakeTelegram::always_ok().await;
        let quiz = r#"{
            "question": "Planet terbesar?",
            "options": ["Mars", "Jupiter"],
            "correct_option_ids": [1],
            "description": "  Petunjuk: raksasa gas.  ",
            "explanation": "Jupiter paling besar.",
            "explanation_image_url": "https://example.com/jupiter.jpg",
            "allows_revoting": true,
            "open_period": "99999999",
            "hide_results_until_closes": true
        }"#;
        let outcome = run_create_quiz(Some(&fake.client), 5, None, quiz).await;
        assert!(outcome.sent, "{}", outcome.result);

        let request = &fake.requests()[0].json;
        assert_eq!(request["description"], "Petunjuk: raksasa gas.");
        assert_eq!(
            request["explanation_media"],
            json!({"type": "photo", "media": "https://example.com/jupiter.jpg"})
        );
        assert_eq!(request["allows_revoting"], true);
        assert_eq!(
            request["open_period"], 2_628_000,
            "clamped to the Bot API maximum"
        );
        assert_eq!(request["hide_results_until_closes"], true);
        assert!(outcome
            .history_summary
            .is_some_and(|summary| summary.contains("Petunjuk: raksasa gas.")));
    }

    #[test]
    fn hidden_results_need_a_closing_time() {
        let mut args: CreateQuizArgs = serde_json::from_str(
            r#"{"question": "1+1?", "options": ["1", "2"], "correct_option_ids": [1],
                "hide_results_until_closes": true}"#,
        )
        .expect("quiz arguments");
        args.sanitize();
        assert_eq!(
            args.hide_results_until_closes, None,
            "results hidden without a closing time would never be shown"
        );
    }

    #[tokio::test]
    async fn rejected_pictures_fall_back_to_a_plain_quiz() {
        let fake = FakeTelegram::start(Arc::new(|request, _| {
            let has_media = request.json.get("media").is_some();
            if has_media {
                api_error(400, "Bad Request: failed to get HTTP URL content")
            } else {
                ok_message(9)
            }
        }))
        .await;
        let outcome = run_create_quiz(Some(&fake.client), 5, None, PICTURE_QUIZ).await;
        assert!(outcome.sent);
        assert!(
            outcome.result.contains("tanpa gambar"),
            "{}",
            outcome.result
        );
        let requests = fake.requests();
        assert_eq!(requests.len(), 2);
        assert!(requests[1].json.get("media").is_none());
        assert!(requests[1].json["options"][0].get("media").is_none());
        assert_eq!(requests[1].json["correct_option_ids"], json!([0, 2]));
    }

    #[tokio::test]
    async fn without_telegram_the_quiz_is_returned_as_text() {
        let outcome = run_create_quiz(None, 5, None, PICTURE_QUIZ).await;
        assert!(!outcome.sent);
        assert!(outcome.result.contains("✅ 1. Paus"), "{}", outcome.result);
        assert!(outcome.result.contains("⚪ 2. Hiu"));
        assert!(outcome.result.contains("✅ 3. Kelelawar"));
    }
}
