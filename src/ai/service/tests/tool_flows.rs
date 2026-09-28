use super::*;

#[test]
fn test_create_quiz_sanitization_and_validation() {
    use crate::ai::tools::CreateQuizArgs;

    let mut args = CreateQuizArgs {
        question: "  ".to_string() + &"Q".repeat(350) + "  ",
        options: (0..15)
            .map(|i| "  ".to_string() + &"Opt".repeat(40) + &format!("_{i}"))
            .collect(),
        correct_option_id: Some(99),
        explanation: Some(
            "  Line 1\nLine 2\nLine 3\nLine 4\nLine 5  ".to_string() + &"E".repeat(250),
        ),
        preamble: Some("  Preamble text  ".to_string()),
        is_anonymous: None,
    };

    args.sanitize();

    // Question truncated to 300
    assert_eq!(args.question.chars().count(), 300);

    // Options truncated to 10
    assert_eq!(args.options.len(), 10);

    // Each option text truncated to 100
    for opt in &args.options {
        assert!(opt.chars().count() <= 100);
    }

    // correct_option_id clamped to valid range 0..10
    assert_eq!(args.correct_option_id, Some(9));

    // Explanation truncated to <= 200 chars and <= 2 line breaks
    let exp = args.explanation.as_ref().expect("explanation present");
    assert!(exp.chars().count() <= 200);
    let line_breaks = exp.chars().filter(|&c| c == '\n').count();
    assert!(line_breaks <= 2);

    // Preamble trimmed
    assert_eq!(args.preamble.as_deref(), Some("Preamble text"));

    // Validation passes after sanitization
    assert!(args.validate().is_ok());

    // Validation fails if options < 2
    let mut invalid = args.clone();
    invalid.options = vec!["Single".to_string()];
    assert!(invalid.validate().is_err());

    // Validation fails if question is empty
    let mut invalid_q = args.clone();
    invalid_q.question = "".to_string();
    assert!(invalid_q.validate().is_err());

    // Explanation with CRLF and > 2 line breaks is normalized without stray \r
    let mut crlf_args = args.clone();
    crlf_args.explanation = Some("Line 1\r\nLine 2\r\nLine 3\r\nLine 4\r\nLine 5".to_string());
    crlf_args.sanitize();
    let crlf_exp = crlf_args.explanation.as_ref().expect("explanation present");
    assert!(!crlf_exp.contains('\r'));
    assert_eq!(crlf_exp.chars().filter(|&c| c == '\n').count(), 2);
    assert!(crlf_exp.contains("Line 3 Line 4 Line 5"));

    // Explanation with only whitespace is sanitized to None
    let mut whitespace_exp_args = args.clone();
    whitespace_exp_args.explanation = Some("    \n\t  ".to_string());
    whitespace_exp_args.sanitize();
    assert!(whitespace_exp_args.explanation.is_none());
}

#[tokio::test]
async fn test_create_quiz_without_preamble_sends_single_bubble_and_emits_sink() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    // 1. Mock Telegram Server
    let tg_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind tg listener succeeds");
    let tg_address = tg_listener.local_addr().expect("tg addr succeeds");

    let tg_server = tokio::spawn(async move {
        let (mut socket, _) = tg_listener.accept().await.expect("accept tg 1 succeeds");
        let mut data = Vec::new();
        let mut buf = [0u8; 1024];
        loop {
            let n = socket.read(&mut buf).await.expect("read chunk");
            if n == 0 {
                break;
            }
            data.extend_from_slice(&buf[..n]);
            if data.windows(4).any(|w| w == b"\r\n\r\n") {
                // Read content-length if any
                let header_str = String::from_utf8_lossy(&data);
                let content_len = header_str
                    .lines()
                    .find(|l| l.to_ascii_lowercase().starts_with("content-length:"))
                    .and_then(|l| l.split(':').nth(1))
                    .and_then(|v| v.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                let body_pos = data
                    .windows(4)
                    .position(|w| w == b"\r\n\r\n")
                    .map(|p| p + 4)
                    .unwrap_or(0);
                if data.len() >= body_pos + content_len {
                    break;
                }
            }
        }
        let req_str = String::from_utf8_lossy(&data).to_string();
        let first_line = req_str.lines().next().unwrap_or("").to_string();
        let body_pos = data
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .map(|p| p + 4)
            .unwrap_or(0);
        let payload: Value = serde_json::from_slice(&data[body_pos..]).unwrap_or(json!({}));

        let resp_body = r#"{"ok":true,"result":{"message_id":7001,"poll":{"id":"poll_7001"}}}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{resp_body}",
            resp_body.len()
        );
        socket
            .write_all(response.as_bytes())
            .await
            .expect("write tg resp");

        vec![(first_line, payload)]
    });

    // 2. Mock AI LLM Server
    let ai_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ai listener succeeds");
    let ai_address = ai_listener.local_addr().expect("ai addr succeeds");

    let ai_server = tokio::spawn(async move {
        let (mut socket, _) = ai_listener.accept().await.expect("accept ai succeeds");
        let mut bytes = Vec::new();
        loop {
            let mut buf = [0u8; 4096];
            let n = socket.read(&mut buf).await.expect("read ai request");
            if n == 0 {
                break;
            }
            bytes.extend_from_slice(&buf[..n]);
            if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&bytes[..end]);
                let length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())?
                    })
                    .unwrap_or(0);
                if bytes.len() >= end + 4 + length {
                    break;
                }
            }
        }

        let sse = "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_quiz_nopre\",\"type\":\"function\",\"function\":{\"name\":\"create_quiz\",\"arguments\":\"{\\\"question\\\":\\\"Berapa 1+1?\\\",\\\"options\\\":[\\\"1\\\",\\\"2\\\"],\\\"correct_option_id\\\":1}\"}}]}}]}\n\ndata: [DONE]\n\n";
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{sse}",
            sse.len()
        );
        socket
            .write_all(response.as_bytes())
            .await
            .expect("write sse");
        let _ = socket.shutdown().await;
    });

    let bot_client = crate::bot::client::TelegramBotClient::with_base_url(
        "test_tok",
        format!("http://{tg_address}"),
    );
    let provider = ProviderConfig {
        id: "main-quiz-test".into(),
        name: "Main Quiz Test".into(),
        endpoint: format!("http://{ai_address}/v1"),
        api_key: String::new(),
        api_key_ref: None,
        models: vec!["quiz-model".into()],
        active_model: "quiz-model".into(),
    };
    let service = isolated_service(provider.clone());
    let snapshot = service.generation_model_snapshot().await;

    struct TestProgressSink {
        actions: std::sync::Mutex<Vec<(String, Option<crate::timeline::ProgressActivity>)>>,
    }
    impl crate::timeline::GenerationProgressSink for TestProgressSink {
        fn on_action(&self, label: &str, activity: Option<crate::timeline::ProgressActivity>) {
            if let Ok(mut l) = self.actions.lock() {
                l.push((label.to_string(), activity));
            }
        }
        fn on_partial_answer(&self, _text: &str) {}
        fn on_failure(&self, _error: &str, _force_sync: bool) {}
        fn on_complete(&self) {}
    }

    let sink = TestProgressSink {
        actions: std::sync::Mutex::new(Vec::new()),
    };

    let (_cancel, mut receiver) = watch::channel(false);
    let gen_input = GenerationInput {
        prompt: "Buatkan kuis 1+1",
        canonical_prompt: None,
        media_to_main: true,
        sink: Some(&sink),
        image_bytes: None,
        document_images: None,
        mime_type: None,
        doc_text: None,
        doc_name: None,
        audio_bytes: None,
        audio_mime: None,
        video_bytes: None,
        video_mime: None,
        video_duration: None,
        bot: Some(bot_client),
        reply_to_message_id: Some(100),
    };

    let (_thinking, answer, _staged_docs, cancelled) = service
        .generate_response_with_snapshot(1234, 0, 1234, gen_input, &snapshot, &mut receiver)
        .await;

    assert!(!cancelled);
    assert_eq!(answer, "[QUIZ_SENT]");

    // Verify progress sink emitted "Quiz"
    let recorded_actions = sink.actions.lock().expect("lock actions").clone();
    assert!(
        recorded_actions.iter().any(
            |(lbl, act)| lbl == "Quiz" && *act == Some(crate::timeline::ProgressActivity::Quiz)
        ),
        "Progress sink must emit Quiz action"
    );

    // Verify exactly 1 request to Telegram (sendPoll)
    let tg_requests = tg_server.await.expect("tg server join");
    assert_eq!(
        tg_requests.len(),
        1,
        "Quiz without preamble must send exactly 1 bubble"
    );
    let (req_line, payload) = &tg_requests[0];
    assert!(req_line.contains("POST /sendPoll"));
    assert_eq!(payload["type"], "quiz");
    assert_eq!(payload["question"], "Berapa 1+1?");
    assert_eq!(payload["correct_option_id"], 1);

    // Verify session history recorded both the user prompt and the assistant quiz summary
    let messages = crate::ai::storage::load_scoped_messages_async(1234, 0, 10).await;
    assert!(
        messages
            .iter()
            .any(|m| m.role == "user" && m.content.to_string().contains("Buatkan kuis 1+1")),
        "User prompt must be saved in session messages"
    );
    assert!(
        messages
            .iter()
            .any(|m| m.role == "assistant" && m.content.to_string().contains("Berapa 1+1?")),
        "Assistant quiz summary must be saved in session messages"
    );

    ai_server.await.expect("ai server join");
}

#[tokio::test]
async fn test_create_quiz_with_preamble_sends_two_connected_messages() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    // 1. Mock Telegram Server (handles sendRichMessage, then sendPoll)
    let tg_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind tg listener succeeds");
    let tg_address = tg_listener.local_addr().expect("tg addr succeeds");

    let tg_server = tokio::spawn(async move {
        let mut requests = Vec::new();
        for req_idx in 0..2 {
            let (mut socket, _) = tg_listener.accept().await.expect("accept tg succeeds");
            let mut data = Vec::new();
            let mut buf = [0u8; 1024];
            loop {
                let n = socket.read(&mut buf).await.expect("read chunk");
                if n == 0 {
                    break;
                }
                data.extend_from_slice(&buf[..n]);
                if data.windows(4).any(|w| w == b"\r\n\r\n") {
                    let header_str = String::from_utf8_lossy(&data);
                    let content_len = header_str
                        .lines()
                        .find(|l| l.to_ascii_lowercase().starts_with("content-length:"))
                        .and_then(|l| l.split(':').nth(1))
                        .and_then(|v| v.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    let body_pos = data
                        .windows(4)
                        .position(|w| w == b"\r\n\r\n")
                        .map(|p| p + 4)
                        .unwrap_or(0);
                    if data.len() >= body_pos + content_len {
                        break;
                    }
                }
            }
            let req_str = String::from_utf8_lossy(&data).to_string();
            let first_line = req_str.lines().next().unwrap_or("").to_string();
            let body_pos = data
                .windows(4)
                .position(|w| w == b"\r\n\r\n")
                .map(|p| p + 4)
                .unwrap_or(0);
            let payload: Value = serde_json::from_slice(&data[body_pos..]).unwrap_or(json!({}));

            let msg_id = if req_idx == 0 { 8001 } else { 8002 };
            let resp_body = format!(r#"{{"ok":true,"result":{{"message_id":{msg_id}}}}}"#);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{resp_body}",
                resp_body.len()
            );
            socket
                .write_all(response.as_bytes())
                .await
                .expect("write tg resp");
            requests.push((first_line, payload));
        }

        requests
    });

    // 2. Mock AI LLM Server
    let ai_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ai listener succeeds");
    let ai_address = ai_listener.local_addr().expect("ai addr succeeds");

    let ai_server = tokio::spawn(async move {
        let (mut socket, _) = ai_listener.accept().await.expect("accept ai succeeds");
        let mut bytes = Vec::new();
        loop {
            let mut buf = [0u8; 4096];
            let n = socket.read(&mut buf).await.expect("read ai request");
            if n == 0 {
                break;
            }
            bytes.extend_from_slice(&buf[..n]);
            if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&bytes[..end]);
                let length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())?
                    })
                    .unwrap_or(0);
                if bytes.len() >= end + 4 + length {
                    break;
                }
            }
        }

        let sse = "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_quiz_pre\",\"type\":\"function\",\"function\":{\"name\":\"create_quiz\",\"arguments\":\"{\\\"preamble\\\":\\\"Perhatikan cuplikan kode ini:\\\\n```rust\\\\nfn main() {}\\\\n```\\\",\\\"question\\\":\\\"Berapakah outputnya?\\\",\\\"options\\\":[\\\"0\\\",\\\"1\\\"],\\\"correct_option_id\\\":0}\"}}]}}]}\n\ndata: [DONE]\n\n";
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{sse}",
            sse.len()
        );
        socket
            .write_all(response.as_bytes())
            .await
            .expect("write sse");
        let _ = socket.shutdown().await;
    });

    let bot_client = crate::bot::client::TelegramBotClient::with_base_url(
        "test_tok",
        format!("http://{tg_address}"),
    );
    let provider = ProviderConfig {
        id: "main-quiz-pre-test".into(),
        name: "Main Quiz Pre Test".into(),
        endpoint: format!("http://{ai_address}/v1"),
        api_key: String::new(),
        api_key_ref: None,
        models: vec!["quiz-model".into()],
        active_model: "quiz-model".into(),
    };
    let service = isolated_service(provider.clone());
    let snapshot = service.generation_model_snapshot().await;

    let (_cancel, mut receiver) = watch::channel(false);
    let gen_input = GenerationInput {
        prompt: "Buatkan kuis dengan studi kasus",
        canonical_prompt: None,
        media_to_main: true,
        sink: None,
        image_bytes: None,
        document_images: None,
        mime_type: None,
        doc_text: None,
        doc_name: None,
        audio_bytes: None,
        audio_mime: None,
        video_bytes: None,
        video_mime: None,
        video_duration: None,
        bot: Some(bot_client),
        reply_to_message_id: Some(100),
    };

    let (_thinking, answer, _staged_docs, cancelled) = service
        .generate_response_with_snapshot(8881, 0, 8881, gen_input, &snapshot, &mut receiver)
        .await;

    assert!(!cancelled);
    assert_eq!(answer, "[QUIZ_SENT]");

    // Verify exactly 2 requests to Telegram connected by reply_parameters
    let tg_requests = tg_server.await.expect("tg server join");
    assert_eq!(
        tg_requests.len(),
        2,
        "Quiz with preamble must send 2 connected messages"
    );

    // Message 1: Preamble (sendRichMessage)
    let (req1_line, payload1) = &tg_requests[0];
    assert!(req1_line.contains("POST /sendRichMessage"));
    assert_eq!(payload1["chat_id"], 8881);

    // Message 2: Native Quiz (sendPoll) linked to message_id 8001
    let (req2_line, payload2) = &tg_requests[1];
    assert!(req2_line.contains("POST /sendPoll"));
    assert_eq!(payload2["type"], "quiz");
    assert_eq!(payload2["question"], "Berapakah outputnya?");
    assert_eq!(
        payload2["reply_parameters"]["message_id"], 8001,
        "Quiz must be linked to preamble message_id"
    );

    // Verify session history recorded both user prompt and assistant preamble+quiz
    let messages = crate::ai::storage::load_scoped_messages_async(8881, 0, 10).await;
    assert!(
        messages.iter().any(|m| m.role == "user"
            && m.content
                .to_string()
                .contains("Buatkan kuis dengan studi kasus")),
        "User prompt must be saved in session messages"
    );
    assert!(
        messages.iter().any(|m| m.role == "assistant"
            && m.content
                .to_string()
                .contains("Perhatikan cuplikan kode ini")
            && m.content.to_string().contains("Berapakah outputnya?")),
        "Assistant quiz summary must contain both preamble and quiz question"
    );

    ai_server.await.expect("ai server join");
}

#[tokio::test]
async fn test_create_quiz_in_forum_topic_preserves_thread_id() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let tg_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind tg listener succeeds");
    let tg_address = tg_listener.local_addr().expect("tg addr succeeds");

    let tg_server = tokio::spawn(async move {
        let mut requests = Vec::new();
        for req_idx in 0..2 {
            let (mut socket, _) = tg_listener.accept().await.expect("accept tg succeeds");
            let mut data = Vec::new();
            let mut buf = [0u8; 1024];
            loop {
                let n = socket.read(&mut buf).await.expect("read chunk");
                if n == 0 {
                    break;
                }
                data.extend_from_slice(&buf[..n]);
                if data.windows(4).any(|w| w == b"\r\n\r\n") {
                    let header_str = String::from_utf8_lossy(&data);
                    let content_len = header_str
                        .lines()
                        .find(|l| l.to_ascii_lowercase().starts_with("content-length:"))
                        .and_then(|l| l.split(':').nth(1))
                        .and_then(|v| v.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    let body_pos = data
                        .windows(4)
                        .position(|w| w == b"\r\n\r\n")
                        .map(|p| p + 4)
                        .unwrap_or(0);
                    if data.len() >= body_pos + content_len {
                        break;
                    }
                }
            }
            let req_str = String::from_utf8_lossy(&data).to_string();
            let first_line = req_str.lines().next().unwrap_or("").to_string();
            let body_pos = data
                .windows(4)
                .position(|w| w == b"\r\n\r\n")
                .map(|p| p + 4)
                .unwrap_or(0);
            let payload: Value = serde_json::from_slice(&data[body_pos..]).unwrap_or(json!({}));

            let msg_id = if req_idx == 0 { 8801 } else { 8802 };
            let resp_body = format!(r#"{{"ok":true,"result":{{"message_id":{msg_id}}}}}"#);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{resp_body}",
                resp_body.len()
            );
            socket
                .write_all(response.as_bytes())
                .await
                .expect("write tg resp");
            requests.push((first_line, payload));
        }

        requests
    });

    let ai_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ai listener succeeds");
    let ai_address = ai_listener.local_addr().expect("ai addr succeeds");

    let ai_server = tokio::spawn(async move {
        let (mut socket, _) = ai_listener.accept().await.expect("accept ai succeeds");
        let mut bytes = Vec::new();
        loop {
            let mut buf = [0u8; 4096];
            let n = socket.read(&mut buf).await.expect("read ai request");
            if n == 0 {
                break;
            }
            bytes.extend_from_slice(&buf[..n]);
            if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&bytes[..end]);
                let length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())?
                    })
                    .unwrap_or(0);
                if bytes.len() >= end + 4 + length {
                    break;
                }
            }
        }

        let sse = "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_quiz_topic\",\"type\":\"function\",\"function\":{\"name\":\"create_quiz\",\"arguments\":\"{\\\"preamble\\\":\\\"Materi Topik:\\\\nInfo topik forum\\\",\\\"question\\\":\\\"Pertanyaan Topik?\\\",\\\"options\\\":[\\\"A\\\",\\\"B\\\"],\\\"correct_option_id\\\":0}\"}}]}}]}\n\ndata: [DONE]\n\n";
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{sse}",
            sse.len()
        );
        socket
            .write_all(response.as_bytes())
            .await
            .expect("write sse");
        let _ = socket.shutdown().await;
    });

    let bot_client = crate::bot::client::TelegramBotClient::with_base_url(
        "test_tok",
        format!("http://{tg_address}"),
    );
    let provider = ProviderConfig {
        id: "main-quiz-topic-test".into(),
        name: "Main Quiz Topic Test".into(),
        endpoint: format!("http://{ai_address}/v1"),
        api_key: String::new(),
        api_key_ref: None,
        models: vec!["quiz-model".into()],
        active_model: "quiz-model".into(),
    };
    let service = isolated_service(provider.clone());
    let snapshot = service.generation_model_snapshot().await;

    let (_cancel, mut receiver) = watch::channel(false);
    let gen_input = GenerationInput {
        prompt: "Buat kuis dalam topik forum",
        canonical_prompt: None,
        media_to_main: true,
        sink: None,
        image_bytes: None,
        document_images: None,
        mime_type: None,
        doc_text: None,
        doc_name: None,
        audio_bytes: None,
        audio_mime: None,
        video_bytes: None,
        video_mime: None,
        video_duration: None,
        bot: Some(bot_client),
        reply_to_message_id: Some(100),
    };

    // thread_id = 9988 represents a Telegram forum topic
    let (_thinking, answer, _staged_docs, cancelled) = service
        .generate_response_with_snapshot(1234, 9988, 1234, gen_input, &snapshot, &mut receiver)
        .await;

    assert!(!cancelled);
    assert_eq!(answer, "[QUIZ_SENT]");

    let tg_requests = tg_server.await.expect("tg server join");
    assert_eq!(
        tg_requests.len(),
        2,
        "Quiz with preamble must send 2 connected messages"
    );

    // Message 1: Preamble must include message_thread_id = 9988
    let (req1_line, payload1) = &tg_requests[0];
    assert!(req1_line.contains("POST /sendRichMessage"));
    assert_eq!(
        payload1["message_thread_id"], 9988,
        "Preamble must preserve message_thread_id"
    );

    // Message 2: sendPoll must include message_thread_id = 9988 AND reply to preamble 8801
    let (req2_line, payload2) = &tg_requests[1];
    assert!(req2_line.contains("POST /sendPoll"));
    assert_eq!(
        payload2["message_thread_id"], 9988,
        "sendPoll must preserve message_thread_id"
    );
    assert_eq!(
        payload2["reply_parameters"]["message_id"], 8801,
        "sendPoll must link to preamble message_id"
    );

    ai_server.await.expect("ai server join");
}

#[tokio::test]
async fn test_create_quiz_deletes_orphan_preamble_if_poll_fails() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let tg_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind tg listener succeeds");
    let tg_address = tg_listener.local_addr().expect("tg addr succeeds");

    let tg_server = tokio::spawn(async move {
        let mut requests = Vec::new();
        // 3 requests expected: sendRichMessage (succeeds), sendPoll (fails 400), deleteMessage (cleans up preamble)
        for req_idx in 0..3 {
            let (mut socket, _) = tg_listener.accept().await.expect("accept tg succeeds");
            let mut data = Vec::new();
            let mut buf = [0u8; 1024];
            loop {
                let n = socket.read(&mut buf).await.expect("read chunk");
                if n == 0 {
                    break;
                }
                data.extend_from_slice(&buf[..n]);
                if data.windows(4).any(|w| w == b"\r\n\r\n") {
                    let header_str = String::from_utf8_lossy(&data);
                    let content_len = header_str
                        .lines()
                        .find(|l| l.to_ascii_lowercase().starts_with("content-length:"))
                        .and_then(|l| l.split(':').nth(1))
                        .and_then(|v| v.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    let body_pos = data
                        .windows(4)
                        .position(|w| w == b"\r\n\r\n")
                        .map(|p| p + 4)
                        .unwrap_or(0);
                    if data.len() >= body_pos + content_len {
                        break;
                    }
                }
            }
            let req_str = String::from_utf8_lossy(&data).to_string();
            let first_line = req_str.lines().next().unwrap_or("").to_string();
            let body_pos = data
                .windows(4)
                .position(|w| w == b"\r\n\r\n")
                .map(|p| p + 4)
                .unwrap_or(0);
            let payload: Value = serde_json::from_slice(&data[body_pos..]).unwrap_or(json!({}));

            let (status, resp_body) = if req_idx == 0 {
                // 1. sendRichMessage succeeds with message_id 9001
                (
                    "200 OK",
                    r#"{"ok":true,"result":{"message_id":9001}}"#.to_string(),
                )
            } else if req_idx == 1 {
                // 2. sendPoll fails with 400 Bad Request
                (
                    "400 Bad Request",
                    r#"{"ok":false,"error_code":400,"description":"Bad Request: poll failed"}"#
                        .to_string(),
                )
            } else {
                // 3. deleteMessage succeeds
                ("200 OK", r#"{"ok":true,"result":true}"#.to_string())
            };

            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{resp_body}",
                resp_body.len()
            );
            socket
                .write_all(response.as_bytes())
                .await
                .expect("write tg resp");
            requests.push((first_line, payload));
        }

        requests
    });

    let ai_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ai listener succeeds");
    let ai_address = ai_listener.local_addr().expect("ai addr succeeds");

    let ai_server = tokio::spawn(async move {
        // AI turn 1: generates create_quiz tool call
        let (mut socket, _) = ai_listener.accept().await.expect("accept ai succeeds");
        let mut bytes = Vec::new();
        loop {
            let mut buf = [0u8; 4096];
            let n = socket.read(&mut buf).await.expect("read ai request");
            if n == 0 {
                break;
            }
            bytes.extend_from_slice(&buf[..n]);
            if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&bytes[..end]);
                let length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())?
                    })
                    .unwrap_or(0);
                if bytes.len() >= end + 4 + length {
                    break;
                }
            }
        }

        let sse = "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_quiz_fail\",\"type\":\"function\",\"function\":{\"name\":\"create_quiz\",\"arguments\":\"{\\\"preamble\\\":\\\"Preamble yang akan dibersihkan\\\",\\\"question\\\":\\\"Pertanyaan?\\\",\\\"options\\\":[\\\"A\\\",\\\"B\\\"],\\\"correct_option_id\\\":0}\"}}]}}]}\n\ndata: [DONE]\n\n";
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{sse}",
            sse.len()
        );
        socket
            .write_all(response.as_bytes())
            .await
            .expect("write sse");
        let _ = socket.shutdown().await;

        // AI turn 2: model responds to tool error
        let (mut socket2, _) = ai_listener.accept().await.expect("accept ai 2 succeeds");
        let mut bytes2 = Vec::new();
        loop {
            let mut buf = [0u8; 4096];
            let n = socket2.read(&mut buf).await.expect("read ai request 2");
            if n == 0 {
                break;
            }
            bytes2.extend_from_slice(&buf[..n]);
            if let Some(end) = bytes2.windows(4).position(|w| w == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&bytes2[..end]);
                let length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())?
                    })
                    .unwrap_or(0);
                if bytes2.len() >= end + 4 + length {
                    break;
                }
            }
        }

        let sse2 = "data: {\"choices\":[{\"delta\":{\"content\":\"Maaf kuis gagal dikirim.\"}}]}\n\ndata: [DONE]\n\n";
        let response2 = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{sse2}",
            sse2.len()
        );
        socket2
            .write_all(response2.as_bytes())
            .await
            .expect("write sse 2");
        let _ = socket2.shutdown().await;
    });

    let bot_client = crate::bot::client::TelegramBotClient::with_base_url(
        "test_tok",
        format!("http://{tg_address}"),
    );
    let provider = ProviderConfig {
        id: "main-quiz-orphan-test".into(),
        name: "Main Quiz Orphan Test".into(),
        endpoint: format!("http://{ai_address}/v1"),
        api_key: String::new(),
        api_key_ref: None,
        models: vec!["quiz-model".into()],
        active_model: "quiz-model".into(),
    };
    let service = isolated_service(provider.clone());
    let snapshot = service.generation_model_snapshot().await;

    let (_cancel, mut receiver) = watch::channel(false);
    let gen_input = GenerationInput {
        prompt: "Buatkan kuis uji pembersihan preamble",
        canonical_prompt: None,
        media_to_main: true,
        sink: None,
        image_bytes: None,
        document_images: None,
        mime_type: None,
        doc_text: None,
        doc_name: None,
        audio_bytes: None,
        audio_mime: None,
        video_bytes: None,
        video_mime: None,
        video_duration: None,
        bot: Some(bot_client),
        reply_to_message_id: None,
    };

    let (_thinking, answer, _staged_docs, cancelled) = service
        .generate_response_with_snapshot(1234, 0, 1234, gen_input, &snapshot, &mut receiver)
        .await;

    assert!(!cancelled);
    assert!(answer.contains("Maaf kuis gagal dikirim"));

    let tg_requests = tg_server.await.expect("tg server join");
    assert_eq!(
        tg_requests.len(),
        3,
        "Must send preamble, attempt poll, and delete orphan preamble on poll failure"
    );

    // Request 1: sendRichMessage
    assert!(tg_requests[0].0.contains("POST /sendRichMessage"));
    // Request 2: sendPoll
    assert!(tg_requests[1].0.contains("POST /sendPoll"));
    // Request 3: deleteMessage with message_id = 9001
    assert!(tg_requests[2].0.contains("POST /deleteMessage"));
    assert_eq!(
        tg_requests[2].1["message_id"], 9001,
        "Must delete orphan preamble message_id 9001"
    );

    ai_server.await.expect("ai server join");
}

#[tokio::test]
async fn test_create_quiz_aborts_if_preamble_fails_and_does_not_send_poll() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let tg_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind tg listener succeeds");
    let tg_address = tg_listener.local_addr().expect("tg addr succeeds");

    let tg_server = tokio::spawn(async move {
        let mut requests = Vec::new();
        // We only expect 1 request: sendRichMessage (which fails). sendPoll must NOT be called.
        let (mut socket, _) = tg_listener.accept().await.expect("accept tg succeeds");
        let mut data = Vec::new();
        let mut buf = [0u8; 1024];
        loop {
            let n = socket.read(&mut buf).await.expect("read chunk");
            if n == 0 {
                break;
            }
            data.extend_from_slice(&buf[..n]);
            if data.windows(4).any(|w| w == b"\r\n\r\n") {
                let header_str = String::from_utf8_lossy(&data);
                let content_len = header_str
                    .lines()
                    .find(|l| l.to_ascii_lowercase().starts_with("content-length:"))
                    .and_then(|l| l.split(':').nth(1))
                    .and_then(|v| v.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                let body_pos = data
                    .windows(4)
                    .position(|w| w == b"\r\n\r\n")
                    .map(|p| p + 4)
                    .unwrap_or(0);
                if data.len() >= body_pos + content_len {
                    break;
                }
            }
        }
        let req_str = String::from_utf8_lossy(&data).to_string();
        let first_line = req_str.lines().next().unwrap_or("").to_string();
        let body_pos = data
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .map(|p| p + 4)
            .unwrap_or(0);
        let payload: Value = serde_json::from_slice(&data[body_pos..]).unwrap_or(json!({}));

        // sendRichMessage fails with 400 Bad Request
        let resp_body =
            r#"{"ok":false,"error_code":400,"description":"Bad Request: preamble failed"}"#;
        let response = format!(
            "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{resp_body}",
            resp_body.len()
        );
        socket
            .write_all(response.as_bytes())
            .await
            .expect("write tg resp");
        requests.push((first_line, payload));

        requests
    });

    let ai_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ai listener succeeds");
    let ai_address = ai_listener.local_addr().expect("ai addr succeeds");

    let ai_server = tokio::spawn(async move {
        // AI turn 1: generates create_quiz tool call with preamble
        let (mut socket, _) = ai_listener.accept().await.expect("accept ai succeeds");
        let mut bytes = Vec::new();
        loop {
            let mut buf = [0u8; 4096];
            let n = socket.read(&mut buf).await.expect("read ai request");
            if n == 0 {
                break;
            }
            bytes.extend_from_slice(&buf[..n]);
            if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&bytes[..end]);
                let length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())?
                    })
                    .unwrap_or(0);
                if bytes.len() >= end + 4 + length {
                    break;
                }
            }
        }

        let sse = "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_preamble_fail\",\"type\":\"function\",\"function\":{\"name\":\"create_quiz\",\"arguments\":\"{\\\"preamble\\\":\\\"Preamble gagal kirim\\\",\\\"question\\\":\\\"Pertanyaan?\\\",\\\"options\\\":[\\\"A\\\",\\\"B\\\"],\\\"correct_option_id\\\":0}\"}}]}}]}\n\ndata: [DONE]\n\n";
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{sse}",
            sse.len()
        );
        socket
            .write_all(response.as_bytes())
            .await
            .expect("write sse");
        let _ = socket.shutdown().await;

        // AI turn 2: model receives preamble error and apologizes
        let (mut socket2, _) = ai_listener.accept().await.expect("accept ai 2 succeeds");
        let mut bytes2 = Vec::new();
        loop {
            let mut buf = [0u8; 4096];
            let n = socket2.read(&mut buf).await.expect("read ai request 2");
            if n == 0 {
                break;
            }
            bytes2.extend_from_slice(&buf[..n]);
            if let Some(end) = bytes2.windows(4).position(|w| w == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&bytes2[..end]);
                let length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())?
                    })
                    .unwrap_or(0);
                if bytes2.len() >= end + 4 + length {
                    break;
                }
            }
        }

        let sse2 = "data: {\"choices\":[{\"delta\":{\"content\":\"Maaf, pesan pengantar kuis gagal terkirim.\"}}]}\n\ndata: [DONE]\n\n";
        let response2 = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{sse2}",
            sse2.len()
        );
        socket2
            .write_all(response2.as_bytes())
            .await
            .expect("write sse 2");
        let _ = socket2.shutdown().await;
    });

    let bot_client = crate::bot::client::TelegramBotClient::with_base_url(
        "test_tok",
        format!("http://{tg_address}"),
    );
    let provider = ProviderConfig {
        id: "main-quiz-pre-fail-test".into(),
        name: "Main Quiz Pre Fail Test".into(),
        endpoint: format!("http://{ai_address}/v1"),
        api_key: String::new(),
        api_key_ref: None,
        models: vec!["quiz-model".into()],
        active_model: "quiz-model".into(),
    };
    let service = isolated_service(provider.clone());
    let snapshot = service.generation_model_snapshot().await;

    let (_cancel, mut receiver) = watch::channel(false);
    let gen_input = GenerationInput {
        prompt: "Buatkan kuis uji gagal preamble",
        canonical_prompt: None,
        media_to_main: true,
        sink: None,
        image_bytes: None,
        document_images: None,
        mime_type: None,
        doc_text: None,
        doc_name: None,
        audio_bytes: None,
        audio_mime: None,
        video_bytes: None,
        video_mime: None,
        video_duration: None,
        bot: Some(bot_client),
        reply_to_message_id: None,
    };

    let (_thinking, answer, _staged_docs, cancelled) = service
        .generate_response_with_snapshot(1234, 0, 1234, gen_input, &snapshot, &mut receiver)
        .await;

    assert!(!cancelled);
    assert!(answer.contains("pesan pengantar kuis gagal terkirim"));

    let tg_requests = tg_server.await.expect("tg server join");
    assert_eq!(
        tg_requests.len(),
        1,
        "Must only attempt sendRichMessage and abort before sendPoll"
    );
    assert!(tg_requests[0].0.contains("POST /sendRichMessage"));

    ai_server.await.expect("ai server join");
}
#[tokio::test]
async fn test_create_document_staging_and_auto_append() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let ai_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ai listener succeeds");
    let ai_address = ai_listener.local_addr().expect("ai addr succeeds");

    let ai_server = tokio::spawn(async move {
        let mut conn_count = 0;
        while let Ok((mut socket, _)) = ai_listener.accept().await {
            conn_count += 1;
            let mut bytes = Vec::new();
            loop {
                let mut buf = [0u8; 4096];
                let n = socket.read(&mut buf).await.expect("read ai request");
                if n == 0 {
                    break;
                }
                bytes.extend_from_slice(&buf[..n]);
                if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&bytes[..end]);
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())?
                        })
                        .unwrap_or(0);
                    if bytes.len() >= end + 4 + length {
                        break;
                    }
                }
            }

            let sse = if conn_count == 1 {
                "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_doc123\",\"type\":\"function\",\"function\":{\"name\":\"create_document\",\"arguments\":\"{\\\"filename\\\":\\\"laporan.pdf\\\",\\\"content\\\":\\\"Ini adalah konten pdf palsu\\\"}\"}}]}}]}\n\ndata: [DONE]\n\n"
            } else {
                "data: {\"choices\":[{\"delta\":{\"content\":\"Ini penjelasannya.\"}}]}\n\ndata: [DONE]\n\n"
            };

            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                sse.len(), sse
            );
            socket
                .write_all(response.as_bytes())
                .await
                .expect("write sse");
            let _ = socket.shutdown().await;
            if conn_count == 2 {
                break;
            }
        }
    });

    let provider = ProviderConfig {
        id: "main-doc-test".into(),
        name: "Main Doc Test".into(),
        endpoint: format!("http://{ai_address}/v1"),
        api_key: String::new(),
        api_key_ref: None,
        models: vec!["doc-model".into()],
        active_model: "doc-model".into(),
    };
    let service = isolated_service(provider.clone());
    let snapshot = service.generation_model_snapshot().await;

    let (_cancel, mut receiver) = tokio::sync::watch::channel(false);

    let gen_input = GenerationInput {
        prompt: "Buatkan dokumen laporan",
        canonical_prompt: None,
        media_to_main: true,
        sink: None,
        image_bytes: None,
        document_images: None,
        mime_type: None,
        doc_text: None,
        doc_name: None,
        audio_bytes: None,
        audio_mime: None,
        video_bytes: None,
        video_mime: None,
        video_duration: None,
        bot: None,
        reply_to_message_id: None,
    };

    let (_thinking, answer, staged_docs, cancelled) = service
        .generate_response_with_snapshot(5555, 0, 5555, gen_input, &snapshot, &mut receiver)
        .await;

    assert!(!cancelled);
    ai_server.await.expect("ai server join");

    // 1. Verify staged doc
    assert_eq!(staged_docs.len(), 1);
    let doc = &staged_docs[0];
    assert_eq!(doc.attach_key, "doc_0");
    assert_eq!(doc.filename, "laporan.pdf");
    assert_eq!(doc.mime_type, "application/pdf");
    assert!(!doc.bytes.is_empty());
    assert!(doc.bytes.starts_with(b"%PDF-1.4"));

    // 2. Verify auto-append logic
    assert!(answer.contains("[document: laporan.pdf](attach://doc_0)"));
    assert!(answer.contains("Ini penjelasannya."));
}

#[tokio::test]
async fn test_create_archive_staging_and_auto_append() {
    use std::io::Read;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let ai_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock ai server");
    let ai_address = ai_listener.local_addr().expect("local addr");

    let ai_server = tokio::spawn(async move {
        let mut conn_count = 0;
        loop {
            let (mut socket, _) = ai_listener.accept().await.expect("accept connection");
            conn_count += 1;
            let mut bytes = Vec::new();
            let mut buf = [0u8; 1024];
            loop {
                let n = socket.read(&mut buf).await.expect("read chunk");
                if n == 0 {
                    break;
                }
                bytes.extend_from_slice(&buf[..n]);
                if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&bytes[..end]);
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())?
                        })
                        .unwrap_or(0);
                    if bytes.len() >= end + 4 + length {
                        break;
                    }
                }
            }

            let sse = if conn_count == 1 {
                "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_arch123\",\"type\":\"function\",\"function\":{\"name\":\"create_archive\",\"arguments\":\"{\\\"filename\\\":\\\"bundle.zip\\\",\\\"files\\\":[{\\\"filename\\\":\\\"cpa.sh\\\",\\\"content\\\":\\\"#!/bin/bash\\\\necho cpa\\\"},{\\\"filename\\\":\\\"setup.sh\\\",\\\"content\\\":\\\"echo setup\\\"},{\\\"filename\\\":\\\"README.md\\\",\\\"content\\\":\\\"# Dokumentasi\\\"}]}\"}}]}}]}\n\ndata: [DONE]\n\n"
            } else {
                "data: {\"choices\":[{\"delta\":{\"content\":\"Berikut adalah berkas bundle project Anda.\"}}]}\n\ndata: [DONE]\n\n"
            };

            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                sse.len(), sse
            );
            socket
                .write_all(response.as_bytes())
                .await
                .expect("write sse");
            let _ = socket.shutdown().await;
            if conn_count == 2 {
                break;
            }
        }
    });

    let provider = ProviderConfig {
        id: "main-archive-test".into(),
        name: "Main Archive Test".into(),
        endpoint: format!("http://{ai_address}/v1"),
        api_key: String::new(),
        api_key_ref: None,
        models: vec!["archive-model".into()],
        active_model: "archive-model".into(),
    };
    let service = isolated_service(provider.clone());
    let snapshot = service.generation_model_snapshot().await;

    let (_cancel, mut receiver) = tokio::sync::watch::channel(false);

    let gen_input = GenerationInput {
        prompt: "Tolong buatkan bundle installer dalam file zip",
        canonical_prompt: None,
        media_to_main: true,
        sink: None,
        image_bytes: None,
        document_images: None,
        mime_type: None,
        doc_text: None,
        doc_name: None,
        audio_bytes: None,
        audio_mime: None,
        video_bytes: None,
        video_mime: None,
        video_duration: None,
        bot: None,
        reply_to_message_id: None,
    };

    let (_thinking, answer, staged_docs, cancelled) = service
        .generate_response_with_snapshot(6666, 0, 6666, gen_input, &snapshot, &mut receiver)
        .await;

    assert!(!cancelled);
    ai_server.await.expect("ai server join");

    // 1. Verify staged doc
    assert_eq!(staged_docs.len(), 1);
    let doc = &staged_docs[0];
    assert_eq!(doc.attach_key, "doc_0");
    assert_eq!(doc.filename, "bundle.zip");
    assert_eq!(doc.mime_type, "application/zip");
    assert!(!doc.bytes.is_empty());
    assert_eq!(&doc.bytes[0..4], &[0x50, 0x4B, 0x03, 0x04]);

    // 2. Verify inner files in zip
    let cursor = std::io::Cursor::new(doc.bytes.clone());
    let mut archive = zip::ZipArchive::new(cursor).expect("valid zip archive");
    assert_eq!(archive.len(), 3);
    {
        let mut file1 = archive.by_name("cpa.sh").expect("cpa.sh entry");
        let mut c1 = String::new();
        file1.read_to_string(&mut c1).expect("read cpa.sh");
        assert_eq!(c1, "#!/bin/bash\necho cpa");
    }
    {
        let mut file2 = archive.by_name("setup.sh").expect("setup.sh entry");
        let mut c2 = String::new();
        file2.read_to_string(&mut c2).expect("read setup.sh");
        assert_eq!(c2, "echo setup");
    }
    {
        let mut file3 = archive.by_name("README.md").expect("README.md entry");
        let mut c3 = String::new();
        file3.read_to_string(&mut c3).expect("read README.md");
        assert_eq!(c3, "# Dokumentasi");
    }

    // 3. Verify auto-append logic
    assert!(answer.contains("[document: bundle.zip](attach://doc_0)"));
    assert!(answer.contains("Berikut adalah berkas bundle project Anda."));
}
