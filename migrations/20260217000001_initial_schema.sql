-- Initial database schema for Watchtower

-- Activity feed events
CREATE TABLE IF NOT EXISTS events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    event_type TEXT NOT NULL,
    summary TEXT NOT NULL,
    detail TEXT,
    session_id TEXT,
    task_id INTEGER,
    metadata TEXT DEFAULT '{}',
    created_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
    FOREIGN KEY (task_id) REFERENCES tasks(id) ON DELETE SET NULL
);

CREATE INDEX IF NOT EXISTS idx_events_created_at ON events(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_events_event_type ON events(event_type);
CREATE INDEX IF NOT EXISTS idx_events_session_id ON events(session_id);

-- Kanban tasks
CREATE TABLE IF NOT EXISTS tasks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    title TEXT NOT NULL,
    description TEXT,
    priority TEXT NOT NULL DEFAULT 'normal',
    status TEXT NOT NULL DEFAULT 'backlog',
    labels TEXT DEFAULT '[]',
    created_by TEXT NOT NULL DEFAULT 'human',
    assigned_to TEXT NOT NULL DEFAULT 'unassigned',
    due_date INTEGER,
    created_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
    updated_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
    CONSTRAINT valid_priority CHECK (priority IN ('low', 'normal', 'high', 'urgent')),
    CONSTRAINT valid_status CHECK (status IN ('backlog', 'todo', 'in_progress', 'blocked', 'in_review', 'done')),
    CONSTRAINT valid_created_by CHECK (created_by IN ('human', 'agent')),
    CONSTRAINT valid_assigned_to CHECK (assigned_to IN ('human', 'agent', 'unassigned'))
);

CREATE INDEX IF NOT EXISTS idx_tasks_status ON tasks(status);
CREATE INDEX IF NOT EXISTS idx_tasks_priority ON tasks(priority);
CREATE INDEX IF NOT EXISTS idx_tasks_assigned_to ON tasks(assigned_to);

-- Task comments
CREATE TABLE IF NOT EXISTS task_comments (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id INTEGER NOT NULL,
    author TEXT NOT NULL DEFAULT 'human',
    content TEXT NOT NULL,
    created_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
    FOREIGN KEY (task_id) REFERENCES tasks(id) ON DELETE CASCADE,
    CONSTRAINT valid_author CHECK (author IN ('human', 'agent'))
);

CREATE INDEX IF NOT EXISTS idx_task_comments_task_id ON task_comments(task_id);

-- Task history (status changes)
CREATE TABLE IF NOT EXISTS task_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id INTEGER NOT NULL,
    from_status TEXT,
    to_status TEXT NOT NULL,
    changed_by TEXT NOT NULL DEFAULT 'human',
    created_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
    FOREIGN KEY (task_id) REFERENCES tasks(id) ON DELETE CASCADE,
    CONSTRAINT valid_changed_by CHECK (changed_by IN ('human', 'agent'))
);

CREATE INDEX IF NOT EXISTS idx_task_history_task_id ON task_history(task_id);

-- Session records
CREATE TABLE IF NOT EXISTS sessions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_key TEXT NOT NULL UNIQUE,
    title TEXT,
    session_type TEXT NOT NULL DEFAULT 'conversation',
    model TEXT,
    input_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    cache_read_tokens INTEGER NOT NULL DEFAULT 0,
    cache_write_tokens INTEGER NOT NULL DEFAULT 0,
    cost_usd REAL NOT NULL DEFAULT 0.0,
    task_id INTEGER,
    parent_session_id INTEGER,
    started_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
    ended_at INTEGER,
    FOREIGN KEY (task_id) REFERENCES tasks(id) ON DELETE SET NULL,
    FOREIGN KEY (parent_session_id) REFERENCES sessions(id) ON DELETE SET NULL,
    CONSTRAINT valid_session_type CHECK (session_type IN ('conversation', 'sub_agent', 'cron'))
);

CREATE INDEX IF NOT EXISTS idx_sessions_session_key ON sessions(session_key);
CREATE INDEX IF NOT EXISTS idx_sessions_session_type ON sessions(session_type);
CREATE INDEX IF NOT EXISTS idx_sessions_started_at ON sessions(started_at DESC);

-- Cron job snapshots
CREATE TABLE IF NOT EXISTS cron_jobs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    job_id TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    schedule TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    last_status TEXT,
    last_run_at INTEGER,
    next_run_at INTEGER,
    consecutive_errors INTEGER NOT NULL DEFAULT 0,
    updated_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
);

CREATE INDEX IF NOT EXISTS idx_cron_jobs_job_id ON cron_jobs(job_id);
CREATE INDEX IF NOT EXISTS idx_cron_jobs_enabled ON cron_jobs(enabled);

-- Daily usage aggregates
CREATE TABLE IF NOT EXISTS usage_daily (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    date TEXT NOT NULL,
    model TEXT NOT NULL,
    input_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    cost_usd REAL NOT NULL DEFAULT 0.0,
    UNIQUE(date, model)
);

CREATE INDEX IF NOT EXISTS idx_usage_daily_date ON usage_daily(date DESC);
CREATE INDEX IF NOT EXISTS idx_usage_daily_model ON usage_daily(model);
