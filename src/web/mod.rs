//! Xiao Console: the WebUI served by `xiao start`.
//!
//! An axum server runs inside the daemon, next to the Telegram poller and
//! the WhatsApp gateway. It serves the embedded Vue app and a JSON API under
//! `/api`. Only the owner can sign in (Telegram code or backup password),
//! every change needs `X-Xiao-Request: 1` and a same-origin `Origin`, and
//! clients outside `XIAO_WEB_ALLOWED_NETWORKS` get no answer at all.

pub(crate) mod api;
pub(crate) mod assets;
pub(crate) mod auth;
pub(crate) mod chat;
pub(crate) mod cli;
pub(crate) mod error;
pub(crate) mod logs;
pub(crate) mod net;
pub(crate) mod settings;
pub(crate) mod wa;

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Instant;

use axum::extract::{ConnectInfo, Request, State};
use axum::http::{header, HeaderValue, Method};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use tokio::sync::{mpsc, watch};
use tracing::{error, info, warn};

use crate::ai::AIChatService;
use crate::bot::client::TelegramBotClient;
use crate::bot::models::Update;

/// Exit status after a restart requested from the WebUI.
pub(crate) const RESTART_EXIT_CODE: i32 = 75;

static RESTART_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Whether the daemon stopped because the WebUI asked for a restart.
pub(crate) fn restart_requested() -> bool {
    RESTART_REQUESTED.load(Ordering::SeqCst)
}

const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob: https:; media-src 'self' blob: https:; connect-src 'self'; font-src 'self' data:; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'";

/// The running Telegram side of the daemon, when it started.
#[derive(Clone)]
pub(crate) struct TelegramLink {
    pub bot: TelegramBotClient,
    pub owner_id: i64,
    pub username: Option<String>,
    pub updates: mpsc::Sender<Update>,
}

pub(crate) struct WebState {
    pub ai: Arc<AIChatService>,
    /// Address the server actually listens on.
    pub bind: SocketAddr,
    networks: Vec<net::Cidr>,
    pub started: Instant,
    pub started_at: chrono::DateTime<chrono::Local>,
    pub auth: auth::AuthRuntime,
    pub restart: settings::RestartTracker,
    pub wa: Arc<wa::WaController>,
    pub chat: chat::ChatRuntime,
    telegram: RwLock<Option<TelegramLink>>,
    shutdown: watch::Sender<bool>,
}

impl WebState {
    pub(crate) fn telegram(&self) -> Option<TelegramLink> {
        self.telegram.read().ok().and_then(|link| link.clone())
    }

    /// The bot client and owner to message, from the running daemon or,
    /// without it, from the saved configuration.
    pub(crate) fn telegram_client(&self) -> Option<(TelegramBotClient, i64)> {
        if let Some(link) = self.telegram() {
            return Some((link.bot, link.owner_id));
        }
        let token = crate::get_configured_token()?;
        let owner = crate::get_configured_owner_id()?;
        Some((TelegramBotClient::new(token), owner))
    }

    pub(crate) fn bot_username(&self) -> Option<String> {
        self.telegram().and_then(|link| link.username)
    }

    /// Called by the daemon once Telegram polling runs.
    pub(crate) fn attach_telegram(
        &self,
        bot: TelegramBotClient,
        owner_id: i64,
        username: Option<String>,
        updates: mpsc::Sender<Update>,
    ) {
        if let Ok(mut link) = self.telegram.write() {
            *link = Some(TelegramLink {
                bot,
                owner_id,
                username,
                updates,
            });
        }
    }

    /// Drops the update sender so the Telegram workers can finish.
    pub(crate) fn detach_telegram(&self) {
        if let Ok(mut link) = self.telegram.write() {
            *link = None;
        }
    }

    pub(crate) fn uptime_secs(&self) -> u64 {
        self.started.elapsed().as_secs()
    }

    /// Restart-only settings saved since startup.
    pub(crate) fn restart_keys(&self) -> Vec<&'static str> {
        self.restart.pending()
    }

    /// Stops the daemon gracefully; `main` then exits with
    /// [`RESTART_EXIT_CODE`] so the service manager starts it again.
    pub(crate) fn request_restart(self: &Arc<Self>) {
        RESTART_REQUESTED.store(true, Ordering::SeqCst);
        let state = Arc::clone(self);
        tokio::spawn(async move {
            // Let the HTTP response reach the browser first.
            tokio::time::sleep(std::time::Duration::from_millis(400)).await;
            warn!("Restart requested from the WebUI");
            state.ai.begin_shutdown().await;
            let _ = state.shutdown.send(true);
        });
    }
}

/// Answers only clients from allowed networks (loopback always).
async fn network_guard(
    State(state): State<Arc<WebState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    request: Request,
    next: Next,
) -> Response {
    if !net::is_allowed(peer.ip(), &state.networks) {
        warn!("WebUI refused a connection from {}", peer.ip());
        return (axum::http::StatusCode::FORBIDDEN, "Forbidden").into_response();
    }
    next.run(request).await
}

/// Changes need `X-Xiao-Request: 1`, which a cross-site page cannot send
/// without a CORS preflight this server never grants, and an `Origin` (when
/// the browser sends one) that matches the requested host.
async fn csrf_guard(request: Request, next: Next) -> Response {
    let method = request.method().clone();
    if matches!(method, Method::GET | Method::HEAD | Method::OPTIONS) {
        return next.run(request).await;
    }
    let headers = request.headers();
    let marked = headers
        .get("x-xiao-request")
        .and_then(|value| value.to_str().ok())
        == Some("1");
    let host = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let same_origin = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        .is_none_or(|origin| {
            origin
                .split_once("://")
                .is_some_and(|(_, rest)| rest.eq_ignore_ascii_case(host))
        });
    if !marked || !same_origin {
        return error::ApiError::forbidden(
            "This request came from another site or lacks the console header.",
            "Permintaan ini berasal dari situs lain atau tanpa header konsol.",
        )
        .into_response();
    }
    next.run(request).await
}

/// Security headers on every response; API answers are never cached.
async fn security_headers(request: Request, next: Next) -> Response {
    let is_api = request.uri().path().starts_with("/api/");
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CONTENT_SECURITY_POLICY),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    if is_api && !headers.contains_key(header::CACHE_CONTROL) {
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    response
}

/// Starts the console unless `XIAO_WEB_BIND=off` or the address is busy.
pub(crate) async fn start(
    ai: Arc<AIChatService>,
    wa: Arc<wa::WaController>,
    shutdown: watch::Sender<bool>,
    shutdown_rx: watch::Receiver<bool>,
) -> Option<Arc<WebState>> {
    let bind_raw = settings::effective("XIAO_WEB_BIND");
    let bind = match net::parse_bind(&bind_raw) {
        Ok(Some(bind)) => bind,
        Ok(None) => {
            info!("WebUI is off (XIAO_WEB_BIND=off)");
            return None;
        }
        Err(error) => {
            warn!(
                "XIAO_WEB_BIND is invalid ({error}); using {}",
                net::DEFAULT_BIND
            );
            net::parse_bind(net::DEFAULT_BIND).ok().flatten()?
        }
    };
    let networks = net::parse_networks(&settings::effective("XIAO_WEB_ALLOWED_NETWORKS"))
        .unwrap_or_else(|error| {
            warn!("XIAO_WEB_ALLOWED_NETWORKS is invalid ({error}); using the private ranges");
            net::parse_networks(net::DEFAULT_ALLOWED_NETWORKS).unwrap_or_default()
        });
    let listener = match tokio::net::TcpListener::bind(bind).await {
        Ok(listener) => listener,
        Err(err) => {
            error!("WebUI could not listen on {bind}: {err}");
            return None;
        }
    };
    let bind = listener.local_addr().unwrap_or(bind);

    let state = Arc::new(WebState {
        ai,
        bind,
        networks,
        started: Instant::now(),
        started_at: chrono::Local::now(),
        auth: auth::AuthRuntime::default(),
        restart: settings::RestartTracker::capture(),
        wa,
        chat: chat::ChatRuntime::default(),
        telegram: RwLock::new(None),
        shutdown,
    });

    let app = api::router(Arc::clone(&state))
        .fallback(assets::serve)
        .layer(axum::middleware::from_fn(csrf_guard))
        .layer(axum::middleware::from_fn_with_state(
            Arc::clone(&state),
            network_guard,
        ))
        .layer(axum::middleware::from_fn(security_headers));

    let mut stop = shutdown_rx;
    tokio::spawn(async move {
        let server = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(async move {
            while !*stop.borrow() {
                if stop.changed().await.is_err() {
                    break;
                }
            }
        });
        if let Err(err) = server.await {
            error!("WebUI server stopped: {err}");
        }
    });

    if bind.ip().is_loopback() {
        info!(
            "Xiao Console: http://{bind} (this machine only; use an SSH tunnel from other devices)"
        );
    } else {
        info!("Xiao Console listening on {bind}");
    }
    if !assets::WEBUI_BUILT {
        warn!(
            "This binary was built without the WebUI files; the console shows a placeholder page"
        );
    }
    Some(state)
}
