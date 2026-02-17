<p align="center">
  <img src="static/favicon.svg" width="64" height="64" alt="Watchtower">
</p>

<h1 align="center">Watchtower</h1>

<p align="center">
  <strong>Real-time monitoring & control dashboard for autonomous AI agents</strong>
</p>

<p align="center">
  <a href="#features">Features</a> •
  <a href="#screenshots">Screenshots</a> •
  <a href="#quick-start">Quick Start</a> •
  <a href="#docker">Docker</a> •
  <a href="#api">API</a> •
  <a href="#keyboard-shortcuts">Shortcuts</a> •
  <a href="#configuration">Configuration</a>
</p>

---

Watchtower gives you a single pane of glass for observing what your AI agent is doing, assigning work through a shared kanban board, tracking token usage and costs, managing scheduled jobs, and browsing historical sessions.

Built for [OpenClaw](https://github.com/openclaw/openclaw) but designed to work with any agent that can push events via REST API.

## Features

### 📡 Live Activity Feed
Real-time stream of everything your agent is doing — tool calls, shell commands, file operations, API requests — all pushed via Server-Sent Events. Filter by type, search by content, auto-scroll or pause to read.

### 📋 Kanban Board
Shared task board between you and your agent. Drag-and-drop cards between columns (Backlog → To Do → In Progress → Blocked → In Review → Done). Both sides can create, move, and comment on tasks. The agent picks up new tasks and moves cards as it works.

### 💰 Usage & Costs
Token consumption and cost tracking per model, per session, per day. Supports 30+ models across Anthropic, OpenAI, Google, xAI, DeepSeek, Meta, and Mistral. Daily cost chart and model comparison breakdown.

### ⏰ Cron Overview
Visual status of all scheduled jobs with enable/disable toggles, run-now buttons, failure indicators, and run history. Syncs with OpenClaw's cron system.

### 📜 Session Browser
Browse all agent conversations, sub-agent runs, and cron executions. Click into any session for a detailed timeline of events with token usage and cost.

### 🔍 Global Search
`⌘K` / `Ctrl+K` to search across tasks, events, and sessions from anywhere.

### 🤖 Agent Integration
Background log watcher tails OpenClaw's gateway log and automatically categorizes events (shell, file, API, message, cron, agent). The agent can also push events directly via the REST API.

## Tech Stack

- **Backend:** Rust + Actix-web
- **Frontend:** HTMX + Tailwind CSS (via CDN)
- **Real-time:** Server-Sent Events (SSE)
- **Database:** SQLite (via SQLx with auto-migrations)
- **Icons:** Lucide
- **Drag & Drop:** SortableJS
- **Fonts:** DM Sans + Bricolage Grotesque

No JavaScript framework. No build step for the frontend. Pages load fast.

## Quick Start

### Prerequisites

- Rust 1.75+ (for building)
- SQLite3

### Build & Run

```bash
git clone https://github.com/iamngoni/watchtower.git
cd watchtower
cargo build --release
./target/release/watchtower
```

Open [http://localhost:3002](http://localhost:3002).

### Seed Example Data

```bash
# Seed example cron jobs
python3 scripts/seed-cron.py

# Seed example tasks
python3 scripts/seed-tasks.py
```

## Docker

### Build

```bash
./scripts/build-docker.sh
# or
docker build -t watchtower:latest .
```

### Run

```bash
docker run -d \
  --name watchtower-dashboard \
  -p 3002:3002 \
  -v $(pwd)/data:/data \
  -v ~/.openclaw/logs:/logs:ro \
  -e DATABASE_URL=sqlite:/data/watchtower.db \
  -e OPENCLAW_LOG_PATH=/logs/gateway.log \
  watchtower:latest
```

### Docker Compose

Add to your existing stack (see `docker-compose.snippet.yml`):

```yaml
watchtower-dashboard:
  image: watchtower:latest
  container_name: watchtower-dashboard
  ports:
    - "3002:3002"
  volumes:
    - ./watchtower/data:/data
    - ~/.openclaw/logs:/logs:ro
  environment:
    - PORT=3002
    - DATABASE_URL=sqlite:/data/watchtower.db
    - OPENCLAW_LOG_PATH=/logs/gateway.log
    - WATCHTOWER_API_TOKEN=${WATCHTOWER_API_TOKEN}
    - WATCHTOWER_USER=${DASHBOARD_USER}
    - WATCHTOWER_PASS=${DASHBOARD_PASS}
  restart: unless-stopped
```

## API

Watchtower exposes a REST API used by both the HTMX frontend and agents. Authenticate with a bearer token (`WATCHTOWER_API_TOKEN`).

### Events

| Method | Endpoint | Description |
|--------|----------|-------------|
| `GET` | `/api/events` | List events (paginated, filterable by type) |
| `POST` | `/api/events` | Push a new event |
| `GET` | `/events/stream` | SSE stream for real-time events |

#### Push an event

```bash
curl -X POST http://localhost:3002/api/events \
  -H "Content-Type: application/json" \
  -d '{"event_type": "shell", "summary": "Ran git status", "detail": "git status --porcelain"}'
```

Or use the helper script:

```bash
./scripts/push-event.sh shell "Ran git status" "git status --porcelain"
```

### Tasks (Kanban)

| Method | Endpoint | Description |
|--------|----------|-------------|
| `GET` | `/api/tasks` | List tasks (filterable by status, priority, assigned_to) |
| `POST` | `/api/tasks` | Create a task |
| `GET` | `/api/tasks/:id` | Get task detail |
| `PATCH` | `/api/tasks/:id` | Update task (status, title, priority, etc.) |
| `DELETE` | `/api/tasks/:id` | Delete task |
| `POST` | `/api/tasks/:id/comments` | Add comment |
| `GET` | `/api/tasks/:id/comments` | List comments |
| `GET` | `/api/tasks/:id/history` | Status change history |

#### Create a task

```bash
curl -X POST http://localhost:3002/api/tasks \
  -H "Content-Type: application/json" \
  -d '{
    "title": "Fix auth middleware",
    "description": "Returns 401 on valid tokens",
    "priority": "high",
    "status": "todo",
    "labels": ["bug", "auth"],
    "created_by": "agent",
    "assigned_to": "agent"
  }'
```

### Sessions

| Method | Endpoint | Description |
|--------|----------|-------------|
| `GET` | `/api/sessions` | List sessions |
| `POST` | `/api/sessions` | Create/update session (upsert by session_key) |
| `GET` | `/api/sessions/:id` | Session detail |

### Usage & Costs

| Method | Endpoint | Description |
|--------|----------|-------------|
| `GET` | `/api/usage` | Usage stats (filterable by date range) |
| `POST` | `/api/usage/report` | Report token usage |

### Cron

| Method | Endpoint | Description |
|--------|----------|-------------|
| `GET` | `/api/cron` | List cron jobs |
| `POST` | `/api/cron/sync` | Sync cron jobs from external source |
| `PATCH` | `/api/cron/:job_id` | Update job (enable/disable) |

### Search

| Method | Endpoint | Description |
|--------|----------|-------------|
| `GET` | `/api/search?q=...` | Search across tasks, events, sessions |

### Health

| Method | Endpoint | Description |
|--------|----------|-------------|
| `GET` | `/health` | Health check |

## Keyboard Shortcuts

| Key | Action |
|-----|--------|
| `⌘K` / `Ctrl+K` | Global search |
| `1` | Go to Feed |
| `2` | Go to Board |
| `3` | Go to Costs |
| `4` | Go to Cron |
| `5` | Go to Sessions |
| `N` | New task (on Board page) |
| `?` | Show all shortcuts |
| `Esc` | Close modal |

## Configuration

All configuration via environment variables. Copy `.env.example` to `.env` and fill in what you need.

| Variable | Default | Description |
|----------|---------|-------------|
| `PORT` | `3002` | Port the server listens on. |
| `DATABASE_URL` | `sqlite:data/watchtower.db` | SQLite database path. The `data/` directory and DB file are created automatically on first run. |
| `WATCHTOWER_API_TOKEN` | *(empty)* | Bearer token required for REST API calls (`Authorization: Bearer <token>`). When empty, **no API auth is enforced** — fine if Watchtower is only accessible on a private network (e.g., behind Tailscale or a VPN). Set this if you expose Watchtower on a shared network or through a reverse proxy. |
| `WATCHTOWER_USER` | *(empty)* | Username for HTTP Basic Auth on the web UI. When empty (along with `WATCHTOWER_PASS`), **the web UI is open** — again, fine behind a private network. |
| `WATCHTOWER_PASS` | *(empty)* | Password for HTTP Basic Auth on the web UI. Both `WATCHTOWER_USER` and `WATCHTOWER_PASS` must be set for web auth to activate. |
| `TELEGRAM_BOT_TOKEN` | *(empty)* | Telegram bot token for sending notifications (task blocked, cron failures, cost alerts). Optional — Watchtower works fully without it. |
| `TELEGRAM_CHAT_ID` | *(empty)* | Telegram chat/user ID to receive notifications. Required alongside `TELEGRAM_BOT_TOKEN`. |
| `OPENCLAW_LOG_PATH` | `$HOME/.openclaw/logs/gateway.log` | Path to the OpenClaw gateway log file. Watchtower tails this file in the background to extract agent activity events for the live feed. If the file doesn't exist yet, the watcher waits until it appears. Set this if your OpenClaw logs live somewhere non-standard. |

> **Note on log tailing:** Watchtower reads agent activity from OpenClaw's gateway log because OpenClaw doesn't currently expose a real-time event stream or webhook API. The gateway log is the only source of granular tool calls, shell commands, and file operations as they happen. If OpenClaw adds an event streaming API in the future, Watchtower will switch to that. For syncing cron jobs and session data, Watchtower uses its own REST API which can be fed by the agent or helper scripts.

## Project Structure

```
watchtower/
├── src/
│   ├── main.rs          # Server entry point & config
│   ├── db.rs            # Database operations (SQLx)
│   ├── handlers.rs      # REST API endpoints
│   ├── models.rs        # Data structures
│   ├── sse.rs           # SSE broadcasting
│   ├── web.rs           # Web UI routes & templates
│   └── log_watcher.rs   # OpenClaw log tail & event extraction
├── templates/           # Askama HTML templates
│   ├── base.html        # Layout with sidebar & nav
│   ├── dashboard.html   # Landing page
│   ├── feed.html        # Live activity feed
│   ├── board.html       # Kanban board
│   ├── costs.html       # Usage & costs
│   ├── cron.html        # Cron job overview
│   ├── sessions.html    # Session browser
│   └── partials/        # HTMX partial templates
├── migrations/          # SQLite schema (auto-run on start)
├── scripts/             # Helper & seed scripts
├── static/              # CSS, favicon, icons
├── docs/
│   └── design.pen       # UI design system (Penpot format)
├── Dockerfile           # Multi-stage production build
└── docker-compose.snippet.yml
```

## Design

The UI design system is in `docs/design.pen` (Penpot format). Dark mode default with indigo accent. Fonts: DM Sans for body, Bricolage Grotesque for headings and numbers. Icons from Lucide.

## Supported Models (Cost Tracking)

Watchtower includes pricing for 30+ models. See `scripts/model_pricing.py` for the full list.

| Provider | Models |
|----------|--------|
| Anthropic | Claude Opus 4.5/4.6, Sonnet 4/4.5, Haiku 3/3.5 |
| OpenAI | GPT-4o, GPT-4o-mini, GPT-4 Turbo, o1/o3/o4-mini, Codex mini |
| Google | Gemini 2.5 Pro/Flash, 2.0 Flash, 1.5 Pro/Flash |
| xAI | Grok 3, Grok 3 mini, Grok 2 |
| DeepSeek | R1, V3 |
| Meta | Llama 3.1/4 variants |
| Mistral | Large, Medium, Small, Codestral |

## License

MIT

---

<p align="center">
  Built with Rust 🦀 for the <a href="https://github.com/openclaw/openclaw">OpenClaw</a> ecosystem
</p>
