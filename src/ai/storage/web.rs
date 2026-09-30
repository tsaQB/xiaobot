//! Storage used by the WebUI: signed-in browser sessions, queue statistics
//! and quarantine actions, conversation scopes, memories with their dates,
//! session renames and database backups.
//!
//! Browser sessions keep only a SHA-256 hash of their token, never the token.

use chrono::Local;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::collections::HashMap;

use super::{open_session_db, run_db};

/// Created with the rest of the schema in `ensure_database_initialized`.
pub(crate) const WEB_SCHEMA: &str = "CREATE TABLE IF NOT EXISTS web_sessions (
        id TEXT PRIMARY KEY,
        token_hash TEXT NOT NULL UNIQUE,
        created_at INTEGER NOT NULL,
        last_seen INTEGER NOT NULL,
        expires_at INTEGER NOT NULL,
        ip TEXT NOT NULL,
        user_agent TEXT NOT NULL
    );";

fn with_conn<T>(f: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> rusqlite::Result<T> {
    let conn = open_session_db()?;
    f(&conn)
}

/* ---------------------------- web sessions ---------------------------- */

#[derive(Debug, Clone)]
pub(crate) struct WebSessionRow {
    pub id: String,
    pub created_at: i64,
    pub last_seen: i64,
    pub expires_at: i64,
    pub ip: String,
    pub user_agent: String,
}

#[derive(Debug, Clone)]
pub(crate) struct NewWebSession {
    pub id: String,
    pub token_hash: String,
    pub now: i64,
    pub expires_at: i64,
    pub ip: String,
    pub user_agent: String,
}

fn map_session(row: &rusqlite::Row<'_>) -> rusqlite::Result<WebSessionRow> {
    Ok(WebSessionRow {
        id: row.get(0)?,
        created_at: row.get(1)?,
        last_seen: row.get(2)?,
        expires_at: row.get(3)?,
        ip: row.get(4)?,
        user_agent: row.get(5)?,
    })
}

fn insert_web_session_on_conn(conn: &Connection, session: &NewWebSession) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM web_sessions WHERE expires_at <= ?1",
        params![session.now],
    )?;
    conn.execute(
        "INSERT INTO web_sessions(id,token_hash,created_at,last_seen,expires_at,ip,user_agent)
         VALUES(?1,?2,?3,?3,?4,?5,?6)",
        params![
            session.id,
            session.token_hash,
            session.now,
            session.expires_at,
            session.ip,
            session.user_agent
        ],
    )?;
    Ok(())
}

fn find_web_session_on_conn(
    conn: &Connection,
    token_hash: &str,
    now: i64,
) -> rusqlite::Result<Option<WebSessionRow>> {
    conn.query_row(
        "SELECT id,created_at,last_seen,expires_at,ip,user_agent FROM web_sessions
         WHERE token_hash=?1 AND expires_at>?2",
        params![token_hash, now],
        map_session,
    )
    .optional()
}

fn list_web_sessions_on_conn(conn: &Connection, now: i64) -> rusqlite::Result<Vec<WebSessionRow>> {
    let mut stmt = conn.prepare(
        "SELECT id,created_at,last_seen,expires_at,ip,user_agent FROM web_sessions
         WHERE expires_at>?1 ORDER BY last_seen DESC",
    )?;
    let rows = stmt.query_map(params![now], map_session)?;
    rows.collect()
}

pub(crate) async fn create_web_session_async(session: NewWebSession) -> bool {
    run_db("create_web_session", move || {
        with_conn(|conn| insert_web_session_on_conn(conn, &session))
    })
    .await
    .is_some()
}

pub(crate) async fn find_web_session_async(token_hash: String, now: i64) -> Option<WebSessionRow> {
    run_db("find_web_session", move || {
        with_conn(|conn| find_web_session_on_conn(conn, &token_hash, now))
    })
    .await
    .flatten()
}

pub(crate) async fn touch_web_session_async(id: String, now: i64) -> bool {
    run_db("touch_web_session", move || {
        with_conn(|conn| {
            conn.execute(
                "UPDATE web_sessions SET last_seen=?2 WHERE id=?1",
                params![id, now],
            )
        })
    })
    .await
    .is_some()
}

pub(crate) async fn list_web_sessions_async(now: i64) -> Vec<WebSessionRow> {
    run_db("list_web_sessions", move || {
        with_conn(|conn| list_web_sessions_on_conn(conn, now))
    })
    .await
    .unwrap_or_default()
}

/// Deletes one session; true when it existed.
pub(crate) async fn delete_web_session_async(id: String) -> bool {
    run_db("delete_web_session", move || {
        with_conn(|conn| conn.execute("DELETE FROM web_sessions WHERE id=?1", params![id]))
    })
    .await
    .is_some_and(|deleted| deleted > 0)
}

/// Deletes every session except `keep` (all of them when `keep` is `None`).
pub(crate) async fn delete_web_sessions_except_async(keep: Option<String>) -> bool {
    run_db("delete_web_sessions", move || {
        with_conn(|conn| match keep {
            Some(keep) => conn.execute("DELETE FROM web_sessions WHERE id<>?1", params![keep]),
            None => conn.execute("DELETE FROM web_sessions", []),
        })
    })
    .await
    .is_some()
}

/* ------------------------------- queues ------------------------------- */

#[derive(Debug, Clone, Copy, Default, Serialize)]
pub(crate) struct QueueCounts {
    pub pending: u64,
    pub processing: u64,
    pub completed: u64,
    pub failed: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Inbox {
    Telegram,
    WhatsApp,
}

impl Inbox {
    fn table(self) -> &'static str {
        match self {
            Self::Telegram => "telegram_inbox",
            Self::WhatsApp => "whatsapp_inbox",
        }
    }
}

fn queue_counts_on_conn(conn: &Connection, inbox: Inbox) -> rusqlite::Result<QueueCounts> {
    let mut stmt = conn.prepare(&format!(
        "SELECT status, COUNT(*) FROM {} GROUP BY status",
        inbox.table()
    ))?;
    let mut counts = QueueCounts::default();
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;
    for row in rows {
        let (status, count) = row?;
        let count = u64::try_from(count).unwrap_or_default();
        match status.as_str() {
            "pending" => counts.pending = count,
            "processing" => counts.processing = count,
            "completed" => counts.completed = count,
            "failed" => counts.failed = count,
            _ => {}
        }
    }
    Ok(counts)
}

pub(crate) async fn queue_counts_async(inbox: Inbox) -> QueueCounts {
    run_db("queue_counts", move || {
        with_conn(|conn| queue_counts_on_conn(conn, inbox))
    })
    .await
    .unwrap_or_default()
}

#[derive(Debug, Clone)]
pub(crate) struct FailedInboxRow {
    /// `update_id` or WhatsApp message key.
    pub id: String,
    /// Telegram update JSON; empty for WhatsApp.
    pub payload_json: String,
    /// WhatsApp chat id; `None` for Telegram (read from the payload instead).
    pub chat_id: Option<i64>,
    pub attempts: i64,
    pub received_at: String,
    pub last_error: Option<String>,
}

fn failed_rows_on_conn(conn: &Connection, limit: usize) -> rusqlite::Result<Vec<FailedInboxRow>> {
    let mut rows = Vec::new();
    let mut stmt = conn.prepare(
        "SELECT update_id,payload_json,attempts,received_at,last_error FROM telegram_inbox
         WHERE status='failed' ORDER BY update_id DESC LIMIT ?1",
    )?;
    let telegram = stmt.query_map(params![limit as i64], |row| {
        Ok(FailedInboxRow {
            id: row.get::<_, i64>(0)?.to_string(),
            payload_json: row.get(1)?,
            chat_id: None,
            attempts: row.get(2)?,
            received_at: row.get(3)?,
            last_error: row.get(4)?,
        })
    })?;
    for row in telegram {
        rows.push(row?);
    }
    let mut stmt = conn.prepare(
        "SELECT message_key,chat_id,attempts,received_at,last_error FROM whatsapp_inbox
         WHERE status='failed' ORDER BY received_at DESC LIMIT ?1",
    )?;
    let whatsapp = stmt.query_map(params![limit as i64], |row| {
        Ok(FailedInboxRow {
            id: row.get(0)?,
            payload_json: String::new(),
            chat_id: Some(row.get(1)?),
            attempts: row.get(2)?,
            received_at: row.get(3)?,
            last_error: row.get(4)?,
        })
    })?;
    for row in whatsapp {
        rows.push(row?);
    }
    Ok(rows)
}

pub(crate) async fn failed_rows_async(limit: usize) -> Vec<FailedInboxRow> {
    run_db("failed_inbox_rows", move || {
        with_conn(|conn| failed_rows_on_conn(conn, limit))
    })
    .await
    .unwrap_or_default()
}

/// Puts a quarantined Telegram update back in the queue with its attempts
/// reset, returning its payload so it can be dispatched right away.
fn retry_telegram_on_conn(conn: &Connection, update_id: i64) -> rusqlite::Result<Option<String>> {
    let updated = conn.execute(
        "UPDATE telegram_inbox SET status='pending',attempts=0,last_error=NULL
         WHERE update_id=?1 AND status='failed'",
        params![update_id],
    )?;
    if updated == 0 {
        return Ok(None);
    }
    conn.query_row(
        "SELECT payload_json FROM telegram_inbox WHERE update_id=?1",
        params![update_id],
        |row| row.get(0),
    )
    .optional()
}

pub(crate) async fn retry_telegram_async(update_id: i64) -> Option<String> {
    run_db("retry_telegram_update", move || {
        with_conn(|conn| retry_telegram_on_conn(conn, update_id))
    })
    .await
    .flatten()
}

pub(crate) async fn retry_whatsapp_async(message_key: String) -> bool {
    run_db("retry_whatsapp_message", move || {
        with_conn(|conn| {
            conn.execute(
                "UPDATE whatsapp_inbox SET status='pending',attempts=0,last_error=NULL
                 WHERE message_key=?1 AND status='failed'",
                params![message_key],
            )
        })
    })
    .await
    .is_some_and(|updated| updated > 0)
}

/// Marks a quarantined row as handled and drops its payload, like a
/// completed row.
fn dismiss_on_conn(conn: &Connection, inbox: Inbox, id: &str) -> rusqlite::Result<bool> {
    let updated = match inbox {
        Inbox::Telegram => {
            let Ok(update_id) = id.parse::<i64>() else {
                return Ok(false);
            };
            let scrubbed = serde_json::json!({
                "update_id": update_id,
                "payload": "redacted_after_completion"
            })
            .to_string();
            conn.execute(
                "UPDATE telegram_inbox SET status='completed',payload_json=?2,last_error=NULL
                 WHERE update_id=?1 AND status='failed'",
                params![update_id, scrubbed],
            )?
        }
        Inbox::WhatsApp => {
            let scrubbed = serde_json::json!({
                "message_key": id,
                "payload": "redacted_after_completion"
            })
            .to_string();
            conn.execute(
                "UPDATE whatsapp_inbox SET status='completed',payload_json=?2,last_error=NULL
                 WHERE message_key=?1 AND status='failed'",
                params![id, scrubbed],
            )?
        }
    };
    Ok(updated > 0)
}

pub(crate) async fn dismiss_async(inbox: Inbox, id: String) -> bool {
    run_db("dismiss_inbox_row", move || {
        with_conn(|conn| dismiss_on_conn(conn, inbox, &id))
    })
    .await
    .unwrap_or(false)
}

/* --------------------------- conversations ---------------------------- */

#[derive(Debug, Clone)]
pub(crate) struct ScopeRow {
    pub chat_id: i64,
    pub thread_id: i64,
    pub messages: u64,
    pub last_at: Option<String>,
}

fn list_scopes_on_conn(conn: &Connection, limit: usize) -> rusqlite::Result<Vec<ScopeRow>> {
    let mut stmt = conn.prepare(
        "SELECT chat_id, thread_id, COUNT(*), MAX(created_at) FROM messages
         GROUP BY chat_id, thread_id ORDER BY MAX(rowid) DESC LIMIT ?1",
    )?;
    let rows = stmt.query_map(params![limit as i64], |row| {
        Ok(ScopeRow {
            chat_id: row.get(0)?,
            thread_id: row.get(1)?,
            messages: u64::try_from(row.get::<_, i64>(2)?).unwrap_or_default(),
            last_at: row.get(3)?,
        })
    })?;
    rows.collect()
}

/// Conversations (chat and topic scopes) by most recent activity.
pub(crate) async fn list_scopes_async(limit: usize) -> Vec<ScopeRow> {
    run_db("list_scopes", move || {
        with_conn(|conn| list_scopes_on_conn(conn, limit))
    })
    .await
    .unwrap_or_default()
}

/// Message count and last activity of every terminal/web chat session of
/// `owner` (negative thread ids), keyed by thread id.
pub(crate) async fn cli_scope_stats_async(owner: i64) -> HashMap<i64, (u64, Option<String>)> {
    run_db("cli_scope_stats", move || {
        with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT thread_id, COUNT(*), MAX(created_at) FROM messages
                 WHERE chat_id=?1 AND thread_id<0 GROUP BY thread_id",
            )?;
            let rows = stmt.query_map(params![owner], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    (
                        u64::try_from(row.get::<_, i64>(1)?).unwrap_or_default(),
                        row.get::<_, Option<String>>(2)?,
                    ),
                ))
            })?;
            rows.collect()
        })
    })
    .await
    .unwrap_or_default()
}

/// Summary text and its update time.
pub(crate) async fn summary_async(chat_id: i64, thread_id: i64) -> Option<(String, String)> {
    run_db("scope_summary", move || {
        with_conn(|conn| {
            conn.query_row(
                "SELECT summary, updated_at FROM scoped_summaries WHERE chat_id=?1 AND thread_id=?2",
                params![chat_id, thread_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
        })
    })
    .await
    .flatten()
}

pub(crate) async fn delete_summary_async(chat_id: i64, thread_id: i64) -> bool {
    run_db("delete_scope_summary", move || {
        with_conn(|conn| {
            conn.execute(
                "DELETE FROM scoped_summaries WHERE chat_id=?1 AND thread_id=?2",
                params![chat_id, thread_id],
            )
        })
    })
    .await
    .is_some()
}

/// Group chats (negative ids) that appear in the stored history.
pub(crate) async fn group_chat_ids_async(limit: usize) -> Vec<i64> {
    run_db("group_chat_ids", move || {
        with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT chat_id FROM messages WHERE chat_id<0
                 GROUP BY chat_id ORDER BY MAX(rowid) DESC LIMIT ?1",
            )?;
            let rows = stmt.query_map(params![limit as i64], |row| row.get(0))?;
            rows.collect()
        })
    })
    .await
    .unwrap_or_default()
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct StorageStats {
    pub messages: u64,
    pub conversations: u64,
    pub memories: u64,
}

pub(crate) async fn storage_stats_async(owner: i64) -> StorageStats {
    run_db("storage_stats", move || {
        with_conn(|conn| {
            let count = |sql: &str, owner: Option<i64>| -> rusqlite::Result<u64> {
                let value: i64 = match owner {
                    Some(owner) => conn.query_row(sql, params![owner], |row| row.get(0))?,
                    None => conn.query_row(sql, [], |row| row.get(0))?,
                };
                Ok(u64::try_from(value).unwrap_or_default())
            };
            Ok(StorageStats {
                messages: count("SELECT COUNT(*) FROM messages", None)?,
                conversations: count(
                    "SELECT COUNT(*) FROM (SELECT 1 FROM messages GROUP BY chat_id, thread_id)",
                    None,
                )?,
                memories: count(
                    "SELECT COUNT(*) FROM user_memories WHERE user_id=?1",
                    Some(owner),
                )?,
            })
        })
    })
    .await
    .unwrap_or_default()
}

/* ------------------------------ activity ------------------------------ */

#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct ActivityDay {
    pub date: String,
    pub prompts: u64,
    pub answers: u64,
    pub telegram: u64,
    pub whatsapp: u64,
}

/// Counts per local date (`YYYY-MM-DD`, the first ten characters of the
/// stored RFC 3339 times) since `since`.
fn activity_on_conn(
    conn: &Connection,
    since: &str,
) -> rusqlite::Result<HashMap<String, ActivityDay>> {
    let mut days: HashMap<String, ActivityDay> = HashMap::new();
    let mut stmt = conn.prepare(
        "SELECT substr(created_at,1,10), role, COUNT(*) FROM messages
         WHERE created_at >= ?1 GROUP BY 1, 2",
    )?;
    let rows = stmt.query_map(params![since], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
        ))
    })?;
    for row in rows {
        let (date, role, count) = row?;
        let count = u64::try_from(count).unwrap_or_default();
        let day = days.entry(date).or_default();
        match role.as_str() {
            "user" => day.prompts += count,
            "assistant" => day.answers += count,
            _ => {}
        }
    }
    for (table, whatsapp) in [("telegram_inbox", false), ("whatsapp_inbox", true)] {
        let mut stmt = conn.prepare(&format!(
            "SELECT substr(received_at,1,10), COUNT(*) FROM {table}
             WHERE received_at >= ?1 GROUP BY 1"
        ))?;
        let rows = stmt.query_map(params![since], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        for row in rows {
            let (date, count) = row?;
            let count = u64::try_from(count).unwrap_or_default();
            let day = days.entry(date).or_default();
            if whatsapp {
                day.whatsapp += count;
            } else {
                day.telegram += count;
            }
        }
    }
    Ok(days)
}

/// The last `days` days including today, oldest first.
pub(crate) async fn activity_async(days: i64) -> Vec<ActivityDay> {
    let today = Local::now().date_naive();
    let dates: Vec<String> = (0..days.max(1))
        .rev()
        .map(|back| {
            (today - chrono::Duration::days(back))
                .format("%Y-%m-%d")
                .to_string()
        })
        .collect();
    let since = dates.first().cloned().unwrap_or_default();
    let counts = run_db("activity", move || {
        with_conn(|conn| activity_on_conn(conn, &since))
    })
    .await
    .unwrap_or_default();
    dates
        .into_iter()
        .map(|date| {
            let mut day = counts.get(&date).cloned().unwrap_or_default();
            day.date = date;
            day
        })
        .collect()
}

/* ------------------------------ memories ------------------------------ */

#[derive(Debug, Clone, Serialize)]
pub(crate) struct MemoryRow {
    pub key: String,
    pub fact: String,
    pub updated_at: Option<String>,
}

pub(crate) async fn list_memories_async(user_id: i64) -> Vec<MemoryRow> {
    run_db("list_memories", move || {
        with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT key, fact, updated_at FROM user_memories
                 WHERE user_id=?1 ORDER BY updated_at DESC, key ASC",
            )?;
            let rows = stmt.query_map(params![user_id], |row| {
                Ok(MemoryRow {
                    key: row.get(0)?,
                    fact: row.get(1)?,
                    updated_at: row.get(2)?,
                })
            })?;
            rows.collect()
        })
    })
    .await
    .unwrap_or_default()
}

/* ------------------------------ sessions ------------------------------ */

/// Renames a terminal/web chat session; true when it exists.
pub(crate) async fn rename_session_async(user_id: i64, session_id: usize, name: String) -> bool {
    let Ok(session_id) = i64::try_from(session_id) else {
        return false;
    };
    run_db("rename_session", move || {
        with_conn(|conn| {
            conn.execute(
                "UPDATE sessions SET name=?3, updated_at=?4 WHERE user_id=?1 AND session_id=?2",
                params![user_id, session_id, name, Local::now().to_rfc3339()],
            )
        })
    })
    .await
    .is_some_and(|updated| updated > 0)
}

/* ------------------------------- backup ------------------------------- */

/// Writes a consistent copy of the database to `target` (which must not
/// exist) without the browser sessions.
pub(crate) async fn backup_database_async(target: std::path::PathBuf) -> Result<(), String> {
    run_db("backup_database", move || {
        let target_str = target.to_string_lossy().into_owned();
        with_conn(|conn| conn.execute("VACUUM INTO ?1", params![target_str]))?;
        let copy = Connection::open(&target)?;
        copy.execute("DELETE FROM web_sessions", [])?;
        drop(copy);
        super::harden_file_mode(&target);
        Ok(())
    })
    .await
    .ok_or_else(|| "database backup failed".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("open_in_memory succeeds");
        conn.execute_batch(WEB_SCHEMA).expect("web schema");
        conn.execute_batch(
            "CREATE TABLE telegram_inbox (
                update_id INTEGER PRIMARY KEY, payload_json TEXT NOT NULL, status TEXT NOT NULL,
                attempts INTEGER NOT NULL DEFAULT 0, received_at TEXT NOT NULL, last_error TEXT
            );
            CREATE TABLE whatsapp_inbox (
                message_key TEXT PRIMARY KEY, chat_id INTEGER NOT NULL, sender_id INTEGER NOT NULL,
                payload_json TEXT NOT NULL, status TEXT NOT NULL, attempts INTEGER NOT NULL DEFAULT 0,
                received_at TEXT NOT NULL, last_error TEXT
            );",
        )
        .expect("inbox schema");
        conn
    }

    #[test]
    fn sessions_are_found_by_hash_only_until_they_expire() {
        let conn = test_conn();
        let session = NewWebSession {
            id: "abc".into(),
            token_hash: "hash-1".into(),
            now: 1_000,
            expires_at: 2_000,
            ip: "127.0.0.1".into(),
            user_agent: "test".into(),
        };
        insert_web_session_on_conn(&conn, &session).expect("insert");
        assert!(find_web_session_on_conn(&conn, "hash-1", 1_500)
            .expect("query")
            .is_some());
        assert!(find_web_session_on_conn(&conn, "hash-2", 1_500)
            .expect("query")
            .is_none());
        assert!(
            find_web_session_on_conn(&conn, "hash-1", 2_000)
                .expect("query")
                .is_none(),
            "expired sessions are refused"
        );
        assert_eq!(
            list_web_sessions_on_conn(&conn, 1_500).expect("list").len(),
            1
        );
    }

    #[test]
    fn activity_is_counted_per_day() {
        let conn = test_conn();
        conn.execute_batch(
            "CREATE TABLE messages (
                user_id INTEGER NOT NULL, session_id INTEGER NOT NULL DEFAULT 0,
                chat_id INTEGER NOT NULL DEFAULT 0, thread_id INTEGER NOT NULL DEFAULT 0,
                role TEXT NOT NULL, content TEXT NOT NULL, created_at TEXT NOT NULL
            );
            INSERT INTO messages VALUES (1,0,1,0,'user','a','2026-09-29T10:00:00+08:00');
            INSERT INTO messages VALUES (1,0,1,0,'assistant','b','2026-09-29T10:00:05+08:00');
            INSERT INTO messages VALUES (1,0,1,0,'user','c','2026-09-30T09:00:00+08:00');
            INSERT INTO messages VALUES (1,0,1,0,'user','old','2026-09-01T09:00:00+08:00');
            INSERT INTO telegram_inbox VALUES (1, '{}', 'completed', 1, '2026-09-30T09:00:00+08:00', NULL);",
        )
        .expect("seed");
        let days = activity_on_conn(&conn, "2026-09-24").expect("activity");
        let monday = days.get("2026-09-29").expect("29th");
        assert_eq!((monday.prompts, monday.answers), (1, 1));
        let tuesday = days.get("2026-09-30").expect("30th");
        assert_eq!((tuesday.prompts, tuesday.telegram), (1, 1));
        assert!(!days.contains_key("2026-09-01"), "older days are left out");
    }

    #[test]
    fn quarantined_rows_can_be_retried_or_dismissed() {
        let conn = test_conn();
        conn.execute_batch(
            "INSERT INTO telegram_inbox VALUES (7, '{\"update_id\":7}', 'failed', 2, '2026-09-29T09:00:00+07:00', 'boom');
             INSERT INTO telegram_inbox VALUES (8, '{\"update_id\":8}', 'failed', 2, '2026-09-29T09:01:00+07:00', 'boom');
             INSERT INTO whatsapp_inbox VALUES ('c:s:m', -5, 9, '{}', 'failed', 2, '2026-09-29T09:02:00+07:00', 'x');",
        )
        .expect("seed");
        assert_eq!(failed_rows_on_conn(&conn, 10).expect("rows").len(), 3);
        assert_eq!(
            retry_telegram_on_conn(&conn, 7).expect("retry").as_deref(),
            Some("{\"update_id\":7}")
        );
        assert!(
            retry_telegram_on_conn(&conn, 7).expect("retry").is_none(),
            "only failed rows are retried"
        );
        assert!(dismiss_on_conn(&conn, Inbox::Telegram, "8").expect("dismiss"));
        assert!(dismiss_on_conn(&conn, Inbox::WhatsApp, "c:s:m").expect("dismiss"));
        let counts = queue_counts_on_conn(&conn, Inbox::Telegram).expect("counts");
        assert_eq!((counts.pending, counts.completed, counts.failed), (1, 1, 0));
        let payload: String = conn
            .query_row(
                "SELECT payload_json FROM telegram_inbox WHERE update_id=8",
                [],
                |row| row.get(0),
            )
            .expect("payload");
        assert!(payload.contains("redacted_after_completion"));
    }
}
