use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;
use tracing::debug;

use super::{provider_url, AIChatService};
use crate::ai::routing::ModelRole;
use crate::ai::storage::{
    count_scoped_messages_async, get_scoped_summary_async, get_user_memories_async,
    load_scoped_messages_async, save_scoped_summary_async, save_user_memory_async, ProviderConfig,
};
use crate::util::truncate_chars;

impl AIChatService {
    pub(crate) async fn process_background_memory_turn(
        &self,
        user_id: i64,
        chat_id: i64,
        thread_id: i64,
        user_prompt: &str,
        assistant_answer: &str,
    ) {
        let curator_route = match self.resolve_model_route(ModelRole::Curator).await {
            Ok(route) => route,
            Err(e) => {
                debug!("Curator model route disabled or unavailable: {e}");
                return;
            }
        };

        let total_count = count_scoped_messages_async(chat_id, thread_id).await;

        let text_lower = user_prompt.to_ascii_lowercase();
        let personal_hints = [
            "nama saya",
            "namaku",
            "my name",
            "saya suka",
            "prefer",
            "tech stack",
            "proyek",
            "project",
            "bekerja sebagai",
            "tinggal di",
            "bahasa",
            "panggil aku",
            "i am",
            "i work",
            "i live",
            "my favorite",
            "hobi",
        ];
        let has_hint = personal_hints.iter().any(|hint| text_lower.contains(hint));
        let has_significant_data = has_hint || user_prompt.chars().count() > 80;
        let should_extract = has_significant_data || total_count <= 6 || (total_count % 6 == 0);

        if should_extract {
            self.extract_user_facts_background(
                user_id,
                &curator_route.provider,
                &curator_route.model,
                user_prompt,
                assistant_answer,
            )
            .await;
        }

        if total_count > 20 && (total_count % 20 == 0) {
            self.summarize_older_history_background(
                user_id,
                chat_id,
                thread_id,
                &curator_route.provider,
                &curator_route.model,
            )
            .await;
        }
    }

    async fn request_completion_text(
        &self,
        provider: &ProviderConfig,
        payload: &Value,
    ) -> Option<String> {
        let url = provider_url(&provider.endpoint, "chat/completions");
        let mut req = self.client.post(&url).json(payload);
        if !provider.api_key.is_empty()
            && !["none", "-", "no"]
                .iter()
                .any(|k| provider.api_key.eq_ignore_ascii_case(k))
        {
            req = req.bearer_auth(&provider.api_key);
        }

        let resp = req.timeout(Duration::from_secs(30)).send().await.ok()?;
        if !resp.status().is_success() {
            return None;
        }

        let body = resp.json::<Value>().await.ok()?;
        body.get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("content"))
            .and_then(Value::as_str)
            .map(|s| s.trim().to_string())
    }

    async fn extract_user_facts_background(
        &self,
        user_id: i64,
        provider: &ProviderConfig,
        model: &str,
        user_prompt: &str,
        assistant_answer: &str,
    ) {
        let existing_memories = get_user_memories_async(user_id).await;
        let mut existing_context = String::new();
        if !existing_memories.is_empty() {
            existing_context.push_str("Current known user facts:\n");
            for (k, f) in &existing_memories {
                existing_context.push_str(&format!("- {k}: {f}\n"));
            }
            existing_context.push('\n');
        }

        let extract_prompt = format!(
            "{}Recent conversation:\nUser input: \"{}\"\nAssistant reply: \"{}\"\n\n\
            Analyze the interaction and extract or update persistent personal profile facts about the user \
            (e.g., Name/Callsign, Preferred Language, Tech Stack, Ongoing Projects, Key Preferences, Style, Role/Work, Location). \
            If a statement updates or contradicts a previously known fact, output the updated fact with the matching key to supersede it. \
            Respond ONLY with a valid JSON array of objects with \"key\" and \"fact\" properties. \
            Example: [{{\"key\": \"Name\", \"fact\": \"Alex\"}}, {{\"key\": \"Tech Stack\", \"fact\": \"Rust, Linux, Termux\"}}]. \
            If there are no personal facts about the user or nothing new/updated, return an empty array []. \
            Do not output any markdown formatting, thoughts, or explanations, only the raw JSON array.",
            existing_context,
            truncate_chars(user_prompt, 800),
            truncate_chars(assistant_answer, 400),
        );

        let payload = json!({
            "model": model,
            "messages": [
                {
                    "role": "system",
                    "content": "You are a concise background fact extraction engine. Always output only valid JSON without code fences or extra text."
                },
                {
                    "role": "user",
                    "content": extract_prompt
                }
            ],
            "temperature": 0.1,
            "max_tokens": 400,
            "stream": false
        });

        let content = match self.request_completion_text(provider, &payload).await {
            Some(c) => c,
            None => return,
        };

        let clean_json = if let Some(stripped) = content.strip_prefix("```json") {
            stripped.trim_end_matches("```").trim()
        } else if let Some(stripped) = content.strip_prefix("```") {
            stripped.trim_end_matches("```").trim()
        } else {
            content.as_str()
        };

        let json_str =
            if let (Some(start), Some(end)) = (clean_json.find('['), clean_json.rfind(']')) {
                if start < end {
                    &clean_json[start..=end]
                } else {
                    clean_json
                }
            } else {
                clean_json
            };

        #[derive(Deserialize)]
        struct ExtractedFact {
            key: String,
            fact: String,
        }

        if let Ok(facts) = serde_json::from_str::<Vec<ExtractedFact>>(json_str) {
            for f in facts {
                let k = f.key.trim().to_string();
                let v = f.fact.trim().to_string();
                if !k.is_empty() && !v.is_empty() && k.len() <= 64 && v.len() <= 500 {
                    save_user_memory_async(user_id, k, v).await;
                }
            }
        }
    }

    async fn summarize_older_history_background(
        &self,
        user_id: i64,
        chat_id: i64,
        thread_id: i64,
        provider: &ProviderConfig,
        model: &str,
    ) {
        let messages = load_scoped_messages_async(chat_id, thread_id, 30).await;
        if messages.len() < 10 {
            return;
        }
        let mut turns_text = String::new();
        for m in &messages[..messages.len().saturating_sub(10)] {
            let preview = match &m.content {
                Value::String(s) => s.as_str(),
                val => val.as_str().unwrap_or(""),
            };
            turns_text.push_str(&format!("{}: {}\n", m.role, truncate_chars(preview, 200)));
        }
        let existing_summary = get_scoped_summary_async(chat_id, thread_id).await;
        let mut context_text = String::new();
        if let Some(ref prev) = existing_summary {
            context_text.push_str(&format!(
                "Previous summary of earlier discussion:\n{prev}\n\n"
            ));
        }
        context_text.push_str(&format!("Recent conversation turns:\n{turns_text}\n\n"));

        let summary_prompt = format!(
            "{}Update and condense the ongoing context, key discussions, and decisions in 2-3 concise sentences. Output only the plain summary.",
            truncate_chars(&context_text, 3500)
        );
        let payload = json!({
            "model": model,
            "messages": [
                { "role": "system", "content": "You are a concise conversational summarizer." },
                { "role": "user", "content": summary_prompt }
            ],
            "temperature": 0.2,
            "max_tokens": 250,
            "stream": false
        });

        if let Some(summary) = self.request_completion_text(provider, &payload).await {
            if !summary.is_empty() {
                save_scoped_summary_async(chat_id, thread_id, summary.clone()).await;

                // Also extract any lingering user profile facts from older turns into Tier 1
                self.extract_user_facts_background(user_id, provider, model, &turns_text, &summary)
                    .await;
            }
        }
    }
}
