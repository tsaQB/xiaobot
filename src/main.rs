mod ai;
mod attachments;
mod bot;
mod cli;
mod document;
pub mod gateway;
mod parser;
mod timeline;
mod util;
mod web;

use std::env;
use std::io;
use std::path::Path;
use std::sync::Arc;

use ai::AIChatService;
use cli::*;

/// A setting from the environment (including `.env`), or from the values
/// saved by the CLI when the environment leaves it unset or empty. The
/// environment wins so deployments stay declarative; the CLI warns when it
/// saves a value the environment overrides ([`warn_if_environment_overrides`]).
pub(crate) fn configured_setting(key: &str) -> Option<String> {
    load_environment();
    let non_empty = |value: String| {
        let value = value.trim().to_string();
        (!value.is_empty()).then_some(value)
    };
    env::var(key)
        .ok()
        .and_then(non_empty)
        .or_else(|| ai::service::load_app_setting(key).and_then(non_empty))
}

/// After the CLI saved `saved` under one of `keys`, tells the user when the
/// environment (usually a `.env` file) sets the same setting to something
/// else, because that value keeps winning. Values are never printed: some
/// of these settings are secrets.
pub(crate) fn warn_if_environment_overrides(keys: &[&str], saved: &str) {
    let overriding = keys.iter().find(|key| {
        env::var(key).is_ok_and(|value| {
            let value = value.trim();
            !value.is_empty() && !value.contains("YOUR_") && value != saved.trim()
        })
    });
    let Some(key) = overriding else {
        return;
    };
    let config_path = get_config_path();
    let source = if config_path.exists() {
        format!("file {}", config_path.display())
    } else {
        "environment proses".to_string()
    };
    println!(
        "  \x1b[33m⚠ {key} juga diatur di {source} dengan nilai lain, dan nilai itu yang dipakai. Hapus atau ubah baris {key} di sana agar perubahan ini berlaku.\x1b[0m"
    );
}

pub(crate) fn get_configured_owner_id() -> Option<i64> {
    configured_setting("OWNER_USER_ID")
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|value| *value > 0)
}

pub(crate) fn get_configured_whatsapp_owner() -> Option<String> {
    configured_setting("WHATSAPP_OWNER_NUMBER")
}

/// Comma-separated `WHATSAPP_DEDICATED_GROUPS` (group JIDs or numeric ids).
pub(crate) fn get_whatsapp_dedicated_groups() -> Vec<String> {
    configured_setting("WHATSAPP_DEDICATED_GROUPS")
        .unwrap_or_default()
        .split(',')
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect()
}

pub(crate) fn is_whatsapp_enabled() -> bool {
    configured_setting("WHATSAPP_ENABLED")
        .map(|v| {
            let s = v.to_lowercase();
            s == "true" || s == "1" || s == "yes"
        })
        .unwrap_or(false)
}

pub(crate) fn get_whatsapp_db_path() -> std::path::PathBuf {
    let dir = crate::ai::storage::xiao_data_dir();
    let _ = std::fs::create_dir_all(&dir);
    crate::ai::storage::harden_dir_mode(&dir);
    dir.join("whatsapp.db")
}

/// Whether a `.env` found relative to the working directory may be loaded.
///
/// The working directory is not necessarily controlled by the owner (a
/// cloned repository, a shared folder). On Unix the file must be owned by the
/// same user as `$HOME` and must not be group/world-writable; otherwise a
/// planted `.env` could silently replace BOT_TOKEN, API keys or endpoints.
#[cfg(unix)]
fn is_trusted_env_file(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if metadata.mode() & 0o022 != 0 {
        eprintln!(
            "[WARN] Mengabaikan {} karena dapat ditulis oleh pengguna lain.",
            path.display()
        );
        return false;
    }
    let owner_ok = env::var_os("HOME")
        .and_then(|home| std::fs::metadata(home).ok())
        .is_none_or(|home| home.uid() == metadata.uid());
    if !owner_ok {
        eprintln!(
            "[WARN] Mengabaikan {} karena bukan milik pengguna ini.",
            path.display()
        );
    }
    owner_ok
}

#[cfg(not(unix))]
fn is_trusted_env_file(_path: &Path) -> bool {
    true
}

pub(crate) fn get_config_path() -> std::path::PathBuf {
    // 1. Current working directory .env (only when it is trusted)
    if Path::new(".env").exists() && is_trusted_env_file(Path::new(".env")) {
        return Path::new(".env").to_path_buf();
    }
    // 2. XDG_CONFIG_HOME for Linux and Termux
    if let Ok(xdg_config) = env::var("XDG_CONFIG_HOME") {
        let trimmed = xdg_config.trim();
        if !trimmed.is_empty() {
            let config_path = Path::new(trimmed);
            let app_env = config_path.join("xiao").join(".env");
            if app_env.exists() {
                return app_env;
            }
            let dot_app_env = config_path.join(".xiao").join(".env");
            if dot_app_env.exists() {
                return dot_app_env;
            }
            let legacy_app_env = config_path.join("xiaoai").join(".env");
            if legacy_app_env.exists() {
                return legacy_app_env;
            }
        }
    }
    // 3. ~/.xiao.env or ~/.xiao/.env across HOME and USERPROFILE
    let home_candidates = [env::var("HOME").ok(), env::var("USERPROFILE").ok()];
    for home_opt in home_candidates.into_iter().flatten() {
        let trimmed = home_opt.trim();
        if trimmed.is_empty() {
            continue;
        }
        let home_path = Path::new(trimmed);
        let home_env = home_path.join(".xiao.env");
        if home_env.exists() {
            return home_env;
        }
        let dot_app_dir_env = home_path.join(".xiao").join(".env");
        if dot_app_dir_env.exists() {
            return dot_app_dir_env;
        }
        let dot_xiaoai_dir_env = home_path.join(".xiaoai").join(".env");
        if dot_xiaoai_dir_env.exists() {
            return dot_xiaoai_dir_env;
        }
        let xdg_config_fallback = home_path.join(".config").join("xiao").join(".env");
        if xdg_config_fallback.exists() {
            return xdg_config_fallback;
        }
        let app_dir_env = home_path.join("xiao").join(".env");
        if app_dir_env.exists() {
            return app_dir_env;
        }
        let app_dir_xiaoai_env = home_path.join("xiaoai").join(".env");
        if app_dir_xiaoai_env.exists() {
            return app_dir_xiaoai_env;
        }
        let legacy_app_dir_env = home_path.join("XiaoAI").join(".env");
        if legacy_app_dir_env.exists() {
            return legacy_app_dir_env;
        }
    }
    // 4. %APPDATA%\xiao\.env or %APPDATA%\XiaoAI\.env
    if let Ok(appdata) = env::var("APPDATA") {
        let trimmed = appdata.trim();
        if !trimmed.is_empty() {
            let appdata_path = Path::new(trimmed);
            let app_dir_env = appdata_path.join("xiao").join(".env");
            if app_dir_env.exists() {
                return app_dir_env;
            }
            let legacy_app_dir_env = appdata_path.join("XiaoAI").join(".env");
            if legacy_app_dir_env.exists() {
                return legacy_app_dir_env;
            }
            let modern_app_dir_env = appdata_path.join("xiaoai").join(".env");
            if modern_app_dir_env.exists() {
                return modern_app_dir_env;
            }
            let appdata_dot_env = appdata_path.join(".xiao.env");
            if appdata_dot_env.exists() {
                return appdata_dot_env;
            }
        }
    }
    // 5. A trusted `.env` in a parent of the working directory (the previous
    //    `dotenvy::dotenv()` fallback searched parents without any check).
    if let Ok(cwd) = env::current_dir() {
        for dir in cwd.ancestors().skip(1) {
            let candidate = dir.join(".env");
            if candidate.exists() && is_trusted_env_file(&candidate) {
                return candidate;
            }
        }
    }
    Path::new(".env").to_path_buf()
}

pub(crate) fn load_environment() {
    let cfg_path = get_config_path();
    if cfg_path.exists() && (cfg_path != Path::new(".env") || is_trusted_env_file(&cfg_path)) {
        let _ = dotenvy::from_path(&cfg_path);
    }
}

/// Saves a setting from the CLI, warning when the environment overrides it.
pub(crate) fn save_env_kv(key: &str, value: &str) -> io::Result<()> {
    ai::service::save_app_setting(key, value)?;
    warn_if_environment_overrides(&[key], value);
    Ok(())
}

pub(crate) fn save_token_to_env(token: &str) -> io::Result<()> {
    save_env_kv("BOT_TOKEN", token)
}

pub(crate) fn get_configured_token() -> Option<String> {
    load_environment();
    if let Ok(token) = env::var("BOT_TOKEN") {
        let trimmed = token.trim().to_string();
        if !trimmed.is_empty() && trimmed != "YOUR_TELEGRAM_BOT_TOKEN_HERE" {
            return Some(trimmed);
        }
    }
    if let Some(token) = ai::service::load_app_setting("BOT_TOKEN") {
        let trimmed = token.trim().to_string();
        if !trimmed.is_empty() && trimmed != "YOUR_TELEGRAM_BOT_TOKEN_HERE" {
            return Some(trimmed);
        }
    }
    None
}

// ==========================================
// Main CLI Entrypoint
// ==========================================

fn init_tracing() {
    use std::sync::Once;
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    static TRACING_INIT: Once = Once::new();
    TRACING_INIT.call_once(|| {
        // RUST_LOG wins; otherwise the level saved from the WebUI. The filter
        // sits behind a reload handle so the WebUI can change it live.
        let (filter, handle) = tracing_subscriber::reload::Layer::new(web::logs::startup_filter());
        web::logs::install_reload_handle(handle);
        // Logs go to stderr so they never interleave with the CLI spinner or
        // with answers printed to stdout (`xiao "question" > answer.txt`).
        // The ring layer keeps the latest lines for the WebUI log page.
        let _ = tracing_subscriber::registry()
            .with(filter)
            .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
            .with(web::logs::RingLayer)
            .try_init();
    });
}

#[tokio::main]
async fn main() {
    load_environment();
    init_tracing();
    use std::io::IsTerminal;
    let args: Vec<String> = env::args().collect();
    let subcommand = args.get(1).map(|s| s.as_str());

    // `xiao <command> help` works for every command: the hubs print their
    // own reference, these print the main one instead of, for example,
    // sending "help" to the model or starting the daemon.
    let asks_help = args.len() == 3 && matches!(args[2].as_str(), "help" | "-h" | "--help");
    if asks_help
        && matches!(
            subcommand,
            Some("chat" | "status" | "setup" | "start" | "menu")
        )
    {
        print_cli_help();
        return;
    }

    let ai_service = Arc::new(AIChatService::new());

    match subcommand {
        None => {
            if std::io::stdout().is_terminal() {
                run_cli_chat(&ai_service, None).await;
            } else {
                print_cli_help();
            }
            return;
        }
        Some("menu") => {
            if std::io::stdout().is_terminal() {
                run_cli_launcher(&ai_service).await;
            } else {
                print_cli_help();
            }
            return;
        }
        Some("-v") | Some("--version") | Some("version") => {
            println!("xiao v{}", env!("CARGO_PKG_VERSION"));
            return;
        }
        Some("ai") => {
            let action_arg = args.get(2).map(|s| s.as_str());
            let target_arg = args.get(3).map(|s| s.as_str());
            run_cli_ai_hub(&ai_service, action_arg, target_arg).await;
            return;
        }
        Some("setup") => {
            let _ = run_cli_quickstart_wizard(&ai_service).await;
            return;
        }
        Some("status") => {
            run_cli_status(&ai_service).await;
            return;
        }
        Some("context") => {
            let chat_arg = args.get(2).map(|s| s.as_str());
            let thread_arg = args.get(3).map(|s| s.as_str());
            run_cli_context(&ai_service, chat_arg, thread_arg).await;
            return;
        }
        Some("memory") => {
            let action_arg = args.get(2).map(|s| s.as_str());
            let target_arg = args.get(3).map(|s| s.as_str());
            run_cli_memory(&ai_service, action_arg, target_arg).await;
            return;
        }
        Some("gateway") => {
            let action = cli::gateway::parse_gateway_cli_args(&args[2..]);
            cli::gateway::run_cli_gateway_hub(action).await;
            return;
        }
        Some("search") => {
            let (action_arg, target_buf) = cli::search::parse_search_args(&args[2..]);
            run_cli_search_hub(&ai_service, action_arg, target_buf.as_deref()).await;
            return;
        }
        Some("mcp") => {
            let action_arg = args.get(2).map(|s| s.as_str());
            // A test or search query may be several words.
            let query = (args.len() > 3).then(|| args[3..].join(" "));
            let target_arg = match action_arg {
                Some("test" | "probe" | "check" | "search") => query.as_deref(),
                _ => args.get(3).map(|s| s.as_str()),
            };
            let extra_arg = args.get(4).map(|s| s.as_str());
            run_cli_mcp_hub(&ai_service, action_arg, target_arg, extra_arg).await;
            return;
        }
        Some("chat") => {
            let prompt_arg = if args.len() > 2 {
                Some(args[2..].join(" "))
            } else {
                None
            };
            run_cli_chat(&ai_service, prompt_arg).await;
            return;
        }
        Some("help") | Some("--help") | Some("-h") => {
            print_cli_help();
            return;
        }
        Some("web") => {
            let action_arg = args.get(2).map(|s| s.as_str());
            let target_arg = args.get(3).map(|s| s.as_str());
            web::cli::run_cli_web(action_arg, target_arg).await;
            return;
        }
        Some("start") => {
            let mut service = ai_service;
            loop {
                crate::bot::daemon::run_daemon(Arc::clone(&service)).await;
                if !web::restart_requested() {
                    break;
                }
                if web::under_service_manager() {
                    // A non-zero status makes systemd (Restart=on-failure or
                    // always) start a fresh process, which also picks up a
                    // newly installed binary.
                    std::process::exit(web::RESTART_EXIT_CODE);
                }
                // From a terminal nobody would start it again: restart here.
                web::clear_restart_request();
                tracing::info!("Restarting the daemon");
                service = Arc::new(AIChatService::new());
            }
        }
        Some(unknown) => {
            if unknown.starts_with('-') {
                println!("\x1b[31m✖ Error: Unknown option '{unknown}'. Run 'xiao help' for usage instructions.\x1b[0m");
                std::process::exit(1);
            }
            let prompt = args[1..].join(" ");
            run_cli_chat(&ai_service, Some(prompt)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::storage::ENV_TEST_LOCK;

    #[test]
    fn get_config_path_discovers_profile_or_appdata_env_file() {
        let _lock = ENV_TEST_LOCK.lock().expect("ENV_TEST_LOCK poisoned");
        let orig_home = env::var("HOME").ok();
        let orig_profile = env::var("USERPROFILE").ok();
        let orig_appdata = env::var("APPDATA").ok();
        let orig_xdg_config = env::var("XDG_CONFIG_HOME").ok();

        let temp_dir = env::temp_dir().join(format!(
            "xiaoai-cfg-test-{}-{:x}",
            std::process::id(),
            rand::random::<u64>()
        ));
        let xiao_dir = temp_dir.join("xiao");
        let _ = std::fs::create_dir_all(&xiao_dir);
        let test_env = xiao_dir.join(".env");
        std::fs::write(&test_env, b"TEST_KEY=123\n").expect("write test env succeeds");

        env::set_var("USERPROFILE", &temp_dir);
        env::remove_var("HOME");
        env::remove_var("APPDATA");
        env::remove_var("XDG_CONFIG_HOME");

        // If local .env doesn't exist, it should find temp_dir/xiao/.env via USERPROFILE
        if !Path::new(".env").exists() {
            let found = get_config_path();
            assert_eq!(found, test_env);
        }

        let _ = std::fs::remove_file(&test_env);
        let _ = std::fs::remove_dir_all(&xiao_dir);

        // Also verify discovery in ~/.xiao/.env (with leading dot)
        let dot_xiao_dir = temp_dir.join(".xiao");
        let _ = std::fs::create_dir_all(&dot_xiao_dir);
        let dot_test_env = dot_xiao_dir.join(".env");
        std::fs::write(&dot_test_env, b"TEST_KEY=456\n").expect("write dot test env succeeds");
        if !Path::new(".env").exists() {
            let found = get_config_path();
            assert_eq!(found, dot_test_env);
        }

        let _ = std::fs::remove_file(dot_test_env);
        let _ = std::fs::remove_dir_all(dot_xiao_dir);
        let _ = std::fs::remove_dir_all(temp_dir);

        if let Some(val) = orig_home {
            env::set_var("HOME", val);
        } else {
            env::remove_var("HOME");
        }
        if let Some(val) = orig_profile {
            env::set_var("USERPROFILE", val);
        } else {
            env::remove_var("USERPROFILE");
        }
        if let Some(val) = orig_appdata {
            env::set_var("APPDATA", val);
        } else {
            env::remove_var("APPDATA");
        }
        if let Some(val) = orig_xdg_config {
            env::set_var("XDG_CONFIG_HOME", val);
        } else {
            env::remove_var("XDG_CONFIG_HOME");
        }
    }

    #[test]
    fn empty_environment_values_do_not_hide_saved_settings() {
        let _lock = ENV_TEST_LOCK.lock().expect("ENV_TEST_LOCK poisoned");
        let key = format!("XIAO_TEST_SETTING_{:x}", rand::random::<u64>());

        env::set_var(&key, "   ");
        assert_eq!(configured_setting(&key), None);

        ai::service::save_app_setting(&key, "dari-cli").expect("save setting");
        assert_eq!(
            configured_setting(&key).as_deref(),
            Some("dari-cli"),
            "an empty .env line (KEY=) must not hide the saved value"
        );

        env::set_var(&key, "dari-env");
        assert_eq!(
            configured_setting(&key).as_deref(),
            Some("dari-env"),
            "a real environment value still wins"
        );

        env::remove_var(&key);
        let _ = ai::service::save_app_setting(&key, "");
    }
}
