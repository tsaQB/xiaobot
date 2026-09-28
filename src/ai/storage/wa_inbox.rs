use chrono::Local;
use rusqlite::{params, Connection};

use super::{open_session_db, run_db};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WhatsAppInboxRecord {
    pub message_key: String,
    pub chat_id: i64,
    pub sender_id: i64,
    pub payload_json: String,
    pub attempts: i64,
}

fn enqueue_whatsapp_message_on_conn(
    conn: &mut Connection,
    message_key: &str,
    chat_id: i64,
    sender_id: i64,
    payload_json: &str,
) -> rusqlite::Result<bool> {
    let tx = conn.transaction()?;
    let inserted = tx.execute(
        "INSERT OR IGNORE INTO whatsapp_inbox(
             message_key,chat_id,sender_id,payload_json,status,attempts,received_at
         ) VALUES(?1,?2,?3,?4,'pending',0,?5)",
        params![
            message_key,
            chat_id,
            sender_id,
            payload_json,
            Local::now().to_rfc3339()
        ],
    )? == 1;
    // Completed tombstones deduplicate a repeated WhatsApp delivery. Keep only a
    // bounded recent window so the inbox itself cannot grow forever.
    tx.execute(
        "DELETE FROM whatsapp_inbox
         WHERE status='completed' AND received_at < (
           SELECT COALESCE(MIN(received_at),'')
           FROM (
             SELECT received_at FROM whatsapp_inbox
             WHERE status='completed'
             ORDER BY received_at DESC
             LIMIT 5000
           )
         )",
        [],
    )?;
    tx.commit()?;
    Ok(inserted)
}

fn enqueue_whatsapp_message_db(
    message_key: &str,
    chat_id: i64,
    sender_id: i64,
    payload_json: &str,
) -> rusqlite::Result<bool> {
    let mut conn = open_session_db()?;
    enqueue_whatsapp_message_on_conn(&mut conn, message_key, chat_id, sender_id, payload_json)
}

fn mark_whatsapp_processing_claim_on_conn(
    conn: &Connection,
    message_key: &str,
) -> rusqlite::Result<Option<i64>> {
    // The payload survives until the handler reaches its completed checkpoint so
    // that a crash right after this claim replays the message instead of losing
    // it. Same at-least-once contract the Telegram inbox provides.
    let affected = conn.execute(
        "UPDATE whatsapp_inbox
         SET status='processing',attempts=attempts+1,last_error=NULL
         WHERE message_key=?1 AND status='pending'",
        params![message_key],
    )?;
    if affected == 0 {
        return Ok(None);
    }
    let attempts: i64 = conn.query_row(
        "SELECT attempts FROM whatsapp_inbox WHERE message_key=?1",
        params![message_key],
        |row| row.get(0),
    )?;
    Ok(Some(attempts))
}

fn mark_whatsapp_processing_claim_db(message_key: &str) -> rusqlite::Result<Option<i64>> {
    let conn = open_session_db()?;
    mark_whatsapp_processing_claim_on_conn(&conn, message_key)
}

fn mark_whatsapp_processing_retry_on_conn(
    conn: &Connection,
    message_key: &str,
    reason: &str,
) -> rusqlite::Result<bool> {
    Ok(conn.execute(
        "UPDATE whatsapp_inbox
         SET status='pending',last_error=?2
         WHERE message_key=?1 AND status='processing'",
        params![message_key, reason],
    )? == 1)
}

fn mark_whatsapp_processing_retry_db(message_key: &str, reason: &str) -> rusqlite::Result<bool> {
    let conn = open_session_db()?;
    mark_whatsapp_processing_retry_on_conn(&conn, message_key, reason)
}

fn mark_whatsapp_processing_failed_on_conn(
    conn: &Connection,
    message_key: &str,
    reason: &str,
) -> rusqlite::Result<bool> {
    Ok(conn.execute(
        "UPDATE whatsapp_inbox
         SET status='failed',last_error=?2
         WHERE message_key=?1 AND status='processing'",
        params![message_key, reason],
    )? == 1)
}

fn mark_whatsapp_processing_failed_db(message_key: &str, reason: &str) -> rusqlite::Result<bool> {
    let conn = open_session_db()?;
    mark_whatsapp_processing_failed_on_conn(&conn, message_key, reason)
}

fn mark_whatsapp_processed_on_conn(conn: &Connection, message_key: &str) -> rusqlite::Result<bool> {
    let scrubbed = serde_json::json!({
        "message_key": message_key,
        "payload": "redacted_after_completion"
    })
    .to_string();
    Ok(conn.execute(
        "UPDATE whatsapp_inbox
         SET status='completed',payload_json=?2,last_error=NULL
         WHERE message_key=?1 AND status='processing'",
        params![message_key, scrubbed],
    )? == 1)
}

fn mark_whatsapp_processed_db(message_key: &str) -> rusqlite::Result<bool> {
    let conn = open_session_db()?;
    mark_whatsapp_processed_on_conn(&conn, message_key)
}

fn recover_whatsapp_processing_on_conn(conn: &Connection) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE whatsapp_inbox
         SET status='pending',last_error='recovered after daemon stopped while processing'
         WHERE status='processing'",
        [],
    )
}

fn recover_whatsapp_processing_db() -> rusqlite::Result<usize> {
    let conn = open_session_db()?;
    recover_whatsapp_processing_on_conn(&conn)
}

fn pending_whatsapp_messages_on_conn(
    conn: &Connection,
    limit: usize,
) -> rusqlite::Result<Vec<WhatsAppInboxRecord>> {
    let mut stmt = conn.prepare(
        "SELECT message_key,chat_id,sender_id,payload_json,attempts
         FROM whatsapp_inbox
         WHERE status='pending'
         ORDER BY received_at
         LIMIT ?1",
    )?;
    let rows = stmt.query_map(params![limit as i64], |row| {
        Ok(WhatsAppInboxRecord {
            message_key: row.get(0)?,
            chat_id: row.get(1)?,
            sender_id: row.get(2)?,
            payload_json: row.get(3)?,
            attempts: row.get(4)?,
        })
    })?;
    rows.collect()
}

fn pending_whatsapp_messages_db(limit: usize) -> rusqlite::Result<Vec<WhatsAppInboxRecord>> {
    let conn = open_session_db()?;
    pending_whatsapp_messages_on_conn(&conn, limit)
}

pub async fn enqueue_whatsapp_message_async(
    message_key: String,
    chat_id: i64,
    sender_id: i64,
    payload_json: String,
) -> Option<bool> {
    run_db("enqueue_whatsapp_message", move || {
        enqueue_whatsapp_message_db(&message_key, chat_id, sender_id, &payload_json)
    })
    .await
}

pub async fn mark_whatsapp_processing_claim_async(message_key: String) -> Option<i64> {
    run_db("mark_whatsapp_processing_claim", move || {
        mark_whatsapp_processing_claim_db(&message_key)
    })
    .await
    .flatten()
}

pub async fn mark_whatsapp_processing_retry_async(message_key: String, reason: &str) -> bool {
    let reason = reason.to_string();
    run_db("mark_whatsapp_processing_retry", move || {
        mark_whatsapp_processing_retry_db(&message_key, &reason)
    })
    .await
    .unwrap_or(false)
}

pub async fn mark_whatsapp_processing_failed_async(message_key: String, reason: &str) -> bool {
    let reason = reason.to_string();
    run_db("mark_whatsapp_processing_failed", move || {
        mark_whatsapp_processing_failed_db(&message_key, &reason)
    })
    .await
    .unwrap_or(false)
}

pub async fn mark_whatsapp_processed_async(message_key: String) -> bool {
    run_db("mark_whatsapp_processed", move || {
        mark_whatsapp_processed_db(&message_key)
    })
    .await
    .unwrap_or(false)
}

pub async fn recover_whatsapp_processing_async() -> usize {
    run_db(
        "recover_whatsapp_processing",
        recover_whatsapp_processing_db,
    )
    .await
    .unwrap_or_default()
}

pub async fn pending_whatsapp_messages_async(limit: usize) -> Vec<WhatsAppInboxRecord> {
    run_db("pending_whatsapp_messages", move || {
        pending_whatsapp_messages_db(limit)
    })
    .await
    .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wa_inbox_test_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("open_in_memory succeeds");
        conn.execute_batch(
            "CREATE TABLE whatsapp_inbox (
                message_key TEXT PRIMARY KEY,
                chat_id     INTEGER NOT NULL,
                sender_id   INTEGER NOT NULL,
                payload_json TEXT NOT NULL,
                status      TEXT NOT NULL,
                attempts    INTEGER NOT NULL DEFAULT 0,
                received_at TEXT NOT NULL,
                last_error  TEXT
            );",
        )
        .expect("execute_batch succeeds");
        conn
    }

    #[test]
    fn whatsapp_inbox_survives_interrupted_processing() {
        let mut conn = wa_inbox_test_conn();
        assert!(
            enqueue_whatsapp_message_on_conn(&mut conn, "KEY-1", 100, 200, "{}")
                .expect("enqueue succeeds")
        );
        let attempt =
            mark_whatsapp_processing_claim_on_conn(&conn, "KEY-1").expect("claim succeeds");
        assert_eq!(attempt, Some(1));

        // Meniru mati mendadak: status tertinggal di 'processing'
        let recovered = recover_whatsapp_processing_on_conn(&conn).expect("recovery succeeds");
        assert_eq!(recovered, 1);

        let pending = pending_whatsapp_messages_on_conn(&conn, 10).expect("listing succeeds");
        assert_eq!(pending.len(), 1, "pesan harus kembali antre, bukan hilang");
        assert_eq!(pending[0].chat_id, 100);
        assert_eq!(pending[0].sender_id, 200);
    }

    #[test]
    fn duplicate_message_keys_are_not_enqueued_twice() {
        let mut conn = wa_inbox_test_conn();
        assert!(
            enqueue_whatsapp_message_on_conn(&mut conn, "KEY-DUP", 1, 2, "{}")
                .expect("first enqueue succeeds")
        );
        assert!(
            !enqueue_whatsapp_message_on_conn(&mut conn, "KEY-DUP", 1, 2, "{}")
                .expect("second enqueue succeeds"),
            "pesan yang sama tidak boleh masuk dua kali"
        );
    }

    #[test]
    fn completed_message_payload_is_scrubbed_and_blocks_replay() {
        let mut conn = wa_inbox_test_conn();
        assert!(enqueue_whatsapp_message_on_conn(
            &mut conn,
            "KEY-DONE",
            7,
            8,
            r#"{"text":"rahasia"}"#
        )
        .expect("enqueue succeeds"));
        assert!(mark_whatsapp_processing_claim_on_conn(&conn, "KEY-DONE")
            .expect("claim succeeds")
            .is_some());
        assert!(mark_whatsapp_processed_on_conn(&conn, "KEY-DONE").expect("complete succeeds"));

        let (status, payload): (String, String) = conn
            .query_row(
                "SELECT status,payload_json FROM whatsapp_inbox WHERE message_key='KEY-DONE'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("query succeeds");
        assert_eq!(status, "completed");
        assert!(!payload.contains("rahasia"), "isi pesan harus dihapus");
        assert_eq!(
            recover_whatsapp_processing_on_conn(&conn).expect("recovery succeeds"),
            0
        );
    }

    #[test]
    fn retry_preserves_attempt_count_and_failure_quarantines() {
        let mut conn = wa_inbox_test_conn();
        assert!(
            enqueue_whatsapp_message_on_conn(&mut conn, "KEY-RETRY", 1, 2, "{}")
                .expect("enqueue succeeds")
        );
        assert_eq!(
            mark_whatsapp_processing_claim_on_conn(&conn, "KEY-RETRY").expect("claim succeeds"),
            Some(1)
        );
        assert!(
            mark_whatsapp_processing_retry_on_conn(&conn, "KEY-RETRY", "transient panic")
                .expect("retry succeeds")
        );
        assert_eq!(
            mark_whatsapp_processing_claim_on_conn(&conn, "KEY-RETRY").expect("claim succeeds"),
            Some(2),
            "percobaan harus bertambah, bukan direset"
        );
        assert!(
            mark_whatsapp_processing_failed_on_conn(&conn, "KEY-RETRY", "poison pill")
                .expect("fail succeeds")
        );
        let pending = pending_whatsapp_messages_on_conn(&conn, 10).expect("listing succeeds");
        assert!(
            pending.is_empty(),
            "pesan terkarantina tidak boleh diantre lagi"
        );
    }
}
