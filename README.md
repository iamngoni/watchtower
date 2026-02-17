# Watchtower

A self-hosted agent monitoring dashboard for OpenClaw.

## Features

- **Live Activity Feed** — Real-time SSE-powered event stream showing agent actions
- **Kanban Board** — Drag-and-drop task management for human/agent collaboration  
- **Usage & Costs** — Token usage and cost tracking by model
- **Cron Jobs** — Monitor scheduled job status
- **Sessions** — Browse conversation and sub-agent session history

## Stack

- **Backend**: Rust + Actix-web
- **Frontend**: HTMX + Tailwind CSS + Lucide Icons
- **Database**: SQLite (via SQLx)
- **Real-time**: Server-Sent Events (SSE)
- **Kanban**: SortableJS for drag-and-drop

## Quick Start

### Local Development

```bash
# Clone and build
cargo build --release

# Run with defaults (port 3002, sqlite:data/watchtower.db)
./target/release/watchtower

# Or with custom config
PORT=3002 \
DATABASE_URL=sqlite:data/watchtower.db \
WATCHTOWER_API_TOKEN=your-token \
./target/release/watchtower
```

### Docker

```bash
# Build image
./scripts/build-docker.sh

# Run container
docker run -p 3002:3002 -v $(pwd)/data:/data watchtower:latest
```

### Add to Docker Compose

See `docker-compose.snippet.yml` for the service definition. Add to your stack:

```yaml
services:
  watchtower-dashboard:
    image: watchtower:latest
    # ... see docker-compose.snippet.yml
```

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `PORT` | `3002` | HTTP server port |
| `DATABASE_URL` | `sqlite:data/watchtower.db` | SQLite database path |
| `WATCHTOWER_API_TOKEN` | (empty) | Bearer token for API auth |
| `WATCHTOWER_USER` | (empty) | Basic auth username for web UI |
| `WATCHTOWER_PASS` | (empty) | Basic auth password for web UI |
| `TELEGRAM_BOT_TOKEN` | (optional) | Telegram bot token for notifications |
| `TELEGRAM_CHAT_ID` | (optional) | Telegram chat ID for notifications |

## API Endpoints

### Events (Activity Feed)
- `GET /api/events` — List events (paginated, filterable by type)
- `POST /api/events` — Create event (broadcasts via SSE)
- `GET /events/stream` — SSE stream for real-time events

### Tasks (Kanban)
- `GET /api/tasks` — List tasks (filterable by status, priority, assignee)
- `POST /api/tasks` — Create task
- `GET /api/tasks/{id}` — Get task details
- `PATCH /api/tasks/{id}` — Update task
- `DELETE /api/tasks/{id}` — Delete task
- `POST /api/tasks/{id}/comments` — Add comment
- `GET /api/tasks/{id}/comments` — List comments
- `GET /api/tasks/{id}/history` — Get status change history

### Sessions
- `GET /api/sessions` — List sessions
- `POST /api/sessions` — Create/update session (upsert by session_key)
- `GET /api/sessions/{id}` — Get session details

### Cron
- `GET /api/cron` — List cron jobs
- `POST /api/cron/sync` — Sync cron job data

### Usage
- `GET /api/usage` — Get usage stats (by model, date range)
- `POST /api/usage/report` — Report usage data

### Health
- `GET /health` — Health check endpoint

## Web UI

- `/` — Redirects to Feed
- `/feed` — Live activity feed
- `/board` — Kanban board
- `/costs` — Usage & costs dashboard
- `/cron` — Cron job status
- `/sessions` — Session browser

## Authentication

**API**: Bearer token via `Authorization: Bearer <token>` header (if `WATCHTOWER_API_TOKEN` is set)

**Web UI**: HTTP Basic Auth (if `WATCHTOWER_USER` and `WATCHTOWER_PASS` are set)

## Design

The UI follows a dark-mode design with:
- **Font**: DM Sans (body), Bricolage Grotesque (headings/numbers)
- **Icons**: Lucide
- **Colors**: Indigo accent with semantic colors (green=success, red=error, orange=warning)
- **Sidebar**: 260px fixed navigation

## License

MIT
