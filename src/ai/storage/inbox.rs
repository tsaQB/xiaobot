use chrono::Local;
use rusqlite::{params, Connection};

use super::{open_session_db, run_db};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelegramInboxRecord {
    pub update_id: i64,
    pub payload_json: String,
    pub attempts: i64,
}

fn load_telegram_offset_db() -> rusqlite::Result<Option<i64>> {
    let conn = open_session_db()?;
    let value = conn
        .query_row(
            "SELECT value FROM telegram_state WHERE key='offset'",
            [],
            |row| row.get::<_, String>(0),
        )
        .ok();
    Ok(value.and_then(|value| value.parse::<i64>().ok()))
}

fn enqueue_telegram_update_db(update_id: i64, payload_json: &str) -> rusqlite::Result<bool> {
    let mut conn = open_session_db()?;
    enqueue_telegram_update_on_conn(&mut conn, update_id, payload_json)
}

fn enqueue_telegram_update_on_conn(
    conn: &mut Connection,
    update_id: i64,
    payload_json: &str,
) -> rusqlite::Result<bool> {
    let tx = conn.transaction()?;
    let inserted = tx.execute(
        "INSERT OR IGNORE INTO telegram_inbox(update_id,payload_json,status,attempts,received_at)
         VALUES(?1,?2,'pending',0,?3)",
        params![update_id, payload_json, Local::now().to_rfc3339()],
    )? == 1;
    tx.execute(
        "INSERT INTO telegram_state(key,value) VALUES('offset',?1)
         ON CONFLICT(key) DO UPDATE SET value=
           CASE
             WHEN CAST(excluded.value AS INTEGER) > CAST(value AS INTEGER)
             THEN excluded.value ELSE value
           END",
        params![update_id.saturating_add(1).to_string()],
    )?;
    // Completed tombstones deduplicate a repeated Telegram delivery. Keep a
    // bounded recent window so the inbox itself cannot grow forever.
    tx.execute(
        "DELETE FROM telegram_inbox
         WHERE status='completed' AND update_id < (
           SELECT COALESCE(MAX(update_id),0) - 5000 FROM telegram_inbox
         )",
        [],
    )?;
    tx.commit()?;
    Ok(inserted)
}

fn recover_telegram_processing_db() -> rusqlite::Result<usize> {
    let conn = open_session_db()?;
    recover_telegram_processing_on_conn(&conn)
}

fn pending_telegram_updates_after_db(
    after_update_id: i64,
    limit: usize,
) -> rusqlite::Result<Vec<TelegramInboxRecord>> {
    let conn = open_session_db()?;
    pending_telegram_updates_after_on_conn(&conn, after_update_id, limit)
}

fn pending_telegram_updates_after_on_conn(
    conn: &Connection,
    after_update_id: i64,
    limit: usize,
) -> rusqlite::Result<Vec<TelegramInboxRecord>> {
    let mut stmt = conn.prepare(
        "SELECT update_id,payload_json,attempts
         FROM telegram_inbox
         WHERE status='pending' AND update_id>?1
         ORDER BY update_id
         LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![after_update_id, limit as i64], |row| {
        Ok(TelegramInboxRecord {
            update_id: row.get(0)?,
            payload_json: row.get(1)?,
            attempts: row.get(2)?,
        })
    })?;
    rows.collect()
}

fn recover_telegram_processing_on_conn(conn: &Connection) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE telegram_inbox
         SET status='pending',last_error='recovered after daemon stopped while processing'
         WHERE status='processing'",
        [],
    )
}

fn mark_telegram_processing_claim_on_conn(
    conn: &Connection,
    update_id: i64,
) -> rusqlite::Result<Option<i64>> {
    // Keep the payload until the handler reaches its completed checkpoint. If
    // XiaoAI crashes immediately after this claim, startup recovery can safely
    // make the update pending again instead of losing it forever. This gives
    // the inbox explicit at-least-once processing semantics; a crash after an
    // external side effect but before completion can still repeat that effect.
    let affected = conn.execute(
        "UPDATE telegram_inbox
         SET status='processing',attempts=attempts+1,last_error=NULL
         WHERE update_id=?1 AND status='pending'",
        params![update_id],
    )?;
    if affected == 0 {
        return Ok(None);
    }
    let attempts: i64 = conn.query_row(
        "SELECT attempts FROM telegram_inbox WHERE update_id=?1",
        params![update_id],
        |row| row.get(0),
    )?;
    Ok(Some(attempts))
}

fn mark_telegram_processing_claim_db(update_id: i64) -> rusqlite::Result<Option<i64>> {
    let conn = open_session_db()?;
    mark_telegram_processing_claim_on_conn(&conn, update_id)
}

#[cfg(test)]
fn mark_telegram_processing_on_conn(conn: &Connection, update_id: i64) -> rusqlite::Result<bool> {
    Ok(mark_telegram_processing_claim_on_conn(conn, update_id)?.is_some())
}

fn mark_telegram_processing_retry_on_conn(
    conn: &Connection,
    update_id: i64,
    reason: &str,
) -> rusqlite::Result<bool> {
    Ok(conn.execute(
        "UPDATE telegram_inbox
         SET status='pending',last_error=?2
         WHERE update_id=?1 AND status='processing'",
        params![update_id, reason],
    )? == 1)
}

fn mark_telegram_processing_retry_db(update_id: i64, reason: &str) -> rusqlite::Result<bool> {
    let conn = open_session_db()?;
    mark_telegram_processing_retry_on_conn(&conn, update_id, reason)
}

fn mark_telegram_processing_failed_on_conn(
    conn: &Connection,
    update_id: i64,
    reason: &str,
) -> rusqlite::Result<bool> {
    Ok(conn.execute(
        "UPDATE telegram_inbox
         SET status='failed',last_error=?2
         WHERE update_id=?1 AND status='processing'",
        params![update_id, reason],
    )? == 1)
}

/// Remembers the owner's latest answered message per chat/topic. Only a hash
/// of its text is kept, never the text itself. Replays of older updates do not
/// move the marker backwards.
fn record_latest_prompt_on_conn(
    conn: &Connection,
    chat_id: i64,
    thread_id: i64,
    message_id: i64,
    content_hash: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO telegram_latest_prompts(chat_id,thread_id,message_id,content_hash)
         VALUES(?1,?2,?3,?4)
         ON CONFLICT(chat_id,thread_id) DO UPDATE SET
           message_id=excluded.message_id, content_hash=excluded.content_hash
         WHERE excluded.message_id >= telegram_latest_prompts.message_id",
        params![chat_id, thread_id, message_id, content_hash],
    )?;
    Ok(())
}

pub async fn record_latest_prompt_async(
    chat_id: i64,
    thread_id: i64,
    message_id: i64,
    content_hash: String,
) -> bool {
    run_db("record_latest_prompt", move || {
        let conn = open_session_db()?;
        record_latest_prompt_on_conn(&conn, chat_id, thread_id, message_id, &content_hash)
    })
    .await
    .is_some()
}

/// Atomically accepts an edit for answering: true only when `message_id` is
/// still the latest answered message in the scope and its text actually
/// changed. Telegram also sends `edited_message` for changes Xiao does not
/// use (e.g. reactions or link previews), which must not trigger a reply.
/// The claiming update is remembered so a crash-recovery replay of that same
/// update is accepted again instead of being lost.
fn claim_edited_prompt_on_conn(
    conn: &Connection,
    chat_id: i64,
    thread_id: i64,
    message_id: i64,
    content_hash: &str,
    update_id: i64,
) -> rusqlite::Result<bool> {
    Ok(conn.execute(
        "UPDATE telegram_latest_prompts SET content_hash=?4, claimed_update_id=?5
         WHERE chat_id=?1 AND thread_id=?2 AND message_id=?3
           AND (content_hash<>?4 OR claimed_update_id=?5)",
        params![chat_id, thread_id, message_id, content_hash, update_id],
    )? == 1)
}

pub async fn claim_edited_prompt_async(
    chat_id: i64,
    thread_id: i64,
    message_id: i64,
    content_hash: String,
    update_id: i64,
) -> bool {
    run_db("claim_edited_prompt", move || {
        let conn = open_session_db()?;
        claim_edited_prompt_on_conn(
            &conn,
            chat_id,
            thread_id,
            message_id,
            &content_hash,
            update_id,
        )
    })
    .await
    .unwrap_or(false)
}

/// Quarantines an update that is still `pending` (not yet claimed). Startup
/// quarantine previously went through the `processing`-only transition and
/// therefore never changed anything, leaving poison updates pending forever.
fn quarantine_telegram_update_on_conn(
    conn: &Connection,
    update_id: i64,
    reason: &str,
) -> rusqlite::Result<bool> {
    Ok(conn.execute(
        "UPDATE telegram_inbox
         SET status='failed',last_error=?2
         WHERE update_id=?1 AND status IN ('pending','processing')",
        params![update_id, reason],
    )? == 1)
}

pub async fn quarantine_telegram_update_async(update_id: i64, reason: &str) -> bool {
    let reason = reason.to_string();
    run_db("quarantine_telegram_update", move || {
        let conn = open_session_db()?;
        quarantine_telegram_update_on_conn(&conn, update_id, &reason)
    })
    .await
    .unwrap_or(false)
}

/// Marks an update that never needs processing (non-owner traffic, stale
/// control updates) as completed straight from `pending`.
fn skip_telegram_update_on_conn(conn: &Connection, update_id: i64) -> rusqlite::Result<bool> {
    let scrubbed = serde_json::json!({
        "update_id": update_id,
        "payload": "redacted_after_completion"
    })
    .to_string();
    Ok(conn.execute(
        "UPDATE telegram_inbox
         SET status='completed',payload_json=?2,last_error=NULL
         WHERE update_id=?1 AND status IN ('pending','processing')",
        params![update_id, scrubbed],
    )? == 1)
}

pub async fn skip_telegram_update_async(update_id: i64) -> bool {
    run_db("skip_telegram_update", move || {
        let conn = open_session_db()?;
        skip_telegram_update_on_conn(&conn, update_id)
    })
    .await
    .unwrap_or(false)
}

fn mark_telegram_processing_failed_db(update_id: i64, reason: &str) -> rusqlite::Result<bool> {
    let conn = open_session_db()?;
    mark_telegram_processing_failed_on_conn(&conn, update_id, reason)
}

fn mark_telegram_processed_db(update_id: i64) -> rusqlite::Result<bool> {
    let conn = open_session_db()?;
    mark_telegram_processed_on_conn(&conn, update_id)
}

fn mark_telegram_processed_on_conn(conn: &Connection, update_id: i64) -> rusqlite::Result<bool> {
    let scrubbed = serde_json::json!({
        "update_id": update_id,
        "payload": "redacted_after_completion"
    })
    .to_string();
    Ok(conn.execute(
        "UPDATE telegram_inbox
         SET status='completed',payload_json=?2,last_error=NULL
         WHERE update_id=?1 AND status='processing'",
        params![update_id, scrubbed],
    )? == 1)
}

pub async fn load_telegram_offset_async() -> Option<i64> {
    run_db("load_telegram_offset", load_telegram_offset_db)
        .await
        .flatten()
}

pub async fn enqueue_telegram_update_async(update_id: i64, payload_json: String) -> Option<bool> {
    run_db("enqueue_telegram_update", move || {
        enqueue_telegram_update_db(update_id, &payload_json)
    })
    .await
}

pub async fn pending_telegram_updates_after_async(
    after_update_id: i64,
    limit: usize,
) -> Vec<TelegramInboxRecord> {
    run_db("pending_telegram_updates_after", move || {
        pending_telegram_updates_after_db(after_update_id, limit)
    })
    .await
    .unwrap_or_default()
}

pub async fn recover_telegram_processing_async() -> usize {
    run_db(
        "recover_telegram_processing",
        recover_telegram_processing_db,
    )
    .await
    .unwrap_or_default()
}

pub async fn mark_telegram_processing_claim_async(update_id: i64) -> Option<i64> {
    run_db("mark_telegram_processing_claim", move || {
        mark_telegram_processing_claim_db(update_id)
    })
    .await
    .flatten()
}

pub async fn mark_telegram_processing_async(update_id: i64) -> bool {
    mark_telegram_processing_claim_async(update_id)
        .await
        .is_some()
}

pub async fn mark_telegram_processing_retry_async(update_id: i64, reason: &str) -> bool {
    let reason = reason.to_string();
    run_db("mark_telegram_processing_retry", move || {
        mark_telegram_processing_retry_db(update_id, &reason)
    })
    .await
    .unwrap_or(false)
}

/// Returns an in-flight update to `pending` without charging the attempt, for
/// work that was interrupted by a shutdown rather than by a failure. Keeping
/// the attempt counter unchanged prevents repeated restarts from quarantining
/// a perfectly good message.
fn mark_telegram_processing_released_on_conn(
    conn: &Connection,
    update_id: i64,
    reason: &str,
) -> rusqlite::Result<bool> {
    Ok(conn.execute(
        "UPDATE telegram_inbox
         SET status='pending',attempts=MAX(attempts-1,0),last_error=?2
         WHERE update_id=?1 AND status='processing'",
        params![update_id, reason],
    )? == 1)
}

pub async fn mark_telegram_processing_released_async(update_id: i64, reason: &str) -> bool {
    let reason = reason.to_string();
    run_db("mark_telegram_processing_released", move || {
        let conn = open_session_db()?;
        mark_telegram_processing_released_on_conn(&conn, update_id, &reason)
    })
    .await
    .unwrap_or(false)
}

pub async fn mark_telegram_processing_failed_async(update_id: i64, reason: &str) -> bool {
    let reason = reason.to_string();
    run_db("mark_telegram_processing_failed", move || {
        mark_telegram_processing_failed_db(update_id, &reason)
    })
    .await
    .unwrap_or(false)
}

pub async fn mark_telegram_processed_async(update_id: i64) -> bool {
    run_db("mark_telegram_processed", move || {
        mark_telegram_processed_db(update_id)
    })
    .await
    .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inbox_test_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("open_in_memory succeeds");
        conn.execute_batch(
            "CREATE TABLE telegram_state (key TEXT PRIMARY KEY, value TEXT NOT NULL);
            CREATE TABLE telegram_inbox (
                update_id INTEGER PRIMARY KEY, payload_json TEXT NOT NULL, status TEXT NOT NULL,
                attempts INTEGER NOT NULL DEFAULT 0, received_at TEXT NOT NULL, last_error TEXT
            );
            CREATE TABLE telegram_latest_prompts (
                chat_id INTEGER NOT NULL, thread_id INTEGER NOT NULL DEFAULT 0,
                message_id INTEGER NOT NULL, content_hash TEXT NOT NULL,
                claimed_update_id INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY(chat_id, thread_id)
            );",
        )
        .expect("execute_batch succeeds");
        conn
    }

    #[test]
    fn only_a_changed_edit_of_the_latest_prompt_is_claimed() {
        let conn = inbox_test_conn();
        record_latest_prompt_on_conn(&conn, 1, 0, 10, "hash-a").expect("record succeeds");
        record_latest_prompt_on_conn(&conn, 1, 0, 11, "hash-b").expect("record succeeds");
        // A replayed older message never moves the marker back.
        record_latest_prompt_on_conn(&conn, 1, 0, 9, "hash-old").expect("record succeeds");

        let claim = |chat_id, message_id, hash, update_id| {
            claim_edited_prompt_on_conn(&conn, chat_id, 0, message_id, hash, update_id)
                .expect("claim succeeds")
        };
        assert!(
            !claim(1, 10, "hash-a2", 100),
            "an edit of an older message is ignored"
        );
        assert!(
            !claim(1, 11, "hash-b", 101),
            "an edit that did not change the text is ignored"
        );
        assert!(claim(1, 11, "hash-b2", 102));
        assert!(
            claim(1, 11, "hash-b2", 102),
            "a crash-recovery replay of the claiming update is accepted again"
        );
        assert!(
            !claim(1, 11, "hash-b2", 103),
            "a later edit event with the same text (e.g. a reaction) is ignored"
        );
        assert!(
            claim(1, 11, "hash-b", 104),
            "reverting the text is a change"
        );
        assert!(!claim(2, 11, "hash-x", 105), "other chats are independent");
    }

    #[test]
    fn telegram_claim_crash_is_recoverable_and_completed_updates_deduplicate() {
        let mut conn = inbox_test_conn();
        let payload = r#"{"update_id":42,"message":{"text":"hello"}}"#;
        assert!(enqueue_telegram_update_on_conn(&mut conn, 42, payload)
            .expect("enqueue_telegram_update succeeds"));
        assert!(
            mark_telegram_processing_on_conn(&conn, 42).expect("mark_telegram_processing succeeds")
        );
        let claimed: (String, String) = conn
            .query_row(
                "SELECT status,payload_json FROM telegram_inbox WHERE update_id=42",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("query status,payload_json succeeds");
        assert_eq!(claimed.0, "processing");
        assert_eq!(claimed.1, payload);

        assert_eq!(
            recover_telegram_processing_on_conn(&conn)
                .expect("recover_telegram_processing succeeds"),
            1
        );
        let recovered: String = conn
            .query_row(
                "SELECT status FROM telegram_inbox WHERE update_id=42",
                [],
                |row| row.get(0),
            )
            .expect("query status succeeds");
        assert_eq!(recovered, "pending");

        assert!(
            mark_telegram_processing_on_conn(&conn, 42).expect("mark_telegram_processing succeeds")
        );
        assert!(
            mark_telegram_processed_on_conn(&conn, 42).expect("mark_telegram_processed succeeds")
        );
        assert_eq!(
            recover_telegram_processing_on_conn(&conn)
                .expect("recover_telegram_processing succeeds"),
            0
        );
        assert!(!enqueue_telegram_update_on_conn(&mut conn, 42, payload)
            .expect("enqueue_telegram_update succeeds"));
        let completed: (String, String) = conn
            .query_row(
                "SELECT status,payload_json FROM telegram_inbox WHERE update_id=42",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("query status,payload_json succeeds");
        assert_eq!(completed.0, "completed");
        assert!(!completed.1.contains("hello"));
    }

    #[test]
    fn telegram_pending_updates_page_past_first_500_without_replaying_queued_rows() {
        let mut conn = inbox_test_conn();
        for update_id in 1..=501 {
            let payload = format!(r#"{{"update_id":{update_id}}}"#);
            assert!(
                enqueue_telegram_update_on_conn(&mut conn, update_id, &payload)
                    .expect("enqueue_telegram_update succeeds")
            );
        }

        let first = pending_telegram_updates_after_on_conn(&conn, i64::MIN, 500)
            .expect("pending_telegram_updates_after succeeds");
        assert_eq!(first.len(), 500);
        assert_eq!(first.first().map(|record| record.update_id), Some(1));
        assert_eq!(first.last().map(|record| record.update_id), Some(500));

        let second = pending_telegram_updates_after_on_conn(&conn, 500, 500)
            .expect("pending_telegram_updates_after succeeds");
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].update_id, 501);
    }

    #[test]
    fn telegram_pending_updates_replayed_and_marked_processing_then_processed() {
        let mut conn = inbox_test_conn();
        assert!(
            enqueue_telegram_update_on_conn(&mut conn, 10, r#"{"update_id":10}"#)
                .expect("enqueue_telegram_update succeeds")
        );
        assert!(
            enqueue_telegram_update_on_conn(&mut conn, 11, r#"{"update_id":11}"#)
                .expect("enqueue_telegram_update succeeds")
        );

        let pending = pending_telegram_updates_after_on_conn(&conn, i64::MIN, 10)
            .expect("pending_telegram_updates_after succeeds");
        assert_eq!(pending.len(), 2);
        assert_eq!(pending[0].update_id, 10);
        assert_eq!(pending[1].update_id, 11);

        assert!(
            mark_telegram_processing_on_conn(&conn, 10).expect("mark_telegram_processing succeeds")
        );
        let pending_after_first = pending_telegram_updates_after_on_conn(&conn, i64::MIN, 10)
            .expect("pending_telegram_updates_after succeeds");
        assert_eq!(pending_after_first.len(), 1);
        assert_eq!(pending_after_first[0].update_id, 11);

        assert!(
            mark_telegram_processed_on_conn(&conn, 10).expect("mark_telegram_processed succeeds")
        );
        assert_eq!(
            pending_telegram_updates_after_on_conn(&conn, i64::MIN, 10)
                .expect("pending_telegram_updates_after succeeds")
                .len(),
            1
        );
    }

    #[test]
    fn telegram_inbox_attempts_increment_on_claim_and_retry_preserves_count() {
        let mut conn = inbox_test_conn();
        assert!(
            enqueue_telegram_update_on_conn(&mut conn, 100, r#"{"update_id":100}"#)
                .expect("enqueue_telegram_update succeeds")
        );

        // First claim: attempts becomes 1
        assert_eq!(
            mark_telegram_processing_claim_on_conn(&conn, 100)
                .expect("mark_telegram_processing_claim succeeds"),
            Some(1)
        );

        // Mark retry: status becomes pending again
        assert!(
            mark_telegram_processing_retry_on_conn(&conn, 100, "transient error")
                .expect("mark_telegram_processing_retry succeeds")
        );

        let pending = pending_telegram_updates_after_on_conn(&conn, 99, 10)
            .expect("pending_telegram_updates_after succeeds");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].update_id, 100);
        assert_eq!(pending[0].attempts, 1);

        // Second claim: attempts becomes 2
        assert_eq!(
            mark_telegram_processing_claim_on_conn(&conn, 100)
                .expect("mark_telegram_processing_claim succeeds"),
            Some(2)
        );

        // Mark failed (quarantine): status becomes failed
        assert!(
            mark_telegram_processing_failed_on_conn(&conn, 100, "poison pill")
                .expect("mark_telegram_processing_failed succeeds")
        );

        // Failed records do not show in pending
        let pending_after_fail = pending_telegram_updates_after_on_conn(&conn, 99, 10)
            .expect("pending_telegram_updates_after succeeds");
        assert!(pending_after_fail.is_empty());
    }
}
