use std::collections::{HashMap, HashSet};
use std::env;
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
use crate::{
    get_configured_owner_id, get_configured_whatsapp_owner, get_whatsapp_db_path,
    get_whatsapp_dedicated_groups, is_whatsapp_enabled, load_environment,
};

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

pub(crate) fn parse_chat_ids_from_str(raw: &str) -> HashSet<i64> {
    raw.split(',')
        .filter_map(|value| value.trim().parse::<i64>().ok())
        .collect()
}

pub(crate) fn parse_chat_ids_from_config(key: &str) -> HashSet<i64> {
    load_environment();
    let raw = env::var(key)
        .ok()
        .or_else(|| ai::service::load_app_setting(key))
        .unwrap_or_default();
    parse_chat_ids_from_str(&raw)
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

            let daemon_rows = [
                ("BOT NAME", bot_val.as_str()),
                ("PROTOCOL", proto_val),
                ("RUNTIME", timeline_val),
                ("ENGINE", engine_val),
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
    info!("Memulai polling pesan dengan durable control/generation queues...");

    loop {
        tokio::select! {
            _ = wait_until_shutdown(&mut shutdown) => {
                break;
            }
            updates_res = bot.get_updates(
                offset,
                100,
                20,
                Some(vec![
                    "message".to_string(),
                    "callback_query".to_string(),
                    "stopped_message_generation".to_string(),
                ]),
            ) => {
                match updates_res {
                    Ok(resp) if resp.ok => {
                        consecutive_failures = 0;
                        if let Some(updates) = resp.result {
                            for update in updates {
                                let update_id = update.update_id;
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
async fn supervise_whatsapp(
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

    let mut failures = 0u32;
    loop {
        let exit = crate::gateway::whatsapp::WhatsAppGateway::start(
            config.clone(),
            Arc::clone(&ai_service),
            shutdown.clone(),
        )
        .await;
        match exit {
            WhatsAppExit::Shutdown => return exit,
            WhatsAppExit::LoggedOut => {
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
                if failures == WHATSAPP_ALERT_AFTER_FAILURES {
                    notify("⚠️ Xiao: gateway WhatsApp gagal tersambung beberapa kali berturut-turut. Xiao terus mencoba ulang di latar belakang.").await;
                }
                let mut wait = shutdown.clone();
                tokio::select! {
                    _ = tokio::time::sleep(delay) => {}
                    _ = wait_until_shutdown(&mut wait) => return WhatsAppExit::Shutdown,
                }
            }
        }
    }
}

pub async fn run_daemon(ai_service: Arc<AIChatService>) {
    let wa_db_path = get_whatsapp_db_path();
    let wa_status = crate::gateway::whatsapp::WhatsAppGateway::check_status(&wa_db_path);
    let wa_enabled =
        is_whatsapp_enabled() || wa_status == crate::gateway::whatsapp::WhatsAppStatus::Linked;

    let tg_bootstrap = try_bootstrap_telegram(&ai_service).await;

    if tg_bootstrap.is_none() && !wa_enabled {
        error!("Tidak ada gateway yang aktif! Konfigurasikan Telegram (`xiao gateway token` & `xiao gateway owner`) atau WhatsApp (`xiao gateway wa pair`).");
        std::process::exit(1);
    }

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let signal_ai = Arc::clone(&ai_service);
    let signal_task = tokio::spawn(async move {
        wait_for_shutdown_signal().await;
        println!("\nReceived shutdown signal. Shutting down gracefully.");
        // Flag first so work cancelled below reports `Interrupted` and stays
        // in the durable inbox for replay instead of being marked answered.
        signal_ai.begin_shutdown().await;
        let _ = shutdown_tx.send(true);
    });

    // Spawn WhatsApp Gateway concurrently if configured or session exists
    let wa_worker = if wa_enabled {
        let wa_config = crate::gateway::whatsapp::WhatsAppConfig {
            db_path: wa_db_path,
            owner_number: get_configured_whatsapp_owner(),
            phone_login: None,
            dedicated_groups: get_whatsapp_dedicated_groups(),
        };
        let alert = tg_bootstrap
            .as_ref()
            .map(|(bot, scope, _)| (bot.clone(), scope.owner_user_id));
        info!("Memulai WhatsApp Gateway di background daemon...");
        Some(tokio::spawn(supervise_whatsapp(
            wa_config,
            Arc::clone(&ai_service),
            shutdown_rx.clone(),
            alert,
        )))
    } else {
        None
    };

    if let Some((bot, route_scope, user_last_image_prompt)) = tg_bootstrap {
        let (update_tx, update_worker) = spawn_workers(
            bot.clone(),
            Arc::clone(&ai_service),
            Arc::clone(&user_last_image_prompt),
            Arc::clone(&route_scope),
        );

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
        signal_task.abort();
        drop(update_tx);

        match tokio::time::timeout(SHUTDOWN_GRACE, update_worker).await {
            Ok(Ok(())) => {}
            Ok(Err(err)) => warn!("Update worker terminated with error: {err}"),
            Err(_) => warn!("Update worker did not stop within shutdown grace period"),
        }
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
    }

    if let Some(wa_handle) = wa_worker {
        // In standalone mode this is the daemon's lifetime: it returns on
        // shutdown or when the device is logged out (nothing left to serve).
        match wa_handle.await {
            Ok(WhatsAppExit::LoggedOut) => {
                error!(
                    "WhatsApp gateway berhenti permanen karena logout; daemon WhatsApp selesai."
                );
            }
            Ok(_) => {}
            Err(err) => warn!("WhatsApp supervisor terminated with error: {err}"),
        }
    }
    ai_service.begin_shutdown().await;
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
