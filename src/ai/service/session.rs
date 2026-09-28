use chrono::Local;
use std::sync::Arc;
use tokio::sync::{watch, Mutex};
use tracing::warn;

use crate::util::truncate_chars;

use super::generation::GenerationGuard;
use super::AIChatService;
use crate::ai::storage::{
    clear_scoped_messages_async, create_session_and_activate_db_async,
    ensure_session_identity_v2_db_async, load_active_session_id_db_async, load_sessions_db_async,
    remove_session_transaction_db_async, replace_session_messages_if_revision_db_async,
    switch_active_session_db_async, ChatSession,
};

pub type GenerationCancelSender = watch::Sender<bool>;

/// History scope (`thread_id`) of a terminal chat session, within the owner's
/// `chat_id`. Telegram topics use positive thread ids and the private chat
/// uses 0, so negative ids never collide. Before this mapping every CLI
/// session wrote to the Telegram private-chat scope: `/new` and `/switch`
/// changed nothing the model saw, and `/clear` wiped the Telegram history.
pub fn cli_session_thread_id(session_id: usize) -> i64 {
    -i64::try_from(session_id).unwrap_or(i64::MAX)
}

/// Lock maps are keyed by chat/topic and used to grow for the whole process
/// lifetime. Once a map passes this size, entries nobody currently holds
/// (only the map owns the `Arc`) are dropped; they are recreated on demand.
const LOCK_MAP_EVICTION_THRESHOLD: usize = 256;

fn evict_idle_locks<K>(locks: &mut std::collections::HashMap<K, Arc<Mutex<()>>>) {
    if locks.len() >= LOCK_MAP_EVICTION_THRESHOLD {
        locks.retain(|_, lock| Arc::strong_count(lock) > 1);
    }
}

pub(super) fn signal_generation_cancel(sender: Option<GenerationCancelSender>) -> bool {
    sender
        .map(|sender| sender.send(true).is_ok())
        .unwrap_or(false)
}

impl AIChatService {
    pub(super) async fn session_lock(&self, user_id: i64) -> Arc<Mutex<()>> {
        if let Some(lock) = self.session_locks.read().await.get(&user_id).cloned() {
            return lock;
        }
        let mut locks = self.session_locks.write().await;
        evict_idle_locks(&mut locks);
        locks
            .entry(user_id)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    pub async fn get_sessions(&self, user_id: i64) -> Vec<ChatSession> {
        if let Some(list) = self.user_sessions.read().await.get(&user_id).cloned() {
            return list;
        }

        let init_lock = self.session_lock(user_id).await;
        let _guard = init_lock.lock().await;
        if let Some(list) = self.user_sessions.read().await.get(&user_id).cloned() {
            return list;
        }

        let mut existing = load_sessions_db_async(user_id).await;
        if existing.is_empty() {
            let now_str = Local::now().format("%d %b %H:%M").to_string();
            let Some(session) = create_session_and_activate_db_async(
                user_id,
                format!("Session {now_str}"),
                now_str,
            )
            .await
            else {
                warn!("Session initialization deferred because durable creation failed");
                return Vec::new();
            };
            existing.push(session);
        }
        if !ensure_session_identity_v2_db_async(user_id, existing.clone()).await {
            warn!("Session identity migration/check could not be persisted");
        }
        self.user_sessions
            .write()
            .await
            .insert(user_id, existing.clone());
        existing
    }

    pub async fn get_active_session_id(&self, user_id: i64) -> Option<usize> {
        let sessions = self.get_sessions(user_id).await;
        if sessions.is_empty() {
            return None;
        }
        if let Some(id) = self.active_session_id.read().await.get(&user_id).copied() {
            if sessions.iter().any(|session| session.id == id) {
                return Some(id);
            }
        }

        if let Some(stored_id) = load_active_session_id_db_async(user_id)
            .await
            .filter(|id| sessions.iter().any(|session| session.id == *id))
        {
            self.active_session_id
                .write()
                .await
                .insert(user_id, stored_id);
            return Some(stored_id);
        }

        let fallback_id = sessions[0].id;
        match switch_active_session_db_async(user_id, fallback_id).await {
            Some(true) => {
                self.active_session_id
                    .write()
                    .await
                    .insert(user_id, fallback_id);
                Some(fallback_id)
            }
            _ => {
                warn!("Active session fallback was not published because persistence failed");
                None
            }
        }
    }

    pub async fn get_active_session(&self, user_id: i64) -> Option<ChatSession> {
        let sessions = self.get_sessions(user_id).await;
        let active_id = self.get_active_session_id(user_id).await?;
        sessions
            .iter()
            .find(|session| session.id == active_id)
            .cloned()
    }

    pub async fn create_new_session(
        &self,
        user_id: i64,
        custom_name: Option<&str>,
    ) -> Option<ChatSession> {
        let _ = self.get_sessions(user_id).await;
        let session_lock = self.session_lock(user_id).await;
        let _guard = session_lock.lock().await;
        let now_str = Local::now().format("%d %b %H:%M").to_string();
        let name = custom_name
            .map(|value| truncate_chars(value.trim(), 60))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| format!("Session {now_str}"));
        let session = create_session_and_activate_db_async(user_id, name, now_str).await?;
        {
            let mut sessions_map = self.user_sessions.write().await;
            sessions_map
                .entry(user_id)
                .or_default()
                .push(session.clone());
        }
        self.active_session_id
            .write()
            .await
            .insert(user_id, session.id);
        Some(session)
    }

    pub async fn switch_session_by_id(&self, user_id: i64, session_id: usize) -> bool {
        let _ = self.get_sessions(user_id).await;
        let session_lock = self.session_lock(user_id).await;
        let _guard = session_lock.lock().await;
        let exists = self
            .user_sessions
            .read()
            .await
            .get(&user_id)
            .is_some_and(|sessions| sessions.iter().any(|session| session.id == session_id));
        if !exists {
            return false;
        }
        if switch_active_session_db_async(user_id, session_id).await != Some(true) {
            return false;
        }
        self.active_session_id
            .write()
            .await
            .insert(user_id, session_id);
        true
    }

    pub async fn remove_session_by_id(&self, user_id: i64, session_id: usize) -> bool {
        let _ = self.get_sessions(user_id).await;
        let _ = self.get_active_session_id(user_id).await;
        let session_lock = self.session_lock(user_id).await;
        let _guard = session_lock.lock().await;
        let exists = self
            .user_sessions
            .read()
            .await
            .get(&user_id)
            .is_some_and(|sessions| sessions.iter().any(|session| session.id == session_id));
        if !exists {
            return false;
        }
        let now_str = Local::now().format("%d %b %H:%M").to_string();
        let Some(Some(outcome)) = remove_session_transaction_db_async(
            user_id,
            session_id,
            format!("Session {now_str}"),
            now_str,
        )
        .await
        else {
            return false;
        };

        {
            let mut sessions_map = self.user_sessions.write().await;
            if let Some(list) = sessions_map.get_mut(&user_id) {
                list.retain(|session| session.id != session_id);
                if let Some(replacement) = outcome.replacement.clone() {
                    list.push(replacement);
                    list.sort_by_key(|session| session.id);
                }
            } else {
                // Durable success is authoritative. Evicting an absent cache is
                // sufficient; reporting failure here would be a false failure
                // after the destructive transaction already committed.
                warn!("Durable session removal committed while RAM cache was unavailable; cache will rehydrate");
                sessions_map.remove(&user_id);
            }
        }
        self.active_session_id
            .write()
            .await
            .insert(user_id, outcome.new_active_id);
        // The removed session's own history scope goes with it (messages,
        // summary, attachments); other scopes are untouched.
        self.clear_scoped_history(user_id, cli_session_thread_id(session_id))
            .await;
        true
    }

    pub async fn clear_history(&self, user_id: i64) -> bool {
        let Some(_) = self.get_active_session_id(user_id).await else {
            return false;
        };
        let session_lock = self.session_lock(user_id).await;
        let _guard = session_lock.lock().await;
        let Some(active_id) = self.active_session_id.read().await.get(&user_id).copied() else {
            return false;
        };
        let mut candidate = {
            let sessions_map = self.user_sessions.read().await;
            let Some(session) = sessions_map
                .get(&user_id)
                .and_then(|list| list.iter().find(|session| session.id == active_id))
            else {
                return false;
            };
            session.clone()
        };
        let expected_revision = candidate.revision;
        candidate.revision = candidate.revision.saturating_add(1);
        candidate.messages.clear();
        match replace_session_messages_if_revision_db_async(
            user_id,
            expected_revision,
            candidate.clone(),
        )
        .await
        {
            Some(true) => {
                let mut sessions_map = self.user_sessions.write().await;
                if let Some(session) = sessions_map
                    .get_mut(&user_id)
                    .and_then(|list| list.iter_mut().find(|session| session.id == active_id))
                {
                    *session = candidate;
                } else {
                    warn!(
                        "Durable history clear committed while RAM cache changed; evicting cache"
                    );
                    sessions_map.remove(&user_id);
                }
                drop(sessions_map);
                self.clear_scoped_history(user_id, cli_session_thread_id(active_id))
                    .await;
                true
            }
            Some(false) => {
                warn!("Clear history rejected because session revision changed");
                false
            }
            None => false,
        }
    }

    pub async fn clear_scoped_history(&self, chat_id: i64, thread_id: i64) -> bool {
        let cleared = clear_scoped_messages_async(chat_id, thread_id).await;
        if cleared {
            crate::attachments::delete_scoped_attachments(chat_id, thread_id).await;
        }
        cleared
    }

    pub async fn generation_lock(&self, chat_id: i64, thread_id: i64) -> Arc<Mutex<()>> {
        let key = (chat_id, thread_id);
        if let Some(lock) = self.generation_locks.read().await.get(&key).cloned() {
            return lock;
        }
        let mut locks = self.generation_locks.write().await;
        evict_idle_locks(&mut locks);
        locks
            .entry(key)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    pub async fn begin_generation(
        &self,
        chat_id: i64,
        draft_id: i64,
    ) -> (watch::Receiver<bool>, GenerationGuard) {
        // A generation that starts after shutdown began is cancelled up front
        // so it reports `Interrupted` instead of racing the shutdown.
        let (sender, receiver) = watch::channel(self.is_shutting_down());
        self.active_generations
            .write()
            .await
            .insert((chat_id, draft_id), sender);
        let guard = GenerationGuard::new(self.active_generations.clone(), chat_id, draft_id);
        (receiver, guard)
    }

    pub async fn cancel_generation(&self, chat_id: i64, draft_id: i64) -> bool {
        let sender = self
            .active_generations
            .read()
            .await
            .get(&(chat_id, draft_id))
            .cloned();
        signal_generation_cancel(sender)
    }

    pub async fn end_generation(&self, chat_id: i64, draft_id: i64) {
        self.active_generations
            .write()
            .await
            .remove(&(chat_id, draft_id));
    }

    /// Marks the service as shutting down and cancels every in-flight
    /// generation. Work cancelled this way reports
    /// [`crate::bot::worker::TaskOutcome::Interrupted`] so its inbox entry is
    /// replayed after restart instead of being recorded as answered.
    pub async fn begin_shutdown(&self) {
        self.shutting_down
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.cancel_all_generations().await;
    }

    pub fn is_shutting_down(&self) -> bool {
        self.shutting_down.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub async fn cancel_all_generations(&self) {
        let senders: Vec<GenerationCancelSender> = self
            .active_generations
            .read()
            .await
            .values()
            .cloned()
            .collect();
        for sender in senders {
            let _ = sender.send(true);
        }
    }
}
