//! `xiao web`: address, status and sign-in of the Xiao WebUI from the
//! terminal. Setting a password here is the way in when Telegram is not
//! configured yet.

use std::io::{self, BufRead, IsTerminal, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream};
use std::time::Duration;

use super::{assets, auth, net, settings};

fn print_help() {
    println!("\n  \x1b[1;37mxiao web\x1b[0m: Xiao WebUI inside `xiao start`\n");
    println!("    \x1b[1;38;5;45mxiao web\x1b[0m [status]           Address, sign-in methods and whether the console answers");
    println!("    \x1b[1;38;5;45mxiao web password\x1b[0m           Set the backup password (asked twice, not shown)");
    println!("    \x1b[1;38;5;45mxiao web password rm\x1b[0m        Remove the backup password");
    println!("    \x1b[1;38;5;45mxiao web bind\x1b[0m <local|lan|off|ADDR:PORT>  Where the console listens (restart needed)");
    println!("    \x1b[1;38;5;45mxiao web logout-all\x1b[0m         Sign out every browser\n");
    println!("  \x1b[38;5;244mDefault address 127.0.0.1:8787 (this machine only). Use an SSH tunnel from other devices,");
    println!(
        "  or `xiao web bind lan` to allow the local network (XIAO_WEB_ALLOWED_NETWORKS).\x1b[0m\n"
    );
}

fn console_answers(bind: SocketAddr) -> bool {
    let target = if bind.ip().is_unspecified() {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), bind.port())
    } else {
        bind
    };
    TcpStream::connect_timeout(&target, Duration::from_secs(1)).is_ok()
}

fn yes_no(value: bool) -> &'static str {
    if value {
        "\x1b[32myes\x1b[0m"
    } else {
        "\x1b[38;5;244mno\x1b[0m"
    }
}

fn print_status() {
    let raw = settings::effective("XIAO_WEB_BIND");
    println!("\n  \x1b[1;37mXiao WebUI\x1b[0m");
    match net::parse_bind(&raw) {
        Ok(None) => println!("    Address      : off (XIAO_WEB_BIND=off)"),
        Ok(Some(bind)) => {
            println!("    Address      : {bind}");
            let port = bind.port();
            if bind.ip().is_loopback() {
                println!("    Open         : http://127.0.0.1:{port} on this machine");
                if let Some(lan) = net::primary_lan_ip() {
                    println!("    From a phone : ssh -L {port}:127.0.0.1:{port} <user>@{lan}, then http://127.0.0.1:{port}");
                }
            } else {
                let host = net::primary_lan_ip()
                    .map_or_else(|| "<server-ip>".to_string(), |ip| ip.to_string());
                println!("    Open         : http://{host}:{port}");
                println!(
                    "    Allowed      : loopback, {}",
                    settings::effective("XIAO_WEB_ALLOWED_NETWORKS")
                );
            }
            println!("    Running      : {}", yes_no(console_answers(bind)));
        }
        Err(error) => println!("    Address      : \x1b[31minvalid ({error})\x1b[0m"),
    }
    println!(
        "    Telegram code: {}",
        yes_no(auth::telegram_login_available())
    );
    println!("    Password     : {}", yes_no(auth::password_is_set()));
    if !assets::WEBUI_BUILT {
        println!("    \x1b[33mThis binary was built without the WebUI files.\x1b[0m");
    }
    if !auth::telegram_login_available() && !auth::password_is_set() {
        println!("\n  \x1b[33mNo sign-in method yet: open the WebUI and enter the setup code that `xiao start` prints (also in the journal), or run `xiao web password`.\x1b[0m");
    }
    println!();
}

/// Reads a line without echoing it when stdin is a terminal.
fn read_secret_line(prompt: &str) -> Option<String> {
    use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
    print!("{prompt}");
    let _ = io::stdout().flush();
    if !io::stdin().is_terminal() || crossterm::terminal::enable_raw_mode().is_err() {
        let mut line = String::new();
        return match io::stdin().lock().read_line(&mut line) {
            Ok(0) | Err(_) => None,
            Ok(_) => Some(line.trim_end_matches(['\r', '\n']).to_string()),
        };
    }
    let mut value = String::new();
    let result = loop {
        match event::read() {
            Ok(Event::Key(KeyEvent {
                code,
                modifiers,
                kind,
                ..
            })) if kind != KeyEventKind::Release => match code {
                KeyCode::Enter => break Some(value),
                KeyCode::Esc => break None,
                KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => break None,
                KeyCode::Backspace => {
                    value.pop();
                }
                KeyCode::Char(ch) => value.push(ch),
                _ => {}
            },
            Ok(_) => {}
            Err(_) => break None,
        }
    };
    let _ = crossterm::terminal::disable_raw_mode();
    println!();
    result
}

fn set_password() {
    if settings::env_value("XIAO_WEB_PASSWORD").is_some() {
        println!("\n  \x1b[31m✖ XIAO_WEB_PASSWORD is set in the environment, which wins. Change it there.\x1b[0m\n");
        std::process::exit(1);
    }
    let Some(first) = read_secret_line("  New WebUI password (min 8 characters): ") else {
        println!("  Cancelled.\n");
        return;
    };
    if first.chars().count() < auth::MIN_PASSWORD_CHARS {
        println!(
            "  \x1b[31m✖ The password needs at least {} characters.\x1b[0m\n",
            auth::MIN_PASSWORD_CHARS
        );
        std::process::exit(1);
    }
    if io::stdin().is_terminal() {
        let Some(second) = read_secret_line("  Repeat it: ") else {
            println!("  Cancelled.\n");
            return;
        };
        if second != first {
            println!("  \x1b[31m✖ The two passwords differ.\x1b[0m\n");
            std::process::exit(1);
        }
    }
    let result = auth::hash_password(&first).and_then(|hash| {
        crate::ai::service::save_app_setting("XIAO_WEB_PASSWORD", &hash)
            .map_err(|error| error.to_string())
    });
    match result {
        Ok(()) => println!("  \x1b[1;32m✔ WebUI password saved (argon2 hash in the vault). It works at once.\x1b[0m\n"),
        Err(error) => {
            println!("  \x1b[31m✖ The password could not be saved: {error}\x1b[0m\n");
            std::process::exit(1);
        }
    }
}

fn remove_password() {
    if !auth::telegram_login_available() {
        println!("\n  \x1b[33m⚠ Telegram code sign-in is off or not set up; without a password nobody could sign in. Kept.\x1b[0m\n");
        std::process::exit(1);
    }
    match crate::ai::service::save_app_setting("XIAO_WEB_PASSWORD", "") {
        Ok(()) => println!("\n  \x1b[1;32m✔ WebUI password removed.\x1b[0m\n"),
        Err(error) => {
            println!("\n  \x1b[31m✖ {error}\x1b[0m\n");
            std::process::exit(1);
        }
    }
}

fn set_bind(target: Option<&str>) {
    let Some(target) = target.map(str::trim).filter(|target| !target.is_empty()) else {
        print_help();
        return;
    };
    let port = net::parse_bind(&settings::effective("XIAO_WEB_BIND"))
        .ok()
        .flatten()
        .map_or(8787, |bind| bind.port());
    let value = match target {
        "local" => format!("127.0.0.1:{port}"),
        "lan" => format!("0.0.0.0:{port}"),
        other => other.to_string(),
    };
    let normalized = match settings::normalize("XIAO_WEB_BIND", &value) {
        Ok(normalized) => normalized,
        Err(_) => {
            println!("\n  \x1b[31m✖ '{value}' is not an address such as 127.0.0.1:8787 (port 1024-65535), local, lan or off.\x1b[0m\n");
            std::process::exit(1);
        }
    };
    match crate::save_env_kv("XIAO_WEB_BIND", &normalized) {
        Ok(()) => println!("\n  \x1b[1;32m✔ XIAO_WEB_BIND = {normalized}. Restart the daemon to apply it.\x1b[0m\n"),
        Err(error) => {
            println!("\n  \x1b[31m✖ {error}\x1b[0m\n");
            std::process::exit(1);
        }
    }
}

pub(crate) async fn run_cli_web(action: Option<&str>, target: Option<&str>) {
    crate::load_environment();
    match action {
        None | Some("status") => print_status(),
        Some("password") => match target {
            Some("rm" | "remove" | "off" | "delete") => remove_password(),
            _ => set_password(),
        },
        Some("bind") => set_bind(target),
        Some("logout-all") => {
            if crate::ai::storage::web::delete_web_sessions_except_async(None).await {
                println!("\n  \x1b[1;32m✔ Every browser was signed out.\x1b[0m\n");
            } else {
                println!("\n  \x1b[31m✖ The sessions could not be removed.\x1b[0m\n");
                std::process::exit(1);
            }
        }
        Some("help" | "-h" | "--help") => print_help(),
        Some(other) => {
            println!("\n  \x1b[31m✖ Unknown action '{other}'.\x1b[0m");
            print_help();
            std::process::exit(1);
        }
    }
}
