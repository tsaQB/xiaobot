<div align="center">

```
██╗  ██╗██╗ █████╗  ██████╗ 
╚██╗██╔╝██║██╔══██╗██╔═══██╗
 ╚███╔╝ ██║███████║██║   ██║
 ██╔██╗ ██║██╔══██║██║   ██║
██╔╝ ██╗██║██║  ██║╚██████╔╝
╚═╝  ╚═╝╚═╝╚═╝  ╚═╝ ╚═════╝ 
```

### Xiao (小)
**A hardened, single-owner AI assistant for Telegram and WhatsApp built with Rust.**  
*Engineered for Telegram Bot API 10.3 • WhatsApp Multi-Device • Durable SQLite Intake Queue • Three-Tier Memory • Multimodal Routing*

---

[![CI](https://github.com/tsaQB/xiaobot/actions/workflows/build.yml/badge.svg)](https://github.com/tsaQB/xiaobot/actions)
![Telegram Bot API](https://img.shields.io/badge/Telegram%20Bot%20API-10.3-2CA5E0?logo=telegram&logoColor=white)
![Rust](https://img.shields.io/badge/Rust-2021%20Edition-DEA584?logo=rust&logoColor=white)
![MSRV](https://img.shields.io/badge/MSRV-1.94%2B-lightgrey)
![Platforms](https://img.shields.io/badge/Platforms-Linux%20%7C%20Armbian%20%7C%20Termux%20%7C%20Windows-097ABB?logo=linux&logoColor=white)
![License](https://img.shields.io/badge/License-MIT-green.svg)

[Key Features](#-key-features) • [Architecture](#-architecture) • [Quickstart](#-quickstart) • [Installation](#-multi-platform-installation) • [CLI & Terminal Chat](#-cli--terminal-chat) • [Configuration](#-configuration-reference)

---

</div>

## 🌟 Highlights

Xiao is an autonomous, single-owner AI gateway designed to run continuously on low-overhead environments, from cloud servers to single-board computers (Armbian) and edge smartphones (Android Termux). It serves two channels: Telegram, treated not as a simple chat wrapper but as a rich display surface powered by **Telegram Bot API 10.3**, and WhatsApp multi-device.

- **Telegram Bot API 10.3 Native**: Real-time streaming drafts (`sendRichMessageDraft`), native stop controls, AST layout blocks (tables, expandable quotes, collages, slideshows, thinking indicators), in-message links and footnotes, and cross-platform LaTeX rendering.
- **Hardened Single-Owner Boundary**: Zero information leakage. Non-owner updates are dropped silently at the network boundary without acknowledging bot existence.
- **Pure Zero-Slash Gateway**: Runs with empty command menus (`set_my_commands(&[])`). Interacts naturally through conversational intent, context-aware mentions, media attachments, or dedicated forum topics.
- **Durable SQLite WAL Intake Queue**: Ingests updates to an ACID SQLite inbox (`telegram_inbox`, `whatsapp_inbox`) before acknowledgment. Dispatches to per-scope FIFO mailboxes with task-level panic isolation, bounded retry (2 attempts), and poison-pill quarantine.
- **WhatsApp Multi-Device Gateway**: Self-service linking by QR scan or phone pairing code, owner authorization by phone number across both legacy and LID addressing modes, ordered per-chat processing, and crash-safe queueing.
- **Three-Tier Long-Term Memory**: Tier 1 (Autonomous profile facts), Tier 2 (Sliding-window topic summaries), and Tier 3 (Full thread-scoped turns).
- **Specialist Context Isolation**: Routes queries across `Main`, `Vision`, `Video`, `Audio STT`, `Image Generation`, and `Curator`. Specialist models only receive transient media payloads, preventing token context exhaustion and preserving privacy.
- **In-Memory Document & Anti-Bomb Inspection**: Safe extraction of PDF, DOCX, XLSX, text, code, and archives (ZIP, TAR, 7Z) with strict memory quotas and anti-zip-bomb limits.
- **Autonomous Tool Calling**: Built-in `web_search` (Brave → Tavily → Exa API when keys are configured, then keyless Exa MCP → DuckDuckGo, with Wikipedia for verified images), SSRF-hardened `fetch_url`, and tools for quizzes, photos, collages, slideshows, audio, voice notes, live photos, locations, documents and archives.

---

## 🏛️ Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                       xiao CLI / Daemon                     │
└──────────────┬───────────────────────────────┬──────────────┘
               │ (Telegram Long Polling)       │ (Direct Terminal REPL)
               ▼                               │
┌───────────────────────────────┐              │
│      TelegramBotClient        │              │
│   (SSRF Firewall & Fallback)  │              │
└──────────────┬────────────────┘              │
               ▼                               │
┌───────────────────────────────┐              │
│     Durable Intake Queue      │              │
│    (SQLite WAL telegram_inbox)│              │
└──────────────┬────────────────┘              │
               ▼                               │
┌───────────────────────────────┐              │
│    ChatRouteScope Evaluator   │              │
│  (Hard Single-Owner Gateway)  │              │
└──────────────┬────────────────┘              ▼
               │                  ┌───────────────────────────┐
               └─────────────────►│       AIChatService       │
                                  └─────────────┬─────────────┘
                                                │
         ┌──────────────────────────────────────┴──────────────────────────────────────┐
         ▼                                                                             ▼
┌───────────────────────────────────┐                         ┌───────────────────────────────────┐
│         Model Role Router         │                         │         Three-Tier Memory         │
│  ├─ Main (Canonical History)      │                         │  ├─ Tier 1: User Profile Facts    │
│  ├─ Vision (Transient Media)      │                         │  ├─ Tier 2: Scoped Topic Summaries│
│  ├─ Video (Whole Clip)            │                         │  └─ Tier 3: Scoped Canonical Turns│
│  ├─ Audio STT (Transcription)     │                         ├───────────────────────────────────┤
│  ├─ Image Gen (OpenAI-compatible) │                         │         SQLite WAL Storage        │
│  └─ Curator (Fact Extraction)     │                         │          (XIAO_DATA_DIR)          │
└───────────────────────────────────┘                         └───────────────────────────────────┘
```

---

## 🚀 Key Features

### 1. 🎨 Native Telegram Bot API 10.3 Engine
- **Streaming Draft Synchronization**: Responses stream live into Telegram drafts using `sendRichMessageDraft` and `sendMessageDraft`, complete with rotating progress spinners and activity indicators.
- **Real-Time Stop Controls**: Interrupt generation at any time with native Telegram stop actions (`stopped_message_generation`). Stop signals bypass worker queues for instant in-flight cancellation.
- **Rich Message AST**: Parses extended Markdown directly into Telegram Bot API 10.3 rich blocks:
  - **Tables**: Multi-column tables with custom text alignments. Unspecified columns in Right-to-Left (Arabic/Hebrew) or Eastern Arabic numeral contexts automatically mirror to the right.
  - **Expandable Quotes & Callouts**: GitHub-style alerts (`> [!NOTE]`, `> [!WARNING]`) and expandable blockquotes.
  - **Visual Media Groups**: Native photo collages and horizontal media slideshows.
  - **Buttons & Ephemeral Messages**: The models and client cover rich-message buttons (Bot API 10.3) and ephemeral messages (Bot API 10.2); an answer to an incoming ephemeral message is delivered as an ephemeral message too. Xiao's own answers do not add buttons.
- **Cross-Platform LaTeX Sanitizer**: Mathematical expressions (`$...$`, `$$...$$`, `\(...\)`) are sanitized downstream to render flawlessly on both Android (`JLaTeXMath`) and iOS (`SwiftMath`), normalizing units (`44\ \mathrm{cm}`), decimal commas, and LaTeX operator symbols.
- **Guest Mode (Bot API 10.0)**: When guest mode is enabled for the bot in @BotFather, the owner can mention Xiao in a chat Xiao is not a member of. Xiao answers with a "thinking" placeholder (`answerGuestQuery`) and edits it into the final answer. Because other people read these replies, guest conversations are stateless: no personal memories or summaries reach the prompt, nothing is written to history, and only `web_search` and `fetch_url` are available. A photo, voice note, video or document in the owner's message (or in the message it replies to) is read for that answer and never stored; the answer itself is text only. The daemon status box shows whether guest mode is enabled.
- **Edited Messages**: Editing your latest message to Xiao within 10 minutes gets a fresh answer that quotes the edited message; the earlier answer is left as it was. Edits of older messages, and edit events that do not change the text (such as live location updates), are ignored.
- **More Message Kinds**: Stickers (regular stickers as images, animated and video stickers via their thumbnail plus emoji), shared locations and venues (as coordinates with a map link), live photos (the still photo, plus the motion clip when a Video model is available), forwarded rich messages (Bot API 10.1, read as text), and shared checklists, polls and quizzes (read as text).
- **Reply Context**: Replying to a message makes Xiao read that message too: its text, who wrote it (Xiao, you, or someone else in the group), the part you quoted, a checklist task you replied to, and replies to messages from other chats. Under a photo, voice note, video or document, a reply without its own attachment lets Xiao see or hear that file. The replied message is passed as quoted material, never as instructions. The model gets the replied message in full (up to 65,536 characters); the stored conversation history keeps only a 1,000-character quote of it, so long replies do not crowd older turns out.
- **Inline Mode**: With inline mode enabled in @BotFather (`/setinline` and `/setinlinefeedback`), the owner can type `@XiaoBot question` in any chat and pick "Tanya Xiao". The posted message shows a placeholder and is edited into the answer. Telegram sends inline questions as text only (at most 256 characters), so files cannot be attached, but a picture link (`.jpg`, `.png`, `.webp`) in the question is downloaded through the SSRF-safe fetcher and shown to the model. Like guest mode, these answers are stateless and use only the read-only research tools; queries from anyone else are never answered. The daemon status box shows whether inline mode is enabled.
- **Advanced Text Formatting (Bot API 10.1)**: Highlighted text (`==text==` or `<mark>`), superscript and subscript (`<sup>`, `<sub>`), date-times that each reader sees in their own time zone (`<time datetime="2026-10-05T14:00:00+07:00">…</time>`, or Telegram's `![22:45](tg://time?unix=…&format=wDT)`), and custom emoji (`![👍](tg://emoji?id=…)`, which Telegram only accepts when the bot owner has Telegram Premium or the bot has a Fragment username). WhatsApp and the terminal get readable equivalents (bold, `x²`, `H₂O`, the visible date text).
- **Quizzes**: Native quizzes may have several correct answers, a picture for the question, for each option and for the explanation, a description under the question, shuffled options, answers that can be changed, and a time limit after which the quiz closes (optionally hiding everyone's results until then). If Telegram cannot load a picture URL, the quiz is sent without pictures instead of failing.
- **In-Message Navigation (Bot API 10.1)**: Long answers can open with a table of contents whose entries jump to the sections of the same message (`[2. DNS](#2-dns)`), and use footnotes (`fact[^1]` with `[^1]: source`). An anchor is placed only on headings that are linked to, and a link whose target is missing keeps only its text. A `[^…]` without a matching note, such as the regex class `[^0-9]`, is left exactly as written. WhatsApp and the terminal show the plain text (`DNS`, `[1]`).
- **Live Photos (Bot API 10.0)**: The `send_live_photo` tool sends a still photo with a motion clip of at most 10 seconds and 10 MB. Telegram does not accept live photos by URL, so Xiao downloads both files through the SSRF-safe fetcher, checks the clip length, and uploads them.

### 2. 🛡️ Hardened Security & Zero-Slash UX
- **Strict Single-Owner Boundary**: Xiao ignores all non-owner interactions at the network gate. Messages from unauthorized users or rogue groups are discarded with `RouteDecision::Ignore`, leaking zero information about the bot's presence.
- **Zero-Slash Gateway**: On startup, Xiao executes `set_my_commands(&[])` to clear Telegram slash menus. Interaction is completely natural—speak naturally, drop files, send voice messages, or mention the bot.
- **Filesystem Secret Vault**: API keys and bot tokens saved by Xiao are never stored in SQLite. They are isolated in permission-hardened local files (`<data dir>/secrets/`, permissions `0o600`/`0o700` on Unix) and referenced internally via opaque `secret://` URIs.
- **Outbound SSRF Firewall**: Remote fetches (`fetch_url`, media downloads) pass through strict IP validation, blocking RFC 1918 private subnets, loopback addresses (`127.0.0.0/8`, `::1`), link-local spaces, and SIIT/NAT64 translated ranges.

### 3. 🧠 Three-Tier Memory & Multimodal Routing
- **Tier 1 (Facts)**: Autonomous background analysis extracts long-term user facts, preferences, and technical stack details into `user_memories`.
- **Tier 2 (Topic Summaries)**: Older conversation turns in busy chats or forum topics are periodically condensed into concise topic summaries (`scoped_summaries`), keeping active context windows lean.
- **Tier 3 (Canonical Turns)**: Complete scoped message logs stored in SQLite WAL tables for exact replay and reference.
- **Specialist Context Isolation**: Canonically, conversation history belongs solely to the `Main` model. Specialists (`Vision`, `Video`, `Audio STT`) only receive the immediate media artifact and user prompt, returning bounded observation turns to `Main`. This eliminates context pollution and token exhaustion.

### 4. ⚡ Durable SQLite WAL Queue & Fault Isolation
- **At-Least-Once Intake Guarantee**: Telegram long-polling commits updates directly to SQLite table `telegram_inbox` with status `pending` before Telegram acknowledgment.
- **Per-Scope Keyed Mailboxes**: Updates are dispatched into dedicated per-scope FIFO queues (`ScopeKey { chat_id, thread_id }`). Messages within the same chat/topic maintain strict sequential order, while different chats process concurrently up to a global semaphore limit (8 permits).
- **Panic Isolation & Bounded Retry**: Each processing task runs within an isolated `tokio::spawn` wrapper with unwinding panic protection. A panic releases the concurrency permit immediately and, after a 1.5-second backoff, the update is retried once (2 attempts in total). Updates exceeding the limit are quarantined as `failed` ("poison pills") to prevent infinite crash loops.
- **Crash Recovery**: On process startup, `recover_telegram_processing_async()` resets in-flight `processing` updates back to `pending`, ensuring zero message loss across reboots. Stop requests and inline queries from before the restart are acknowledged instead of replayed, since they no longer apply.

### 5. 📱 WhatsApp Multi-Device Gateway
- **Self-Service Linking**: Link by scanning a QR code or by entering a phone pairing code.
- **Single-Owner Boundary**: Authorization is decided on the phone number, recognizing both legacy and LID addressing modes. Device suffixes such as `:12` never corrupt the match. Other senders are dropped silently.
- **Durable Queue (at-least-once)**: A durability hook records every authorized message to `whatsapp_inbox` *before* WhatsApp acknowledges it, so messages survive restarts, crashes, and dropped events. Media is re-downloadable on replay.
- **Ordered, Concurrent Processing**: Messages within a chat are processed one at a time in arrival order; different chats are processed concurrently, and a slow answer never blocks intake.
- **Groups**: In group chats Xiao answers owner messages only when mentioned, replied to, addressed with a leading `/`, or when the group is listed in `WHATSAPP_DEDICATED_GROUPS`.
- **Files & Long Replies**: Documents created by tools are sent as WhatsApp documents; long answers are split into numbered parts.
- **Self-Healing**: The daemon restarts a failed WhatsApp session with backoff and notifies the owner on Telegram when the device is unlinked from the phone.
- **Hardened Credentials**: The session database holding Signal keys is locked to `0o600` on Unix, together with its WAL and SHM sidecars.

---

## 📦 Multi-Platform Installation

Xiao is a single self-contained binary (TLS through `rustls`, SQLite compiled in, no system OpenSSL or SQLite needed). There are no GitHub Releases: CI uploads prebuilt binaries as workflow artifacts for Linux ARM64 (`xiao-linux-arm64-armbian`), Android ARM64 (`xiao-android-arm64`) and Windows x86_64 (`xiao-windows-x86_64`) on every push to `main` and every pull request, after the quality and security jobs pass. Artifacts are zip files downloaded from the workflow run page while signed in to GitHub. Other targets, including Linux x86_64, are built from source (Option D). Choose your deployment environment:

### Option A: Linux x86_64 Server (Systemd Service)

Ideal for dedicated servers, VPS instances (Ubuntu, Debian, Arch Linux), or home servers.

1. **Build and install the binary** (see Option D for the build itself):
   ```bash
   cargo build --release --locked
   sudo mkdir -p /var/lib/xiaoai
   sudo install -m 755 target/release/xiao /usr/local/bin/xiao
   ```

2. **Create a dedicated system user**:
   ```bash
   sudo useradd -r -s /usr/sbin/nologin -d /var/lib/xiaoai xiao
   sudo chown -R xiao:xiao /var/lib/xiaoai
   sudo chmod 700 /var/lib/xiaoai
   ```

3. **Install the hardened Systemd service**:
   Create `/etc/systemd/system/xiao.service`:
   ```ini
   [Unit]
   Description=Xiao Telegram AI Assistant Daemon
   After=network.target network-online.target
   Wants=network-online.target

   [Service]
   Type=simple
   User=xiao
   Group=xiao
   WorkingDirectory=/var/lib/xiaoai
   ExecStart=/usr/local/bin/xiao start
   Restart=on-failure
   RestartSec=5s
   LimitNOFILE=65535

   # Security Sandboxing
   ProtectSystem=strict
   ProtectHome=read-only
   ReadWritePaths=/var/lib/xiaoai
   PrivateTmp=true
   NoNewPrivileges=true

   Environment=XIAO_DATA_DIR=/var/lib/xiaoai
   EnvironmentFile=-/var/lib/xiaoai/.env

   [Install]
   WantedBy=multi-user.target
   ```

4. **Configure credentials & start**:
   ```bash
   sudo cp .env.example /var/lib/xiaoai/.env
   sudo nano /var/lib/xiaoai/.env
   sudo chown xiao:xiao /var/lib/xiaoai/.env && sudo chmod 600 /var/lib/xiaoai/.env

   sudo systemctl daemon-reload
   sudo systemctl enable --now xiao
   sudo systemctl status xiao
   ```

---

### Option B: Armbian / Single-Board Computers (ARM64)

Optimized for Orange Pi, Raspberry Pi, Radxa, or any SBC running Armbian or Debian ARM64 (`aarch64-unknown-linux-gnu`).

1. **Get the prebuilt ARM64 binary**:
   Download the `xiao-linux-arm64-armbian` artifact from a successful run of the **Build XiaoAI** workflow (GitHub → Actions), then:
   ```bash
   unzip xiao-linux-arm64-armbian.zip
   sudo install -m 755 xiao /usr/local/bin/xiao
   ```

2. **Quick onboarding setup**:
   ```bash
   # Run the interactive onboarding wizard to configure bot token and AI provider
   xiao setup
   ```
   Run it as the user the service runs as (or with the same `XIAO_DATA_DIR`), so the service finds the configuration.

3. **Enable the systemd background service** (create `xiao.service` and the `xiao` user as in Option A first):
   ```bash
   sudo systemctl enable --now xiao
   journalctl -u xiao -f
   ```

---

### Option C: Android Termux (Zero-Root Mobile Edge)

Run Xiao 24/7 directly on your Android device without requiring root access.

1. **Install required packages in Termux** (a C compiler is needed for the bundled SQLite):
   ```bash
   pkg update && pkg install -y git clang rust
   ```

2. **Clone & build native Android binary** (or use the `xiao-android-arm64` CI artifact instead):
   ```bash
   git clone https://github.com/tsaQB/xiaobot.git
   cd xiaobot
   cargo build --release --locked
   cp target/release/xiao $PREFIX/bin/
   ```

3. **Acquire Termux wake-lock & start daemon**:
   ```bash
   termux-wake-lock
   xiao setup
   xiao start
   ```

---

### Option D: Build from Source

Requirements: **Rust 1.94+** (`cargo`, `rustc`; the WhatsApp crates require 1.94, CI builds with 1.98.0) and a C compiler for the bundled SQLite.

```bash
git clone https://github.com/tsaQB/xiaobot.git
cd xiaobot

# Verify compilation
cargo check --locked

# Build optimized release binary
cargo build --release --locked

# Binary is available at target/release/xiao
./target/release/xiao --help
```

---

### Option E: Windows x86_64

Download the `xiao-windows-x86_64` artifact (it contains `xiao.exe`) or build from source as in Option D. Run `xiao setup` once, then `xiao start` in a terminal; no Windows service wrapper is included. On Windows, data lives in `%APPDATA%\xiaoai` and a `.env` can be placed in `%APPDATA%\xiao\.env` (see the configuration order below).

---

## ⚡ Quickstart

### 1. Configure Environment
Xiao loads the first `.env` file it finds, in this order (`src/main.rs::get_config_path`):
1. `.env` in the current working directory (on Unix only when it is owned by you and not writable by group or others)
2. `$XDG_CONFIG_HOME/xiao/.env`, `$XDG_CONFIG_HOME/.xiao/.env`, `$XDG_CONFIG_HOME/xiaoai/.env`
3. In `$HOME` (then `%USERPROFILE%`): `.xiao.env`, `.xiao/.env`, `.xiaoai/.env`, `.config/xiao/.env`, `xiao/.env`, `xiaoai/.env`, `XiaoAI/.env`
4. On Windows, in `%APPDATA%`: `xiao\.env`, `XiaoAI\.env`, `xiaoai\.env`, `.xiao.env`
5. A trusted `.env` in a parent directory of the working directory

Variables already set in the process environment (for example by systemd) take precedence over the file. Settings saved through the CLI (`xiao setup`, `xiao gateway`, `xiao ai`, `xiao search`, `xiao mcp`) are stored in the data directory and are used when the environment does not set them; an empty line such as `BRAVE_API_KEY=` counts as not set. When the environment or `.env` still overrides a value the CLI just saved, the CLI says so and names the file. `.env.example` therefore leaves the owner, WhatsApp and MCP settings commented out.

Create `.env` based on the template:
```bash
cp .env.example .env
```

Edit the core settings:
```env
# Required Telegram Gateway Credentials
BOT_TOKEN=1234567890:ABCdefGHIjklMNOpqrsTUVwxyz
OWNER_USER_ID=5385399301

# Primary AI Provider (Defaults to OpenRouter, or any OpenAI-compatible /v1 endpoint)
AI_ENDPOINT=https://openrouter.ai/api/v1
AI_API_KEY=sk-or-v1-xxxxxxxxxxxxxxxxxxxx
AI_MODEL=google/gemini-2.0-flash-001
```

> [!TIP]
> **Zero-Prompt Headless Startup**: Populating `.env` allows `xiao start` to initialize the database, seed provider credentials, and run the Telegram long-polling loop with zero interactive prompts.
> If you prefer a visual onboarding walkthrough, simply run:
> ```bash
> cargo run -- setup
> ```

### 2. Start the Daemon
```bash
cargo run --release -- start
```

---

## 💻 CLI & Terminal Chat

Xiao includes a feature-rich terminal CLI for administration, diagnostics, and direct interactive chat without opening Telegram:

```bash
xiao <subcommand> [arguments]
```

### CLI Command Reference

| Subcommand | Description |
| :--- | :--- |
| *(none)* | Start terminal chat session (default mode; auto-launches setup on first run; prints help when stdout is not a terminal). |
| `<question...>` | Ask a quick one-shot question directly (e.g. `xiao "What is Rust?"`). |
| `menu` | Open interactive Control Center (TUI). |
| `chat [prompt]` | Terminal chat mode: run an interactive REPL or execute a one-shot query. |
| `setup` | Interactive initial configuration and onboarding wizard. |
| `start` | Start the gateway daemon (Telegram polling plus the WhatsApp gateway when configured). |
| `status` | Display system telemetry, provider health, SQLite database size, and active models. |
| `context [chat] [th]` | Inspect token usage, sliding-window consumption, and context breakdown. |
| `memory` | Open interactive long-term memory management menu. |
| `memory list` | Output formatted table of Tier-1 profile facts directly to stdout. |
| `memory rm [key]` | Delete a remembered fact (interactive picker if key is omitted). |
| `memory clear` | Wipe all Tier-1 persistent memories. |
| `ai` | Interactive AI hub for model catalog, capability tests, and routing. |
| `ai use [model]` | Switch the active Main model (interactive picker if omitted; suffix matching supported). |
| `ai list` | List all registered providers and their configured models. |
| `ai add` | Interactively register a new OpenAI-compatible AI provider. |
| `ai rm` | Interactively remove a registered AI provider. |
| `ai provider [name]` | Switch the active AI provider or view provider details. |
| `ai addon` | Configure multimodal specialist roles (`Vision`, `Video`, `Audio STT`, `Image Generation`, `Curator`). |
| `ai test [role]` | Run live diagnostic capability probes against configured endpoints. |
| `search` | Web search engine hub and interactive configuration menu. |
| `search test [query]` | Run a live web search through the engine chain (a sample query is used when omitted; `xiao search <words>` does the same). |
| `search brave [key]` | Configure Brave Search API key (or remove with `rm`). |
| `search tavily [key]` | Configure Tavily Search API key (or remove with `rm`). |
| `search exa [key]` | Configure Exa Search API key (or remove with `rm`). |
| `search engine [name]` | Show the active engine and the automatic priority order (engines are chosen by configured keys, not by name). |
| `mcp` | Interactive Model Context Protocol (MCP) server & tool hub. |
| `mcp list` | Show the active MCP endpoint and status. |
| `mcp url <URL>` | Set the active MCP endpoint (SSRF guarded; alias: `mcp set`). |
| `mcp add <URL>` | Alias for `mcp url`. |
| `mcp rm` | Restore the default MCP endpoint (alias: `mcp remove`). |
| `mcp tools` | List registered tool schemas exposed to AI. |
| `mcp test [query]` | Direct JSON-RPC handshake and latency probe to MCP server. |
| `mcp reset` | Reset MCP endpoint to default (`https://mcp.exa.ai/`). |
| `gateway` | Open interactive Telegram Gateway configuration menu. |
| `gateway check` | Check Telegram Bot API connectivity and token health (aliases: `test`, `status`). |
| `gateway token [val]` | Interactively or directly bind Telegram Bot Token. |
| `gateway owner [id]` | Set authorized owner user ID (alias: `gateway id [id]`). |
| `gateway wa` | Open the interactive WhatsApp gateway configuration menu. |
| `gateway wa pair` | Link WhatsApp by scanning a QR code (alias: `gateway wa qr`). Refused when a session is already linked or another `xiao` process (such as `xiao start`) is using it. |
| `gateway wa code <NUM>` | Link WhatsApp using a phone pairing code. Spaces, `+` and dashes in the number are ignored. |
| `gateway wa owner <NUM>` | Set the authorized owner phone number (E.164 without the plus sign). |
| `gateway wa status` | Inspect WhatsApp link status, owner, and session path (alias: `check`). |
| `gateway wa unlink` | Delete the stored WhatsApp session (alias: `logout`). |
| `version`, `-v`, `--version` | Display the version. |
| `help`, `-h`, `--help` | Display the command-line help screen. Each hub also accepts `help` (for example `xiao ai help`). |

### Terminal Interactive REPL
Launch direct chat mode without Telegram:
```bash
xiao
# Or with cargo:
cargo run
```

Inside the REPL, manage independent chat sessions seamlessly:
```text
Xiao Interactive Chat REPL Commands:
  /sessions         List all conversation sessions with IDs and turn counts
  /switch [id]      Switch session (without an id, lists them and asks for one)
  /new [name]       Create and switch to a new isolated session
  /rm <id>          Remove a conversation session
  /clear            Reset conversation history in active session
  /model            Inspect active Main model and provider endpoint
  /help             Show available REPL commands
  /exit             Exit chat mode (or Ctrl+C / Ctrl+D)
```

Run one-shot queries directly from shell scripts or terminal:
```bash
xiao "Analyze the concurrency guarantees of SQLite in WAL mode"
# Or:
xiao chat "Analyze the concurrency guarantees of SQLite in WAL mode"
```

---

## 📋 Document & Media Processing

Xiao inspects documents and rich media locally inside bounded memory buffers:

| Media Type | Processing & Ingestion Method |
| :--- | :--- |
| **Plain Text / Code** | UTF-8 ingestion with BOM stripping; invalid byte sequences are replaced (lossy UTF-8) instead of failing. |
| **Photos & Images** | Routed through the `Vision` role (a dedicated Vision model, otherwise the main model). |
| **Stickers** | Regular stickers are analyzed as images; animated and video stickers via their thumbnail, together with the emoji and set name. |
| **Live Photos** | The motion clip goes to the `Video` role when a dedicated Video model, or a main model with verified video input, is available; otherwise the still photo goes to `Vision`. |
| **Locations & Venues** | Passed to the model as text: coordinates, place name and address, and a map link. |
| **Forwarded Rich Messages** | Converted to readable text that keeps headings, list items, table rows, quotes and link targets; media blocks are named (for example `[Foto: caption]`) instead of exposing file ids. Telegram caps a rich message at 32,768 characters, so a forwarded message is passed on in full. |
| **Voice Notes & Audio** | Transcribed via `Audio STT` (Whisper-compatible `/audio/transcriptions`, falling back to chat completions when the provider has no transcription endpoint); up to 20 MB. |
| **Video & Video Notes** | The whole clip is sent to the `Video` role (a dedicated Video model, otherwise the main model); there is no local frame extraction. Telegram limits bot downloads to 20 MB. |
| **PDF Documents** | Text extracted via `lopdf`. Scanned pages (up to 6) are rendered to images and routed to `Vision`. |
| **DOCX Documents** | In-memory XML text and paragraph extraction. |
| **XLSX Spreadsheets** | In-memory streaming XML parsing of sheets and shared string tables (`<si>`). |
| **Archives (ZIP, TAR, TAR.GZ, 7Z)** | In-memory text extraction with strict safety caps: max 30 MB uncompressed, max 2 MB per file, nested archives are not unpacked. |
| **Checklists, Polls & Quizzes** | A shared checklist, poll or quiz is read as text (tasks with their done state, options with the correct answers when known). |
| **Replies** | The replied-to message is added as quoted context; a reply without its own file reuses the photo, voice note, video or document it answers (best effort). |

---

## ⚙️ Configuration Reference

Settings can be provided via `.env` (or the process environment) or managed through the CLI:

| Environment Variable | Default | Description |
| :--- | :--- | :--- |
| `BOT_TOKEN` | *Required* | Telegram Bot API token from [@BotFather](https://t.me/BotFather). Can also be set with `xiao setup` or `xiao gateway token`. |
| `OWNER_USER_ID` | *Required* | Numerical Telegram user ID of the only authorized user. Can also be set with `xiao gateway owner`. |
| `ALLOWED_CHAT_IDS` | *Empty* | Comma-separated group IDs where the owner may use Xiao. Empty means every group is allowed. Either way, in an ordinary group Xiao answers only the owner, and only when mentioned, replied to, or addressed with a leading `/`. |
| `DEDICATED_CHAT_IDS` | *Empty* | Comma-separated group IDs used as dedicated workspaces, where every owner message is answered without a mention. A forum supergroup in which the bot is an administrator is treated as a dedicated workspace automatically. |
| `AI_ENDPOINT` | `https://openrouter.ai/api/v1` | OpenAI-compatible endpoint used to seed the first provider when none is configured yet. Afterwards providers are managed with `xiao ai`. |
| `AI_API_KEY` | *Required for seeding* | API key for that first provider (not needed for a local endpoint such as `localhost`). |
| `AI_MODEL` | `google/gemini-2.0-flash-001` | Main model for that first provider (`default` for endpoints other than OpenRouter). |
| `IMAGE_FALLBACK_PROVIDER` | `none` | Fallback provider for image generation (`none` or `pollinations`). |
| `AI_PROVIDER_CONNECT_TIMEOUT_SECS` | `10` | Connect timeout for chat completions, capability probes and speech-to-text. |
| `IMAGE_PROVIDER_CONNECT_TIMEOUT_SECS` | `10` | Connect timeout for downloading generated images. |
| `IMAGE_GENERATION_TIMEOUT_SECS` | `120` | Request timeout for image generation endpoints. |
| `IMAGE_DOWNLOAD_TIMEOUT_SECS` | `30` | Timeout for downloading generated image payloads. |
| `BRAVE_API_KEY` | *Optional* | API key for Brave Search in `web_search`. |
| `TAVILY_API_KEY` | *Optional* | API key for Tavily search (the legacy name `TAVILY_KEY` is also read). |
| `EXA_API_KEY` | *Optional* | API key for the Exa REST API (the legacy name `EXA_KEY` is also read). |
| `EXA_MCP_URL` | `https://mcp.exa.ai/` | Keyless Exa MCP search endpoint (`xiao mcp url` changes it). |
| `WHATSAPP_ENABLED` | `false` | Enables the WhatsApp gateway in the daemon. A linked session also enables it automatically. |
| `WHATSAPP_OWNER_NUMBER` | *Empty* | Owner phone number in E.164 form without the plus sign. |
| `WHATSAPP_DEDICATED_GROUPS` | *Empty* | Comma-separated group JIDs (or numeric ids) where Xiao answers every owner message without a mention. |
| `XIAO_HISTORY_RETENTION` | `2000` | Messages kept per chat/topic in canonical history (older context lives on in the topic summary). `0` disables pruning. Read from the environment only. |
| `XIAO_DATA_DIR` | see below | Base directory for the database, secrets, attachments and the WhatsApp session. |
| `RUST_LOG` | `info` | Log filter (for example `debug` or `xiao=debug`). |

The four timeouts take whole seconds and are capped at 600. Without `XIAO_DATA_DIR`, the data directory is `%APPDATA%\xiaoai` on Windows, otherwise `$XDG_DATA_HOME/xiaoai`, otherwise `~/.local/share/xiaoai`.

> API keys and tokens set through the CLI (`xiao setup`, `xiao gateway`, `xiao ai`, `xiao search`) are stored in the file vault, never in the SQLite `settings` table. On Unix, a `.env` in the working directory (or a parent directory) is only loaded when it is owned by you and not writable by group or others.

---

## 🧪 Verification & Quality Gates

The codebase strictly enforces clean quality gates and zero `.unwrap()` calls across both production code and test suites:

```bash
# Code formatting check (CI enforced)
cargo fmt --all -- --check

# Compiler check without emitting binaries
cargo check --locked

# Unit, behavioural (fake Bot API server) and Bot API 10.3 contract tests
cargo test --locked

# Strict Clippy lint check (CI enforced)
cargo clippy --locked --all-targets --all-features -- -D warnings
```

---

## 🔒 Security Invariants

1. **Hard Single-Owner Invariant**: Non-owner updates are dropped silently at the gateway (`src/bot/router.rs`). Never acknowledge unauthorized Telegram IDs. Guest-mode and inline queries from anyone but the owner are never answered.
2. **Pure Zero-Slash Gateway**: Menus are cleared on boot. Xiao interacts conversationally or via CLI.
3. **Outbound SSRF Firewall**: Every fetch of a user-, model- or web-supplied URL (media downloads, `fetch_url`, search result scraping) validates each redirect hop against RFC 1918, RFC 4193, loopback, and SIIT/NAT64 ranges and pins the connection to the vetted address.
4. **Secret Isolation**: Secrets are stored in `<data dir>/secrets/` with mode `0o600`/`0o700` on Unix (on Windows the per-user profile ACL protects them). The database only stores opaque `secret://` references; the values themselves are protected by file permissions, not encryption.
5. **Zero `.unwrap()` Policy**: Handled idiomatically with `?`, pattern matching, or `.expect()` with descriptive invariant explanations in tests.
6. **WhatsApp Single-Owner Boundary**: Authorization is decided on the phone number, not raw JID text. Both `sender` and `sender_alt` are inspected so LID mode is recognized, and device or agent suffixes never corrupt the match. Unauthorized senders are dropped with no reply and no identity trace in the logs.
7. **WhatsApp Credential Handling**: `whatsapp.db` holds Signal session keys and is treated the same as the secret vault. The file and its `-wal` and `-shm` sidecars are locked to `0o600` on Unix systems.

---

## 📄 License

This project is licensed under the [MIT License](LICENSE).
