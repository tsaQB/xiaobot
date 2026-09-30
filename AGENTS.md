# AGENTS.md

> Operational and architectural guide for AI coding agents working in the `xiaobot` repository (crate and binary name: `xiao`).

---

## 1. Project Overview

`xiao` is a hardened, single-owner AI assistant built in Rust (2021 edition) that serves **two channels**: Telegram (targeting **Telegram Bot API 10.3**) and WhatsApp multi-device. It supports Rich Messages (AST blocks), streaming drafts with native stop controls, durable inbox queueing with at-least-once recovery on both channels, three-tier long-term memory, and modular OpenAI-compatible multimodal AI routing (Main, Vision, Video, Audio STT, Image Generation, Curator). `xiao start` also serves the **Xiao WebUI**, an owner-only dashboard (Vue 3 in `webui/`, embedded into the binary, axum backend in `src/web/`).

---

## 2. Essential Commands

### Build & Check
Toolchain: Rust 1.94 or newer (the WhatsApp crates require 1.94; `rust-version` in `Cargo.toml`), CI pins 1.98.0. A C compiler is needed for the bundled SQLite and `ring`. The WebUI needs Node.js 20.19+ or 22.12+ (CI uses 24).

`build.rs` embeds `webui/dist/**` into the binary (`$OUT_DIR/webui_assets.rs`). Build the WebUI first; without `webui/dist` a placeholder page is embedded so the crate still compiles.
```bash
# WebUI: install (no install scripts), type check and build into webui/dist
cd webui && npm ci --ignore-scripts && npm run build && cd ..

# Verify compilation without emitting binaries
cargo check --locked

# Build development binary
cargo build --locked

# Build optimized release binary
cargo build --release --locked
```

### Testing
```bash
# Run all unit tests and contract tests
cargo test --locked

# Run a specific test by filter
cargo test <filter_name> --locked

# Run tests with stdout output enabled
cargo test --locked -- --nocapture

# Run the Telegram Bot API 10.3 contract suite only
cargo test --test bot_api_10_3_contract --locked

# Run the multimedia (tool arguments, media groups, rich media) contract suite only
cargo test --test telegram_multimedia_contract --locked
```

### Formatting & Linting
```bash
# Check code formatting (CI enforced)
cargo fmt --all -- --check

# Auto-format codebase
cargo fmt --all

# Run Clippy with strict warnings (CI enforced)
cargo clippy --locked --all-targets --all-features -- -D warnings
```

### CLI Subcommands (`xiao`)
The binary supports direct execution for testing, administration, and daemon operation:
```bash
# Default mode: Direct terminal chat REPL (interactive or one-shot query)
cargo run
cargo run -- "Hello, who are you?"
cargo run -- chat

# Open interactive Control Center launcher
cargo run -- menu

# Start Telegram daemon (default long-polling loop)
cargo run -- start

# Display system, database, and provider status dashboard
cargo run -- status

# Inspect token usage and context breakdown for a chat/thread
cargo run -- context [chat_id] [thread_id]

# Manage long-term user memories
cargo run -- memory
cargo run -- memory list
cargo run -- memory rm [key]
cargo run -- memory clear

# AI Provider and Specialist Addon Management
cargo run -- ai
cargo run -- ai list
cargo run -- ai use [model]
cargo run -- ai add
cargo run -- ai rm
cargo run -- ai provider [name]
cargo run -- ai addon
cargo run -- ai test [role]

# Web Search Engine Hub & API Keys (Brave, Tavily, Exa)
cargo run -- search
cargo run -- search test [query]
cargo run -- search brave [KEY|rm]
cargo run -- search tavily [KEY|rm]
cargo run -- search exa [KEY|rm]
cargo run -- search engine          # shows the automatic engine order (chosen by configured keys)

# Model Context Protocol (MCP) Server Registry & Tools
cargo run -- mcp
cargo run -- mcp list
cargo run -- mcp add <URL>
cargo run -- mcp rm                  # restores the default endpoint
cargo run -- mcp tools
cargo run -- mcp test [query]
cargo run -- mcp url [URL]
cargo run -- mcp reset

# Telegram Gateway & Owner Configuration
cargo run -- gateway
cargo run -- gateway check
cargo run -- gateway token <BOT_TOKEN>
cargo run -- gateway owner <OWNER_USER_ID>
cargo run -- gateway id <OWNER_USER_ID>

# WhatsApp Multi-Device Gateway
cargo run -- gateway wa                 # WhatsApp configuration menu
cargo run -- gateway wa pair            # Link by scanning a QR code
cargo run -- gateway wa code <NUMBER>   # Link using a phone pairing code
cargo run -- gateway wa owner <NUMBER>  # Set the owner phone number
cargo run -- gateway wa status          # Inspect link status
cargo run -- gateway wa unlink          # Delete the stored session

# Xiao WebUI (inside `xiao start`)
cargo run -- web                        # address, sign-in methods, whether it answers
cargo run -- web password               # set the backup password (argon2 hash in the vault)
cargo run -- web password rm            # remove it (refused while code sign-in is unavailable)
cargo run -- web bind <local|lan|off|ADDR:PORT>
cargo run -- web logout-all             # sign out every browser

# Interactive initial setup wizard
cargo run -- setup
```

---

## 3. Architecture & Control Flow

### High-Level Component Map
```
┌─────────────────────────────────────────────────────────────┐
│                       xiao CLI / Daemon                     │
└──────────────┬───────────────────────────────┬──────────────┘
               │                               │
       (Telegram Polling)               (Direct CLI Chat)
               ▼                               │
┌───────────────────────────────┐              │
│      TelegramBotClient        │              │
│  (Transport, SSRF, Fallback)  │              │
└──────────────┬────────────────┘              │
               ▼                               │
┌───────────────────────────────┐              │
│    Durable Intake Queue       │              │
│   (SQLite telegram_inbox)     │              │
└──────────────┬────────────────┘              │
               ▼                               │
┌───────────────────────────────┐              │
│   ChatRouteScope Evaluator    │              │
│  (Owner & Workspace Filter)   │              │
└──────────────┬────────────────┘              │
               ▼                               ▼
┌─────────────────────────────────────────────────────────────┐
│                       AIChatService                         │
│  ┌─────────────────────────┐   ┌─────────────────────────┐  │
│  │    Model Role Router    │   │    Three-Tier Memory    │  │
│  │ (Main, Vision, STT, etc)│   │ (Facts, Summary, Turns) │  │
│  └───────────┬─────────────┘   └────────────┬────────────┘  │
│              ▼                              ▼               │
│  ┌─────────────────────────┐   ┌─────────────────────────┐  │
│  │  Streaming SSE Decoder  │   │ SQLite Storage & WAL    │  │
│  │  & Timeline Draft Sync  │   │   (~/.local/share/...)  │  │
│  └─────────────────────────┘   └─────────────────────────┘  │
└─────────────────────────────────────────────────────────────┘
```

### Module Responsibilities
- `src/cli/`: Modular command-line subcommands and interactive interfaces:
  - `tui.rs`: Terminal UI engine, RAII raw-mode lifecycle guard (`CleanRawMode`), and ANSI layout formatters.
  - `launcher.rs`: Control Center interactive hub (`xiao menu`).
  - `status.rs`: Dashboard and diagnostic system status rendering.
  - `gateway.rs`: Telegram and WhatsApp gateway management (bot token, owner, QR/code pairing, link status).
  - `wizard.rs`: Interactive setup and onboarding quickstart. `get_or_prompt_token` opens it only while no AI provider exists and a terminal is attached; otherwise a missing bot token just keeps Telegram off.
  - `chat.rs`: Terminal chat REPL, smart one-shot queries, and multi-session manager (`/sessions`, `/switch`, `/rm`, `/new`). Each CLI session has its own history scope (`thread_id = cli_session_thread_id(session_id)`, a negative id), separate from the Telegram private chat. A one-shot query and leaving the REPL wait up to 30 seconds for background memory curation, so the process does not exit in the middle of it.
  - `memory.rs`: Tier-1 persistent memory management (`xiao memory`).
  - `context.rs`: Token usage and sliding-window breakdown inspector (`xiao context`).
  - `search.rs`: Web search engine hub and retrieval keys (`xiao search`).
  - `mcp.rs`: Model Context Protocol active endpoint and dynamic tool introspection (`xiao mcp`). XiaoBot manages one active endpoint rather than a server list, so `rm` restores the default endpoint.
  - `tests.rs`: Shared CLI argument-parsing tests.
  - `ai_hub.rs`: Provider, model catalog, and multimodal specialist routing (`xiao ai`).
  - `help.rs`: Global CLI help screen.
- `src/bot/`:
  - `daemon.rs`: Bot initialization (with the WebUI running, Telegram starts by itself once the bot token and owner are set, and a failed start is retried with backoff), Telegram connection handshake, command clearing (`pure zero-slash`), concurrent WhatsApp gateway spawn, and long-polling loop with graceful shutdown.
  - `worker.rs`: Keyed per-scope mailboxes (`ScopeKey`), worker concurrency limits, durable inbox queue replay, and bounded retry with panic isolation.
  - `router.rs`: Incoming update routing (new and edited messages), media and document classification, context overflow policies, and AI chat dispatch. End-to-end routing tests live in `router/flow_tests.rs`.
  - `inbound.rs`: Message kinds beyond text and classic media: stickers, locations/venues, live photos, forwarded rich messages, checklists and polls; reply context (`reply_context`, `reply_author`); and the edited-message window and text fingerprint.
  - `guest.rs`: Bot API 10.0 guest mode (`guest_message` → `answerGuestQuery` placeholder → inline edit with the final answer), plus the stateless generation and inline-edit delivery shared with inline mode.
  - `inline.rs`: Inline mode (`inline_query` → one placeholder result with a keyboard → `chosen_inline_result` → stateless generation → inline edit).
  - `media.rs`: Downloads and reads the file a message carries (photo or live-photo clip, sticker, voice, audio, video, video note, document) into `MessageMedia`, reporting problems instead of messaging the chat; shared by ordinary chats and guest mode.
  - `image_flow.rs`: Multi-step conversational image generation pipeline, prompt extraction, and structured fallback cards.
  - `client.rs`: The single Telegram Bot API client (every network call, retries, Rich → HTML → plain fallback chain, draft preparation, file downloads). An answer with media goes out in stages: remote media is downloaded (4 at a time) and uploaded with the message, and Telegram fetches the addresses that could not be downloaded (or returned a web page). If Telegram rejects the message, those addresses become links, then every remote picture does, and as a last resort the text is sent on its own, with links in place of media, and the staged files follow as documents. Only a 400 leads to the next stage: any other error is returned at once, since resending could deliver the answer twice. Behavioural tests live in `client/tests.rs` and run against the fake server in `test_support.rs`.
  - `client/raw.rs` / `client/raw/render.rs`: Transport-independent helpers the client delegates to: per-task delivery context, bounded SSRF-safe media downloads, remote-media-to-link conversion (all remote media, or only selected addresses, so the pictures that load stay in a collage or slideshow), text chunking, and the HTML/plain-text fallback renderers. (It no longer duplicates the Bot API calls.)
  - `models.rs` / `models/base.rs`: Type-safe Telegram API models, rich message block definitions (`RichBlock`), and validation bounds. `models/extras.rs` holds the reply, checklist and inline-mode objects (`TextQuote`, `Checklist`, `InlineQuery`, `ChosenInlineResult`); `models/rich_text.rs` turns received rich messages into readable text.
  - `transport_policy.rs`: Retry backoff logic, HTTP 429 rate limit parsing, and Bad Request fallback gates.
  - `url_policy.rs`: Outbound SSRF firewall preventing requests to private, loopback, link-local, and SIIT/NAT64 translated IP ranges.
- `src/ai/`:
  - `service/`: Modular AI orchestration engine:
    - `session.rs`: Session state transitions, active generation tracking, and cancellation signals.
    - `context.rs`: Token budget estimation, sliding-window message context assembly, and conversation trimming.
    - `prompt.rs`: System prompt assembly; long-term memory and summaries are sanitized, capped and fenced as untrusted data.
    - `generation.rs`: Streaming SSE lifecycle, provider HTTP dispatch, retry backoff, the tool execution loop, and the shared `race_with_cancel` cancellation helper.
    - `tool_round.rs`: Limits of that loop. The model may call tools in up to `MAX_TOOL_ROUNDS` (5) rounds, and one more request without tools follows so it always gets to write the answer. A generation runs at most `MAX_RESEARCH_CALLS` (10) `web_search`/`fetch_url` calls, 3 at a time, and all tool output shares one token budget (`ToolResultBudget`). A quiz or live photo that was already sent is not sent again, a reply that only confirms the quizzes are done (`QUIZ_DONE_REPLY`) is not shown, and tool steps that never ran are named in a notice asking the user to reply "lanjutkan".
    - `quiz.rs`: The `create_quiz` tool: preamble, native quiz with one or several correct answers, question/option/explanation pictures, description, shuffled options, revoting, open period (zero or negative means no limit) with results hidden until close (only with an open period), and a retry without pictures when Telegram cannot load them.
    - `live_photo.rs`: The `send_live_photo` tool. Live photos cannot be sent by URL, so both files are downloaded with `download_media_bytes` (SSRF-safe, bounded), the MP4 duration is checked (at most 10 seconds) before the photo is fetched, and the files are uploaded with `sendLivePhoto`. The call runs under `race_with_cancel`, so Stop and shutdown interrupt it, and only a successful send counts as delivered media.
    - `curator.rs`: Background memory curation: profile fact extraction and older-history summarization.
    - `image.rs`: Image generation providers, prompt translation, and base64/download resolution.
    - `multimodal.rs`: Specialist inputs (Vision, Video, Audio STT) and observation turn formatting.
  - `storage/`: Modular SQLite (WAL mode) persistence layer and secret store:
    - `secrets.rs`: Atomic filesystem secret vault (`0o600`/`0o700`), opaque file references (`secret://`, not encryption), and application settings.
    - `inbox.rs`: Durable Telegram inbox queue, state transitions, in-flight processing claims, and crash recovery.
    - `web.rs`: WebUI storage: `web_sessions` (token hashes only), queue counters and quarantine actions (retry resets attempts, dismiss marks completed and scrubs the payload), conversation scopes, memories with dates, session renames, and `VACUUM INTO` backups without browser sessions.
    - `wa_inbox.rs`: Durable WhatsApp inbox queue with the same at-least-once contract, keyed by `chat:sender:message_id`, with batch inserts for the durability hook and cursor-based replay paging.
    - `session.rs`: Chat sessions, scoped conversation turns, thread context queries, and topic summaries.
    - `memory.rs`: Tier-1 persistent user profile facts (key-value memory operations).
    - `provider.rs`: AI provider configurations, model registry, capability probe records, and specialist routes.
  - `routing.rs`: Specialist model role resolution (`ModelRole`: Main, Vision, Video, AudioStt, ImageGeneration, Curator).
  - `capability.rs` / `provider.rs`: Live model probe harness and capability verification (e.g. confirming whether an endpoint actually supports vision or tool calling).
  - `stream.rs`: UTF-8 chunk-safe Server-Sent Events (SSE) streaming decoder.
  - `tools.rs` / `tools/search.rs`: Function calling engine. `tools.rs` holds tool schemas, argument validation, and `fetch_url`; `tools/search.rs` holds the `web_search` engine chain: Brave → Tavily → Exa API (each only with a key) → keyless Exa MCP → DuckDuckGo, and Wikipedia when all of them fail. Engines listed in `XIAO_SEARCH_DISABLED` are skipped (all are on by default). An engine that just failed is skipped for a while (Exa MCP: 10 minutes after a 429, or its `Retry-After`, and 2 minutes after other failures; DuckDuckGo: 10 minutes after a failure, block or captcha). DuckDuckGo ads are dropped. A picture search with fewer than 3 images is topped up from Wikipedia and Wikimedia Commons, and the verified images are listed before the text results.
  - `http.rs`: Shared provider retry policy, retryable status classification, and `Retry-After` handling.
- `src/document.rs` & `src/document/archive.rs`: In-memory safe extraction of text, archives (ZIP, TAR, TAR.GZ, 7Z), Office files (DOCX, XLSX), and PDF page extraction/rendering.
- `src/attachments.rs`: Content attachment persistence scoped by chat/thread.
- `src/timeline.rs`: Real-time streaming draft management with progress spinner and activity state indicators. A private draft streams the answer, with a thinking block until the first words arrive; a group placeholder is a real message, so it shows the status as a paragraph (Telegram accepts thinking blocks in drafts only).
- `src/util.rs`: Shared string helpers, including character-safe truncation.
- `src/web/`: the Xiao WebUI, the owner-only dashboard served by `xiao start` (`web::start`, called from `run_daemon` before Telegram starts so it also works in web-only mode). The frontend (Vue 3 + Vite + TypeScript) lives in `webui/`; `webui/src/api/types.ts` is the API contract and must stay in step with the handlers.
  - `mod.rs`: `WebState` (AI service, bind, auth runtime, restart tracker, `WaController`, chat runtime, the attached Telegram link), the network guard (`XIAO_WEB_ALLOWED_NETWORKS`, loopback always allowed), the CSRF guard (mutations need `X-Xiao-Request: 1` and a same-origin `Origin`), security headers (strict CSP without eval, `no-store` on `/api`), and `request_restart` (graceful shutdown; under systemd `main` exits with `RESTART_EXIT_CODE` 75, otherwise it runs `run_daemon` again in the same process after `stop_server` released the address).
  - `auth.rs`: first-run setup (while no password and no Telegram code sign-in exist, a one-time setup code is printed to the terminal and the log, and `POST /api/auth/setup` creates the first password with it), sign-in with a 6-digit Telegram code (5 minutes, single use, at most 5 tries, resend after 30 s) or the backup password (`XIAO_WEB_PASSWORD`, argon2 via `spawn_blocking`); lockout after 5 failures in 15 minutes per address; session cookie `xiao_session` (HttpOnly, SameSite=Strict, 1/7/30 days) of which only the SHA-256 hash is stored; `require_session` middleware.
  - `settings.rs`: the settings the WebUI may change (validation and normalization), value sources, environment locks (`env_locks`), secret summaries (`SecretMeta`: set, last four characters, where; never the value) and `RestartTracker` (hashes of the restart-only settings at startup).
  - `net.rs`: `XIAO_WEB_BIND` parsing (`off`, `host:port`, bare port; ports below 1024 refused), a hand-written CIDR parser and matcher, LAN address detection.
  - `logs.rs`: `RingLayer`, a tracing layer keeping the last 500 lines for `GET /api/logs` (installed next to the stderr formatter in `init_tracing`), and the reloadable filter: `XIAO_LOG_LEVEL` applies at once unless `RUST_LOG` is set.
  - `wa.rs`: `WaController` starts, stops and pairs the WhatsApp gateway inside the daemon. Each run of `supervise_whatsapp` has its own stop signal that also follows the daemon shutdown; pairing QR codes and pairing codes arrive through `WaHooks` and are rendered as SVG path data (`qr_matrix`). An unlinked run stops pairing after 3 minutes (`PAIR_LIMIT`). `WHATSAPP_ENABLED` decides when set; unset, a linked session turns the gateway on.
  - `chat.rs`: web chat runtime. Web sessions are the `xiao chat` sessions (`thread_id = cli_session_thread_id(id)` in the owner's chat). A generation takes the scope's `generation_lock`, registers with `begin_generation` (Stop and shutdown reach it), streams progress through a `GenerationProgressSink` as SSE, keeps running when the browser leaves, and stores uploads (20 MB each, 30 minutes) and generated files (2 hours) in memory.
  - `api/*.rs`: the JSON API (`overview`, `ai`, `search` + MCP, `memory`, `channels` (Telegram and WhatsApp), `queue`, `context`, `system` (settings, secrets, restart, backup, logs), `security`, `chat`). Errors are `{error, error_id, code}` (`error.rs`).
  - `assets.rs` serves the embedded files; `cli.rs` implements `xiao web`.
- `src/gateway/`:
  - `mod.rs`: `DeliverySink` contract for plain-text channels. Telegram deliberately does not implement it, because rich blocks, streaming drafts, and interactive buttons cannot be expressed through a flat text interface.
  - `whatsapp/client.rs`: Connection loop, pairing, ordered intake, durable queueing, and generation dispatch.
  - `whatsapp/delivery.rs`: Outbound delivery, JID cache, and typing indicators.
  - `whatsapp/mapper.rs`: JID normalization, id mapping, and owner authorization.
- `src/parser/`:
  - `markdown.rs`: Converts extended markdown to Telegram Bot API 10.3 `RichBlock` AST representations. Media tags and media blocks are parsed in `markdown/media.rs`. Inline text is parsed in `markdown/inline.rs`; `markdown/extended.rs` handles highlight (`==x==`, `<mark>`), `<sup>`/`<sub>`, date-times (`<time datetime>`, `<tg-time>`, `tg://time` links) and custom emoji (`<tg-emoji>`, `tg://emoji` links), and flattens them for WhatsApp and the terminal. Tags inside inline code stay literal. `markdown/links.rs` handles in-message navigation: the parser emits placeholders, and `links::resolve` (run once in `parse_markdown_to_rich_blocks`) resolves them against the finished blocks. `[text](#section)` becomes an `anchor_link` to an `anchor` block placed before the matching top-level heading (an exact match wins over one without the numbering); a link without a matching heading keeps only its text. Footnotes (`[^id]` / `[^id]: note`, the note may start on the next line) become `reference_link` / `reference`, numbered by first mention; a `[^…]` without a note, such as the regex class `[^0-9]`, stays literal text, also on WhatsApp and in the terminal.
  - `whatsapp.rs`: Converts markdown to WhatsApp formatting and splits replies on character boundaries.
  - `web.rs`: `render_media_markup_for_web` turns tool media markup into Markdown for the WebUI chat (pictures stay pictures, audio/video/documents become links, `tg-map` a map link, `attach://` documents are dropped because they are downloaded separately); code is left untouched.
  - `latex.rs`: Sanitizes mathematical expressions for cross-platform Android and iOS rendering.
  - `rtl.rs`: Detects Right-to-Left (RTL) scripts (Arabic, Hebrew, Persian, Urdu, etc.) and Eastern Arabic numerals, automatically setting layout direction and right-aligned table cells.
  - `terminal.rs`: ANSI terminal rendering for CLI chat and logs.

---

## 4. Key Architectural & Security Invariants

When modifying or adding features, you **must** preserve these invariants:

### 1. Hard Single-Owner Invariant
- **Rule**: Non-owner updates are dropped silently at the gateway (`src/bot/router.rs`).
- **Implementation**: `ctx.user_id != self.owner_user_id` immediately yields `RouteDecision::Ignore`. Never respond to, log, or leak bot existence to unauthorized Telegram IDs.

### 2. Pure Zero-Slash Gateway
- **Rule**: Xiao is designed as a natural conversational gateway.
- **Implementation**: `bot.set_my_commands(&[])` is executed at startup to clear Telegram slash menus. User requests (including image generation and clear requests) are detected conversationally via regex/heuristics or handled via CLI.

### 3. Outbound SSRF & Network Security
- **Rule**: Any remote fetch of a URL that came from a user, a model, or a web page must go through `bot::url_policy::fetch_public_url` (or, for single-hop downloads without redirects, `resolve_download_url` plus a pinned client). `fetch_public_url` re-validates every redirect hop with `resolve_redirect_hop`, pins the connection to the vetted IP, bypasses ambient proxies, and bounds the body.
- **Users**: `fetch_url` (`ai::tools::fetch_web_content`), the DuckDuckGo result scraper (including the result pages it reads for pictures when a picture search found none), and `client/raw.rs::download_media_bytes` (the Telegram media re-upload downloader, the `send_live_photo` tool, and picture links in inline questions). Generated-image downloads (`service/image.rs::download_generated_image`) use `resolve_download_url` with a pinned client and no redirects.
- **Blocked**: Loopback (`127.0.0.0/8`, `::1`), RFC 1918 private subnets, link-local addresses, SIIT/NAT64-mapped IPv6, and unsafe URI schemes.

### 4. Secret Isolation
- **Rule**: Plaintext API keys and bot tokens are **never** committed, written to SQLite in plaintext, or printed in debug logs.
- **Mechanism**: Secrets are written to disk under `~/.local/share/xiaoai/secrets/` with strict `0o600` file / `0o700` directory permissions. The database only stores a `secret://` URI reference (`api_key_ref`). The reference is an opaque file name, not encryption: protection comes from the file permissions (on Windows, from the per-user profile ACL).
- **Coverage**: `secrets.rs::secret_setting_namespace` routes `BOT_TOKEN`, `AI_API_KEY`, the search keys (`BRAVE_API_KEY`, `TAVILY_API_KEY`, `EXA_API_KEY` and the legacy `TAVILY_KEY` / `EXA_KEY`) and any setting ending in `_API_KEY`, `_TOKEN`, `_SECRET` or `_PASSWORD` to the vault. Legacy plaintext rows are migrated on first read.

### 5. Durable Intake Queue, Keyed Mailbox Isolation & Native Stop
- **Queue**: Telegram long-polling writes incoming updates directly to SQLite table `telegram_inbox` as `pending`.
- **Owner Prefilter**: The dispatcher drops non-owner updates (marking them completed) before they reach a mailbox or a concurrency permit; the router's owner check remains as the second line.
- **Per-Scope Keyed Mailboxes**: Updates are dispatched into dedicated per-scope channels (`ScopeKey { chat_id, thread_id }`). This guarantees strict FIFO processing within any individual chat or topic while processing different chats concurrently up to a global semaphore limit (`GLOBAL_WORKER_PERMITS` = 8). When a scope mailbox is full (`SCOPE_MAILBOX_CAPACITY`), the dispatcher waits for room (backpressure) instead of dropping the update. Mailbox workers gracefully despawn after `SCOPE_IDLE_TIMEOUT` using an atomic critical section to prevent lost messages.
- **Task Outcomes**: Handlers report `TaskOutcome` through `bot::worker::record_task_outcome`. `Completed` marks the row done; `DeliveryFailed(reason)` quarantines it as `failed` with the reason (the reply could not be delivered after `deliver_final_answer`'s retries); `Interrupted` (cancelled by shutdown) returns it to `pending` without spending an attempt so it is answered after restart.
- **Graceful Shutdown**: `AIChatService::begin_shutdown` sets the shutdown flag and cancels generations; queued-but-unstarted updates stay `pending`; the dispatcher closes all mailboxes and waits (bounded by `SHUTDOWN_GRACE`) for scope workers. Signals: Ctrl+C everywhere, SIGTERM/SIGHUP on Unix, console close/logoff/shutdown on Windows (`daemon::wait_for_shutdown_signal`).
- **Panic Isolation & Bounded Retry**: Each update is executed inside an isolated `tokio::spawn` task with unwinding panic protection (`JoinError::is_panic()`).
  - Upon transient panic, the global concurrency permit is immediately released (`drop(permit)`), a 1.5-second backoff sleep occurs, and the update is retried immediately in-worker up to a maximum of 2 attempts (`attempts <= 2`, seeded and tracked directly in SQLite).
  - If a task panics consecutively or exceeds 2 attempts, it is quarantined as `failed` ("poison pill") to protect the queue from infinite crash loops.
- **Operational Trade-Off & Duplicate Side-Effect Risk**:
  - Because in-worker retries trigger rapidly (~1.5s) on *any* panic without killing the daemon process, external side effects executed before an unexpected panic (e.g. an outbound Telegram message draft/reply already dispatched or an intermediate turn committed to SQLite before final inbox checkpointing) **will repeat upon retry**.
  - This is an intentional operational trade-off of at-least-once processing semantics: Xiao guarantees zero message loss over exactly-once execution.
- **Crash Recovery**: On startup, `recover_telegram_processing_async()` resets any in-flight `processing` updates back to `pending` to guarantee at-least-once recovery across process restarts, while quarantining updates with `attempts >= 2` (`quarantine_telegram_update_async` works on `pending` rows too). Stop updates and inline queries from before a restart are acknowledged, not replayed.
- **Native Stop Priority**: `stopped_message_generation` updates bypass worker mailboxes and execute immediately (in their own task, so a panic cannot take down the poll loop) to cancel in-flight generation tokens with zero latency. When the user stops a private-chat answer, the partial text is sent as a real message, because Bot API drafts disappear after ~30 seconds.
- **Drafts Are Ephemeral**: `sendRichMessageDraft` is only a temporary preview. Every generation, including failed or interrupted ones, must end with a real `sendRichMessage` (`timeline::finalize_answer_with_media`); drafts never carry uploads or URL media (`TelegramBotClient::prepare_draft_message`). Thinking blocks are accepted in drafts only, so a group placeholder, which is a real message, shows its status as a paragraph.
- **Retry Safety**: Timeouts are retried only for idempotent methods (`transport_policy::is_idempotent_method`); a timed-out `send*` is reported rather than risking a duplicate message.

### 6. Specialist Context Isolation
- **Rule**: Canonical conversational history belongs solely to the `Main` model.
- **Specialists**:
  - `Vision` / `Video`: Receive only the current media payload and the immediate user prompt; never the full conversation history.
  - `AudioStt`: Returns transcripts to Main; never receives previous conversation history.
  - Specialist outputs are returned to Main as bounded observation turns.

### 6a. Guest Mode Is Stateless
- Only the owner's `guest_message` is answered; anyone else is dropped with no reply.
- Guest generations run with `GenerationInput.guest_mode = true`: the guest system prompt instead of memories and summaries, no history loaded or saved, no curator, and only `GUEST_MODE_TOOLS` (`web_search`, `fetch_url`). Other tool calls get a refusal result.
- Nothing is ever sent to the guest `chat.id` (it may coincide with an unrelated chat): `bot` is `None` for the generation and the reply is edited through `inline_message_id` only. Guest updates use their own mailbox (`thread_id = GUEST_SCOPE_THREAD_ID`).
- A photo, voice note, video or document in the owner's guest message, or in the message it replies to, is loaded with `bot::media::load_message_media` after the placeholder is up and passed to the generation; it is never stored, and a download failure only adds a note to the prompt.
- The guest query is answered once with a placeholder. An interrupted generation edits it into a "call again" notice rather than returning `Interrupted`, because the query cannot be answered twice.

### 6a-bis. Inline Mode Is Stateless Too
- Only the owner's `inline_query` is answered (one result, `is_personal`, `cache_time: 0`); anyone else gets no answer. The result carries an inline keyboard because Telegram only reports `inline_message_id` for messages with one.
- The generation starts on `chosen_inline_result` (requires `/setinlinefeedback` in @BotFather) and follows the guest-mode rules: guest system prompt, no history or memories, read-only tools, `bot` is `None`, and only the inline message is edited.
- Inline questions are text only; the first picture link (`.jpg`, `.jpeg`, `.png`, `.webp`) is fetched with `download_media_bytes` (SSRF-safe, 10 MB) and passed as an image. Other links are left to `fetch_url`.
- Inline queries and chosen results use separate owner mailboxes (`INLINE_QUERY_SCOPE_THREAD_ID`, `INLINE_SCOPE_THREAD_ID`) so a placeholder answer never waits behind a generation. Stale `inline_query` rows are acknowledged on replay; an interrupted chosen result returns `Interrupted` and is answered after restart, since an inline message can still be edited.

### 6b. Edited Messages
- An `edited_message` is answered again only if its text/caption changed, the edit is within `inbound::EDIT_WINDOW_SECS` (10 minutes), and the message is still the latest answered prompt in its chat/topic (`telegram_latest_prompts`). Live location updates are never answered.
- `claim_edited_prompt_async` is the dedup point: it stores only a hash of the text, and accepts a crash-recovery replay of the same `update_id`.
- The new answer replies to the edited message; the earlier answer is not touched.

### 6c. Reply Context
- A reply carries the replied-to message to the model through `ChatInput.reply_context`, fenced and labelled as quoted material, never as instructions. Image-intent detection reads only the owner's own words.
- `inbound::reply_context` returns an `inbound::ReplyContext` in two sizes: `for_model` (up to `MAX_QUOTED_CHARS`) goes to the model, `for_history` (up to `HISTORY_QUOTE_CHARS`, 1,000) is what `GenerationInput.canonical_prompt` stores in history. A `TextQuote` (the part the user selected) is always included.
- A reply without its own attachment reuses the replied message's media. That media is best effort: download or format failures are not reported and the question is answered from the quoted text.

### 7. WhatsApp Single-Owner Boundary
- Authorization is decided on the **phone number**, never on raw JID text.
- Both `sender` and `sender_alt` are inspected so LID addressing mode is still recognized.
- Device suffixes (`:12`) and agent suffixes (`.0`) must never leak into the parsed number; use the structured `Jid.user` field, not the string form.
- Unauthorized senders are dropped with no reply and no identity trace in the logs.
- Group chats map to a negative `chat_id`; direct messages map to the positive sender number. History is stored under `chat_id`, and the sender number is passed as `user_id`.

### 7b. Xiao WebUI
- **Owner only**: every page and API call except `/api/auth/*` requires a session; sign-in is always required, also from loopback (tunnels arrive as local connections). Only the owner can receive the Telegram code; the backup password is an argon2 hash in the vault.
- **Network**: `XIAO_WEB_BIND` defaults to `127.0.0.1:8787`; `off` disables the WebUI. When it listens beyond loopback, clients outside `XIAO_WEB_ALLOWED_NETWORKS` get a bare 403.
- **Secrets**: never returned to the browser (`SecretMeta` only). Settings the environment overrides are reported as locked and refused on write.
- **CSRF and CSP**: mutations need `X-Xiao-Request: 1` and a matching `Origin`; the CSP forbids inline scripts and eval, and the frontend renders Markdown to VNodes (never `v-html`).
- **Invariants still hold**: the MCP URL passes `url_policy::resolve_download_url` before it is saved; provider keys and tokens go through `save_provider_store` / `save_app_setting` (vault); retried Telegram updates go back through the durable queue and the normal worker path.
- **Restarts**: settings read only at startup (`RESTART_KEYS` in `web/settings.rs`) are tracked; a restart from the WebUI shuts down gracefully, then exits with status 75 under systemd (the service must use `Restart=always` or `Restart=on-failure`) or runs the daemon again in the same process when started from a terminal.

### 8. WhatsApp Credential Handling
- `whatsapp.db` holds Signal session keys and is treated the same as the secret vault.
- The file and its `-wal` / `-shm` sidecars are locked to `0o600` on Unix systems.
- Only one process may use the session: `WhatsAppGateway::start` and `xiao gateway wa unlink` hold an exclusive `whatsapp.lock` (`WhatsAppGateway::lock_session`, `File::try_lock`), so CLI pairing never runs alongside the daemon, and `wa pair` refuses an already linked session.

### 9. WhatsApp Ordering and Durability
- **At-least-once intake**: `DurableIntakeHook` (an `InboundDurabilityHook`) writes every authorized message to `whatsapp_inbox` in one transaction **before** the SDK acknowledges it. Without the hook the SDK acks on decrypt (at-most-once).
- **Fast callback**: The ordered `on_message` callback only classifies the message and enqueues it into a per-chat mailbox (`ChatMailboxes`). Generation runs in per-chat workers, so messages within a chat keep arrival order while different chats run concurrently (8 permits). If the SDK still reports dropped events, a replay sweep processes the durable rows.
- **Dedup key**: `"{chat_jid}:{sender}:{message_id}"`; stanza ids are only unique per chat and sender. Claims (`pending` → `processing`) are the dedup point for redeliveries.
- **Media replay**: The encoded protobuf message is stored with the row (`message_b64`), so attachments can be downloaded again after a restart. Older rows without it are answered with an explanation instead of being silently dropped. Payloads are scrubbed once a row is completed.
- Every job runs through the same `execute_with_scoped_retry` engine Telegram uses (panic isolation, bounded retry, quarantine, `TaskOutcome`).
- Generations register with `begin_generation` so application-wide cancellation on shutdown actually reaches WhatsApp, and hold a per-chat `generation_lock` so two generations cannot interleave history writes.
- **Groups**: Owner messages in a group are answered only when they mention the bot, reply to it, start with `/`, or the group is listed in `WHATSAPP_DEDICATED_GROUPS`.
- **Delivery**: Long replies are split into numbered parts (`(n/m)`) with per-part retries; staged documents are uploaded and sent as WhatsApp documents.
- **Lifecycle**: `daemon::supervise_whatsapp` restarts a failed session with backoff and alerts the owner on Telegram; a server-side logout deletes the dead session and stops the gateway. On startup, `recover_whatsapp_processing_async()` returns in-flight rows to `pending` and the backlog is replayed page by page.

---

## 5. Storage & State Layout

SQLite database location and files default to:
- **Base directory** (`storage::xiao_data_dir`): `$XIAO_DATA_DIR` if set; otherwise `%APPDATA%\xiaoai` on Windows, `$XDG_DATA_HOME/xiaoai`, or `~/.local/share/xiaoai`
- **Database**: `xiaoai.db` (configured with `PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;`)
- **Secrets directory**: `<base>/secrets/` (e.g. `~/.local/share/xiaoai/secrets/`)
- **Attachments directory**: `<base>/attachments/<chat_id>/<thread_id>/`

### Database Tables:
- `settings`: Key-value application configuration.
- `sessions`, `active_sessions`, `session_counters`: Chat session tracking.
- `messages`: Canonical conversation history, pruned per chat/topic to the newest `XIAO_HISTORY_RETENTION` messages (default 2000, `0` disables). Each exchange is stored in one transaction.
- `user_memories`: Tier-1 persistent user profile facts (Name, Tech Stack, Preferences).
- `scoped_summaries`: Tier-2 condensed summaries of older topics per chat/thread.
- `telegram_inbox`: Durable update queue for Telegram intake.
- `telegram_state`: Long-polling offset.
- `telegram_latest_prompts`: Latest answered owner message per chat/topic (message id, text hash, claiming update id) for edited-message handling.
- `whatsapp_inbox`: Durable message queue for WhatsApp intake, keyed by `chat:sender:message_id`; completed rows are kept as dedup tombstones (newest 5000 and anything from the last 14 days).
- `web_sessions`: Signed-in WebUI browsers (public id, SHA-256 of the token, created/last seen/expiry as Unix seconds, address, user agent).

### Other files:
- **WhatsApp session**: `<base>/whatsapp.db` plus `-wal` / `-shm` sidecars, locked to `0o600` on Unix.

### Configuration Resolution Order
Configuration is resolved by `get_config_path()` in `src/main.rs`, in order:
1. `.env` in the current working directory — only if trusted (on Unix: owned by the `$HOME` owner and not group/world-writable)
2. `$XDG_CONFIG_HOME` descendants (`xiao/`, `.xiao/`, `xiaoai/`)
3. `$HOME` or `$USERPROFILE` descendants (`.xiao.env`, `.xiao/`, `.config/xiao/`, and legacy variants)
4. `%APPDATA%` descendants on Windows
5. A trusted `.env` in a parent of the working directory

The full effective list lives in `src/main.rs`; treat the code as the source of truth.

Settings the WebUI changes are saved the same way (`save_app_setting`, secrets in the vault). Image timeouts (`timeout_from_env`), `IMAGE_FALLBACK_PROVIDER` and `XIAO_HISTORY_RETENTION` are read through `configured_setting` on every use, so saved changes apply without a restart.

Individual settings are read with `configured_setting` (and `ai::tools::search`'s `setting` for search keys and the MCP URL): the environment (including `.env`) wins, an empty value counts as unset, and otherwise the value saved by the CLI is used. CLI writes go through `save_env_kv`, which calls `warn_if_environment_overrides` so a saved value that the environment overrides is reported instead of silently ignored.

---

## 6. Testing Patterns & Guidelines

- **Unit tests**: Colocated in each source file within `#[cfg(test)] mod tests { ... }`.
- **Contract tests**: Two suites include the model and parser sources directly (`#[path]`):
  - `tests/bot_api_10_3_contract.rs`: serialization and wire compatibility against Telegram Bot API 10.3: discriminator fields (e.g., `type: "voice_note"`, `type: "button"`), rich block bounds (`RICH_MESSAGE_MAX_TEXT_CHARS = 32_768`, `RICH_MESSAGE_MAX_BLOCKS = 500`), media group constraints (albums must contain 2 to 10 homogeneous items), update shapes, extended rich-text entities, and in-message navigation.
  - `tests/telegram_multimedia_contract.rs`: multimedia tool arguments (collage, location, document, audio), `InputMedia` wire formats, and rich-message media references.
- **WebUI**: CI job `webui` runs `npm ci --ignore-scripts`, `vue-tsc`, `vite build` and `npm audit`, then uploads `webui/dist`; every Rust job downloads it before building.
- **Behavioural tests**: `bot/test_support.rs` provides an in-process fake Telegram Bot API server (`FakeTelegram`) and a fake streaming provider (`FakeProvider`); client, router, guest, inline, quiz and live-photo tests assert on the requests the real code sends. Service-level tests use the process-wide SQLite database, so each test owns a distinct chat id.
- **Mocking & Isolation**:
  - Tests do not require a live Telegram bot token or active AI provider; network calls in tests use mock HTTP responses or test synthetic structs.
  - When writing tests involving `rusqlite`, use in-memory SQLite connections (`Connection::open_in_memory()`) or isolated temporary directories.

---

## 7. Development Gotchas & Conventions

1. **Telegram Rich Block Serialization**:
   - `RichBlock` types serialize directly to JSON structures required by Telegram 10.3. Ensure all variants adhere to base schemas in `src/bot/models/base.rs`.
   - When converting markdown, use `build_full_rich_message` or `parse_streaming_markdown_to_rich_blocks`.
2. **Decompression & Archive Limits**:
   - Archive extraction in `src/document/archive.rs` enforces strict resource caps to defeat zip bombs:
     - Max total uncompressed bytes: 30 MB (`MAX_ARCHIVE_TOTAL_UNCOMPRESSED_BYTES`).
     - Max single entry: 2 MB (`MAX_ARCHIVE_SINGLE_ENTRY_BYTES`).
     - Nested archives are classified as `NestedArchive` and **not** unpacked recursively.
3. **Android / Termux Platform Context**:
   - This project compiles and runs inside Termux on Android (`aarch64-linux-android`), on standard Linux servers (`aarch64-unknown-linux-gnu` and `x86_64`), and on Windows (`x86_64-pc-windows-msvc`); CI uploads ARM64 Linux, Android and Windows binaries as workflow artifacts.
   - Keep build dependencies pure Rust where possible (e.g., `rustls-tls` is enabled in `reqwest`, `rusqlite` uses the `bundled` feature).
4. **Git Commits & Formatting**:
   - Always run `cargo fmt --all -- --check` and `cargo clippy --locked --all-targets --all-features -- -D warnings` before committing.
   - Commit messages must follow project conventions: clear, descriptive, under 72 characters on the first line, focusing on why the change exists.
5. **RTL & Bidirectional Layout (Bot API 10.3)**:
   - When text or rich block contents contain RTL scripts (Arabic, Hebrew, Persian, Urdu) or Eastern Arabic-Indic / Hindi numerals (`٠..٩` / `\u0660..\u0669`), `InputRichMessage.is_rtl` must be set to `Some(true)`.
   - Markdown and Unicode box tables automatically detect RTL content in headers and cells, defaulting unspecified column alignments to `"right"` so Telegram mirrors and renders them naturally from right to left.
6. **Zero `.unwrap()` Policy & `clippy::unwrap_used = "deny"`**:
   - The entire codebase strictly enforces zero `.unwrap()` calls across both production code and test suites.
   - Any runtime production code must use idiomatic error handling (`?`, `match`, `if let`, `unwrap_or`, `unwrap_or_else`, etc.).
   - Static constants (such as compiled Regex) and unit/contract test assertions use `.expect("descriptive invariant or failure explanation")`.
   - `clippy::unwrap_used = "deny"` is configured in `Cargo.toml [lints.clippy]`, causing any compiler check or CI run containing `.unwrap()` to fail immediately.

