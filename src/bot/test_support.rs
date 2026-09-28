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
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind fake telegram listener");
        let address = listener.local_addr().expect("fake telegram local addr");
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
                    let mut guard = recorded.lock().expect("fake telegram request log");
                    guard.push(request.clone());
                    guard.len() - 1
                };
                let (status, body) = responder(&request, index);
                let body = body.to_string();
                let reason = if status == 200 { "OK" } else { "Error" };
                let response = format!(
                    "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
            }
        });
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
