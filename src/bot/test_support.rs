//! In-process fake Telegram Bot API server for behavioural tests.
//!
//! Tests point [`TelegramBotClient::with_base_url`] at this server and then
//! assert on the requests the *real* client produced, instead of asserting on
//! JSON literals the test wrote itself.

use serde_json::Value;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::bot::client::TelegramBotClient;

/// One request received by the fake server.
#[derive(Debug, Clone)]
pub struct RecordedRequest {
    /// Bot API method, e.g. `sendMessage`.
    pub method: String,
    /// JSON body, or `Value::Null` for multipart/other bodies.
    pub json: Value,
    /// Raw body text (useful for multipart assertions).
    pub raw_body: String,
}

/// Decides the reply for a request: `(http_status, json_body)`.
pub type Responder = Arc<dyn Fn(&RecordedRequest, usize) -> (u16, Value) + Send + Sync>;

pub struct FakeTelegram {
    pub client: TelegramBotClient,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
    server: tokio::task::JoinHandle<()>,
}

impl FakeTelegram {
    /// Starts a server that answers every request with `responder`, which also
    /// receives the zero-based index of the request.
    pub async fn start(responder: Responder) -> Self {
        let (address, requests, server) = serve(Arc::new(move |request, index| {
            let (status, body) = responder(request, index);
            (status, "application/json", body.to_string())
        }))
        .await;
        let client =
            TelegramBotClient::with_base_url("123:TEST", format!("http://{address}/bot123:TEST"));
        Self {
            client,
            requests,
            server,
        }
    }

    /// Server that answers every call with `{"ok":true,"result":{"message_id":1}}`.
    pub async fn always_ok() -> Self {
        Self::start(Arc::new(|_, _| ok_message(1))).await
    }

    pub fn requests(&self) -> Vec<RecordedRequest> {
        self.requests
            .lock()
            .expect("fake telegram request log")
            .clone()
    }

    pub fn methods(&self) -> Vec<String> {
        self.requests()
            .into_iter()
            .map(|request| request.method)
            .collect()
    }
}

impl Drop for FakeTelegram {
    fn drop(&mut self) {
        self.server.abort();
    }
}

/// Raw reply for a request: `(http_status, content_type, body)`.
type RawResponder =
    Arc<dyn Fn(&RecordedRequest, usize) -> (u16, &'static str, String) + Send + Sync>;

/// Accepts connections on a loopback port, records every request and answers
/// it with `responder`. One request per connection (`Connection: close`).
async fn serve(
    responder: RawResponder,
) -> (
    std::net::SocketAddr,
    Arc<Mutex<Vec<RecordedRequest>>>,
    tokio::task::JoinHandle<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake server listener");
    let address = listener.local_addr().expect("fake server local addr");
    let requests: Arc<Mutex<Vec<RecordedRequest>>> = Arc::default();
    let recorded = Arc::clone(&requests);
    let server = tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let Some((method, raw_body)) = read_request(&mut socket).await else {
                continue;
            };
            let json = serde_json::from_str(&raw_body).unwrap_or(Value::Null);
            let request = RecordedRequest {
                method,
                json,
                raw_body,
            };
            let index = {
                let mut guard = recorded.lock().expect("fake server request log");
                guard.push(request.clone());
                guard.len() - 1
            };
            let (status, content_type, body) = responder(&request, index);
            let reason = if status == 200 { "OK" } else { "Error" };
            let response = format!(
                "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = socket.write_all(response.as_bytes()).await;
        }
    });
    (address, requests, server)
}

/// In-process OpenAI-compatible provider that streams one fixed answer for
/// every chat completion request.
pub struct FakeProvider {
    pub endpoint: String,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
    server: tokio::task::JoinHandle<()>,
}

impl FakeProvider {
    pub async fn streaming(answer: &str) -> Self {
        let chunk = serde_json::json!({"choices": [{"delta": {"content": answer}}]});
        let body = format!("data: {chunk}\n\ndata: [DONE]\n\n");
        let (address, requests, server) = serve(Arc::new(move |_, _| {
            (200, "text/event-stream", body.clone())
        }))
        .await;
        Self {
            endpoint: format!("http://{address}/v1"),
            requests,
            server,
        }
    }

    /// A provider that answers the n-th chat request with `replies[n]`, each a
    /// raw SSE body (see [`sse_text`] and [`sse_tool_call`]). The last reply
    /// is repeated for any further request.
    pub async fn scripted(replies: Vec<String>) -> Self {
        let (address, requests, server) = serve(Arc::new(move |_, index| {
            let body = replies
                .get(index)
                .or_else(|| replies.last())
                .cloned()
                .unwrap_or_default();
            (200, "text/event-stream", body)
        }))
        .await;
        Self {
            endpoint: format!("http://{address}/v1"),
            requests,
            server,
        }
    }

    /// A provider configuration pointing at this server.
    pub fn config(&self) -> crate::ai::storage::ProviderConfig {
        crate::ai::storage::ProviderConfig {
            id: "fake-provider".into(),
            name: "Fake Provider".into(),
            endpoint: self.endpoint.clone(),
            api_key: String::new(),
            api_key_ref: None,
            models: vec!["fake-model".into()],
            active_model: "fake-model".into(),
        }
    }

    /// Chat completion requests received so far (other probes filtered out).
    pub fn chat_requests(&self) -> Vec<Value> {
        self.requests
            .lock()
            .expect("fake provider request log")
            .iter()
            .filter(|request| request.json.get("messages").is_some())
            .map(|request| request.json.clone())
            .collect()
    }
}

impl Drop for FakeProvider {
    fn drop(&mut self) {
        self.server.abort();
    }
}

/// SSE body of a streamed text answer.
pub fn sse_text(text: &str) -> String {
    let chunk = serde_json::json!({"choices": [{"delta": {"content": text}}]});
    format!("data: {chunk}\n\ndata: [DONE]\n\n")
}

/// SSE body of a single streamed tool call.
pub fn sse_tool_call(id: &str, name: &str, arguments: &Value) -> String {
    let chunk = serde_json::json!({"choices": [{"delta": {"tool_calls": [{
        "index": 0,
        "id": id,
        "type": "function",
        "function": {"name": name, "arguments": arguments.to_string()}
    }]}}]});
    format!("data: {chunk}\n\ndata: [DONE]\n\n")
}

pub fn ok_message(message_id: i64) -> (u16, Value) {
    (
        200,
        serde_json::json!({"ok": true, "result": {"message_id": message_id}}),
    )
}

pub fn api_error(code: u16, description: &str) -> (u16, Value) {
    (
        200,
        serde_json::json!({"ok": false, "error_code": code, "description": description}),
    )
}

async fn read_request(socket: &mut tokio::net::TcpStream) -> Option<(String, String)> {
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 8192];
    loop {
        let count = socket.read(&mut buffer).await.ok()?;
        if count == 0 {
            return None;
        }
        bytes.extend_from_slice(&buffer[..count]);
        let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") else {
            continue;
        };
        let headers = String::from_utf8_lossy(&bytes[..end]).to_string();
        let length = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())
                    .flatten()
            })
            .unwrap_or(0);
        if bytes.len() < end + 4 + length {
            continue;
        }
        let request_line = headers.lines().next().unwrap_or_default();
        let path = request_line.split_whitespace().nth(1).unwrap_or_default();
        let method = path.rsplit('/').next().unwrap_or_default().to_string();
        let body = String::from_utf8_lossy(&bytes[end + 4..end + 4 + length]).to_string();
        return Some((method, body));
    }
}
