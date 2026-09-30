//! Settings the WebUI may change: validation, where each value comes from,
//! environment locks, secret summaries and restart tracking.
//!
//! Values are saved with `save_app_setting` (credentials go to the vault).
//! The environment always wins over a saved value, so a key set there is
//! shown as locked instead of being silently ignored after a save.

use std::collections::BTreeMap;

use serde::Serialize;

use super::auth::sha256;
use super::error::ApiError;
use super::net;

/// When a change takes effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Effect {
    Live,
    Restart,
    Gateway,
    Locked,
    Readonly,
}

/// Where the effective value comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ValueSource {
    Environment,
    Vault,
    Database,
    Default,
    None,
}

pub(crate) struct SettingSpec {
    pub key: &'static str,
    pub effect: Effect,
    pub default: &'static str,
}

/// Plain settings `PUT /api/settings` accepts, in display order.
pub(crate) const SETTINGS: &[SettingSpec] = &[
    spec("OWNER_USER_ID", Effect::Restart, ""),
    spec("ALLOWED_CHAT_IDS", Effect::Restart, ""),
    spec("DEDICATED_CHAT_IDS", Effect::Restart, ""),
    spec("WHATSAPP_ENABLED", Effect::Gateway, "false"),
    spec("WHATSAPP_OWNER_NUMBER", Effect::Gateway, ""),
    spec("WHATSAPP_DEDICATED_GROUPS", Effect::Gateway, ""),
    spec("AI_PROVIDER_CONNECT_TIMEOUT_SECS", Effect::Restart, "10"),
    spec("IMAGE_FALLBACK_PROVIDER", Effect::Live, "none"),
    spec("IMAGE_GENERATION_TIMEOUT_SECS", Effect::Live, "120"),
    spec("IMAGE_PROVIDER_CONNECT_TIMEOUT_SECS", Effect::Live, "10"),
    spec("IMAGE_DOWNLOAD_TIMEOUT_SECS", Effect::Live, "30"),
    spec("XIAO_HISTORY_RETENTION", Effect::Live, "2000"),
    spec("XIAO_WEB_BIND", Effect::Restart, net::DEFAULT_BIND),
    spec(
        "XIAO_WEB_ALLOWED_NETWORKS",
        Effect::Restart,
        net::DEFAULT_ALLOWED_NETWORKS,
    ),
    spec("XIAO_WEB_SESSION_DAYS", Effect::Live, "7"),
    spec("XIAO_WEB_TELEGRAM_LOGIN", Effect::Live, "true"),
];

const fn spec(key: &'static str, effect: Effect, default: &'static str) -> SettingSpec {
    SettingSpec {
        key,
        effect,
        default,
    }
}

pub(crate) fn find_spec(key: &str) -> Option<&'static SettingSpec> {
    SETTINGS.iter().find(|spec| spec.key == key)
}

/// Secrets `PUT /api/secrets/:key` accepts besides provider keys, with the
/// legacy names that are read as the same credential.
pub(crate) const SECRETS: &[(&str, &[&str], Effect)] = &[
    ("BOT_TOKEN", &["BOT_TOKEN"], Effect::Restart),
    ("BRAVE_API_KEY", &["BRAVE_API_KEY"], Effect::Live),
    (
        "TAVILY_API_KEY",
        &["TAVILY_API_KEY", "TAVILY_KEY"],
        Effect::Live,
    ),
    ("EXA_API_KEY", &["EXA_API_KEY", "EXA_KEY"], Effect::Live),
    ("XIAO_WEB_PASSWORD", &["XIAO_WEB_PASSWORD"], Effect::Live),
];

pub(crate) fn secret_aliases(key: &str) -> Option<&'static [&'static str]> {
    SECRETS
        .iter()
        .find(|(name, _, _)| *name == key)
        .map(|(_, aliases, _)| *aliases)
}

/// Settings that are only read at startup; saving them needs a restart.
pub(crate) const RESTART_KEYS: &[&str] = &[
    "OWNER_USER_ID",
    "ALLOWED_CHAT_IDS",
    "DEDICATED_CHAT_IDS",
    "BOT_TOKEN",
    "AI_PROVIDER_CONNECT_TIMEOUT_SECS",
    "XIAO_WEB_BIND",
    "XIAO_WEB_ALLOWED_NETWORKS",
];

/// The environment value of `key` when it overrides the saved one. Empty
/// values and `YOUR_…` placeholders do not count, like everywhere else.
pub(crate) fn env_value(key: &str) -> Option<String> {
    crate::load_environment();
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty() && !value.contains("YOUR_"))
}

/// Human description of where an environment value is set.
fn env_origin(key: &str) -> String {
    let path = crate::get_config_path();
    let declared = std::fs::read_to_string(&path).is_ok_and(|content| {
        content.lines().any(|line| {
            let line = line.trim_start();
            let line = line.strip_prefix("export ").unwrap_or(line).trim_start();
            line.strip_prefix(key)
                .is_some_and(|rest| rest.trim_start().starts_with('='))
        })
    });
    if declared {
        format!("{key} in {}", path.display())
    } else {
        format!("{key} in the process environment (for example Environment= in the service)")
    }
}

/// Keys among `keys` whose environment value wins, with where it is set.
pub(crate) fn env_locks<'a>(keys: impl IntoIterator<Item = &'a str>) -> BTreeMap<String, String> {
    keys.into_iter()
        .filter(|key| env_value(key).is_some())
        .map(|key| (key.to_string(), env_origin(key)))
        .collect()
}

/// Every key the WebUI shows, for the pages that list locks.
pub(crate) fn all_env_locks() -> BTreeMap<String, String> {
    env_locks(
        SETTINGS
            .iter()
            .map(|spec| spec.key)
            .chain(
                SECRETS
                    .iter()
                    .flat_map(|(_, aliases, _)| aliases.iter().copied()),
            )
            .chain(["EXA_MCP_URL", "RUST_LOG"]),
    )
}

/// Saved (database or vault) value, trimmed; `None` when empty.
pub(crate) fn saved_value(key: &str) -> Option<String> {
    crate::ai::service::load_app_setting(key)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Effective value of a plain setting: environment, then saved, then default.
pub(crate) fn effective(key: &str) -> String {
    crate::configured_setting(key)
        .or_else(|| find_spec(key).map(|spec| spec.default.to_string()))
        .unwrap_or_default()
}

pub(crate) fn source_of(key: &str, secret: bool) -> ValueSource {
    if env_value(key).is_some() {
        ValueSource::Environment
    } else if saved_value(key).is_some() {
        if secret {
            ValueSource::Vault
        } else {
            ValueSource::Database
        }
    } else if find_spec(key).is_some_and(|spec| !spec.default.is_empty()) {
        ValueSource::Default
    } else {
        ValueSource::None
    }
}

/// What the browser learns about a secret: never the value itself.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct SecretMeta {
    pub set: bool,
    pub tail: String,
    #[serde(rename = "where")]
    pub location: &'static str,
}

impl SecretMeta {
    pub(crate) fn unset() -> Self {
        Self {
            set: false,
            tail: String::new(),
            location: "none",
        }
    }

    /// Summary of a known value stored in the vault.
    pub(crate) fn stored(value: &str, show_tail: bool) -> Self {
        let value = value.trim();
        if value.is_empty()
            || ["none", "-", "no", "null"].contains(&value.to_ascii_lowercase().as_str())
        {
            return Self::unset();
        }
        Self {
            set: true,
            tail: if show_tail {
                tail(value)
            } else {
                String::new()
            },
            location: "vault",
        }
    }
}

/// Last four characters of a long secret; nothing of a short one.
fn tail(value: &str) -> String {
    let count = value.chars().count();
    if count < 12 {
        return String::new();
    }
    value.chars().skip(count - 4).collect()
}

/// Summary of one of the named secrets in [`SECRETS`].
pub(crate) fn secret_meta(key: &str) -> SecretMeta {
    let aliases = secret_aliases(key).unwrap_or(&[]);
    let show_tail = key != "XIAO_WEB_PASSWORD";
    if let Some(value) = aliases.iter().find_map(|alias| env_value(alias)) {
        return SecretMeta {
            set: true,
            tail: if show_tail {
                tail(&value)
            } else {
                String::new()
            },
            location: "environment",
        };
    }
    aliases
        .iter()
        .find_map(|alias| saved_value(alias))
        .map(|value| SecretMeta::stored(&value, show_tail))
        .unwrap_or_else(SecretMeta::unset)
}

fn parse_bool(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Some(true),
        "false" | "0" | "no" | "off" | "" => Some(false),
        _ => None,
    }
}

/// Boolean setting with the usual spellings; anything else is false.
pub(crate) fn effective_bool(key: &str) -> bool {
    parse_bool(&effective(key)).unwrap_or(false)
}

fn invalid(key: &str, en: &str, id: &str) -> ApiError {
    ApiError::invalid(format!("{key}: {en}"), format!("{key}: {id}"))
}

fn integer_in(key: &str, raw: &str, min: u64, max: u64) -> Result<String, ApiError> {
    raw.trim()
        .parse::<u64>()
        .ok()
        .filter(|value| (min..=max).contains(value))
        .map(|value| value.to_string())
        .ok_or_else(|| {
            invalid(
                key,
                &format!("must be a whole number from {min} to {max}"),
                &format!("harus bilangan bulat {min} sampai {max}"),
            )
        })
}

fn chat_id_list(key: &str, raw: &str) -> Result<String, ApiError> {
    let mut ids = Vec::new();
    for part in raw
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
    {
        let id = part.parse::<i64>().map_err(|_| {
            invalid(
                key,
                &format!("'{part}' is not a chat id"),
                &format!("'{part}' bukan ID chat"),
            )
        })?;
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    Ok(ids
        .iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(", "))
}

/// Checks and normalizes a value for `key`. Unknown keys are rejected.
pub(crate) fn normalize(key: &str, raw: &str) -> Result<String, ApiError> {
    let value = raw.trim();
    match key {
        "OWNER_USER_ID" => value
            .parse::<i64>()
            .ok()
            .filter(|id| *id > 0)
            .map(|id| id.to_string())
            .ok_or_else(|| invalid(key, "must be a positive number", "harus angka positif")),
        "ALLOWED_CHAT_IDS" | "DEDICATED_CHAT_IDS" => chat_id_list(key, value),
        "AI_PROVIDER_CONNECT_TIMEOUT_SECS"
        | "IMAGE_GENERATION_TIMEOUT_SECS"
        | "IMAGE_PROVIDER_CONNECT_TIMEOUT_SECS"
        | "IMAGE_DOWNLOAD_TIMEOUT_SECS" => integer_in(key, value, 1, 600),
        "XIAO_HISTORY_RETENTION" => integer_in(key, value, 0, 1_000_000),
        "IMAGE_FALLBACK_PROVIDER" => match value.to_ascii_lowercase().as_str() {
            "none" | "" => Ok("none".to_string()),
            "pollinations" => Ok("pollinations".to_string()),
            _ => Err(invalid(
                key,
                "must be none or pollinations",
                "harus none atau pollinations",
            )),
        },
        "WHATSAPP_ENABLED" | "XIAO_WEB_TELEGRAM_LOGIN" => parse_bool(value)
            .map(|flag| flag.to_string())
            .ok_or_else(|| invalid(key, "must be true or false", "harus true atau false")),
        "WHATSAPP_OWNER_NUMBER" => {
            let digits: String = value.chars().filter(char::is_ascii_digit).collect();
            if digits.is_empty() || (8..=15).contains(&digits.len()) {
                Ok(digits)
            } else {
                Err(invalid(
                    key,
                    "needs 8 to 15 digits in international format",
                    "perlu 8 sampai 15 digit dalam format internasional",
                ))
            }
        }
        "WHATSAPP_DEDICATED_GROUPS" => {
            let mut groups = Vec::new();
            for part in value
                .split(',')
                .map(str::trim)
                .filter(|part| !part.is_empty())
            {
                let valid = part.chars().all(|ch| {
                    ch.is_ascii_alphanumeric() || matches!(ch, '@' | '.' | '-' | '_' | ':')
                });
                if !valid {
                    return Err(invalid(
                        key,
                        &format!("'{part}' is not a group id"),
                        &format!("'{part}' bukan ID grup"),
                    ));
                }
                groups.push(part.to_string());
            }
            Ok(groups.join(","))
        }
        "XIAO_WEB_BIND" => match net::parse_bind(value) {
            Ok(Some(addr)) => Ok(addr.to_string()),
            Ok(None) => Ok("off".to_string()),
            Err(error) => Err(invalid(
                key,
                &error,
                "alamat atau port tidak valid (port 1024-65535)",
            )),
        },
        "XIAO_WEB_ALLOWED_NETWORKS" => net::parse_networks(value)
            .map(|_| {
                value
                    .split(',')
                    .map(str::trim)
                    .filter(|part| !part.is_empty())
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .map_err(|error| invalid(key, &error, "daftar jaringan tidak valid")),
        "XIAO_WEB_SESSION_DAYS" => match value {
            "1" | "7" | "30" => Ok(value.to_string()),
            _ => Err(invalid(key, "must be 1, 7 or 30", "harus 1, 7, atau 30")),
        },
        _ => Err(ApiError::invalid(
            format!("{key} cannot be changed here"),
            format!("{key} tidak bisa diubah di sini"),
        )),
    }
}

/// Remembers the startup value of every restart-only setting (as a hash, so
/// no extra copy of the bot token is kept) to tell which changes still wait
/// for a restart.
pub(crate) struct RestartTracker {
    snapshot: Vec<(&'static str, Option<[u8; 32]>)>,
}

fn restart_fingerprint(key: &str) -> Option<[u8; 32]> {
    let value = if key == "BOT_TOKEN" {
        crate::get_configured_token()
    } else {
        crate::configured_setting(key)
    };
    value.map(|value| sha256(value.as_bytes()))
}

impl RestartTracker {
    pub(crate) fn capture() -> Self {
        Self {
            snapshot: RESTART_KEYS
                .iter()
                .map(|key| (*key, restart_fingerprint(key)))
                .collect(),
        }
    }

    /// Restart-only settings whose value changed since startup.
    pub(crate) fn pending(&self) -> Vec<&'static str> {
        self.snapshot
            .iter()
            .filter(|(key, at_start)| restart_fingerprint(key) != *at_start)
            .map(|(key, _)| *key)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_are_normalized_or_rejected() {
        assert_eq!(
            normalize("ALLOWED_CHAT_IDS", " -100, 42 ,-100,").expect("list"),
            "-100, 42"
        );
        assert!(normalize("ALLOWED_CHAT_IDS", "abc").is_err());
        assert_eq!(
            normalize("OWNER_USER_ID", " 6120045871 ").expect("id"),
            "6120045871"
        );
        assert!(normalize("OWNER_USER_ID", "-5").is_err());
        assert!(normalize("IMAGE_GENERATION_TIMEOUT_SECS", "601").is_err());
        assert_eq!(normalize("XIAO_HISTORY_RETENTION", "0").expect("zero"), "0");
        assert_eq!(normalize("WHATSAPP_ENABLED", "yes").expect("bool"), "true");
        assert_eq!(
            normalize("WHATSAPP_OWNER_NUMBER", "+62 812-3456-7890").expect("digits"),
            "6281234567890"
        );
        assert!(normalize("WHATSAPP_OWNER_NUMBER", "12").is_err());
        assert_eq!(
            normalize("XIAO_WEB_BIND", "0.0.0.0:9000").expect("bind"),
            "0.0.0.0:9000"
        );
        assert_eq!(normalize("XIAO_WEB_BIND", "OFF").expect("off"), "off");
        assert!(normalize("XIAO_WEB_BIND", "0.0.0.0:22").is_err());
        assert!(normalize("XIAO_WEB_ALLOWED_NETWORKS", "10.0.0.0/8,bad").is_err());
        assert!(normalize("XIAO_WEB_SESSION_DAYS", "3").is_err());
        assert!(
            normalize("BOT_TOKEN", "x").is_err(),
            "secrets have their own endpoint"
        );
    }

    #[test]
    fn secret_tails_never_reveal_short_values() {
        assert_eq!(tail("short"), "");
        assert_eq!(tail("123456:ABCDEFGHIJKLMNOP"), "MNOP");
        assert!(!SecretMeta::stored("none", true).set);
        assert!(SecretMeta::stored("sk-abcdefghijklmnop", false)
            .tail
            .is_empty());
    }
}
