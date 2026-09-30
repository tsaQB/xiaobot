/*
 * Xiao Console API contract.
 *
 * Every endpoint lives under `/api` and speaks JSON unless noted. Requests
 * that change anything (POST, PUT, PATCH, DELETE) must send the header
 * `X-Xiao-Request: 1`, and the browser's `Origin` must match the console's
 * own origin. The session is an HttpOnly, SameSite=Strict cookie, so the
 * client never sees or stores a token.
 *
 * Times are RFC 3339 strings in the server's local zone unless the field
 * name ends in `_secs` (a duration in whole seconds) or `_ms`.
 * Chat and user ids are sent as strings: Telegram ids can exceed 2^53.
 *
 * This file is the single source of truth for the Rust handlers in
 * `src/web/api/*.rs`; keep the two in step.
 */

/* ------------------------------------------------------------------ */
/* Shared                                                             */
/* ------------------------------------------------------------------ */

export type Lang = 'id' | 'en'

export type PageId =
  | 'home'
  | 'chat'
  | 'ai'
  | 'search'
  | 'mcp'
  | 'memory'
  | 'telegram'
  | 'whatsapp'
  | 'queue'
  | 'context'
  | 'logs'
  | 'system'
  | 'security'
  | 'login'

export type ErrorCode =
  | 'unauthorized' // no or expired session (401)
  | 'locked' // too many failed sign-ins from this address (429, see retry_after)
  | 'bad_code' // wrong or expired sign-in code (401)
  | 'bad_password' // wrong password (401)
  | 'not_configured' // the feature needs setup first, e.g. no bot token (409)
  | 'forbidden' // CSRF header/origin missing, or address not allowed (403)
  | 'invalid' // a field failed validation (400)
  | 'not_found' // (404)
  | 'conflict' // the change would break something else (409)
  | 'busy' // e.g. a chat session is still answering (409)
  | 'upstream' // Telegram, a provider or a search engine failed (502)
  | 'internal' // (500)

/** Body of every non-2xx response. */
export interface ApiErrorBody {
  /** English message. */
  error: string
  /** Indonesian message; falls back to `error` when absent. */
  error_id?: string
  code: ErrorCode
  /** Seconds until a `locked` address may try again. */
  retry_after?: number
}

export interface Ok {
  ok: true
}

/**
 * Where a setting's effective value comes from. The environment (including
 * a trusted `.env` and systemd `Environment=`) always wins over the value
 * saved in the database; the default applies when neither is set.
 */
export type ValueSource = 'environment' | 'vault' | 'database' | 'default' | 'none'

/**
 * Settings whose environment value overrides the saved one. The UI shows a
 * locked field with this description, e.g.
 * `".env file /root/.xiao.env"` or `"process environment"`.
 */
export type EnvLocks = Record<string, string>

/** A secret is never sent to the browser, only whether it is set. */
export interface SecretMeta {
  set: boolean
  /** Last four characters; always "" for the web password. */
  tail: string
  /** `vault` (file vault), `environment` (overridden, read-only) or `none`. */
  where: 'vault' | 'environment' | 'none'
}

/** Keys a `PUT /secrets/:key` accepts. Provider keys use `provider:<id>`. */
export type SecretKey =
  | 'BOT_TOKEN'
  | 'BRAVE_API_KEY'
  | 'TAVILY_API_KEY'
  | 'EXA_API_KEY'
  | 'XIAO_WEB_PASSWORD'
  | `provider:${string}`

/** Plain settings a `PUT /settings` accepts (secrets and the MCP URL have their own endpoints). */
export type SettingKey =
  | 'OWNER_USER_ID' // restart
  | 'ALLOWED_CHAT_IDS' // restart
  | 'DEDICATED_CHAT_IDS' // restart
  | 'AI_PROVIDER_CONNECT_TIMEOUT_SECS' // restart, 1..600
  | 'IMAGE_FALLBACK_PROVIDER' // live, "none" | "pollinations"
  | 'IMAGE_GENERATION_TIMEOUT_SECS' // live, 1..600
  | 'IMAGE_PROVIDER_CONNECT_TIMEOUT_SECS' // live, 1..600
  | 'IMAGE_DOWNLOAD_TIMEOUT_SECS' // live, 1..600
  | 'XIAO_HISTORY_RETENTION' // live, 0 = no limit
  | 'WHATSAPP_ENABLED' // gateway, "true" | "false"
  | 'WHATSAPP_OWNER_NUMBER' // gateway, digits only
  | 'WHATSAPP_DEDICATED_GROUPS' // gateway, comma separated
  | 'XIAO_WEB_BIND' // restart, "127.0.0.1:8787" | "0.0.0.0:8787" | "off"
  | 'XIAO_WEB_ALLOWED_NETWORKS' // restart, comma separated CIDRs
  | 'XIAO_WEB_SESSION_DAYS' // new sign-ins, "1" | "7" | "30"
  | 'XIAO_WEB_TELEGRAM_LOGIN' // live, "true" | "false" (false only while a password is set)

/** When a change takes effect. */
export type Effect = 'live' | 'restart' | 'gateway' | 'locked' | 'readonly'

/** Result of any write that may need a daemon restart to apply. */
export interface WriteResult {
  ok: true
  /** True while any saved change still waits for a restart. */
  restart_needed: boolean
  /** True when the WhatsApp gateway was restarted to apply the change. */
  gateway_restarted?: boolean
}

/* ------------------------------------------------------------------ */
/* Auth                                                               */
/* ------------------------------------------------------------------ */

/** GET /api/auth/state (works without a session) */
export interface AuthState {
  authenticated: boolean
  /** A sign-in code can be sent on Telegram (bot token and owner set, not turned off). */
  telegram_login: boolean
  /** A backup password is set. */
  password_login: boolean
  bot_username: string | null
  version: string
  /** The address the console listens on, e.g. "127.0.0.1:8787". */
  bind: string
  /** Only when authenticated. */
  owner_id: string | null
  /** Only when authenticated: when this session ends. */
  session_expires: string | null
}

/** POST /api/auth/code/send → a 6-digit code goes to the owner's private chat. */
export interface CodeSent {
  ok: true
  /** Seconds the code stays valid (300). */
  expires_in: number
}

/** POST /api/auth/code/verify */
export interface CodeVerifyRequest {
  code: string
}

/** POST /api/auth/password */
export interface PasswordLoginRequest {
  password: string
}

/* POST /api/auth/logout → Ok (this session)
 * POST /api/auth/logout-all → Ok (every session, this one included) */

/* ------------------------------------------------------------------ */
/* Home                                                               */
/* ------------------------------------------------------------------ */

export interface SystemSummary {
  version: string
  pid: number
  /** e.g. "armbian (aarch64)" */
  host: string
  started_at: string
  uptime_secs: number
}

export type AttentionCode =
  | 'restart_needed'
  | 'queue_failed' // count = failed updates
  | 'no_provider'
  | 'telegram_offline'
  | 'privacy_mode'
  | 'inline_feedback'
  | 'no_search_key'
  | 'search_paused' // names = paused engines
  | 'whatsapp_unlinked'
  | 'whatsapp_failed'

export interface AttentionItem {
  kind: 'err' | 'warn' | 'info'
  code: AttentionCode
  count?: number
  names?: string[]
  /** Page that fixes it. */
  to: PageId
}

/** GET /api/overview */
export interface Overview {
  system: SystemSummary
  telegram: {
    configured: boolean
    online: boolean
    username: string | null
    owner_id: string | null
    /** Seconds since the last successful getUpdates, null before the first. */
    last_poll_secs: number | null
  }
  whatsapp: {
    enabled: boolean
    linked: boolean
    phase: WaPhase
  }
  main_model: {
    provider: string | null
    model: string | null
    /** Models in the provider catalogue. */
    catalogue: number
  }
  search: {
    /** Name of the first engine that would answer now. */
    first: string | null
    /** Names of engines in a cooldown. */
    paused: string[]
    /** Number of search API keys set. */
    keyed: number
  }
  storage: {
    db_bytes: number
    attachments_bytes: number
    memories: number
    messages: number
    conversations: number
  }
  queue: {
    pending: number
    failed: number
  }
  attention: AttentionItem[]
  restart_needed: boolean
}

/* ------------------------------------------------------------------ */
/* AI and models                                                      */
/* ------------------------------------------------------------------ */

export type RoleId = 'main' | 'vision' | 'video' | 'audio_stt' | 'image_gen' | 'curator'

export type CapKey =
  | 'text_chat'
  | 'tools'
  | 'reasoning'
  | 'image_input'
  | 'video_input'
  | 'audio_input'
  | 'audio_transcription'
  | 'image_generation'
  | 'image_editing'
  | 'structured_output'
  | 'native_file_input'

export type CapState = 'supported' | 'unsupported' | 'unknown'
export type CapSource = 'provider_metadata' | 'active_probe' | 'known_provider_profile' | 'user_override'

export interface CapCell {
  state: CapState
  /** Evidence that decided the state; null when unknown. */
  source: CapSource | null
  /** The manual correction for this capability, if any. */
  override: 'yes' | 'no' | null
}

/** Every CapKey is present. */
export type CapMap = Record<CapKey, CapCell>

export type RouteView =
  | { type: 'main_model' }
  | { type: 'specific'; provider_id: string; model: string }
  | { type: 'disabled' }

export interface ProviderView {
  id: string
  name: string
  endpoint: string
  models: string[]
  active_model: string
  active: boolean
  key: SecretMeta
}

export interface RoleView {
  id: RoleId
  /** The main role's route is always `specific` (the active provider and model). */
  route: RouteView
  /** Resolved provider name and model, null when the role is off or unavailable. */
  provider: string | null
  model: string | null
  /** Why the role cannot run, e.g. "Provider 'x' not found". */
  error: string | null
  caps: CapMap
  /** Last capability check of the resolved model. */
  checked_at: string | null
}

/** GET /api/ai */
export interface AiState {
  providers: ProviderView[]
  roles: RoleView[]
  image: {
    fallback: 'none' | 'pollinations'
    gen_timeout: number
    connect_timeout: number
    download_timeout: number
  }
  ai_limits: { connect_timeout: number }
  env_locks: EnvLocks
  restart_needed: boolean
}

/** POST /api/ai/providers/test: checks an endpoint before it is saved. */
export interface ProviderTestRequest {
  endpoint: string
  /** Empty with `provider_id` = reuse that provider's stored key. Empty alone = keyless. */
  api_key?: string
  provider_id?: string
}

export interface ProviderTestResult {
  ok: boolean
  ms: number
  /** Normalized endpoint that was contacted. */
  endpoint: string
  models: string[]
  error: string | null
}

/** POST /api/ai/providers → ProviderView (saved; becomes active when it is the first) */
export interface ProviderCreateRequest {
  name: string
  endpoint: string
  /** Empty = keyless (local endpoint). */
  api_key: string
  /** Initial active model; defaults to the first catalogue entry. */
  model?: string
}

/** PUT /api/ai/providers/:id → ProviderView. An empty or missing api_key keeps the stored key. */
export interface ProviderUpdateRequest {
  name?: string
  endpoint?: string
  api_key?: string
}

/* DELETE /api/ai/providers/:id → Ok. 409 `conflict` while it is active or a route uses it. */

/** POST /api/ai/providers/:id/models: refetches the catalogue. */
export interface CatalogueRefreshResult {
  ok: true
  models: number
  added: number
  removed: number
  provider: ProviderView
}

/** POST /api/ai/active → Ok: sets the main provider and model. */
export interface ActiveModelRequest {
  provider_id: string
  model: string
}

/** PUT /api/ai/routes/:role → Ok (not for `main`). */
export type RouteRequest =
  | { type: 'main_model' }
  | { type: 'specific'; provider_id: string; model: string }
  | { type: 'disabled' }

/** PUT /api/ai/caps/:role → Ok: manual corrections for the role's resolved model. */
export interface CapOverrideRequest {
  overrides: Partial<Record<CapKey, 'auto' | 'yes' | 'no'>>
}

export type ProbeOutcome =
  | 'supported'
  | 'unsupported'
  | 'inconclusive'
  | 'auth_failed'
  | 'rate_limited'
  | 'timeout'
  | 'network_error'
  | 'protocol_mismatch'
  | 'provider_error'

/** POST /api/ai/probe/:role: capability probe of the role's resolved model. */
export interface ProbeResult {
  role: RoleId
  model: string
  outcome: ProbeOutcome
  saved: boolean
  caps: CapMap
  checked_at: string | null
  /** Human-readable progress lines from the probe. */
  log: string[]
}

/**
 * POST /api/ai/test/:role: a real request. `main` and `curator` send a short
 * chat; `image_gen` generates one test picture (may use credits); the other
 * roles run the capability probe.
 */
export interface RoleTestResult {
  role: RoleId
  ok: boolean
  ms: number
  model: string | null
  /** Short reply or outcome, for a toast. */
  detail: string
}

/* ------------------------------------------------------------------ */
/* Search and MCP                                                     */
/* ------------------------------------------------------------------ */

export type EngineId = 'brave' | 'tavily' | 'exa' | 'exa_mcp' | 'ddg' | 'wiki'

export interface EngineView {
  id: EngineId
  name: string
  /** Needs an API key. */
  keyed: boolean
  /** off = no key; cool = skipped for now after a failure; on = tried in order. */
  state: 'on' | 'off' | 'cool'
  /** Seconds left in the cooldown when state is `cool`. */
  cooldown_secs: number | null
}

/** GET /api/search */
export interface SearchState {
  /** In the order they are tried. */
  engines: EngineView[]
  keys: {
    BRAVE_API_KEY: SecretMeta
    TAVILY_API_KEY: SecretMeta
    EXA_API_KEY: SecretMeta
  }
  env_locks: EnvLocks
}

/* POST /api/search/cooldowns/reset → Ok */

/** POST /api/search/test */
export interface SearchTestRequest {
  query: string
  /** Picture mode: look for photos (at least 3). */
  pictures?: boolean
}

export interface SearchHit {
  title: string
  url: string
  summary: string
}

export interface SearchTestResult {
  ms: number
  /** Engine named in the result header, e.g. "Brave", "Wikipedia". */
  engine: string | null
  answer: string | null
  hits: SearchHit[]
  images: string[]
  /** The full tool output the model would see. */
  raw: string
}

export interface ToolView {
  name: string
  /** Model-facing description from the tool schema. */
  description: string
  /** Also offered in guest and inline mode. */
  guest: boolean
}

/** GET /api/mcp */
export interface McpState {
  url: string
  default_url: string
  tools: ToolView[]
  env_locks: EnvLocks
}

/** PUT /api/mcp → Ok. The URL passes the SSRF policy or the request fails with `invalid`. */
export interface McpUpdateRequest {
  url: string
}

/* POST /api/mcp/reset → Ok (restores the default endpoint) */

/** POST /api/mcp/test: a search through the endpoint (the saved one unless `url` is given). */
export interface McpTestRequest {
  query?: string
  url?: string
}

export interface McpTestResult {
  ok: boolean
  ms: number
  host: string | null
  snippet: string | null
  error: string | null
}

/* ------------------------------------------------------------------ */
/* Memory                                                             */
/* ------------------------------------------------------------------ */

export interface MemoryView {
  key: string
  fact: string
  updated_at: string | null
}

/** GET /api/memory */
export interface MemoryState {
  memories: MemoryView[]
  /** Facts that fit in the prompt (40) and characters kept per fact (300). */
  max_prompt: number
  max_chars: number
}

/** PUT /api/memory/:key → Ok. Key: 1..64 chars; fact: 1..300 chars. */
export interface MemoryUpsertRequest {
  fact: string
}

/* DELETE /api/memory/:key → Ok
 * DELETE /api/memory → Ok (every fact) */

/* ------------------------------------------------------------------ */
/* Channels                                                           */
/* ------------------------------------------------------------------ */

export interface BotInfo {
  id: string
  username: string
  first_name: string
  can_join_groups: boolean | null
  can_read_all_group_messages: boolean | null
  supports_inline_queries: boolean | null
  supports_guest_queries: boolean | null
}

/** GET /api/telegram */
export interface TelegramState {
  configured: boolean
  token: SecretMeta
  owner_id: string
  allowed_chat_ids: string
  dedicated_chat_ids: string
  /** From getMe; null when the token is missing or Telegram did not answer. */
  bot: BotInfo | null
  bot_error: string | null
  /** The daemon is polling. */
  online: boolean
  last_poll_secs: number | null
  /** getUpdates long-poll timeout in seconds. */
  poll_timeout: number
  /** Since the daemon started. Used to guess whether inline feedback is on. */
  inline_queries_seen: number
  chosen_results_seen: number
  /** Negative chat ids found in the stored history. */
  groups_seen: string[]
  env_locks: EnvLocks
  restart_needed: boolean
}

/** POST /api/telegram/check */
export interface TelegramCheck {
  ok: boolean
  ms: number
  bot: BotInfo | null
  webhook_url: string | null
  pending_updates: number | null
  error: string | null
}

export type WaPhase =
  | 'off' // gateway not running
  | 'starting'
  | 'pairing' // waiting for a QR scan or a pairing code
  | 'online'
  | 'retrying' // failed, restarting with backoff
  | 'logged_out'

export interface QrMatrix {
  /** Modules per side. */
  size: number
  /** SVG path data for the dark modules, in module units. */
  path: string
}

export interface PairingView {
  mode: 'qr' | 'code'
  /** Current QR code (mode qr). Replaced every ~20 s. */
  qr: QrMatrix | null
  /** 8-character pairing code, e.g. "K7QM-2XPA" (mode code). */
  code: string | null
  phone: string | null
  /** Seconds the current QR or code stays valid. */
  expires_in: number | null
  /** Linking finished; the gateway is online. */
  done: boolean
  error: string | null
}

/** GET /api/whatsapp */
export interface WhatsAppState {
  enabled: boolean
  linked: boolean
  phase: WaPhase
  /** Last failure reason while retrying. */
  last_error: string | null
  owner_number: string
  dedicated_groups: string
  queue: { pending: number; failed: number }
  pairing: PairingView | null
  env_locks: EnvLocks
}

/** POST /api/whatsapp/pair → PairingView. Refused with `conflict` when already linked. */
export interface PairRequest {
  mode: 'qr' | 'code'
  /** Required for `code`: 8 to 15 digits, international format without "+". */
  phone?: string
}

/* GET /api/whatsapp/pair → PairingView | null (poll every 2 s while pairing)
 * POST /api/whatsapp/pair/cancel → Ok
 * POST /api/whatsapp/unlink → Ok (stops the gateway and deletes whatsapp.db) */

/* ------------------------------------------------------------------ */
/* Queue                                                              */
/* ------------------------------------------------------------------ */

export interface QueueCounts {
  pending: number
  processing: number
  completed: number
  failed: number
}

export type QueueKind =
  | 'message'
  | 'edited'
  | 'guest'
  | 'inline' // inline query: expired, cannot be retried
  | 'inline_result'
  | 'callback'
  | 'stop'
  | 'other'

export interface FailedRow {
  channel: 'telegram' | 'whatsapp'
  /** update_id (Telegram) or message key (WhatsApp). */
  id: string
  kind: QueueKind
  chat_id: string | null
  thread_id: number | null
  /** True for the owner's private chat. */
  private: boolean
  attempts: number
  received_at: string
  error: string | null
  retryable: boolean
}

/** GET /api/queue */
export interface QueueState {
  telegram: QueueCounts
  whatsapp: QueueCounts
  /** Newest first, at most 200. */
  failed: FailedRow[]
}

/* POST /api/queue/:channel/:id/retry → Ok (back to pending with attempts reset, handled now)
 * DELETE /api/queue/:channel/:id → Ok (marked completed) */

/* ------------------------------------------------------------------ */
/* Context                                                            */
/* ------------------------------------------------------------------ */

export type ScopeKind = 'private' | 'group' | 'topic' | 'cli' | 'chat'

export interface ScopeTokens {
  system: number
  memory: number
  summary: number
  history: number
  /** Context window of the main model. */
  budget: number
}

export interface ScopeView {
  chat: string
  thread: number
  kind: ScopeKind
  /** Session name for `cli` scopes. */
  name: string | null
  /** Session id for `cli` scopes. */
  session_id: number | null
  messages: number
  summary: string | null
  summary_updated_at: string | null
  last_at: string | null
  tokens: ScopeTokens
}

/** GET /api/context */
export interface ContextState {
  /** Most recently active first, at most 100. */
  scopes: ScopeView[]
  retention: number
  model: string | null
  env_locks: EnvLocks
}

/** POST /api/context/clear → Ok */
export interface ContextClearRequest {
  chat: string
  thread: number
  op: 'history' | 'summary'
}

/* ------------------------------------------------------------------ */
/* Logs                                                               */
/* ------------------------------------------------------------------ */

export type LogLevel = 'ERROR' | 'WARN' | 'INFO' | 'DEBUG' | 'TRACE'

export interface LogLine {
  seq: number
  /** RFC 3339 */
  ts: string
  level: LogLevel
  target: string
  msg: string
}

/** GET /api/logs?after=<seq> → lines newer than `after`, oldest first (at most 500). */
export interface LogsPage {
  lines: LogLine[]
  /** Highest seq so far; pass it as `after` next time. */
  last: number
  /** Effective RUST_LOG filter. */
  filter: string
}

/* ------------------------------------------------------------------ */
/* System                                                             */
/* ------------------------------------------------------------------ */

export interface SettingRow {
  /** Setting key, or a label such as "provider" for derived rows. */
  key: string
  source: ValueSource
  /** Display value; secrets are masked. Empty = unset. */
  value: string
  effect: Effect
}

/** GET /api/system */
export interface SystemState {
  system: SystemSummary & {
    os: string
    arch: string
    data_dir: string
    /** Loaded `.env`, null when none. */
    config_file: string | null
    db_bytes: number
    attachments_bytes: number
    /** e.g. "xiao.service (systemd)"; null when not under a service manager. */
    service: string | null
    rust_log: string
  }
  settings: SettingRow[]
  env_locks: EnvLocks
  restart_needed: boolean
  /** Keys saved since start that wait for a restart. */
  restart_keys: string[]
}

/** PUT /api/settings → WriteResult. Only the keys sent are changed. */
export type SettingsRequest = Partial<Record<SettingKey, string>>

/** PUT /api/secrets/:key → WriteResult & { secret }. BOT_TOKEN is checked with getMe first. */
export interface SecretRequest {
  value: string
}

export interface SecretWriteResult extends WriteResult {
  secret: SecretMeta
  /** e.g. "@xiaofreebot" after a token check. */
  detail: string | null
}

/* DELETE /api/secrets/:key → WriteResult
 * POST /api/system/restart → Ok; the daemon stops gracefully and exits with
 *   code 75 so systemd starts it again. Poll /api/auth/state until it answers.
 * GET /api/system/backup → the SQLite database as a download (no secrets). */

/* ------------------------------------------------------------------ */
/* WebUI security                                                     */
/* ------------------------------------------------------------------ */

export type DeviceOs = 'android' | 'ios' | 'windows' | 'mac' | 'linux' | 'other'

export interface WebSessionView {
  id: string
  /** e.g. "Chrome on Android" */
  device: string
  os: DeviceOs
  ip: string
  created_at: string
  last_seen: string
  expires_at: string
  current: boolean
}

/** GET /api/security */
export interface SecurityState {
  /** Effective bind the server listens on now. */
  bind: string
  /** Saved bind (may differ until restart). */
  saved_bind: string
  port: number
  lan: boolean
  /** Primary LAN address of the server, for the "open from a phone" link. */
  lan_ip: string | null
  allowed_networks: string
  telegram_login: boolean
  /** Telegram is configured, so code sign-in can work. */
  telegram_available: boolean
  password: SecretMeta
  session_days: 1 | 7 | 30
  sessions: WebSessionView[]
  env_locks: EnvLocks
  restart_needed: boolean
}

/** PUT /api/security/password → Ok. At least 8 characters. */
export interface PasswordSetRequest {
  password: string
}

/* DELETE /api/security/password → Ok. 409 `conflict` while code sign-in is off or unavailable.
 * DELETE /api/security/sessions/:id → Ok
 * POST /api/security/sessions/revoke-others → Ok */

/* ------------------------------------------------------------------ */
/* Chat                                                               */
/* ------------------------------------------------------------------ */

export type UploadRoute = 'vision' | 'stt' | 'video' | 'doc'

export interface ChatSessionView {
  id: number
  name: string
  created_at: string
  messages: number
  last_at: string | null
  /** An answer is being generated from the web. */
  busy: boolean
}

/** GET /api/chat/sessions */
export interface ChatSessionsState {
  sessions: ChatSessionView[]
  /** Session last used in the terminal (`xiao chat`). */
  active: number | null
  /** Main model that answers. */
  model: string | null
}

/** POST /api/chat/sessions → ChatSessionView */
export interface ChatSessionCreateRequest {
  name?: string
}

/** PATCH /api/chat/sessions/:id → Ok. Name: 1..60 chars. */
export interface ChatSessionRenameRequest {
  name: string
}

/* DELETE /api/chat/sessions/:id → Ok (409 `busy` while answering)
 * POST /api/chat/sessions/:id/clear → Ok (409 `busy` while answering) */

export interface FileRef {
  /** Download id for GET /api/chat/files/:id; null for files that cannot be downloaded again. */
  id: string | null
  name: string
  size: number | null
  mime: string | null
}

export interface ChatMessageView {
  role: 'user' | 'assistant'
  /** User text, or the answer as Markdown (media shown as images and links). */
  text: string
  files: FileRef[]
  at: string | null
}

/** GET /api/chat/sessions/:id/messages → the newest 200 messages, oldest first. */
export interface ChatMessagesState {
  messages: ChatMessageView[]
  busy: boolean
}

/**
 * POST /api/chat/uploads: raw file body, header `X-File-Name` (URI-encoded
 * name) and `Content-Type`. At most 20 MB. Uploads expire after 30 minutes.
 */
export interface UploadView {
  id: string
  name: string
  size: number
  mime: string
  route: UploadRoute
}

/** POST /api/chat/sessions/:id/send → `text/event-stream` (see ChatEvent). 409 `busy` while answering. */
export interface ChatSendRequest {
  text: string
  /** Upload ids from POST /api/chat/uploads. */
  uploads: string[]
}

export type Activity =
  | 'thinking'
  | 'looking'
  | 'reading'
  | 'searching'
  | 'fetching'
  | 'writing'
  | 'listening'
  | 'drawing'
  | 'watching'
  | 'summarizing'
  | 'quiz'

/** `event: status` */
export interface ChatStatusEvent {
  label: string
  activity: Activity
}

/** `event: text`: the answer so far (replaces the previous partial). */
export interface ChatTextEvent {
  partial: string
}

/** `event: done` */
export interface ChatDoneEvent {
  answer: string
  thinking: string | null
  files: FileRef[]
  model: string | null
  secs: number
  stopped: boolean
  /** The provider failed; `answer` holds the explanation. */
  failed: boolean
}

/** `event: error`: the request could not start or broke off. */
export interface ChatErrorEvent {
  error: string
}

export type ChatEvent =
  | { event: 'status'; data: ChatStatusEvent }
  | { event: 'text'; data: ChatTextEvent }
  | { event: 'done'; data: ChatDoneEvent }
  | { event: 'error'; data: ChatErrorEvent }

/* POST /api/chat/sessions/:id/stop → Ok (like Telegram's stop button; the text so far is kept)
 * GET /api/chat/files/:id → the file, as an attachment download */
