//! Limits and helpers for the tool-calling loop of one generation: how many
//! rounds the model gets, how research calls are budgeted and run, how much
//! tool output may re-enter the prompt, and what the model is told between
//! rounds.

use futures_util::StreamExt;
use serde_json::Value;
use tokio::sync::watch;

use super::context::estimate_text_tokens;
use super::generation::{bound_tool_result, race_with_cancel, PendingToolCall, Raced};
use crate::timeline::{GenerationProgressSink, ProgressActivity};
use crate::util::truncate_chars;

/// Rounds in which the model may call tools. One more request follows
/// without tools, so the model always gets a turn to write the answer.
pub(crate) const MAX_TOOL_ROUNDS: usize = 5;
/// `web_search` and `fetch_url` calls allowed in one generation.
pub(crate) const MAX_RESEARCH_CALLS: usize = 10;
/// Research calls of one round that run at the same time.
const RESEARCH_CONCURRENCY: usize = 3;
/// Ceiling on all tool output of one generation, in estimated tokens.
const MAX_TOOL_OUTPUT_TOKENS: usize = 32_000;
/// A result is never cut below this, so the image list and the first results
/// at its head survive even when the budget is nearly spent.
const MIN_BUDGETED_RESULT_CHARS: usize = 1_500;
/// Reply the model is asked for once every requested quiz has been sent.
pub(crate) const QUIZ_DONE_REPLY: &str = "SELESAI";

/// Shares what is left of the context window after history among all tool
/// results of one generation. Extra tool rounds would otherwise let search
/// dumps push the request past the model's window.
pub(crate) struct ToolResultBudget {
    remaining_tokens: usize,
}

impl ToolResultBudget {
    pub(crate) fn new(available_tokens: usize) -> Self {
        Self {
            remaining_tokens: available_tokens.min(MAX_TOOL_OUTPUT_TOKENS),
        }
    }

    /// Bounds one result to the per-result cap and to the remaining budget.
    pub(crate) fn bound(&mut self, result: &str) -> String {
        let mut bounded = bound_tool_result(result);
        let tokens = estimate_text_tokens(&bounded);
        if tokens > self.remaining_tokens {
            let chars = bounded.chars().count();
            let keep = (chars.saturating_mul(self.remaining_tokens) / tokens.max(1))
                .max(MIN_BUDGETED_RESULT_CHARS);
            if keep < chars {
                bounded = truncate_chars(&bounded, keep);
                bounded.push_str(
                    "\n\n[Hasil tool dipotong Xiao karena jatah context untuk hasil tool hampir habis. Lanjutkan dengan informasi yang sudah ada.]",
                );
            }
        }
        self.remaining_tokens = self
            .remaining_tokens
            .saturating_sub(estimate_text_tokens(&bounded));
        bounded
    }
}

fn is_research_tool(name: &str) -> bool {
    name == "web_search" || name == "fetch_url"
}

/// The query or URL of a research call. Models occasionally send the bare
/// value instead of a JSON object, which is used as-is.
fn research_argument(name: &str, arguments: &str) -> String {
    let key = if name == "web_search" { "query" } else { "url" };
    serde_json::from_str::<Value>(arguments)
        .ok()
        .and_then(|value| value.get(key).and_then(Value::as_str).map(str::to_string))
        .unwrap_or_else(|| arguments.to_string())
}

/// Runs the `web_search` and `fetch_url` calls of one round concurrently.
///
/// Returns one entry per call (`None` for calls that are not research) and
/// whether the batch was cancelled. Calls beyond [`MAX_RESEARCH_CALLS`] for
/// the whole generation are answered with a notice instead of being run.
pub(crate) async fn run_research_calls(
    calls: &[PendingToolCall],
    research_calls_used: &mut usize,
    cancel_rx: &mut watch::Receiver<bool>,
    sink: Option<&dyn GenerationProgressSink>,
) -> (Vec<Option<String>>, bool) {
    let mut results: Vec<Option<String>> = vec![None; calls.len()];
    let mut jobs = Vec::new();
    for (index, call) in calls.iter().enumerate() {
        let name = call.name.trim();
        if !is_research_tool(name) {
            continue;
        }
        if *research_calls_used >= MAX_RESEARCH_CALLS {
            results[index] = Some(format!(
                "Batas {MAX_RESEARCH_CALLS} pencarian/pembacaan web untuk satu jawaban sudah tercapai. Lanjutkan dengan hasil yang sudah ada."
            ));
            continue;
        }
        *research_calls_used += 1;
        jobs.push((
            index,
            name == "web_search",
            research_argument(name, &call.arguments),
        ));
    }
    if jobs.is_empty() {
        return (results, false);
    }
    if let Some(sink) = sink {
        if jobs.iter().any(|(_, is_search, _)| *is_search) {
            sink.on_action("Searching", Some(ProgressActivity::Searching));
        } else {
            sink.on_action("Fetching", Some(ProgressActivity::Fetching));
        }
    }

    let work = futures_util::stream::iter(jobs.into_iter().map(
        |(index, is_search, argument)| async move {
            let result = if is_search {
                crate::ai::tools::execute_web_search(&argument).await
            } else {
                crate::ai::tools::fetch_web_content(&argument)
                    .await
                    .unwrap_or_else(|error| format!("Gagal membaca URL: {error}"))
            };
            (index, result)
        },
    ))
    .buffer_unordered(RESEARCH_CONCURRENCY)
    .collect::<Vec<_>>();

    match race_with_cancel(cancel_rx, work).await {
        Raced::Completed(done) => {
            for (index, result) in done {
                results[index] = Some(result);
            }
            (results, false)
        }
        Raced::Cancelled => (results, true),
    }
}

/// Identity of a quiz for duplicate detection: its normalized question.
pub(crate) fn quiz_question_key(arguments: &str) -> Option<String> {
    let value = serde_json::from_str::<Value>(arguments).ok()?;
    let question = value.get("question")?.as_str()?;
    let key = question
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    (!key.is_empty()).then_some(key)
}

/// Identity of a live photo for duplicate detection: both of its URLs.
pub(crate) fn live_photo_key(arguments: &str) -> Option<String> {
    let value = serde_json::from_str::<Value>(arguments).ok()?;
    let video = value.get("video_url")?.as_str()?.trim();
    let photo = value.get("photo_url")?.as_str()?.trim();
    Some(format!("{video}\n{photo}"))
}

/// Whether the model's last reply only confirms that the quizzes are done.
pub(crate) fn is_quiz_done_reply(text: &str) -> bool {
    let cleaned: String = text
        .chars()
        .filter(|ch| ch.is_alphanumeric())
        .collect::<String>()
        .to_uppercase();
    cleaned.is_empty() || cleaned == QUIZ_DONE_REPLY
}

/// Whether raw model output contains tool-call markup, parseable or not.
pub(crate) fn has_tool_call_markup(raw: &str) -> bool {
    let lower = raw.to_ascii_lowercase();
    lower.contains("<tool_call") || lower.contains("<function_call")
}

/// Tells the user plainly that some tool steps were not run.
pub(crate) fn unexecuted_tools_notice(names: &[String]) -> String {
    let named: Vec<String> = names
        .iter()
        .filter(|name| !name.is_empty())
        .map(|name| format!("`{name}`"))
        .collect();
    if named.is_empty() {
        "_⚠️ Model mencoba memanggil tool dengan format yang tidak bisa dibaca Xiao, jadi langkah itu tidak dijalankan. Balas \"lanjutkan\" untuk mencoba lagi._".to_string()
    } else {
        format!(
            "_⚠️ Xiao sudah memakai batas {MAX_TOOL_ROUNDS} putaran tool untuk jawaban ini, jadi langkah berikut belum dijalankan: {}. Balas \"lanjutkan\" untuk meneruskan._",
            named.join(", ")
        )
    }
}

/// What the model has already done in this generation.
pub(crate) struct RoundState {
    /// Tool rounds still available after the one just finished.
    pub rounds_left: usize,
    pub guest_mode: bool,
    pub media_staged: bool,
    pub quizzes_sent: usize,
}

/// Instruction appended after the tool results of a round.
pub(crate) fn follow_up_prompt(state: &RoundState) -> String {
    let quiz_done = format!(
        "Jika semua kuis yang diminta sudah terkirim dan tidak ada hal lain yang perlu disampaikan, balas hanya dengan kata {QUIZ_DONE_REPLY}."
    );
    if state.rounds_left == 0 {
        let mut prompt = String::from(
            "Tool tidak tersedia lagi untuk jawaban ini. Tulis jawaban akhir sekarang berdasarkan hasil di atas, dan jangan menuliskan atau menjanjikan pemanggilan tool. Jika ada bagian permintaan yang belum sempat dikerjakan dengan tool, sebutkan terus terang bagian mana dan sarankan pengguna membalas \"lanjutkan\".",
        );
        if state.quizzes_sent > 0 {
            prompt.push(' ');
            prompt.push_str(&quiz_done);
        }
        return prompt;
    }

    let mut prompt = format!(
        "Lanjutkan permintaan pengguna berdasarkan hasil tool di atas (sisa putaran tool: {}).",
        state.rounds_left
    );
    if state.guest_mode {
        prompt.push_str(" Jika masih perlu informasi, panggil web_search atau fetch_url; jika sudah cukup, tulis jawaban akhir yang lengkap beserta tautan sumber yang relevan.");
        return prompt;
    }
    if state.quizzes_sent > 0 {
        prompt.push_str(&format!(
            " {} kuis sudah terkirim ke chat. Jika pengguna meminta lebih banyak kuis, panggil create_quiz untuk semua kuis yang belum terkirim sekarang, sekaligus dalam satu giliran; jangan menulis soal kuis sebagai teks dan jangan mengirim ulang kuis yang sudah terkirim. {quiz_done}",
            state.quizzes_sent
        ));
    }
    if state.media_staged {
        prompt.push_str(" Media yang sudah disiapkan tidak perlu dipanggil ulang.");
    }
    prompt.push_str(" Jika permintaan masih membutuhkan tool (create_quiz untuk kuis, send_photo/send_collage/send_slideshow untuk gambar, create_document untuk berkas), panggil tool itu sekarang; jangan menuliskannya sebagai teks biasa. Untuk gambar, pakai hanya URL dari daftar URL Foto/Gambar Raster Terverifikasi dan jangan mengarang URL. Jika semuanya sudah cukup, tulis jawaban akhir yang lengkap beserta tautan sumber yang relevan.");
    prompt
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_keeps_small_results_and_cuts_once_spent() {
        let mut budget = ToolResultBudget::new(1_000);
        let small = "a".repeat(2_000);
        assert_eq!(budget.bound(&small), small, "500 tokens fit");

        let large = "b".repeat(8_000);
        let bounded = budget.bound(&large);
        assert!(bounded.contains("jatah context"), "{bounded}");
        assert!(bounded.chars().count() < 8_000);

        let after = budget.bound(&"c".repeat(4_000));
        assert!(
            after.chars().filter(|ch| *ch == 'c').count() >= MIN_BUDGETED_RESULT_CHARS,
            "the head of a result always survives"
        );
    }

    #[test]
    fn budget_never_exceeds_the_generation_ceiling() {
        let budget = ToolResultBudget::new(usize::MAX);
        assert_eq!(budget.remaining_tokens, MAX_TOOL_OUTPUT_TOKENS);
    }

    #[test]
    fn research_arguments_accept_json_or_bare_values() {
        assert_eq!(
            research_argument("web_search", r#"{"query":"Borobudur foto"}"#),
            "Borobudur foto"
        );
        assert_eq!(
            research_argument("fetch_url", r#"{"url":"https://example.com"}"#),
            "https://example.com"
        );
        assert_eq!(research_argument("web_search", "Borobudur"), "Borobudur");
    }

    #[tokio::test]
    async fn research_calls_beyond_the_budget_are_not_run() {
        let calls = vec![
            PendingToolCall {
                id: "1".into(),
                name: "web_search".into(),
                arguments: r#"{"query":"x"}"#.into(),
            },
            PendingToolCall {
                id: "2".into(),
                name: "create_quiz".into(),
                arguments: "{}".into(),
            },
        ];
        let mut used = MAX_RESEARCH_CALLS;
        let (_cancel, mut receiver) = watch::channel(false);
        let (results, cancelled) = run_research_calls(&calls, &mut used, &mut receiver, None).await;
        assert!(!cancelled);
        assert!(results[0]
            .as_deref()
            .is_some_and(|result| result.contains("Batas")));
        assert!(results[1].is_none(), "only research calls are handled");
        assert_eq!(used, MAX_RESEARCH_CALLS);
    }

    #[test]
    fn quiz_keys_ignore_case_and_spacing() {
        assert_eq!(
            quiz_question_key(r#"{"question":"  Siapa   pendiri Roma? "}"#),
            quiz_question_key(r#"{"question":"siapa pendiri roma?"}"#)
        );
        assert_eq!(quiz_question_key("bukan json"), None);
        assert_eq!(quiz_question_key(r#"{"question":"  "}"#), None);
    }

    #[test]
    fn quiz_done_reply_accepts_only_the_confirmation() {
        assert!(is_quiz_done_reply("SELESAI"));
        assert!(is_quiz_done_reply(" selesai. "));
        assert!(is_quiz_done_reply(""));
        assert!(!is_quiz_done_reply("Selamat mengerjakan kuisnya!"));
    }

    #[test]
    fn tool_markup_is_detected_even_when_unparseable() {
        assert!(has_tool_call_markup("teks <tool_call>{rusak</tool_call>"));
        assert!(has_tool_call_markup("<FUNCTION_CALL>"));
        assert!(!has_tool_call_markup("jawaban biasa"));
    }

    #[test]
    fn notices_name_the_skipped_tools() {
        let notice = unexecuted_tools_notice(&["create_quiz".to_string()]);
        assert!(notice.contains("`create_quiz`"));
        assert!(notice.contains(&MAX_TOOL_ROUNDS.to_string()));
        assert!(unexecuted_tools_notice(&[String::new()]).contains("format"));
    }

    #[test]
    fn follow_up_prompts_match_the_round_state() {
        let last = follow_up_prompt(&RoundState {
            rounds_left: 0,
            guest_mode: false,
            media_staged: false,
            quizzes_sent: 1,
        });
        assert!(last.contains("Tool tidak tersedia lagi"));
        assert!(last.contains(QUIZ_DONE_REPLY));

        let middle = follow_up_prompt(&RoundState {
            rounds_left: 3,
            guest_mode: false,
            media_staged: true,
            quizzes_sent: 0,
        });
        assert!(middle.contains("sisa putaran tool: 3"));
        assert!(middle.contains("create_quiz"));
        assert!(!middle.contains(QUIZ_DONE_REPLY));

        let guest = follow_up_prompt(&RoundState {
            rounds_left: 2,
            guest_mode: true,
            media_staged: false,
            quizzes_sent: 0,
        });
        assert!(
            !guest.contains("create_quiz"),
            "guests only get research tools"
        );
    }
}
