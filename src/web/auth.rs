//! Owner sign-in for the WebUI.
//!
//! Two ways in: a 6-digit code sent to the owner's private Telegram chat
//! (valid 5 minutes), or a backup password stored as an argon2 hash under
//! `XIAO_WEB_PASSWORD` (a `_PASSWORD` key, so it lives in the vault). A
//! successful sign-in creates a browser session: a random token in an
//! HttpOnly, SameSite=Strict cookie, of which only a SHA-256 hash is stored.
//! Five failures within 15 minutes lock the client address for 15 minutes.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{header, HeaderMap, HeaderValue};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use rand::{Rng, RngCore};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tracing::{info, warn};

use super::error::{ApiError, ApiResult};
use super::settings;
use super::WebState;
use crate::ai::storage::web as store;

pub(crate) const COOKIE_NAME: &str = "xiao_session";
const CODE_TTL: Duration = Duration::from_secs(5 * 60);
const CODE_RESEND_AFTER: Duration = Duration::from_secs(30);
const MAX_CODE_TRIES: u8 = 5;
const MAX_FAILURES: usize = 5;
const FAILURE_WINDOW: Duration = Duration::from_secs(15 * 60);
const LOCK_TIME: Duration = Duration::from_secs(15 * 60);
/// A session's `last_seen` is written at most this often.
const TOUCH_INTERVAL_SECS: i64 = 60;
pub(crate) const MIN_PASSWORD_CHARS: usize = 8;
const MAX_PASSWORD_CHARS: usize = 256;

pub(crate) fn sha256(data: &[u8]) -> [u8; 32] {
    let digest = Sha256::digest(data);
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from(DIGITS[usize::from(byte >> 4)]));
        out.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    out
}

pub(crate) fn random_hex(len_bytes: usize) -> String {
    let mut bytes = vec![0u8; len_bytes];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex(&bytes)
}

fn ct_eq(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
}

pub(crate) fn now_unix() -> i64 {
    chrono::Utc::now().timestamp()
}

pub(crate) fn unix_to_rfc3339(secs: i64) -> String {
    chrono::DateTime::from_timestamp(secs, 0)
        .map(|time| time.with_timezone(&chrono::Local).to_rfc3339())
        .unwrap_or_default()
}

/* ------------------------------ passwords ----------------------------- */

/// Argon2id hash in PHC string form.
pub(crate) fn hash_password(password: &str) -> Result<String, String> {
    let mut salt = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut salt);
    let salt = SaltString::encode_b64(&salt).map_err(|error| error.to_string())?;
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|error| error.to_string())
}

/// Checks a password against the stored argon2 hash. A value set directly in
/// the environment may also be a plain password, compared in constant time.
pub(crate) fn verify_password(stored: &str, password: &str) -> bool {
    if stored.starts_with("$argon2") {
        PasswordHash::new(stored).is_ok_and(|parsed| {
            Argon2::default()
                .verify_password(password.as_bytes(), &parsed)
                .is_ok()
        })
    } else {
        ct_eq(stored.as_bytes(), password.as_bytes())
    }
}

pub(crate) fn validate_new_password(password: &str) -> Result<(), ApiError> {
    let count = password.chars().count();
    if !(MIN_PASSWORD_CHARS..=MAX_PASSWORD_CHARS).contains(&count) {
        return Err(ApiError::invalid(
            format!("The password needs {MIN_PASSWORD_CHARS} to {MAX_PASSWORD_CHARS} characters."),
            format!("Kata sandi perlu {MIN_PASSWORD_CHARS} sampai {MAX_PASSWORD_CHARS} karakter."),
        ));
    }
    Ok(())
}

fn stored_password() -> Option<String> {
    settings::env_value("XIAO_WEB_PASSWORD").or_else(|| settings::saved_value("XIAO_WEB_PASSWORD"))
}

pub(crate) fn password_is_set() -> bool {
    stored_password().is_some()
}

/// Sign-in with a Telegram code is possible: not turned off, and the bot
/// token and owner are configured.
pub(crate) fn telegram_login_available() -> bool {
    telegram_configured() && settings::effective_bool("XIAO_WEB_TELEGRAM_LOGIN")
}

pub(crate) fn telegram_configured() -> bool {
    crate::get_configured_token().is_some() && crate::get_configured_owner_id().is_some()
}

fn session_days() -> i64 {
    match settings::effective("XIAO_WEB_SESSION_DAYS").trim() {
        "1" => 1,
        "30" => 30,
        _ => 7,
    }
}

/* ------------------------------ lockouts ------------------------------ */

#[derive(Default)]
struct FailureLog {
    recent: Vec<Instant>,
    locked_until: Option<Instant>,
}

struct PendingCode {
    digest: [u8; 32],
    sent_at: Instant,
    expires_at: Instant,
    tries: u8,
}

/// In-memory sign-in state: failure counters per address and the one code
/// that may currently be used.
#[derive(Default)]
pub(crate) struct AuthRuntime {
    failures: Mutex<HashMap<IpAddr, FailureLog>>,
    code: Mutex<Option<PendingCode>>,
}

impl AuthRuntime {
    fn check_lock(&self, ip: IpAddr) -> Result<(), ApiError> {
        let Ok(mut failures) = self.failures.lock() else {
            return Ok(());
        };
        let now = Instant::now();
        if let Some(log) = failures.get_mut(&ip) {
            match log.locked_until {
                Some(until) if until > now => {
                    return Err(ApiError::locked(until.duration_since(now).as_secs().max(1)));
                }
                Some(_) => *log = FailureLog::default(),
                None => {}
            }
        }
        Ok(())
    }

    fn record_failure(&self, ip: IpAddr) {
        let Ok(mut failures) = self.failures.lock() else {
            return;
        };
        let now = Instant::now();
        // Forget idle addresses so the map cannot grow without bound.
        failures.retain(|_, log| {
            log.locked_until.is_some_and(|until| until > now)
                || log
                    .recent
                    .iter()
                    .any(|at| now.duration_since(*at) < FAILURE_WINDOW)
        });
        let log = failures.entry(ip).or_default();
        log.recent
            .retain(|at| now.duration_since(*at) < FAILURE_WINDOW);
        log.recent.push(now);
        if log.recent.len() >= MAX_FAILURES {
            log.locked_until = Some(now + LOCK_TIME);
            log.recent.clear();
            warn!("WebUI sign-in locked for {ip} after {MAX_FAILURES} failures");
        }
    }

    fn clear_failures(&self, ip: IpAddr) {
        if let Ok(mut failures) = self.failures.lock() {
            failures.remove(&ip);
        }
    }

    /// Stores a new code unless one was sent moments ago; returns the wait.
    fn issue_code(&self, code: &str) -> Result<(), u64> {
        let Ok(mut slot) = self.code.lock() else {
            return Err(1);
        };
        let now = Instant::now();
        if let Some(pending) = slot.as_ref() {
            let since = now.duration_since(pending.sent_at);
            if since < CODE_RESEND_AFTER {
                return Err((CODE_RESEND_AFTER - since).as_secs().max(1));
            }
        }
        *slot = Some(PendingCode {
            digest: sha256(code.as_bytes()),
            sent_at: now,
            expires_at: now + CODE_TTL,
            tries: 0,
        });
        Ok(())
    }

    fn forget_code(&self) {
        if let Ok(mut slot) = self.code.lock() {
            *slot = None;
        }
    }

    /// Checks a code. A wrong code counts against it; after five the code is gone.
    fn take_code(&self, code: &str) -> bool {
        let Ok(mut slot) = self.code.lock() else {
            return false;
        };
        let Some(pending) = slot.as_mut() else {
            return false;
        };
        if pending.expires_at <= Instant::now() {
            *slot = None;
            return false;
        }
        if ct_eq(&pending.digest, &sha256(code.as_bytes())) {
            *slot = None;
            return true;
        }
        pending.tries = pending.tries.saturating_add(1);
        if pending.tries >= MAX_CODE_TRIES {
            *slot = None;
        }
        false
    }
}

/* ------------------------------ sessions ------------------------------ */

/// The signed-in browser session, added to protected requests.
#[derive(Debug, Clone)]
pub(crate) struct CurrentSession {
    pub id: String,
}

fn token_from_headers(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .find_map(|pair| {
            pair.trim()
                .strip_prefix(COOKIE_NAME)
                .and_then(|rest| rest.strip_prefix('='))
                .map(str::to_string)
        })
        .filter(|token| token.len() == 64 && token.chars().all(|ch| ch.is_ascii_hexdigit()))
}

async fn session_from_headers(headers: &HeaderMap) -> Option<store::WebSessionRow> {
    let token = token_from_headers(headers)?;
    store::find_web_session_async(hex(&sha256(token.as_bytes())), now_unix()).await
}

fn cookie_header(value: &str) -> Option<HeaderValue> {
    HeaderValue::from_str(value).ok()
}

fn set_cookie(response: &mut Response, token: &str, max_age: i64) {
    if let Some(value) = cookie_header(&format!(
        "{COOKIE_NAME}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age={max_age}"
    )) {
        response.headers_mut().append(header::SET_COOKIE, value);
    }
}

fn clear_cookie(response: &mut Response) {
    if let Some(value) = cookie_header(&format!(
        "{COOKIE_NAME}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0"
    )) {
        response.headers_mut().append(header::SET_COOKIE, value);
    }
}

/// "Chrome on Android" and an OS key from a User-Agent header.
pub(crate) fn describe_device(user_agent: &str) -> (String, &'static str) {
    let ua = user_agent.to_ascii_lowercase();
    let os = if ua.contains("android") {
        "android"
    } else if ua.contains("iphone") || ua.contains("ipad") || ua.contains("ios") {
        "ios"
    } else if ua.contains("windows") {
        "windows"
    } else if ua.contains("mac os") || ua.contains("macintosh") {
        "mac"
    } else if ua.contains("linux") || ua.contains("x11") {
        "linux"
    } else {
        "other"
    };
    let browser = if ua.contains("edg/") {
        "Edge"
    } else if ua.contains("opr/") || ua.contains("opera") {
        "Opera"
    } else if ua.contains("samsungbrowser") {
        "Samsung Internet"
    } else if ua.contains("firefox/") || ua.contains("fxios") {
        "Firefox"
    } else if ua.contains("chrome/") || ua.contains("crios") {
        "Chrome"
    } else if ua.contains("safari/") {
        "Safari"
    } else if ua.contains("curl") {
        "curl"
    } else {
        "Browser"
    };
    let os_name = match os {
        "android" => "Android",
        "ios" => "iOS",
        "windows" => "Windows",
        "mac" => "macOS",
        "linux" => "Linux",
        _ => "",
    };
    let device = if os_name.is_empty() {
        browser.to_string()
    } else {
        format!("{browser} on {os_name}")
    };
    (device, os)
}

fn user_agent(headers: &HeaderMap) -> String {
    headers
        .get(header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .map(|value| crate::util::truncate_chars(value, 300))
        .unwrap_or_default()
}

/// Creates a session and answers with its cookie.
async fn start_session(
    ip: IpAddr,
    headers: &HeaderMap,
    method: &str,
) -> Result<Response, ApiError> {
    let token = random_hex(32);
    let id = random_hex(8);
    let now = now_unix();
    let max_age = session_days() * 24 * 60 * 60;
    let agent = user_agent(headers);
    let created = store::create_web_session_async(store::NewWebSession {
        id,
        token_hash: hex(&sha256(token.as_bytes())),
        now,
        expires_at: now + max_age,
        ip: ip.to_string(),
        user_agent: agent.clone(),
    })
    .await;
    if !created {
        return Err(ApiError::internal("web session could not be stored"));
    }
    let (device, _) = describe_device(&agent);
    info!("WebUI sign-in with {method} from {ip} ({device})");
    let mut response = Json(json!({"ok": true})).into_response();
    set_cookie(&mut response, &token, max_age);
    Ok(response)
}

/// Middleware for every protected route: requires a valid session cookie.
pub(crate) async fn require_session(mut request: Request, next: Next) -> Response {
    let Some(session) = session_from_headers(request.headers()).await else {
        return ApiError::unauthorized().into_response();
    };
    let now = now_unix();
    if now - session.last_seen >= TOUCH_INTERVAL_SECS {
        store::touch_web_session_async(session.id.clone(), now).await;
    }
    request
        .extensions_mut()
        .insert(CurrentSession { id: session.id });
    next.run(request).await
}

/* ------------------------------ handlers ------------------------------ */

/// GET /api/auth/state
pub(crate) async fn auth_state(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
) -> Json<Value> {
    let session = session_from_headers(&headers).await;
    let authenticated = session.is_some();
    let owner = crate::get_configured_owner_id().filter(|_| authenticated);
    Json(json!({
        "authenticated": authenticated,
        "telegram_login": telegram_login_available(),
        "password_login": password_is_set(),
        "bot_username": state.bot_username(),
        "version": env!("CARGO_PKG_VERSION"),
        "bind": state.bind.to_string(),
        "owner_id": owner.map(|id| id.to_string()),
        "session_expires": session.map(|session| unix_to_rfc3339(session.expires_at)),
    }))
}

/// POST /api/auth/code/send
pub(crate) async fn code_send(
    State(state): State<Arc<WebState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
) -> ApiResult<Value> {
    let ip = peer.ip().to_canonical();
    state.auth.check_lock(ip)?;
    if !telegram_login_available() {
        return Err(ApiError::not_configured(
            "Sign-in codes need the Telegram bot token and owner, and code sign-in turned on.",
            "Kode masuk butuh token bot dan owner Telegram, serta masuk dengan kode dalam keadaan aktif.",
        ));
    }
    let (Some(token), Some(owner)) = (
        crate::get_configured_token(),
        crate::get_configured_owner_id(),
    ) else {
        return Err(ApiError::not_configured(
            "Telegram is not configured.",
            "Telegram belum dikonfigurasi.",
        ));
    };
    let code = format!("{:06}", rand::thread_rng().gen_range(0..1_000_000u32));
    if let Err(wait) = state.auth.issue_code(&code) {
        return Err(ApiError::busy(
            format!("A code was just sent. Wait {wait} s before asking again."),
            format!("Kode baru saja dikirim. Tunggu {wait} detik sebelum meminta lagi."),
        ));
    }
    let text = format!(
        "🔐 Kode masuk Xiao Console: {code}\nBerlaku 5 menit. Jangan bagikan kode ini. Abaikan pesan ini bila Anda tidak sedang masuk (permintaan dari {ip}).\n\nXiao Console sign-in code: {code} (valid for 5 minutes)."
    );
    let bot = crate::bot::client::TelegramBotClient::new(token);
    match bot.send_message(owner, &text, None, None, None, None).await {
        Ok(_) => {
            info!("WebUI sign-in code sent to the owner (requested from {ip})");
            Ok(Json(json!({"ok": true, "expires_in": CODE_TTL.as_secs()})))
        }
        Err(error) => {
            state.auth.forget_code();
            warn!("WebUI sign-in code could not be sent: {error}");
            Err(ApiError::upstream(
                "Telegram did not accept the message. Check the bot token and that the owner has started the bot.",
            ))
        }
    }
}

#[derive(Deserialize)]
pub(crate) struct CodeVerifyRequest {
    code: String,
}

/// POST /api/auth/code/verify
pub(crate) async fn code_verify(
    State(state): State<Arc<WebState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<CodeVerifyRequest>,
) -> Result<Response, ApiError> {
    let ip = peer.ip().to_canonical();
    state.auth.check_lock(ip)?;
    let code: String = body.code.chars().filter(char::is_ascii_digit).collect();
    if code.len() != 6 || !state.auth.take_code(&code) {
        state.auth.record_failure(ip);
        return Err(ApiError::bad_code());
    }
    state.auth.clear_failures(ip);
    start_session(ip, &headers, "a Telegram code").await
}

#[derive(Deserialize)]
pub(crate) struct PasswordLoginRequest {
    password: String,
}

/// POST /api/auth/password
pub(crate) async fn password_login(
    State(state): State<Arc<WebState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<PasswordLoginRequest>,
) -> Result<Response, ApiError> {
    let ip = peer.ip().to_canonical();
    state.auth.check_lock(ip)?;
    let Some(stored) = stored_password() else {
        return Err(ApiError::not_configured(
            "No password is set. Run `xiao web password` on the server.",
            "Belum ada kata sandi. Jalankan `xiao web password` di server.",
        ));
    };
    let password = body.password;
    let matches = tokio::task::spawn_blocking(move || verify_password(&stored, &password))
        .await
        .unwrap_or(false);
    if !matches {
        state.auth.record_failure(ip);
        return Err(ApiError::bad_password());
    }
    state.auth.clear_failures(ip);
    let response = start_session(ip, &headers, "the password").await?;
    // Password sign-ins do not involve Telegram, so the owner hears about them.
    let (device, _) = describe_device(&user_agent(&headers));
    if let Some((bot, owner)) = state.telegram_client() {
        tokio::spawn(async move {
            let text = format!(
                "🔐 Xiao Console: masuk dengan kata sandi dari {ip} ({device}). Bila bukan Anda, buka Keamanan WebUI dan keluarkan semua perangkat."
            );
            if bot
                .send_message(owner, &text, None, None, None, None)
                .await
                .is_err()
            {
                warn!("WebUI sign-in notice could not be sent");
            }
        });
    }
    Ok(response)
}

/// POST /api/auth/logout
pub(crate) async fn logout(Extension(session): Extension<CurrentSession>) -> Response {
    store::delete_web_session_async(session.id).await;
    let mut response = Json(json!({"ok": true})).into_response();
    clear_cookie(&mut response);
    response
}

/// POST /api/auth/logout-all
pub(crate) async fn logout_all() -> Response {
    store::delete_web_sessions_except_async(None).await;
    info!("WebUI: every browser session was signed out");
    let mut response = Json(json!({"ok": true})).into_response();
    clear_cookie(&mut response);
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(raw: &str) -> IpAddr {
        raw.parse().expect("test address")
    }

    #[test]
    fn five_failures_lock_the_address_only() {
        let auth = AuthRuntime::default();
        let attacker = ip("192.168.1.50");
        for _ in 0..4 {
            auth.record_failure(attacker);
            assert!(auth.check_lock(attacker).is_ok());
        }
        auth.record_failure(attacker);
        assert!(auth.check_lock(attacker).is_err(), "fifth failure locks");
        assert!(
            auth.check_lock(ip("192.168.1.51")).is_ok(),
            "others unaffected"
        );
        auth.clear_failures(attacker);
        assert!(auth.check_lock(attacker).is_ok());
    }

    #[test]
    fn codes_are_single_use_and_limited() {
        let auth = AuthRuntime::default();
        auth.issue_code("123456").expect("first code");
        assert!(auth.issue_code("654321").is_err(), "resend waits 30 s");
        assert!(!auth.take_code("000000"));
        assert!(auth.take_code("123456"));
        assert!(!auth.take_code("123456"), "a code works once");

        auth.forget_code();
        auth.issue_code("111111").expect("new code");
        for _ in 0..MAX_CODE_TRIES {
            assert!(!auth.take_code("222222"));
        }
        assert!(
            !auth.take_code("111111"),
            "too many wrong tries burn the code"
        );
    }

    #[test]
    fn tokens_are_hashed_and_hex_encoded() {
        assert_eq!(
            hex(&sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let token = random_hex(32);
        assert_eq!(token.len(), 64);
        assert_ne!(token, random_hex(32));
        assert!(ct_eq(b"same", b"same"));
        assert!(!ct_eq(b"same", b"diff"));
        assert!(!ct_eq(b"short", b"longer"));
    }

    #[test]
    fn cookies_are_parsed_strictly() {
        let token = "a".repeat(64);
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_str(&format!("theme=dark; {COOKIE_NAME}={token}"))
                .expect("cookie header"),
        );
        assert_eq!(token_from_headers(&headers), Some(token));
        let mut bad = HeaderMap::new();
        bad.insert(
            header::COOKIE,
            HeaderValue::from_static("xiao_session=../../etc/passwd"),
        );
        assert_eq!(token_from_headers(&bad), None);
    }

    #[test]
    fn passwords_hash_and_verify() {
        let hash = hash_password("correct horse").expect("hash");
        assert!(hash.starts_with("$argon2"));
        assert!(verify_password(&hash, "correct horse"));
        assert!(!verify_password(&hash, "wrong horse"));
        assert!(verify_password("plain-from-env", "plain-from-env"));
        assert!(validate_new_password("short").is_err());
        assert!(validate_new_password("long enough").is_ok());
    }

    #[test]
    fn devices_are_named_from_the_user_agent() {
        let (device, os) = describe_device(
            "Mozilla/5.0 (Linux; Android 14) AppleWebKit/537.36 Chrome/128.0 Mobile Safari/537.36",
        );
        assert_eq!((device.as_str(), os), ("Chrome on Android", "android"));
        let (device, os) = describe_device(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/128.0 Safari/537.36 Edg/128.0",
        );
        assert_eq!((device.as_str(), os), ("Edge on Windows", "windows"));
    }
}
