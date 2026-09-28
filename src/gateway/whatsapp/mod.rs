use std::path::{Path, PathBuf};

pub mod client;
pub mod delivery;
pub mod mapper;

pub use delivery::WhatsAppDeliverySink;

#[derive(Debug, Clone)]
pub struct WhatsAppConfig {
    pub db_path: PathBuf,
    pub owner_number: Option<String>,
    pub phone_login: Option<String>,
    /// Groups (JID or numeric id) where every owner message is answered
    /// without mentioning the bot. Elsewhere in groups a mention, a reply to
    /// the bot, or a leading `/` is required.
    pub dedicated_groups: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WhatsAppStatus {
    Linked,
    Unlinked,
}

pub struct WhatsAppGateway;

/// Exclusive use of the WhatsApp session by this process, released on drop.
/// Two clients with the same device keys make the server drop one of them,
/// and both would answer the owner, so pairing, unlinking and the daemon
/// never run on the same session at once.
pub struct SessionLock {
    _file: std::fs::File,
}

impl WhatsAppGateway {
    /// Takes the session lock (`whatsapp.lock` next to the session database).
    pub fn lock_session(db_path: &Path) -> Result<SessionLock, String> {
        let lock_path = db_path.with_extension("lock");
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock_path)
            .map_err(|error| format!("Kunci sesi WhatsApp tidak dapat dibuat: {error}"))?;
        match file.try_lock() {
            Ok(()) => Ok(SessionLock { _file: file }),
            Err(std::fs::TryLockError::WouldBlock) => Err(
                "Sesi WhatsApp sedang dipakai proses xiao lain (misalnya daemon `xiao start` atau pairing di terminal lain). Hentikan proses itu dulu."
                    .to_string(),
            ),
            Err(std::fs::TryLockError::Error(error)) => {
                Err(format!("Kunci sesi WhatsApp tidak dapat dipasang: {error}"))
            }
        }
    }

    /// Memeriksa status keberadaan sesi login WhatsApp pada path database lokal.
    ///
    /// Menghindari false-positive dengan memverifikasi bahwa tabel `device`
    /// benar-benar memuat kredensial sesi multi-device yang telah terverifikasi (`account IS NOT NULL`).
    pub fn check_status(db_path: &Path) -> WhatsAppStatus {
        if !db_path.exists() {
            return WhatsAppStatus::Unlinked;
        }

        let conn = match rusqlite::Connection::open_with_flags(
            db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        ) {
            Ok(c) => c,
            Err(_) => return WhatsAppStatus::Unlinked,
        };
        // The running daemon writes to this database. Without a busy timeout a
        // momentary lock made the query fail and a linked session was
        // reported as unlinked.
        let _ = conn.busy_timeout(std::time::Duration::from_secs(5));

        let table_exists: bool = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='device'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map(|count| count > 0)
            .unwrap_or(false);

        if !table_exists {
            return WhatsAppStatus::Unlinked;
        }

        let is_linked: bool = conn
            .query_row(
                "SELECT count(*) FROM device WHERE account IS NOT NULL AND length(account) > 0",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map(|count| count > 0)
            .unwrap_or(false);

        if is_linked {
            WhatsAppStatus::Linked
        } else {
            WhatsAppStatus::Unlinked
        }
    }

    /// Menghapus file database sesi WhatsApp (Logout / Unlink).
    pub fn logout(db_path: &Path) -> std::io::Result<()> {
        if db_path.exists() {
            std::fs::remove_file(db_path)?;
            let wal = format!("{}-wal", db_path.display());
            let shm = format!("{}-shm", db_path.display());
            let _ = std::fs::remove_file(wal);
            let _ = std::fs::remove_file(shm);
        }
        Ok(())
    }

    /// Menjalankan satu sesi client WhatsApp sampai shutdown, logout, atau error.
    pub async fn start(
        config: WhatsAppConfig,
        ai_service: std::sync::Arc<crate::ai::AIChatService>,
        shutdown: tokio::sync::watch::Receiver<bool>,
    ) -> client::WhatsAppExit {
        let _lock = match Self::lock_session(&config.db_path) {
            Ok(lock) => lock,
            Err(error) => return client::WhatsAppExit::Failed(error),
        };
        client::WhatsAppClientRunner::run(config, ai_service, shutdown).await
    }
}

#[cfg(test)]
mod lock_tests {
    use super::*;

    #[test]
    fn only_one_holder_of_the_session_at_a_time() {
        let dir = std::env::temp_dir().join(format!(
            "xiaoai-wa-lock-{}-{:x}",
            std::process::id(),
            rand::random::<u64>()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let db_path = dir.join("whatsapp.db");

        let first = WhatsAppGateway::lock_session(&db_path).expect("first lock");
        let busy = WhatsAppGateway::lock_session(&db_path)
            .err()
            .expect("a second holder is refused");
        assert!(busy.contains("sedang dipakai"), "{busy}");
        drop(first);
        assert!(
            WhatsAppGateway::lock_session(&db_path).is_ok(),
            "the lock is released on drop"
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
