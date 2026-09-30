use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{watch, RwLock};
use tracing::{error, info, warn};

use crate::ai::{self, AIChatService};
use crate::bot::client::TelegramBotClient;
use crate::bot::image_flow::UserLastImagePrompt;
use crate::bot::models::{BotCommand, Update};
use crate::bot::router::ChatRouteScope;
use crate::bot::worker::{process_durable_update, replay_durable_inbox, spawn_workers};
use crate::cli::get_or_prompt_token;
use crate::gateway::whatsapp::client::WhatsAppExit;
use crate::get_configured_owner_id;

/// Time allowed for in-flight work to finish once shutdown starts.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(10);
/// Polling backoff after consecutive `getUpdates` failures.
const POLL_BACKOFF_INITIAL: Duration = Duration::from_secs(2);
const POLL_BACKOFF_MAX: Duration = Duration::from_secs(60);
/// Restart backoff for a WhatsApp session that ended unexpectedly.
const WHATSAPP_RESTART_INITIAL: Duration = Duration::from_secs(5);
const WHATSAPP_RESTART_MAX: Duration = Duration::from_secs(300);
/// Consecutive WhatsApp failures before the owner is alerted on Telegram.
const WHATSAPP_ALERT_AFTER_FAILURES: u32 = 3;
/// Long-poll timeout of `getUpdates`, in seconds.
pub(crate) const POLL_TIMEOUT_SECS: i32 = 20;

/// Unix time of the last successful `getUpdates` (0 before the first).
static LAST_POLL_UNIX: AtomicI64 = AtomicI64::new(0);
/// Inline queries and chosen inline results seen since start. Telegram only
/// sends chosen results when inline feedback is on in @BotFather, so many
/// queries without a single chosen result means it is probably off.
static INLINE_QUERIES_SEEN: AtomicU64 = AtomicU64::new(0);
static CHOSEN_RESULTS_SEEN: AtomicU64 = AtomicU64::new(0);

/// Seconds since the last successful poll, `None` before the first.
pub(crate) fn last_poll_age_secs() -> Option<u64> {
    let last = LAST_POLL_UNIX.load(Ordering::Relaxed);
    (last > 0).then(|| u64::try_from(chrono::Utc::now().timestamp() - last).unwrap_or_default())
}

pub(crate) fn inline_counters() -> (u64, u64) {
    (
        INLINE_QUERIES_SEEN.load(Ordering::Relaxed),
        CHOSEN_RESULTS_SEEN.load(Ordering::Relaxed),
    )
}

pub(crate) fn parse_chat_ids_from_str(raw: &str) -> HashSet<i64> {
    raw.split(',')
        .filter_map(|value| value.trim().parse::<i64>().ok())
        .collect()
}

pub(crate) fn parse_chat_ids_from_config(key: &str) -> HashSet<i64> {
    parse_chat_ids_from_str(&crate::configured_setting(key).unwrap_or_default())
}

pub(crate) fn get_allowed_chat_ids() -> HashSet<i64> {
    parse_chat_ids_from_config("ALLOWED_CHAT_IDS")
}

pub(crate) fn get_dedicated_chat_ids() -> HashSet<i64> {
    parse_chat_ids_from_config("DEDICATED_CHAT_IDS")
}

pub async fn try_bootstrap_telegram(
    ai_service: &AIChatService,
) -> Option<(TelegramBotClient, Arc<ChatRouteScope>, UserLastImagePrompt)> {
    let token = get_or_prompt_token(ai_service).await?;
    bootstrap_telegram_with_token(token).await
}

/// Connects the bot without asking anything on the terminal.
async fn bootstrap_telegram_with_token(
    token: String,
) -> Option<(TelegramBotClient, Arc<ChatRouteScope>, UserLastImagePrompt)> {
    let owner_user_id = get_configured_owner_id()?;

    let bot = TelegramBotClient::new(token);
    let user_last_image_prompt: UserLastImagePrompt = Arc::new(RwLock::new(HashMap::new()));

    // Test connection & get bot identity
    let (bot_id, bot_username) = match bot.get_me().await {
        Ok(resp) if resp.ok => {
            let Some(bot_info) = resp.result else {
                warn!("Telegram getMe returned ok=true without a result");
                return None;
            };
            let bar_width = crate::cli::tui::get_terminal_bar_width();
            let uname = bot_info.username.as_deref().unwrap_or("XiaoBot");
            let first_name = bot_info.first_name.as_str();
            let bot_val = format!(
                "\x1b[38;2;16;185;129m●\x1b[0m \x1b[1;37m@{uname}\x1b[0m \x1b[38;5;244m({first_name})\x1b[0m"
            );
            let proto_val = "\x1b[38;2;6;182;212m●\x1b[0m \x1b[1;37mTelegram Bot API 10.3\x1b[0m \x1b[38;5;244m(Rich Messages + Drafts)\x1b[0m";
            let timeline_val = "\x1b[38;2;139;92;246m●\x1b[0m \x1b[1;37mStreaming Timeline\x1b[0m \x1b[38;5;244m· Native Stop Button Active\x1b[0m";
            let engine_val = "\x1b[38;2;16;185;129m●\x1b[0m \x1b[1;37mOpenAI-Compatible Core\x1b[0m \x1b[38;5;244m· Long-Polling Active\x1b[0m";
            let guest_val = if bot_info.supports_guest_queries == Some(true) {
                format!(
                    "\x1b[38;2;16;185;129m●\x1b[0m \x1b[1;37mAktif\x1b[0m \x1b[38;5;244m· sebut @{uname} di chat mana pun\x1b[0m"
                )
            } else {
                "\x1b[38;5;244m○ Nonaktif · atur di @BotFather\x1b[0m".to_string()
            };
            let inline_val = if bot_info.supports_inline_queries == Some(true) {
                format!(
                    "\x1b[38;2;16;185;129m●\x1b[0m \x1b[1;37mAktif\x1b[0m \x1b[38;5;244m· ketik @{uname} lalu pertanyaan\x1b[0m"
                )
            } else {
                "\x1b[38;5;244m○ Nonaktif · /setinline & /setinlinefeedback di @BotFather\x1b[0m"
                    .to_string()
            };

            let daemon_rows = [
                ("BOT NAME", bot_val.as_str()),
                ("PROTOCOL", proto_val),
                ("RUNTIME", timeline_val),
                ("ENGINE", engine_val),
                ("GUEST MODE", guest_val.as_str()),
                ("INLINE MODE", inline_val.as_str()),
            ];
            crate::cli::tui::print_mini_header("Daemon Service");
            let hud =
                crate::cli::tui::render_hud_box("TELEGRAM DAEMON ACTIVE", &daemon_rows, bar_width);
            println!("{hud}");
            println!("\n  \x1b[38;5;244mService actively running. Press \x1b[1;37m[Ctrl+C]\x1b[0m \x1b[38;5;244mto stop daemon.\x1b[0m\n");
            (Some(bot_info.id), bot_info.username)
        }
        Ok(resp) => {
            warn!(
                "Gagal terhubung ke Telegram Bot API: {:?}",
                resp.description
            );
            return None;
        }
        Err(e) => {
            warn!("Telegram HTTP connection error: {e}");
            return None;
        }
    };

    let route_scope = Arc::new(ChatRouteScope::new(
        owner_user_id,
        get_allowed_chat_ids(),
        get_dedicated_chat_ids(),
        bot_id,
        bot_username,
        bot.clone(),
    ));

    // Register Bot Commands - Clear all commands for pure zero-slash conversational gateway
    let empty_commands: Vec<BotCommand> = vec![];

    if let Err(e) = bot.set_my_commands(&empty_commands).await {
        warn!("Gagal mengosongkan bot commands di Telegram: {e}");
    } else {
        info!("Bot commands berhasil dikosongkan (pure zero-slash gateway).");
    }

    Some((bot, route_scope, user_last_image_prompt))
}

#[allow(dead_code)]
pub async fn bootstrap_bot(
    ai_service: &AIChatService,
) -> (TelegramBotClient, Arc<ChatRouteScope>, UserLastImagePrompt) {
    match try_bootstrap_telegram(ai_service).await {
        Some(res) => res,
        None => std::process::exit(1),
    }
}

/// Resolves when the process is asked to stop: Ctrl+C everywhere, plus
/// SIGTERM/SIGHUP on Unix and console close/logoff/shutdown on Windows.
/// Shared by the Telegram and the standalone WhatsApp daemon so that both
/// stop gracefully under systemd/Termux as well as interactively.
pub async fn wait_for_shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut sigterm = signal(SignalKind::terminate())
            .map_err(|e| warn!("Gagal mendaftarkan SIGTERM handler: {e}"))
            .ok();
        let mut sighup = signal(SignalKind::hangup())
            .map_err(|e| warn!("Gagal mendaftarkan SIGHUP handler: {e}"))
            .ok();
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = async {
                match sigterm.as_mut() {
                    Some(sig) => { sig.recv().await; }
                    None => std::future::pending::<()>().await,
                }
            } => {}
            _ = async {
                match sighup.as_mut() {
                    Some(sig) => { sig.recv().await; }
                    None => std::future::pending::<()>().await,
                }
            } => {}
        }
    }
    #[cfg(windows)]
    {
        use tokio::signal::windows;
        let mut ctrl_close = windows::ctrl_close().ok();
        let mut ctrl_shutdown = windows::ctrl_shutdown().ok();
        let mut ctrl_logoff = windows::ctrl_logoff().ok();
        let mut ctrl_break = windows::ctrl_break().ok();
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = async {
                match ctrl_close.as_mut() {
                    Some(sig) => { sig.recv().await; }
                    None => std::future::pending::<()>().await,
                }
            } => {}
            _ = async {
                match ctrl_shutdown.as_mut() {
                    Some(sig) => { sig.recv().await; }
                    None => std::future::pending::<()>().await,
                }
            } => {}
            _ = async {
                match ctrl_logoff.as_mut() {
                    Some(sig) => { sig.recv().await; }
                    None => std::future::pending::<()>().await,
                }
            } => {}
            _ = async {
                match ctrl_break.as_mut() {
                    Some(sig) => { sig.recv().await; }
                    None => std::future::pending::<()>().await,
                }
            } => {}
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

/// Exponential polling backoff: 2s, 4s, 8s ... capped at 60s.
pub(crate) fn poll_backoff(consecutive_failures: u32) -> Duration {
    let exponent = consecutive_failures.saturating_sub(1).min(5);
    POLL_BACKOFF_INITIAL
        .saturating_mul(1u32 << exponent)
        .min(POLL_BACKOFF_MAX)
}

async fn wait_until_shutdown(shutdown: &mut watch::Receiver<bool>) {
    while !*shutdown.borrow() {
        if shutdown.changed().await.is_err() {
            return;
        }
    }
}

pub async fn poll_loop(
    bot: &TelegramBotClient,
    ai_service: &Arc<AIChatService>,
    user_last_image_prompt: &UserLastImagePrompt,
    route_scope: &Arc<ChatRouteScope>,
    update_tx: &tokio::sync::mpsc::Sender<Update>,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut offset = ai::storage::load_telegram_offset_async().await;
    let mut consecutive_failures = 0u32;
    // getMe just succeeded, so the bot counts as online from the first poll.
    LAST_POLL_UNIX.store(chrono::Utc::now().timestamp(), Ordering::Relaxed);
    info!("Memulai polling pesan dengan durable control/generation queues...");

    loop {
        tokio::select! {
            _ = wait_until_shutdown(&mut shutdown) => {
                break;
            }
            updates_res = bot.get_updates(
                offset,
                100,
                POLL_TIMEOUT_SECS,
                Some(vec![
                    "message".to_string(),
                    "edited_message".to_string(),
                    "guest_message".to_string(),
                    "inline_query".to_string(),
                    "chosen_inline_result".to_string(),
                    "callback_query".to_string(),
                    "stopped_message_generation".to_string(),
                ]),
            ) => {
                match updates_res {
                    Ok(resp) if resp.ok => {
                        consecutive_failures = 0;
                        LAST_POLL_UNIX.store(chrono::Utc::now().timestamp(), Ordering::Relaxed);
                        if let Some(updates) = resp.result {
                            for update in updates {
                                let update_id = update.update_id;
                                if update.inline_query.is_some() {
                                    INLINE_QUERIES_SEEN.fetch_add(1, Ordering::Relaxed);
                                }
                                if update.chosen_inline_result.is_some() {
                                    CHOSEN_RESULTS_SEEN.fetch_add(1, Ordering::Relaxed);
                                }
                                let payload_json = match serde_json::to_string(&update) {
                                    Ok(payload) => payload,
                                    Err(error) => {
                                        error!("Gagal serialize Telegram update {update_id}: {error}");
                                        break;
                                    }
                                };
                                let Some(accepted) = ai::storage::enqueue_telegram_update_async(
                                    update_id,
                                    payload_json,
                                )
                                .await
                                else {
                                    // Never acknowledge a later Telegram update if the durable
                                    // acceptance transaction for this update failed.
                                    error!(
                                        "Durable Telegram intake gagal untuk update {update_id}; offset tidak dimajukan"
                                    );
                                    break;
                                };
                                offset = Some(update_id.saturating_add(1));
                                if !accepted {
                                    continue;
                                }

                                if update.stopped_message_generation.is_some() {
                                    // Native Stop bypasses the queue so cancellation cannot be
                                    // blocked by queued generation work.
                                    if update
                                        .stopped_message_generation
                                        .as_ref()
                                        .is_some_and(|stop| stop.chat.id == route_scope.owner_user_id)
                                    {
                                        process_durable_update(
                                            bot,
                                            ai_service,
                                            user_last_image_prompt,
                                            route_scope,
                                            update,
                                        )
                                        .await;
                                    } else {
                                        let _ = ai::storage::skip_telegram_update_async(update_id).await;
                                    }
                                } else if update_tx.send(update).await.is_err() {
                                    error!("Update worker stopped unexpectedly");
                                    return;
                                }
                            }
                        }
                    }
                    Ok(resp) => {
                        consecutive_failures = consecutive_failures.saturating_add(1);
                        let delay = poll_backoff(consecutive_failures);
                        warn!(
                            "Telegram polling update not ok: {:?}; mencoba lagi dalam {:?}",
                            resp.description, delay
                        );
                        tokio::select! {
                            _ = tokio::time::sleep(delay) => {}
                            _ = wait_until_shutdown(&mut shutdown) => break,
                        }
                    }
                    Err(e) => {
                        consecutive_failures = consecutive_failures.saturating_add(1);
                        let delay = poll_backoff(consecutive_failures);
                        error!("Polling network error: {e}; mencoba lagi dalam {delay:?}");
                        tokio::select! {
                            _ = tokio::time::sleep(delay) => {}
                            _ = wait_until_shutdown(&mut shutdown) => break,
                        }
                    }
                }
            }
        }
    }
}

/// Keeps the WhatsApp gateway alive: restarts it with backoff when it ends
/// unexpectedly and alerts the owner on Telegram when the phone unlinks the
/// device or the gateway keeps failing. Previously a single failure left
/// WhatsApp offline until the whole process was restarted, silently.
pub(crate) async fn supervise_whatsapp(
    config: crate::gateway::whatsapp::WhatsAppConfig,
    ai_service: Arc<AIChatService>,
    shutdown: watch::Receiver<bool>,
    alert: Option<(TelegramBotClient, i64)>,
) -> WhatsAppExit {
    let notify = |text: &'static str| {
        let alert = alert.clone();
        async move {
            if let Some((bot, owner)) = alert {
                if bot
                    .send_message(owner, text, None, None, None, None)
                    .await
                    .is_err()
                {
                    warn!("Gagal mengirim notifikasi status WhatsApp ke pemilik");
                }
            }
        }
    };

    let set_phase = |phase: crate::gateway::whatsapp::LinkPhase, error: Option<String>| {
        if let Some(hooks) = config.hooks.as_ref() {
            hooks.update(|state| {
                state.phase = phase;
                if error.is_some() {
                    state.error = error;
                }
            });
        }
    };

    let mut failures = 0u32;
    loop {
        let exit = crate::gateway::whatsapp::WhatsAppGateway::start(
            config.clone(),
            Arc::clone(&ai_service),
            shutdown.clone(),
        )
        .await;
        match exit {
            WhatsAppExit::Shutdown => {
                set_phase(crate::gateway::whatsapp::LinkPhase::Off, None);
                return exit;
            }
            WhatsAppExit::LoggedOut => {
                set_phase(crate::gateway::whatsapp::LinkPhase::LoggedOut, None);
                warn!("WhatsApp gateway berhenti: perangkat telah di-logout dari HP");
                notify("⚠️ Xiao: sesi WhatsApp dilepas dari HP (logout). Gateway WhatsApp berhenti; jalankan `xiao gateway wa pair` untuk menautkan ulang.").await;
                return exit;
            }
            WhatsAppExit::Failed(reason) => {
                failures = failures.saturating_add(1);
                let exponent = failures.saturating_sub(1).min(6);
                let delay = WHATSAPP_RESTART_INITIAL
                    .saturating_mul(1u32 << exponent)
                    .min(WHATSAPP_RESTART_MAX);
                warn!(
                    "WhatsApp gateway berhenti ({reason}); restart ke-{failures} dalam {delay:?}"
                );
                set_phase(crate::gateway::whatsapp::LinkPhase::Retrying, Some(reason));
                if failures == WHATSAPP_ALERT_AFTER_FAILURES {
                    notify("⚠️ Xiao: gateway WhatsApp gagal tersambung beberapa kali berturut-turut. Xiao terus mencoba ulang di latar belakang.").await;
                }
                let mut wait = shutdown.clone();
                tokio::select! {
                    _ = tokio::time::sleep(delay) => {}
                    _ = wait_until_shutdown(&mut wait) => {
                        set_phase(crate::gateway::whatsapp::LinkPhase::Off, None);
                        return WhatsAppExit::Shutdown;
                    }
                }
                set_phase(crate::gateway::whatsapp::LinkPhase::Starting, None);
            }
        }
    }
}

/// How often a daemon without Telegram checks whether the bot token and
/// owner have been set (for example in the WebUI).
const TELEGRAM_SETUP_CHECK: Duration = Duration::from_secs(5);

/// Starts Telegram as soon as it can, while the WebUI keeps the daemon
/// alive: waits until the bot token and owner are set, then retries with
/// the polling backoff while Telegram cannot be reached (for example before
/// the network is up). Returns `None` when the daemon stops first.
async fn wait_for_telegram(
    mut shutdown: watch::Receiver<bool>,
) -> Option<(TelegramBotClient, Arc<ChatRouteScope>, UserLastImagePrompt)> {
    let mut failures = 0u32;
    loop {
        let delay = if crate::web::auth::telegram_configured() {
            failures = failures.saturating_add(1);
            let delay = poll_backoff(failures);
            warn!("Telegram could not be started; trying again in {delay:?}");
            delay
        } else {
            failures = 0;
            TELEGRAM_SETUP_CHECK
        };
        tokio::select! {
            _ = tokio::time::sleep(delay) => {}
            _ = wait_until_shutdown(&mut shutdown) => return None,
        }
        let Some(token) = crate::get_configured_token() else {
            continue;
        };
        if get_configured_owner_id().is_none() {
            continue;
        }
        if let Some(telegram) = bootstrap_telegram_with_token(token).await {
            info!("Telegram started");
            return Some(telegram);
        }
    }
}

pub async fn run_daemon(ai_service: Arc<AIChatService>) {
    LAST_POLL_UNIX.store(0, Ordering::Relaxed);
    let wa_enabled = crate::web::wa::WaController::should_run();

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let wa = crate::web::wa::WaController::new(Arc::clone(&ai_service), shutdown_rx.clone());
    // The console starts first so it can be used to finish the setup even
    // when Telegram cannot start.
    let web = crate::web::start(
        Arc::clone(&ai_service),
        Arc::clone(&wa),
        shutdown_tx.clone(),
        shutdown_rx.clone(),
    )
    .await;

    let tg_bootstrap = try_bootstrap_telegram(&ai_service).await;

    if tg_bootstrap.is_none() && !wa_enabled && web.is_none() {
        error!("Tidak ada gateway yang aktif! Konfigurasikan Telegram (`xiao gateway token` & `xiao gateway owner`) atau WhatsApp (`xiao gateway wa pair`), atau aktifkan WebUI (XIAO_WEB_BIND).");
        std::process::exit(1);
    }

    let signal_ai = Arc::clone(&ai_service);
    let signal_shutdown = shutdown_tx.clone();
    let signal_task = tokio::spawn(async move {
        wait_for_shutdown_signal().await;
        println!("\nReceived shutdown signal. Shutting down gracefully.");
        // Flag first so work cancelled below reports `Interrupted` and stays
        // in the durable inbox for replay instead of being marked answered.
        signal_ai.begin_shutdown().await;
        let _ = signal_shutdown.send(true);
    });

    // WhatsApp runs under the controller so the WebUI can restart or pair it.
    wa.set_alert(
        tg_bootstrap
            .as_ref()
            .map(|(bot, scope, _)| (bot.clone(), scope.owner_user_id)),
    );
    if wa_enabled {
        info!("Memulai WhatsApp Gateway di background daemon...");
        wa.start(None).await;
    }

    let tg_bootstrap = match tg_bootstrap {
        None if web.is_some() => {
            if !crate::web::auth::telegram_configured() {
                info!("Telegram is not set up yet; it starts once the bot token and owner are set in the WebUI");
            }
            let telegram = wait_for_telegram(shutdown_rx.clone()).await;
            if let Some((bot, scope, _)) = telegram.as_ref() {
                wa.set_alert(Some((bot.clone(), scope.owner_user_id)));
            }
            telegram
        }
        other => other,
    };

    if let Some((bot, route_scope, user_last_image_prompt)) = tg_bootstrap {
        let (update_tx, update_worker) = spawn_workers(
            bot.clone(),
            Arc::clone(&ai_service),
            Arc::clone(&user_last_image_prompt),
            Arc::clone(&route_scope),
        );
        if let Some(web) = web.as_ref() {
            web.attach_telegram(
                bot.clone(),
                route_scope.owner_user_id,
                route_scope.bot_username.clone(),
                update_tx.clone(),
            );
        }

        replay_durable_inbox(&ai_service, &update_tx).await;

        poll_loop(
            &bot,
            &ai_service,
            &user_last_image_prompt,
            &route_scope,
            &update_tx,
            shutdown_rx.clone(),
        )
        .await;

        // poll_loop also returns if the dispatcher died; make sure every
        // component observes shutdown in that case too.
        ai_service.begin_shutdown().await;
        let _ = shutdown_tx.send(true);
        signal_task.abort();
        if let Some(web) = web.as_ref() {
            web.detach_telegram();
        }
        drop(update_tx);

        match tokio::time::timeout(SHUTDOWN_GRACE, update_worker).await {
            Ok(Ok(())) => {}
            Ok(Err(err)) => warn!("Update worker terminated with error: {err}"),
            Err(_) => warn!("Update worker did not stop within shutdown grace period"),
        }
        wa.stop().await;
    } else if web.is_some() {
        // Without Telegram the daemon lives as long as the console, so a
        // WhatsApp logout or a missing bot token can be fixed from there.
        let mode = if wa_enabled {
            "WhatsApp + WebUI"
        } else {
            "WebUI only (configure Telegram or WhatsApp from the console)"
        };
        info!("Daemon stopping without Telegram: {mode}");
        let mut wait = shutdown_rx.clone();
        wait_until_shutdown(&mut wait).await;
        wa.stop().await;
    } else {
        // WhatsApp Standalone Daemon Mode
        let bar_width = crate::cli::tui::get_terminal_bar_width();
        let proto_val = "\x1b[38;2;6;182;212m●\x1b[0m \x1b[1;37mWhatsApp Gateway\x1b[0m \x1b[38;5;244m(Multi-Device)\x1b[0m";
        let engine_val = "\x1b[38;2;16;185;129m●\x1b[0m \x1b[1;37mOpenAI-Compatible Core\x1b[0m \x1b[38;5;244m· WebSocket Active\x1b[0m";
        let daemon_rows = [
            ("GATEWAY", proto_val),
            ("MODE", "Standalone WhatsApp Daemon"),
            ("ENGINE", engine_val),
        ];
        crate::cli::tui::print_mini_header("Daemon Service");
        let hud =
            crate::cli::tui::render_hud_box("WHATSAPP DAEMON ACTIVE", &daemon_rows, bar_width);
        println!("{hud}");
        println!("\n  \x1b[38;5;244mService actively running. Press \x1b[1;37m[Ctrl+C]\x1b[0m \x1b[38;5;244mto stop daemon.\x1b[0m\n");

        // In standalone mode this is the daemon's lifetime: it returns on
        // shutdown or when the device is logged out (nothing left to serve).
        if wa.join().await == Some(WhatsAppExit::LoggedOut) {
            error!("WhatsApp gateway berhenti permanen karena logout; daemon WhatsApp selesai.");
        }
    }
    signal_task.abort();
    ai_service.begin_shutdown().await;
    let _ = shutdown_tx.send(true);
    // A restart in the same process binds the address again.
    if let Some(web) = web.as_ref() {
        web.stop_server().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_chat_ids_from_str_splits_correctly() {
        let ids = parse_chat_ids_from_str("12345, -1009988, 54321, invalid, 0");
        assert!(ids.contains(&12345));
        assert!(ids.contains(&-1009988));
        assert!(ids.contains(&54321));
        assert!(ids.contains(&0));
        assert_eq!(ids.len(), 4);
    }
}
