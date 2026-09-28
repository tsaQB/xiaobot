use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tracing::{error, warn};

use crate::ai::{self, AIChatService};
use crate::bot::client::TelegramBotClient;
use crate::bot::image_flow::UserLastImagePrompt;
use crate::bot::models::Update;
use crate::bot::router::{delivery_context_for_update, handle_update, ChatRouteScope};

/// Maximum number of updates processed at the same time across all chats.
pub const GLOBAL_WORKER_PERMITS: usize = 8;
/// Updates buffered per chat/topic before the dispatcher applies backpressure.
pub const SCOPE_MAILBOX_CAPACITY: usize = 32;
/// Updates buffered between the poll loop and the dispatcher.
pub const DISPATCH_CHANNEL_CAPACITY: usize = 64;
/// Idle time after which a per-scope worker exits.
pub const SCOPE_IDLE_TIMEOUT: Duration = Duration::from_secs(30);
/// Delay before retrying an update whose handler panicked.
pub const PANIC_RETRY_BACKOFF: Duration = Duration::from_millis(1500);
/// Attempts (including the first) before an update is quarantined.
pub const MAX_TASK_ATTEMPTS: i64 = 2;
/// Mailbox thread id reserved for guest-mode updates (never a real topic id).
pub const GUEST_SCOPE_THREAD_ID: i64 = i64::MIN;
/// Mailbox thread id for inline queries. They only need a quick placeholder
/// answer, so they never wait behind an inline generation.
pub const INLINE_QUERY_SCOPE_THREAD_ID: i64 = i64::MIN + 1;
/// Mailbox (and generation) thread id for chosen inline results.
pub const INLINE_SCOPE_THREAD_ID: i64 = i64::MIN + 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ScopeKey {
    pub chat_id: i64,
    pub thread_id: i64,
}

impl ScopeKey {
    pub fn from_update(update: &Update) -> Self {
        if let Some(msg) = update.guest_message.as_ref() {
            // Guest chats get their own mailbox: their ids may coincide with
            // a chat the bot is a member of (Bot API 10.0).
            Self {
                chat_id: msg.chat.id,
                thread_id: GUEST_SCOPE_THREAD_ID,
            }
        } else if let Some(msg) = update.message.as_ref().or(update.edited_message.as_ref()) {
            Self {
                chat_id: msg.chat.id,
                thread_id: msg.message_thread_id.unwrap_or(0),
            }
        } else if let Some(query) = update.inline_query.as_ref() {
            Self {
                chat_id: query.from.id,
                thread_id: INLINE_QUERY_SCOPE_THREAD_ID,
            }
        } else if let Some(chosen) = update.chosen_inline_result.as_ref() {
            Self {
                chat_id: chosen.from.id,
                thread_id: INLINE_SCOPE_THREAD_ID,
            }
        } else if let Some(cb) = update.callback_query.as_ref() {
            if let Some(msg) = cb.message.as_ref() {
                Self {
                    chat_id: msg.chat.id,
                    thread_id: msg.message_thread_id.unwrap_or(0),
                }
            } else {
                Self {
                    chat_id: cb.from.id,
                    thread_id: 0,
                }
            }
        } else if let Some(stopped) = update.stopped_message_generation.as_ref() {
            Self {
                chat_id: stopped.chat.id,
                thread_id: 0,
            }
        } else {
            Self {
                chat_id: 0,
                thread_id: 0,
            }
        }
    }
}

/// Identity of whoever caused an update, used to drop non-owner traffic
/// before it occupies a mailbox or a concurrency permit. Mirrors the router:
/// a message without `from` falls back to its chat id.
pub fn update_actor_id(update: &Update) -> Option<i64> {
    if let Some(msg) = update
        .message
        .as_ref()
        .or(update.edited_message.as_ref())
        .or(update.guest_message.as_ref())
    {
        return Some(msg.from.as_ref().map_or(msg.chat.id, |user| user.id));
    }
    if let Some(cb) = update.callback_query.as_ref() {
        return Some(cb.from.id);
    }
    if let Some(query) = update.inline_query.as_ref() {
        return Some(query.from.id);
    }
    if let Some(chosen) = update.chosen_inline_result.as_ref() {
        return Some(chosen.from.id);
    }
    update
        .stopped_message_generation
        .as_ref()
        .map(|stopped| stopped.chat.id)
}

/// How a durable task ended. Anything other than [`TaskOutcome::Completed`]
/// leaves a trace in the inbox instead of silently looking "answered".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskOutcome {
    /// Handled; the reply (if any) was delivered.
    Completed,
    /// Cancelled because the process is shutting down. The entry returns to
    /// `pending` without spending an attempt and is replayed after restart.
    Interrupted,
    /// Processing finished but the reply could not be delivered even after
    /// retries. The entry is quarantined as `failed` with this reason.
    DeliveryFailed(&'static str),
}

tokio::task_local! {
    static TASK_OUTCOME: Arc<std::sync::Mutex<TaskOutcome>>;
}

/// Records the outcome of the durable task currently running. A no-op outside
/// [`with_outcome_recorder`], e.g. in the CLI chat.
pub fn record_task_outcome(outcome: TaskOutcome) {
    let _ = TASK_OUTCOME.try_with(|cell| {
        if let Ok(mut guard) = cell.lock() {
            *guard = outcome;
        }
    });
}

/// Runs `future` with an outcome recorder in scope and returns what the
/// handler recorded (defaulting to [`TaskOutcome::Completed`]).
pub async fn with_outcome_recorder<F>(future: F) -> TaskOutcome
where
    F: std::future::Future<Output = ()>,
{
    let cell = Arc::new(std::sync::Mutex::new(TaskOutcome::Completed));
    TASK_OUTCOME.scope(Arc::clone(&cell), future).await;
    let outcome = match cell.lock() {
        Ok(guard) => *guard,
        Err(poisoned) => *poisoned.into_inner(),
    };
    outcome
}

#[derive(Clone)]
pub struct WorkerContext {
    pub bot: TelegramBotClient,
    pub ai_service: Arc<AIChatService>,
    pub user_last_image_prompt: UserLastImagePrompt,
    pub route_scope: Arc<ChatRouteScope>,
}

pub type ActiveScopes =
    Arc<tokio::sync::Mutex<HashMap<ScopeKey, tokio::sync::mpsc::Sender<Update>>>>;

pub async fn scoped_chat_worker(
    scope_key: ScopeKey,
    mut rx: tokio::sync::mpsc::Receiver<Update>,
    active_scopes: ActiveScopes,
    global_concurrency: Arc<tokio::sync::Semaphore>,
    ctx: WorkerContext,
) {
    loop {
        let update_opt = tokio::select! {
            msg = rx.recv() => msg,
            _ = tokio::time::sleep(SCOPE_IDLE_TIMEOUT) => None,
        };

        match update_opt {
            Some(update) => {
                process_scoped_update(&global_concurrency, &ctx, update).await;
            }
            None => {
                let mut scopes = active_scopes.lock().await;
                match rx.try_recv() {
                    Ok(late_update) => {
                        drop(scopes);
                        process_scoped_update(&global_concurrency, &ctx, late_update).await;
                        continue;
                    }
                    Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {
                        scopes.remove(&scope_key);
                        break;
                    }
                    Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                        scopes.remove(&scope_key);
                        break;
                    }
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScopedRetryPolicy {
    pub max_attempts: i64,
    pub backoff: Duration,
}

impl Default for ScopedRetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: MAX_TASK_ATTEMPTS,
            backoff: PANIC_RETRY_BACKOFF,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryOutcome {
    Success,
    PoisonPill,
    ExceededMaxAttempts,
    ClaimFailed,
    Cancelled,
    Interrupted,
    DeliveryFailed,
}

pub trait ScopedRetryStorage: Send + Sync {
    fn claim(&self) -> impl std::future::Future<Output = Option<i64>> + Send;
    fn retry(&self, reason: &'static str) -> impl std::future::Future<Output = bool> + Send;
    fn failed(&self, reason: &'static str) -> impl std::future::Future<Output = bool> + Send;
    fn processed(&self) -> impl std::future::Future<Output = bool> + Send;
    /// Returns the entry to `pending` without charging the attempt.
    fn release(&self, reason: &'static str) -> impl std::future::Future<Output = bool> + Send;
}

pub struct DurableInboxStorage {
    pub update_id: i64,
}

impl ScopedRetryStorage for DurableInboxStorage {
    async fn claim(&self) -> Option<i64> {
        ai::storage::mark_telegram_processing_claim_async(self.update_id).await
    }
    async fn retry(&self, reason: &'static str) -> bool {
        ai::storage::mark_telegram_processing_retry_async(self.update_id, reason).await
    }
    async fn failed(&self, reason: &'static str) -> bool {
        ai::storage::mark_telegram_processing_failed_async(self.update_id, reason).await
    }
    async fn processed(&self) -> bool {
        ai::storage::mark_telegram_processed_async(self.update_id).await
    }
    async fn release(&self, reason: &'static str) -> bool {
        ai::storage::mark_telegram_processing_released_async(self.update_id, reason).await
    }
}

pub async fn execute_with_scoped_retry<S, F, Fut>(
    global_concurrency: &Arc<tokio::sync::Semaphore>,
    storage: &S,
    policy: ScopedRetryPolicy,
    mut make_task: F,
) -> RetryOutcome
where
    S: ScopedRetryStorage,
    F: FnMut() -> Fut + Send,
    Fut: std::future::Future<Output = TaskOutcome> + Send + 'static,
{
    loop {
        // Acquire global permit only while actively executing
        let permit = match global_concurrency.acquire().await {
            Ok(p) => p,
            Err(_) => return RetryOutcome::Cancelled,
        };

        // Seed/read attempt count directly from durable storage claim
        let attempt = match storage.claim().await {
            Some(a) => a,
            None => {
                drop(permit);
                return RetryOutcome::ClaimFailed;
            }
        };

        if attempt > policy.max_attempts {
            warn!(
                "Task melebihi batas percobaan ({attempt} attempts); mengarantina sebagai failed"
            );
            let _ = storage
                .failed("quarantined after exceeding max attempts")
                .await;
            drop(permit);
            return RetryOutcome::ExceededMaxAttempts;
        }

        let fut = make_task();
        let join_res = tokio::spawn(fut).await;

        match join_res {
            Ok(TaskOutcome::Completed) => {
                if !storage.processed().await {
                    warn!("Gagal menyelesaikan durable processing checkpoint");
                }
                drop(permit);
                return RetryOutcome::Success;
            }
            Ok(TaskOutcome::Interrupted) => {
                if !storage
                    .release("interrupted by shutdown before completion")
                    .await
                {
                    warn!("Gagal mengembalikan pekerjaan yang terpotong shutdown ke antrean");
                }
                drop(permit);
                return RetryOutcome::Interrupted;
            }
            Ok(TaskOutcome::DeliveryFailed(reason)) => {
                warn!("Balasan gagal dikirim setelah beberapa percobaan: {reason}");
                if !storage.failed(reason).await {
                    warn!("Gagal mencatat kegagalan pengiriman ke durable inbox");
                }
                drop(permit);
                return RetryOutcome::DeliveryFailed;
            }
            Err(join_err) if join_err.is_panic() => {
                // REQUIREMENT: RELEASE PERMIT BEFORE BACKOFF SLEEP
                drop(permit);

                if attempt < policy.max_attempts {
                    warn!(
                        "Task panic pada percobaan {attempt}/{}. Menandai retry dan backoff {:?} (permit dilepas)...",
                        policy.max_attempts, policy.backoff
                    );
                    let _ = storage
                        .retry("transient panic during processing, scheduled for retry")
                        .await;

                    // Backoff without holding any concurrency permit
                    tokio::time::sleep(policy.backoff).await;

                    // Next iteration re-acquires permit and re-claims via storage.claim()
                    continue;
                } else {
                    error!(
                        "Task gagal setelah {attempt} kali percobaan (poison pill). Mengarantina sebagai failed."
                    );
                    let _ = storage
                        .failed("quarantined after max consecutive panics")
                        .await;
                    return RetryOutcome::PoisonPill;
                }
            }
            Err(join_err) => {
                drop(permit);
                warn!("Task cancelled or aborted: {join_err}");
                return RetryOutcome::Cancelled;
            }
        }
    }
}

pub async fn process_scoped_update(
    global_concurrency: &Arc<tokio::sync::Semaphore>,
    ctx: &WorkerContext,
    update: Update,
) {
    // Updates still queued when shutdown starts are left untouched in the
    // durable inbox (`pending`) and replayed after restart, instead of
    // starting new generations that would be cancelled immediately.
    if ctx.ai_service.is_shutting_down() {
        return;
    }
    let update_id = update.update_id;
    let storage = DurableInboxStorage { update_id };
    let policy = ScopedRetryPolicy::default();

    let worker_bot = ctx.bot.clone();
    let worker_ai = Arc::clone(&ctx.ai_service);
    let worker_last_image = Arc::clone(&ctx.user_last_image_prompt);
    let worker_route = Arc::clone(&ctx.route_scope);

    execute_with_scoped_retry(global_concurrency, &storage, policy, move || {
        let update_clone = update.clone();
        let worker_bot = worker_bot.clone();
        let worker_ai = Arc::clone(&worker_ai);
        let worker_last_image = Arc::clone(&worker_last_image);
        let worker_route = Arc::clone(&worker_route);

        async move {
            let delivery_context = delivery_context_for_update(&update_clone);
            with_outcome_recorder(TelegramBotClient::with_delivery_context(
                delivery_context,
                handle_update(
                    &worker_bot,
                    &worker_ai,
                    &worker_last_image,
                    &worker_route,
                    update_clone,
                ),
            ))
            .await
        }
    })
    .await;
}

/// Handles a native-stop update immediately, outside the per-scope queues.
///
/// Runs the handler in its own task so a panic is contained instead of
/// unwinding into the poll loop and taking the daemon down.
pub async fn process_durable_update(
    bot: &TelegramBotClient,
    ai_service: &Arc<AIChatService>,
    user_last_image_prompt: &UserLastImagePrompt,
    route_scope: &Arc<ChatRouteScope>,
    update: Update,
) {
    let update_id = update.update_id;
    if !ai::storage::mark_telegram_processing_async(update_id).await {
        return;
    }

    let bot = bot.clone();
    let ai_service = Arc::clone(ai_service);
    let user_last_image_prompt = Arc::clone(user_last_image_prompt);
    let route_scope = Arc::clone(route_scope);
    let handled = tokio::spawn(async move {
        let delivery_context = delivery_context_for_update(&update);
        TelegramBotClient::with_delivery_context(
            delivery_context,
            handle_update(
                &bot,
                &ai_service,
                &user_last_image_prompt,
                &route_scope,
                update,
            ),
        )
        .await;
    })
    .await;

    match handled {
        Ok(()) => {
            if !ai::storage::mark_telegram_processed_async(update_id).await {
                warn!("Gagal menyelesaikan durable Telegram inbox update {update_id}");
            }
        }
        Err(join_err) => {
            error!("Handler native stop gagal ({join_err}); update dikarantina");
            let _ = ai::storage::mark_telegram_processing_failed_async(
                update_id,
                "native stop handler panicked",
            )
            .await;
        }
    }
}

/// Dispatches one update into its scope mailbox, spawning the scope worker if
/// needed. When the mailbox is full the dispatcher waits for room (outside
/// the scope-map lock) instead of dropping the update, which previously left
/// it stranded until restart and let later messages overtake it.
async fn dispatch_to_scope(
    update: Update,
    active_scopes: &ActiveScopes,
    global_concurrency: &Arc<tokio::sync::Semaphore>,
    ctx: &WorkerContext,
    workers: &mut tokio::task::JoinSet<()>,
) {
    let scope_key = ScopeKey::from_update(&update);
    let mut pending = Some(update);

    while let Some(update) = pending.take() {
        let mut scopes = active_scopes.lock().await;
        if let Some(sender) = scopes.get(&scope_key).cloned() {
            match sender.try_send(update) {
                Ok(()) => return,
                Err(tokio::sync::mpsc::error::TrySendError::Full(rejected)) => {
                    drop(scopes);
                    warn!(
                        "Scope {:?} backlog penuh ({SCOPE_MAILBOX_CAPACITY} pesan); dispatcher menunggu ruang (backpressure)",
                        scope_key
                    );
                    match sender.send(rejected).await {
                        Ok(()) => return,
                        // The worker exited while we waited; retry with a fresh one.
                        Err(tokio::sync::mpsc::error::SendError(returned)) => {
                            pending = Some(returned);
                            continue;
                        }
                    }
                }
                Err(tokio::sync::mpsc::error::TrySendError::Closed(rejected)) => {
                    scopes.remove(&scope_key);
                    pending = Some(rejected);
                    continue;
                }
            }
        }

        let (tx, rx) = tokio::sync::mpsc::channel::<Update>(SCOPE_MAILBOX_CAPACITY);
        let _ = tx.try_send(update);
        scopes.insert(scope_key, tx);
        workers.spawn(scoped_chat_worker(
            scope_key,
            rx,
            Arc::clone(active_scopes),
            Arc::clone(global_concurrency),
            ctx.clone(),
        ));
    }
}

pub fn spawn_workers(
    bot: TelegramBotClient,
    ai_service: Arc<AIChatService>,
    user_last_image_prompt: UserLastImagePrompt,
    route_scope: Arc<ChatRouteScope>,
) -> (
    tokio::sync::mpsc::Sender<Update>,
    tokio::task::JoinHandle<()>,
) {
    let (update_tx, mut update_rx) =
        tokio::sync::mpsc::channel::<Update>(DISPATCH_CHANNEL_CAPACITY);
    let active_scopes: ActiveScopes = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
    let global_concurrency = Arc::new(tokio::sync::Semaphore::new(GLOBAL_WORKER_PERMITS));

    let worker_ctx = WorkerContext {
        bot,
        ai_service,
        user_last_image_prompt,
        route_scope,
    };

    let update_worker = tokio::spawn(async move {
        let mut workers = tokio::task::JoinSet::new();
        while let Some(update) = update_rx.recv().await {
            // Hard single-owner boundary, enforced before any queueing so
            // non-owner traffic can never occupy mailboxes or permits.
            let owner = worker_ctx.route_scope.owner_user_id;
            if update_actor_id(&update).is_some_and(|actor| actor != owner) {
                let _ = ai::storage::skip_telegram_update_async(update.update_id).await;
                continue;
            }

            if update.stopped_message_generation.is_some() {
                // Native Stop bypasses worker queues for immediate zero-latency cancellation
                process_durable_update(
                    &worker_ctx.bot,
                    &worker_ctx.ai_service,
                    &worker_ctx.user_last_image_prompt,
                    &worker_ctx.route_scope,
                    update,
                )
                .await;
                continue;
            }

            dispatch_to_scope(
                update,
                &active_scopes,
                &global_concurrency,
                &worker_ctx,
                &mut workers,
            )
            .await;

            // Reap finished scope workers so the set does not grow unbounded.
            while workers.try_join_next().is_some() {}
        }

        // Shutdown: close every mailbox so scope workers finish their current
        // item (queued items stay `pending` because the service is shutting
        // down) and exit, then wait for them.
        active_scopes.lock().await.clear();
        while workers.join_next().await.is_some() {}
    });

    (update_tx, update_worker)
}

pub async fn replay_durable_inbox(
    ai_service: &Arc<AIChatService>,
    update_tx: &tokio::sync::mpsc::Sender<Update>,
) {
    let interrupted = ai::storage::recover_telegram_processing_async().await;
    if interrupted > 0 {
        warn!(
            "{interrupted} Telegram update berstatus processing dikembalikan ke pending untuk replay at-least-once; side effect eksternal sebelum crash dapat terulang"
        );
    }

    let mut replay_after_update_id = i64::MIN;
    loop {
        let replay_batch =
            ai::storage::pending_telegram_updates_after_async(replay_after_update_id, 500).await;
        if replay_batch.is_empty() {
            break;
        }
        for record in replay_batch {
            replay_after_update_id = record.update_id;
            if record.attempts >= MAX_TASK_ATTEMPTS {
                warn!(
                    "Durable Telegram update {} sudah mencapai batas percobaan ({} attempts) saat startup; mengarantina sebagai failed",
                    record.update_id, record.attempts
                );
                let _ = ai::storage::quarantine_telegram_update_async(
                    record.update_id,
                    "quarantined after exceeding max attempts across restarts",
                )
                .await;
                continue;
            }
            match serde_json::from_str::<Update>(&record.payload_json) {
                Ok(update) => {
                    if update.stopped_message_generation.is_some() || update.inline_query.is_some()
                    {
                        // A stop request from before the restart refers to a
                        // generation that no longer exists, and an inline
                        // query expires within seconds; acknowledge both.
                        let _ = ai::storage::skip_telegram_update_async(record.update_id).await;
                        continue;
                    }
                    if ai_service.is_shutting_down() {
                        return;
                    }
                    if update_tx.send(update).await.is_err() {
                        error!("Update worker stopped while replaying durable inbox");
                        return;
                    }
                }
                Err(error) => {
                    warn!(
                        "Durable Telegram update {} tidak dapat didecode: {error}",
                        record.update_id
                    );
                    let _ = ai::storage::quarantine_telegram_update_async(
                        record.update_id,
                        "payload could not be decoded",
                    )
                    .await;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_key_extraction_for_messages_threads_and_callbacks() {
        // 1. Direct message (no thread)
        let update_direct: Update = serde_json::from_str(
            r#"{
                "update_id": 1,
                "message": {
                    "message_id": 10,
                    "date": 1234567,
                    "chat": {"id": 12345, "type": "private"},
                    "text": "hello"
                }
            }"#,
        )
        .expect("deserialize update succeeds");
        assert_eq!(
            ScopeKey::from_update(&update_direct),
            ScopeKey {
                chat_id: 12345,
                thread_id: 0
            }
        );

        // 2. Forum topic message (with thread)
        let update_topic: Update = serde_json::from_str(
            r#"{
                "update_id": 2,
                "message": {
                    "message_id": 11,
                    "message_thread_id": 99,
                    "date": 1234567,
                    "chat": {"id": -100123456, "type": "supergroup"},
                    "text": "in topic"
                }
            }"#,
        )
        .expect("deserialize update succeeds");
        assert_eq!(
            ScopeKey::from_update(&update_topic),
            ScopeKey {
                chat_id: -100123456,
                thread_id: 99
            }
        );

        // 3. Callback query with message
        let update_cb: Update = serde_json::from_str(
            r#"{
                "update_id": 3,
                "callback_query": {
                    "id": "cb1",
                    "from": {"id": 555, "is_bot": false, "first_name": "Test"},
                    "message": {
                        "message_id": 12,
                        "message_thread_id": 77,
                        "date": 1234567,
                        "chat": {"id": -100789, "type": "supergroup"}
                    }
                }
            }"#,
        )
        .expect("deserialize update succeeds");
        assert_eq!(
            ScopeKey::from_update(&update_cb),
            ScopeKey {
                chat_id: -100789,
                thread_id: 77
            }
        );
    }

    #[derive(Default)]
    struct MockRetryStorage {
        claim_count: Arc<std::sync::atomic::AtomicI64>,
        retry_count: Arc<std::sync::atomic::AtomicUsize>,
        failed_count: Arc<std::sync::atomic::AtomicUsize>,
        processed_count: Arc<std::sync::atomic::AtomicUsize>,
        released_count: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl ScopedRetryStorage for MockRetryStorage {
        async fn claim(&self) -> Option<i64> {
            Some(
                self.claim_count
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                    + 1,
            )
        }
        async fn retry(&self, _reason: &'static str) -> bool {
            self.retry_count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            true
        }
        async fn failed(&self, _reason: &'static str) -> bool {
            self.failed_count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            true
        }
        async fn processed(&self) -> bool {
            self.processed_count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            true
        }
        async fn release(&self, _reason: &'static str) -> bool {
            self.released_count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            true
        }
    }

    #[tokio::test]
    async fn execute_with_scoped_retry_drops_permit_during_panic_backoff_runtime() {
        let sem = Arc::new(tokio::sync::Semaphore::new(1));
        let storage = MockRetryStorage::default();
        let policy = ScopedRetryPolicy {
            max_attempts: 2,
            backoff: Duration::from_millis(200),
        };

        let call_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let call_count_clone = Arc::clone(&call_count);
        let sem_worker = Arc::clone(&sem);

        let worker_handle = tokio::spawn(async move {
            execute_with_scoped_retry(&sem_worker, &storage, policy, move || {
                let count = call_count_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                async move {
                    if count == 0 {
                        panic!("simulated transient panic on attempt 1");
                    }
                    TaskOutcome::Completed
                }
            })
            .await
        });

        // Give the task time to acquire permit, spawn handler, panic, and enter the 200ms backoff sleep
        tokio::time::sleep(Duration::from_millis(50)).await;

        // While the worker is sleeping in its backoff window, prove at runtime that the permit is free
        let acquired_during_backoff = match sem.try_acquire() {
            Ok(test_permit) => {
                // Drop our test permit so the worker can re-acquire it for attempt 2
                drop(test_permit);
                true
            }
            Err(_) => false,
        };

        let outcome = worker_handle.await.expect("worker task joins");

        assert!(
            acquired_during_backoff,
            "Semaphore permit must be free and acquirable by other tasks during backoff sleep"
        );
        assert_eq!(outcome, RetryOutcome::Success);
        assert_eq!(call_count.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn execute_with_scoped_retry_quarantines_poison_pill_after_consecutive_panics() {
        let sem = Arc::new(tokio::sync::Semaphore::new(1));
        let storage = MockRetryStorage::default();
        let claim_counter = Arc::clone(&storage.claim_count);
        let retry_counter = Arc::clone(&storage.retry_count);
        let failed_counter = Arc::clone(&storage.failed_count);
        let processed_counter = Arc::clone(&storage.processed_count);

        let policy = ScopedRetryPolicy {
            max_attempts: 2,
            backoff: Duration::from_millis(20),
        };

        let outcome = execute_with_scoped_retry(&sem, &storage, policy, || async {
            if std::hint::black_box(true) {
                panic!("unrecoverable panic");
            }
            TaskOutcome::Completed
        })
        .await;

        assert_eq!(outcome, RetryOutcome::PoisonPill);
        assert_eq!(claim_counter.load(std::sync::atomic::Ordering::SeqCst), 2);
        assert_eq!(retry_counter.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(failed_counter.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(
            processed_counter.load(std::sync::atomic::Ordering::SeqCst),
            0
        );
    }

    #[tokio::test]
    async fn interrupted_task_is_released_not_completed() {
        let sem = Arc::new(tokio::sync::Semaphore::new(1));
        let storage = MockRetryStorage::default();
        let released = Arc::clone(&storage.released_count);
        let processed = Arc::clone(&storage.processed_count);
        let outcome =
            execute_with_scoped_retry(&sem, &storage, ScopedRetryPolicy::default(), || async {
                TaskOutcome::Interrupted
            })
            .await;
        assert_eq!(outcome, RetryOutcome::Interrupted);
        assert_eq!(released.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(processed.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn failed_delivery_is_recorded_instead_of_completed() {
        let sem = Arc::new(tokio::sync::Semaphore::new(1));
        let storage = MockRetryStorage::default();
        let failed = Arc::clone(&storage.failed_count);
        let processed = Arc::clone(&storage.processed_count);
        let outcome =
            execute_with_scoped_retry(&sem, &storage, ScopedRetryPolicy::default(), || async {
                with_outcome_recorder(async {
                    record_task_outcome(TaskOutcome::DeliveryFailed(
                        "final answer delivery failed",
                    ));
                })
                .await
            })
            .await;
        assert_eq!(outcome, RetryOutcome::DeliveryFailed);
        assert_eq!(failed.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(processed.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[test]
    fn non_owner_actor_is_identified_for_prefiltering() {
        let group_message: Update = serde_json::from_str(
            r#"{"update_id": 9, "message": {"message_id": 1, "date": 1,
                "chat": {"id": -100555, "type": "supergroup"},
                "from": {"id": 42, "is_bot": false, "first_name": "Guest"},
                "text": "hi"}}"#,
        )
        .expect("deserialize update succeeds");
        assert_eq!(update_actor_id(&group_message), Some(42));

        let anonymous: Update = serde_json::from_str(
            r#"{"update_id": 10, "message": {"message_id": 2, "date": 1,
                "chat": {"id": -100555, "type": "supergroup"}, "text": "hi"}}"#,
        )
        .expect("deserialize update succeeds");
        assert_eq!(update_actor_id(&anonymous), Some(-100555));
    }

    #[test]
    fn edits_share_their_chat_mailbox_and_guest_queries_get_their_own() {
        let edited: Update = serde_json::from_str(
            r#"{"update_id": 11, "edited_message": {"message_id": 3, "date": 1,
                "edit_date": 5, "message_thread_id": 7,
                "chat": {"id": -100555, "type": "supergroup", "is_forum": true},
                "from": {"id": 42, "is_bot": false, "first_name": "Owner"},
                "text": "halo lagi"}}"#,
        )
        .expect("deserialize edited update succeeds");
        assert_eq!(
            ScopeKey::from_update(&edited),
            ScopeKey {
                chat_id: -100555,
                thread_id: 7
            }
        );
        assert_eq!(update_actor_id(&edited), Some(42));

        let guest: Update = serde_json::from_str(
            r#"{"update_id": 12, "guest_message": {"message_id": 4, "date": 1,
                "chat": {"id": -100555, "type": "supergroup"},
                "from": {"id": 42, "is_bot": false, "first_name": "Owner"},
                "guest_query_id": "gq-1", "text": "@XiaoBot halo"}}"#,
        )
        .expect("deserialize guest update succeeds");
        assert_eq!(
            ScopeKey::from_update(&guest),
            ScopeKey {
                chat_id: -100555,
                thread_id: GUEST_SCOPE_THREAD_ID
            },
            "a guest chat id may collide with a real chat, so it gets its own mailbox"
        );
        assert_eq!(update_actor_id(&guest), Some(42));
        assert_eq!(
            guest
                .guest_message
                .as_ref()
                .and_then(|message| message.guest_query_id.as_deref()),
            Some("gq-1")
        );
    }

    #[test]
    fn inline_queries_and_chosen_results_have_separate_owner_mailboxes() {
        let query: Update = serde_json::from_str(
            r#"{"update_id": 13, "inline_query": {"id": "iq", "query": "halo", "offset": "",
                "from": {"id": 42, "is_bot": false, "first_name": "Owner"}}}"#,
        )
        .expect("deserialize inline query succeeds");
        assert_eq!(
            ScopeKey::from_update(&query),
            ScopeKey {
                chat_id: 42,
                thread_id: INLINE_QUERY_SCOPE_THREAD_ID
            }
        );
        assert_eq!(update_actor_id(&query), Some(42));

        let chosen: Update = serde_json::from_str(
            r#"{"update_id": 14, "chosen_inline_result": {"result_id": "r", "query": "halo",
                "inline_message_id": "im",
                "from": {"id": 7, "is_bot": false, "first_name": "Orang lain"}}}"#,
        )
        .expect("deserialize chosen inline result succeeds");
        assert_eq!(
            ScopeKey::from_update(&chosen),
            ScopeKey {
                chat_id: 7,
                thread_id: INLINE_SCOPE_THREAD_ID
            },
            "a quick query answer never waits behind an inline generation"
        );
        assert_eq!(
            update_actor_id(&chosen),
            Some(7),
            "non-owner inline traffic is dropped by the dispatcher"
        );
    }
}
