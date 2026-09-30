//! Starts, stops and pairs the WhatsApp gateway from inside the daemon.
//!
//! Each run of `supervise_whatsapp` gets its own stop signal (which also
//! follows the daemon's shutdown), so changing a WhatsApp setting restarts
//! only the gateway. Pairing runs inside the daemon too: the QR code or the
//! pairing code is reported through [`WaHooks`] and shown in the WebUI.

use std::fmt::Write as _;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;
use tokio::sync::{watch, Mutex};
use tokio::task::JoinHandle;
use tracing::{info, warn};

use crate::ai::AIChatService;
use crate::bot::client::TelegramBotClient;
use crate::gateway::whatsapp::client::WhatsAppExit;
use crate::gateway::whatsapp::{
    LinkPhase, LinkState, WaHooks, WhatsAppConfig, WhatsAppGateway, WhatsAppStatus,
};

/// Time a stopping gateway gets before its task is aborted.
const STOP_GRACE: Duration = Duration::from_secs(15);
/// An unlinked gateway stops pairing after this long, so QR codes do not
/// rotate forever.
const PAIR_LIMIT: Duration = Duration::from_secs(3 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PairMode {
    Qr,
    Code,
}

#[derive(Debug, Clone)]
struct PairIntent {
    mode: PairMode,
    phone: Option<String>,
}

struct WaRun {
    stop: watch::Sender<bool>,
    task: JoinHandle<WhatsAppExit>,
}

/// Dark modules of a QR code as SVG path data (one unit per module).
#[derive(Debug, Clone, Serialize)]
pub(crate) struct QrMatrix {
    pub size: usize,
    pub path: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct PairingView {
    pub mode: PairMode,
    pub qr: Option<QrMatrix>,
    pub code: Option<String>,
    pub phone: Option<String>,
    pub expires_in: Option<u64>,
    pub stops_in: Option<u64>,
    pub done: bool,
    pub error: Option<String>,
}

pub(crate) fn phase_name(phase: LinkPhase) -> &'static str {
    match phase {
        LinkPhase::Off => "off",
        LinkPhase::Starting => "starting",
        LinkPhase::Pairing => "pairing",
        LinkPhase::Online => "online",
        LinkPhase::Retrying => "retrying",
        LinkPhase::LoggedOut => "logged_out",
    }
}

/// Renders a QR payload as path data; horizontal runs are merged.
pub(crate) fn qr_matrix(payload: &str) -> Option<QrMatrix> {
    let code = qrcode::QrCode::new(payload.as_bytes()).ok()?;
    let size = code.width();
    let colors = code.to_colors();
    let mut path = String::new();
    for y in 0..size {
        let mut x = 0;
        while x < size {
            if colors.get(y * size + x) == Some(&qrcode::Color::Dark) {
                let start = x;
                while x < size && colors.get(y * size + x) == Some(&qrcode::Color::Dark) {
                    x += 1;
                }
                let run = x - start;
                let _ = write!(path, "M{start} {y}h{run}v1h-{run}z");
            } else {
                x += 1;
            }
        }
    }
    Some(QrMatrix { size, path })
}

/// "K7QM2XPA" → "K7QM-2XPA", like WhatsApp shows it.
fn format_pair_code(code: &str) -> String {
    let clean: String = code.chars().filter(|ch| *ch != '-').collect();
    if clean.chars().count() == 8 {
        let (head, tail) = clean.split_at(4);
        format!("{head}-{tail}")
    } else {
        code.to_string()
    }
}

pub(crate) struct WaController {
    ai: Arc<AIChatService>,
    daemon_shutdown: watch::Receiver<bool>,
    alert: std::sync::RwLock<Option<(TelegramBotClient, i64)>>,
    hooks: WaHooks,
    run: Mutex<Option<WaRun>>,
    intent: Arc<std::sync::Mutex<Option<PairIntent>>>,
    /// When the current unlinked run stops pairing by itself.
    pair_deadline: Arc<std::sync::Mutex<Option<Instant>>>,
}

async fn wait_true(signal: &mut watch::Receiver<bool>) {
    while !*signal.borrow() {
        if signal.changed().await.is_err() {
            return;
        }
    }
}

impl WaController {
    pub(crate) fn new(ai: Arc<AIChatService>, daemon_shutdown: watch::Receiver<bool>) -> Arc<Self> {
        Arc::new(Self {
            ai,
            daemon_shutdown,
            alert: std::sync::RwLock::new(None),
            hooks: WaHooks::default(),
            run: Mutex::new(None),
            intent: Arc::default(),
            pair_deadline: Arc::default(),
        })
    }

    /// Telegram chat that hears about logouts and repeated failures.
    pub(crate) fn set_alert(&self, alert: Option<(TelegramBotClient, i64)>) {
        if let Ok(mut slot) = self.alert.write() {
            *slot = alert;
        }
    }

    pub(crate) fn linked() -> bool {
        WhatsAppGateway::check_status(&crate::get_whatsapp_db_path()) == WhatsAppStatus::Linked
    }

    /// `WHATSAPP_ENABLED` decides when it is set; otherwise a linked
    /// session turns the gateway on.
    pub(crate) fn should_run() -> bool {
        match crate::configured_setting("WHATSAPP_ENABLED") {
            Some(_) => crate::is_whatsapp_enabled(),
            None => Self::linked(),
        }
    }

    pub(crate) fn state(&self) -> LinkState {
        self.hooks.snapshot()
    }

    /// Starts a fresh run, stopping the current one first.
    pub(crate) async fn start(&self, phone_login: Option<String>) {
        let mut run = self.run.lock().await;
        if let Some(current) = run.take() {
            finish(current).await;
        }
        let config = WhatsAppConfig {
            db_path: crate::get_whatsapp_db_path(),
            owner_number: crate::get_configured_whatsapp_owner(),
            phone_login,
            dedicated_groups: crate::get_whatsapp_dedicated_groups(),
            hooks: Some(self.hooks.clone()),
        };
        self.hooks.update(|state| {
            *state = LinkState {
                phase: LinkPhase::Starting,
                ..LinkState::default()
            }
        });
        let (stop, stop_rx) = watch::channel(false);
        let forward = stop.clone();
        let mut daemon_shutdown = self.daemon_shutdown.clone();
        tokio::spawn(async move {
            tokio::select! {
                () = wait_true(&mut daemon_shutdown) => {
                    let _ = forward.send(true);
                }
                () = forward.closed() => {}
            }
        });
        self.watch_pairing(&stop);
        let alert = self.alert.read().ok().and_then(|slot| slot.clone());
        let task = tokio::spawn(crate::bot::daemon::supervise_whatsapp(
            config,
            Arc::clone(&self.ai),
            stop_rx,
            alert,
        ));
        info!("WhatsApp gateway started");
        *run = Some(WaRun { stop, task });
    }

    /// Limits pairing of an unlinked session to [`PAIR_LIMIT`]: the run is
    /// stopped when no phone linked it in time.
    fn watch_pairing(&self, stop: &watch::Sender<bool>) {
        let deadline = (!Self::linked()).then(|| Instant::now() + PAIR_LIMIT);
        if let Ok(mut slot) = self.pair_deadline.lock() {
            *slot = deadline;
        }
        if deadline.is_none() {
            return;
        }
        let stop = stop.clone();
        let hooks = self.hooks.clone();
        let intent = Arc::clone(&self.intent);
        let deadline_slot = Arc::clone(&self.pair_deadline);
        tokio::spawn(async move {
            tokio::select! {
                () = tokio::time::sleep(PAIR_LIMIT) => {}
                () = stop.closed() => return,
            }
            if hooks.snapshot().phase == LinkPhase::Online || Self::linked() {
                return;
            }
            let _ = stop.send(true);
            hooks.update(|state| {
                state.error = Some(
                    "Penautan dihentikan setelah 3 menit tanpa pindaian. / Pairing stopped after 3 minutes without a scan."
                        .to_string(),
                );
            });
            if let Ok(mut slot) = intent.lock() {
                *slot = None;
            }
            if let Ok(mut slot) = deadline_slot.lock() {
                *slot = None;
            }
            info!("WhatsApp pairing stopped after the time limit");
        });
    }

    /// Stops the current run, if any.
    pub(crate) async fn stop(&self) {
        let current = self.run.lock().await.take();
        if let Some(current) = current {
            finish(current).await;
            info!("WhatsApp gateway stopped");
        }
        self.hooks.update(|state| *state = LinkState::default());
        if let Ok(mut slot) = self.pair_deadline.lock() {
            *slot = None;
        }
    }

    /// Applies changed WhatsApp settings by restarting (or stopping) the
    /// gateway. Returns whether it was restarted or started.
    pub(crate) async fn apply_settings(&self) -> bool {
        if Self::should_run() {
            self.start(None).await;
            true
        } else {
            self.stop().await;
            false
        }
    }

    /// Waits until the current run ends on its own (standalone daemon mode).
    pub(crate) async fn join(&self) -> Option<WhatsAppExit> {
        let current = self.run.lock().await.take()?;
        current.task.await.ok()
    }

    /// Starts pairing with a QR code or a pairing code for `phone`.
    pub(crate) async fn pair(&self, mode: PairMode, phone: Option<String>) {
        if let Ok(mut intent) = self.intent.lock() {
            *intent = Some(PairIntent {
                mode,
                phone: phone.clone(),
            });
        }
        let phone_login = match mode {
            PairMode::Code => phone,
            PairMode::Qr => None,
        };
        self.start(phone_login).await;
    }

    pub(crate) async fn cancel_pair(&self) {
        if let Ok(mut intent) = self.intent.lock() {
            *intent = None;
        }
        if Self::linked() {
            self.start(None).await;
        } else {
            self.stop().await;
        }
    }

    /// Stops the gateway and deletes the local session.
    pub(crate) async fn unlink(&self) -> Result<(), String> {
        if let Ok(mut intent) = self.intent.lock() {
            *intent = None;
        }
        self.stop().await;
        let db_path = crate::get_whatsapp_db_path();
        tokio::task::spawn_blocking(move || {
            let _lock = WhatsAppGateway::lock_session(&db_path)?;
            WhatsAppGateway::logout(&db_path).map_err(|error| error.to_string())
        })
        .await
        .map_err(|error| error.to_string())?
    }

    /// Asks a running gateway to process pending rows now.
    pub(crate) fn replay(&self) {
        self.hooks.replay.notify_one();
    }

    /// The pairing in progress, or its result once the gateway is online.
    pub(crate) fn pairing_view(&self) -> Option<PairingView> {
        let state = self.hooks.snapshot();
        let intent = self.intent.lock().ok().and_then(|intent| intent.clone());
        let done = state.phase == LinkPhase::Online && intent.is_some();
        if state.phase != LinkPhase::Pairing && !done && intent.is_none() {
            return None;
        }
        if done {
            if let Ok(mut slot) = self.intent.lock() {
                *slot = None;
            }
        }
        let mode = intent.as_ref().map_or_else(
            || {
                if state.code.is_some() {
                    PairMode::Code
                } else {
                    PairMode::Qr
                }
            },
            |intent| intent.mode,
        );
        let now = Instant::now();
        let stops_in = self
            .pair_deadline
            .lock()
            .ok()
            .and_then(|slot| *slot)
            .filter(|at| *at > now && !done)
            .map(|at| at.duration_since(now).as_secs());
        Some(PairingView {
            mode,
            qr: match mode {
                PairMode::Qr => state.qr.as_deref().and_then(qr_matrix),
                PairMode::Code => None,
            },
            code: state.code.as_deref().map(format_pair_code),
            phone: intent.and_then(|intent| intent.phone),
            expires_in: state
                .expires_at
                .filter(|at| *at > now)
                .map(|at| at.duration_since(now).as_secs()),
            stops_in,
            done,
            error: state.error,
        })
    }
}

async fn finish(mut run: WaRun) {
    let _ = run.stop.send(true);
    if tokio::time::timeout(STOP_GRACE, &mut run.task)
        .await
        .is_err()
    {
        warn!("WhatsApp gateway did not stop in time; aborting it");
        run.task.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qr_paths_cover_the_code() {
        let matrix = qr_matrix("2@abc,def,ghi").expect("QR encodes");
        assert!(matrix.size >= 21);
        assert!(matrix.path.starts_with('M'));
        assert!(matrix.path.contains('h'));
        assert!(!matrix.path.contains('<'), "path data only, never markup");
    }

    #[test]
    fn pair_codes_are_grouped() {
        assert_eq!(format_pair_code("K7QM2XPA"), "K7QM-2XPA");
        assert_eq!(format_pair_code("K7QM-2XPA"), "K7QM-2XPA");
        assert_eq!(format_pair_code("SHORT"), "SHORT");
    }
}
