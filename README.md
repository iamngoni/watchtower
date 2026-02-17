# 🗼 Watchtower

A self-hosted monitoring dashboard for [OpenClaw](https://github.com/openclaw/openclaw) AI agents. Real-time visibility into what your agent is doing, what it costs, and how to control it.

Built with **Rust** + **HTMX** + **Tailwind** — no JavaScript frameworks, no bloat.

![License](https://img.shields.io/badge/license-MIT-blue)
![Rust](https://img.shields.io/badge/rust-1.88+-orange)

## Why

OpenClaw agents run autonomously — processing messages, executing tools, running cron jobs, spawning sub-agents. Watchtower gives you a single pane of glass to see it all happening in real-time, without digging through logs.

## Features

### 📡 Live Feed
Real-time activity stream via SSE. Every tool call, shell command, API request, and agent action appears as a styled card with output previews.

### 📊 Dashboard  
Agent status (active/idle), current model, today's cost, cron job health, active sub-agents, recent completions, and blocked tasks — all at a glance.

### 💰 Usage & Costs
Daily cost timeseries (31 days), cost breakdown by category (cache write/read, input/output tokens), model comparison pie chart.

### 🕐 Cron Jobs
All your OpenClaw cron jobs with schedules, last/next run times, error counts, and **Run Now** buttons that trigger directly via the gateway WebSocket.

### 📋 Sessions Browser
Browse all active and recent sessions — main, cron, sub-agent — with model, channel, and token usage info.

### 📌 Kanban Board
Task management with 6 columns (Backlog → Done), drag-and-drop, swimlane view, labels, due dates, and quick creation.

### 🔍 Global Search
`⌘K` to search across events, tasks, sessions, and cron jobs.

### ⌨️ Keyboard Shortcuts
`1-5` for page navigation, `N` for new task, `?` for help overlay.

## Architecture

```
┌──────────────┐     WebSocket (port 18789)     ┌──────────────────┐
│   OpenClaw   │◄──────────────────────────────►│    Watchtower     │
│   Gateway    │   Ed25519 device auth           │  (Actix-web)     │
│              │   Real-time events              │                  │
│              │   RPC: sessions, costs,         │  ┌────────────┐  │
│              │        cron, chat               │  │  SQLite DB  │  │
└──────────────┘                                 │  └────────────┘  │
                                                 │  ┌────────────┐  │
       ┌──────────────┐    Log tailing           │  │ SSE Server  │  │
       │ gateway.log  │────────────────────────►│  └────────────┘  │
       └──────────────┘    (fallback)            │  ┌────────────┐  │
                                                 │  │Session JSONL│  │
       ┌──────────────┐    File tailing          │  │  Watcher    │  │
       │ sessions/*.  │────────────────────────►│  └────────────┘  │
       │   jsonl      │    (tool results)        └────────┬─────────┘
       └──────────────┘                                   │
                                                    Port 3002
                                                 ┌────────┴─────────┐
                                                 │   Browser (HTMX)  │
                                                 └──────────────────┘
```

**Three data sources, layered for reliability:**

1. **Gateway WebSocket** (primary) — Direct connection to OpenClaw's RPC API. Real sessions, costs, cron state. Ed25519 device identity auth with challenge/response.
2. **Log Watcher** (fallback) — Tails `gateway.log` for events when WebSocket is unavailable.
3. **Session JSONL Watcher** — Tails active session files to capture tool results (command output, exit codes, duration).

## Stack

| Component | Technology |
|-----------|-----------|
| Backend | Rust + [Actix-web](https://actix.rs/) |
| Templates | [Askama](https://github.com/djc/askama) (compile-time) |
| Frontend | [HTMX](https://htmx.org/) + vanilla JS |
| Styling | [Tailwind CSS](https://tailwindcss.com/) (CDN) |
| Database | SQLite via [sqlx](https://github.com/launchbadge/sqlx) |
| Real-time | Server-Sent Events (SSE) |
| WebSocket | [tokio-tungstenite](https://github.com/snapview/tokio-tungstenite) |
| Auth | Ed25519 via [ed25519-dalek](https://github.com/dalek-cryptography/ed25519-dalek) |
| Icons | [Lucide](https://lucide.dev/) |
| Fonts | DM Sans + Bricolage Grotesque |

## Quick Start

### Prerequisites
- Rust 1.88+
- OpenClaw running locally (gateway on port 18789)
- OpenClaw device identity (`~/.openclaw/identity/device.json`)

### Build & Run

```bash
git clone https://github.com/iamngoni/watchtower.git
cd watchtower
cargo build --release
./target/release/watchtower
```

Open `http://localhost:3002`

### Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `PORT` | `3002` | HTTP server port |
| `DATABASE_URL` | `sqlite:data/watchtower.db` | SQLite database path |
| `OPENCLAW_GATEWAY_URL` | `ws://127.0.0.1:18789` | Gateway WebSocket URL |
| `OPENCLAW_LOG_PATH` | `~/.openclaw/logs/gateway.log` | Log file to tail |
| `OPENCLAW_SESSIONS_DIR` | `~/.openclaw/agents/main/sessions` | Session files directory |
| `WEB_USER` | *(empty)* | Basic auth username (optional) |
| `WEB_PASS` | *(empty)* | Basic auth password (optional) |

### Systemd Service

```ini
[Unit]
Description=Watchtower Agent Dashboard
After=network.target

[Service]
Type=simple
User=iamngoni
WorkingDirectory=/home/iamngoni/watchtower
ExecStart=/home/iamngoni/watchtower/target/release/watchtower
Restart=on-failure
RestartSec=5
Environment=RUST_LOG=info

[Install]
WantedBy=multi-user.target
```

### Docker

```dockerfile
FROM rust:1.88 AS builder
WORKDIR /app
COPY . .
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/watchtower /usr/local/bin/
COPY --from=builder /app/templates /app/templates
COPY --from=builder /app/static /app/static
COPY --from=builder /app/migrations /app/migrations
WORKDIR /app
EXPOSE 3002
CMD ["watchtower"]
```

## Design

- **Dark mode** by default (light mode planned)
- **Indigo** (#6366F1) accent color
- **Mobile responsive** — works on phone, tablet, and desktop
- **No auth required by default** — designed to run behind Tailscale or a VPN

## Codebase

```
src/
├── main.rs              # App setup, routing, startup
├── web.rs               # Page handlers, templates, gateway data converters
├── handlers.rs          # API endpoints (REST + gateway proxy)
├── db.rs                # SQLite queries, migrations
├── models.rs            # Data models (Event, Task, Session, CronJob, etc.)
├── gateway_client.rs    # OpenClaw WebSocket client + Ed25519 auth
├── log_watcher.rs       # Gateway log file tailer + parser
├── session_watcher.rs   # Session JSONL tailer (tool results)
└── sse.rs               # SSE broadcaster (HTML + JSON)

templates/               # Askama HTML templates
├── base.html            # Layout with sidebar navigation
├── dashboard.html       # Main dashboard
├── feed.html            # Live activity feed
├── board.html           # Kanban task board
├── costs.html           # Usage & costs
├── cron.html            # Cron jobs
├── sessions.html        # Sessions browser
├── settings.html        # Settings page
└── partials/            # HTMX partial templates

scripts/                 # Utility scripts
├── push-event.sh        # Push events via API
├── sync-cron.py         # Sync cron jobs from OpenClaw
├── model_pricing.py     # Model pricing data (30+ models)
└── parse-historical-logs.py  # Backfill from logs

migrations/              # SQLite migrations
└── 001_init.sql         # Schema: events, tasks, sessions, cron_jobs, usage
```

~5,900 lines of Rust · 16 templates · 24 commits

## License

MIT
