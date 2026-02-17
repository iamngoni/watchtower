use crate::models::*;
use anyhow::Result;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use sqlx::Row;
use std::str::FromStr;
use tracing::{debug, info};

/// Initialize database connection pool
pub async fn init_pool(db_url: &str) -> Result<SqlitePool> {
    info!(db_url = %db_url, "Initializing database connection pool");

    let options = SqliteConnectOptions::from_str(db_url)?
        .create_if_missing(true)
        .foreign_keys(true);

    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await?;

    debug!("Database connection pool created");

    // Run migrations
    info!("Running database migrations");
    sqlx::migrate!("./migrations").run(&pool).await?;
    info!("Database migrations completed successfully");

    Ok(pool)
}

// ============================================================================
// Events
// ============================================================================

pub async fn insert_event(pool: &SqlitePool, event: &CreateEvent) -> Result<Event> {
    let metadata = event.metadata.as_ref()
        .map(|m| serde_json::to_string(m).unwrap_or_else(|_| "{}".to_string()))
        .unwrap_or_else(|| "{}".to_string());
    
    let now = chrono::Utc::now().timestamp();
    
    let result = sqlx::query(
        r#"
        INSERT INTO events (event_type, summary, detail, session_id, task_id, metadata, created_at)
        VALUES (?, ?, ?, ?, ?, ?, ?)
        "#
    )
    .bind(&event.event_type)
    .bind(&event.summary)
    .bind(&event.detail)
    .bind(&event.session_id)
    .bind(event.task_id)
    .bind(&metadata)
    .bind(now)
    .execute(pool)
    .await?;

    let id = result.last_insert_rowid();
    
    Ok(Event {
        id,
        event_type: event.event_type.clone(),
        summary: event.summary.clone(),
        detail: event.detail.clone(),
        session_id: event.session_id.clone(),
        task_id: event.task_id,
        metadata,
        created_at: now,
    })
}

pub async fn list_events(
    pool: &SqlitePool,
    event_type: Option<&str>,
    limit: i64,
    offset: i64,
) -> Result<Vec<Event>> {
    let events = if let Some(et) = event_type {
        sqlx::query_as::<_, Event>(
            r#"
            SELECT id, event_type, summary, detail, session_id, task_id, metadata, created_at
            FROM events
            WHERE event_type = ?
            ORDER BY created_at DESC
            LIMIT ? OFFSET ?
            "#
        )
        .bind(et)
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await?
    } else {
        sqlx::query_as::<_, Event>(
            r#"
            SELECT id, event_type, summary, detail, session_id, task_id, metadata, created_at
            FROM events
            ORDER BY created_at DESC
            LIMIT ? OFFSET ?
            "#
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await?
    };
    
    Ok(events)
}

// ============================================================================
// Tasks
// ============================================================================

pub async fn insert_task(pool: &SqlitePool, task: &CreateTask) -> Result<Task> {
    let labels = task.labels.as_ref()
        .map(|l| serde_json::to_string(l).unwrap_or_else(|_| "[]".to_string()))
        .unwrap_or_else(|| "[]".to_string());
    
    let now = chrono::Utc::now().timestamp();
    
    let result = sqlx::query(
        r#"
        INSERT INTO tasks (title, description, priority, status, labels, created_by, assigned_to, due_date, created_at, updated_at)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#
    )
    .bind(&task.title)
    .bind(&task.description)
    .bind(&task.priority)
    .bind(&task.status)
    .bind(&labels)
    .bind(&task.created_by)
    .bind(&task.assigned_to)
    .bind(task.due_date)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await?;

    let id = result.last_insert_rowid();
    
    // Record initial history
    sqlx::query(
        r#"
        INSERT INTO task_history (task_id, from_status, to_status, changed_by, created_at)
        VALUES (?, NULL, ?, ?, ?)
        "#
    )
    .bind(id)
    .bind(&task.status)
    .bind(&task.created_by)
    .bind(now)
    .execute(pool)
    .await?;
    
    Ok(Task {
        id,
        title: task.title.clone(),
        description: task.description.clone(),
        priority: task.priority.clone(),
        status: task.status.clone(),
        labels,
        created_by: task.created_by.clone(),
        assigned_to: task.assigned_to.clone(),
        due_date: task.due_date,
        created_at: now,
        updated_at: now,
    })
}

pub async fn get_task(pool: &SqlitePool, id: i64) -> Result<Option<Task>> {
    let task = sqlx::query_as::<_, Task>(
        r#"
        SELECT id, title, description, priority, status, labels, created_by, assigned_to, due_date, created_at, updated_at
        FROM tasks
        WHERE id = ?
        "#
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    
    Ok(task)
}

pub async fn list_tasks(
    pool: &SqlitePool,
    status: Option<&str>,
    priority: Option<&str>,
    assigned_to: Option<&str>,
) -> Result<Vec<Task>> {
    let tasks = sqlx::query_as::<_, Task>(
        r#"
        SELECT id, title, description, priority, status, labels, created_by, assigned_to, due_date, created_at, updated_at
        FROM tasks
        ORDER BY CASE priority WHEN 'urgent' THEN 0 WHEN 'high' THEN 1 WHEN 'normal' THEN 2 WHEN 'low' THEN 3 END, created_at DESC
        "#
    )
    .fetch_all(pool)
    .await?;
    
    let filtered: Vec<Task> = tasks.into_iter()
        .filter(|t| {
            let status_match = status.map(|s| t.status == s).unwrap_or(true);
            let priority_match = priority.map(|p| t.priority == p).unwrap_or(true);
            let assigned_match = assigned_to.map(|a| t.assigned_to == a).unwrap_or(true);
            status_match && priority_match && assigned_match
        })
        .collect();
    
    Ok(filtered)
}

pub async fn update_task(
    pool: &SqlitePool,
    id: i64,
    update: &UpdateTask,
    changed_by: &str,
) -> Result<Option<Task>> {
    // Get current task
    let current = match get_task(pool, id).await? {
        Some(t) => t,
        None => return Ok(None),
    };
    
    let now = chrono::Utc::now().timestamp();
    let title = update.title.as_ref().unwrap_or(&current.title);
    let description = update.description.as_ref().or(current.description.as_ref());
    let priority = update.priority.as_ref().unwrap_or(&current.priority);
    let status = update.status.as_ref().unwrap_or(&current.status);
    let labels = update.labels.as_ref()
        .map(|l| serde_json::to_string(l).unwrap_or_else(|_| current.labels.clone()))
        .unwrap_or_else(|| current.labels.clone());
    let assigned_to = update.assigned_to.as_ref().unwrap_or(&current.assigned_to);
    let due_date = update.due_date.or(current.due_date);
    
    // Record status change in history if status changed
    if let Some(new_status) = &update.status {
        if new_status != &current.status {
            sqlx::query(
                r#"
                INSERT INTO task_history (task_id, from_status, to_status, changed_by, created_at)
                VALUES (?, ?, ?, ?, ?)
                "#
            )
            .bind(id)
            .bind(&current.status)
            .bind(new_status)
            .bind(changed_by)
            .bind(now)
            .execute(pool)
            .await?;
        }
    }
    
    sqlx::query(
        r#"
        UPDATE tasks
        SET title = ?, description = ?, priority = ?, status = ?, labels = ?, assigned_to = ?, due_date = ?, updated_at = ?
        WHERE id = ?
        "#
    )
    .bind(title)
    .bind(description)
    .bind(priority)
    .bind(status)
    .bind(&labels)
    .bind(assigned_to)
    .bind(due_date)
    .bind(now)
    .bind(id)
    .execute(pool)
    .await?;
    
    get_task(pool, id).await
}

pub async fn delete_task(pool: &SqlitePool, id: i64) -> Result<bool> {
    let result = sqlx::query("DELETE FROM tasks WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    
    Ok(result.rows_affected() > 0)
}

// ============================================================================
// Task Comments
// ============================================================================

pub async fn add_comment(pool: &SqlitePool, task_id: i64, comment: &CreateComment) -> Result<TaskComment> {
    let now = chrono::Utc::now().timestamp();
    
    let result = sqlx::query(
        r#"
        INSERT INTO task_comments (task_id, author, content, created_at)
        VALUES (?, ?, ?, ?)
        "#
    )
    .bind(task_id)
    .bind(&comment.author)
    .bind(&comment.content)
    .bind(now)
    .execute(pool)
    .await?;

    let id = result.last_insert_rowid();
    
    Ok(TaskComment {
        id,
        task_id,
        author: comment.author.clone(),
        content: comment.content.clone(),
        created_at: now,
    })
}

pub async fn get_task_comments(pool: &SqlitePool, task_id: i64) -> Result<Vec<TaskComment>> {
    let comments = sqlx::query_as::<_, TaskComment>(
        r#"
        SELECT id, task_id, author, content, created_at
        FROM task_comments
        WHERE task_id = ?
        ORDER BY created_at ASC
        "#
    )
    .bind(task_id)
    .fetch_all(pool)
    .await?;
    
    Ok(comments)
}

// ============================================================================
// Task History
// ============================================================================

pub async fn get_task_history(pool: &SqlitePool, task_id: i64) -> Result<Vec<TaskHistory>> {
    let history = sqlx::query_as::<_, TaskHistory>(
        r#"
        SELECT id, task_id, from_status, to_status, changed_by, created_at
        FROM task_history
        WHERE task_id = ?
        ORDER BY created_at DESC
        "#
    )
    .bind(task_id)
    .fetch_all(pool)
    .await?;
    
    Ok(history)
}

// ============================================================================
// Sessions
// ============================================================================

pub async fn upsert_session(pool: &SqlitePool, session: &CreateSession) -> Result<Session> {
    let now = chrono::Utc::now().timestamp();
    
    // Try to find existing session
    let existing = sqlx::query_as::<_, Session>(
        r#"
        SELECT id, session_key, title, session_type, model, input_tokens, output_tokens, 
               cache_read_tokens, cache_write_tokens, cost_usd, task_id, parent_session_id, started_at, ended_at
        FROM sessions
        WHERE session_key = ?
        "#
    )
    .bind(&session.session_key)
    .fetch_optional(pool)
    .await?;
    
    if let Some(existing) = existing {
        // Update existing
        sqlx::query(
            r#"
            UPDATE sessions
            SET title = COALESCE(?, title),
                model = COALESCE(?, model),
                input_tokens = input_tokens + ?,
                output_tokens = output_tokens + ?,
                cache_read_tokens = cache_read_tokens + ?,
                cache_write_tokens = cache_write_tokens + ?,
                cost_usd = cost_usd + ?
            WHERE id = ?
            "#
        )
        .bind(&session.title)
        .bind(&session.model)
        .bind(session.input_tokens)
        .bind(session.output_tokens)
        .bind(session.cache_read_tokens)
        .bind(session.cache_write_tokens)
        .bind(session.cost_usd)
        .bind(existing.id)
        .execute(pool)
        .await?;
        
        get_session(pool, existing.id).await.map(|o| o.unwrap())
    } else {
        // Insert new
        let result = sqlx::query(
            r#"
            INSERT INTO sessions (session_key, title, session_type, model, input_tokens, output_tokens, 
                                  cache_read_tokens, cache_write_tokens, cost_usd, task_id, parent_session_id, started_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#
        )
        .bind(&session.session_key)
        .bind(&session.title)
        .bind(&session.session_type)
        .bind(&session.model)
        .bind(session.input_tokens)
        .bind(session.output_tokens)
        .bind(session.cache_read_tokens)
        .bind(session.cache_write_tokens)
        .bind(session.cost_usd)
        .bind(session.task_id)
        .bind(session.parent_session_id)
        .bind(now)
        .execute(pool)
        .await?;

        let id = result.last_insert_rowid();
        
        Ok(Session {
            id,
            session_key: session.session_key.clone(),
            title: session.title.clone(),
            session_type: session.session_type.clone(),
            model: session.model.clone(),
            input_tokens: session.input_tokens,
            output_tokens: session.output_tokens,
            cache_read_tokens: session.cache_read_tokens,
            cache_write_tokens: session.cache_write_tokens,
            cost_usd: session.cost_usd,
            task_id: session.task_id,
            parent_session_id: session.parent_session_id,
            started_at: now,
            ended_at: None,
        })
    }
}

pub async fn get_session(pool: &SqlitePool, id: i64) -> Result<Option<Session>> {
    let session = sqlx::query_as::<_, Session>(
        r#"
        SELECT id, session_key, title, session_type, model, input_tokens, output_tokens, 
               cache_read_tokens, cache_write_tokens, cost_usd, task_id, parent_session_id, started_at, ended_at
        FROM sessions
        WHERE id = ?
        "#
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    
    Ok(session)
}

pub async fn list_sessions(
    pool: &SqlitePool,
    session_type: Option<&str>,
    limit: i64,
    offset: i64,
) -> Result<Vec<Session>> {
    let sessions = if let Some(st) = session_type {
        sqlx::query_as::<_, Session>(
            r#"
            SELECT id, session_key, title, session_type, model, input_tokens, output_tokens, 
                   cache_read_tokens, cache_write_tokens, cost_usd, task_id, parent_session_id, started_at, ended_at
            FROM sessions
            WHERE session_type = ?
            ORDER BY started_at DESC
            LIMIT ? OFFSET ?
            "#
        )
        .bind(st)
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await?
    } else {
        sqlx::query_as::<_, Session>(
            r#"
            SELECT id, session_key, title, session_type, model, input_tokens, output_tokens, 
                   cache_read_tokens, cache_write_tokens, cost_usd, task_id, parent_session_id, started_at, ended_at
            FROM sessions
            ORDER BY started_at DESC
            LIMIT ? OFFSET ?
            "#
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await?
    };
    
    Ok(sessions)
}

// ============================================================================
// Cron Jobs
// ============================================================================

pub async fn sync_cron_job(pool: &SqlitePool, job: &SyncCronJob) -> Result<CronJob> {
    let now = chrono::Utc::now().timestamp();
    let enabled = if job.enabled { 1i64 } else { 0i64 };
    
    sqlx::query(
        r#"
        INSERT INTO cron_jobs (job_id, name, schedule, enabled, last_status, last_run_at, next_run_at, consecutive_errors, updated_at)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(job_id) DO UPDATE SET
            name = excluded.name,
            schedule = excluded.schedule,
            enabled = excluded.enabled,
            last_status = excluded.last_status,
            last_run_at = excluded.last_run_at,
            next_run_at = excluded.next_run_at,
            consecutive_errors = excluded.consecutive_errors,
            updated_at = excluded.updated_at
        "#
    )
    .bind(&job.job_id)
    .bind(&job.name)
    .bind(&job.schedule)
    .bind(enabled)
    .bind(&job.last_status)
    .bind(job.last_run_at)
    .bind(job.next_run_at)
    .bind(job.consecutive_errors)
    .bind(now)
    .execute(pool)
    .await?;
    
    let cron = sqlx::query_as::<_, CronJob>(
        r#"
        SELECT id, job_id, name, schedule, enabled, last_status, last_run_at, next_run_at, consecutive_errors, updated_at
        FROM cron_jobs
        WHERE job_id = ?
        "#
    )
    .bind(&job.job_id)
    .fetch_one(pool)
    .await?;
    
    Ok(cron)
}

pub async fn list_cron_jobs(pool: &SqlitePool) -> Result<Vec<CronJob>> {
    let jobs = sqlx::query_as::<_, CronJob>(
        r#"
        SELECT id, job_id, name, schedule, enabled, last_status, last_run_at, next_run_at, consecutive_errors, updated_at
        FROM cron_jobs
        ORDER BY name ASC
        "#
    )
    .fetch_all(pool)
    .await?;
    
    Ok(jobs)
}

pub async fn get_cron_job(pool: &SqlitePool, job_id: &str) -> Result<Option<CronJob>> {
    let job = sqlx::query_as::<_, CronJob>(
        r#"
        SELECT id, job_id, name, schedule, enabled, last_status, last_run_at, next_run_at, consecutive_errors, updated_at
        FROM cron_jobs
        WHERE job_id = ?
        "#
    )
    .bind(job_id)
    .fetch_optional(pool)
    .await?;
    
    Ok(job)
}

pub async fn update_cron_job_enabled(pool: &SqlitePool, job_id: &str, enabled: bool) -> Result<Option<CronJob>> {
    let now = chrono::Utc::now().timestamp();
    let enabled_val = if enabled { 1i64 } else { 0i64 };
    
    let result = sqlx::query(
        r#"
        UPDATE cron_jobs
        SET enabled = ?, updated_at = ?
        WHERE job_id = ?
        "#
    )
    .bind(enabled_val)
    .bind(now)
    .bind(job_id)
    .execute(pool)
    .await?;
    
    if result.rows_affected() == 0 {
        return Ok(None);
    }
    
    get_cron_job(pool, job_id).await
}

// ============================================================================
// Usage
// ============================================================================

pub async fn report_usage(pool: &SqlitePool, usage: &ReportUsage) -> Result<UsageDaily> {
    sqlx::query(
        r#"
        INSERT INTO usage_daily (date, model, input_tokens, output_tokens, cost_usd)
        VALUES (?, ?, ?, ?, ?)
        ON CONFLICT(date, model) DO UPDATE SET
            input_tokens = usage_daily.input_tokens + excluded.input_tokens,
            output_tokens = usage_daily.output_tokens + excluded.output_tokens,
            cost_usd = usage_daily.cost_usd + excluded.cost_usd
        "#
    )
    .bind(&usage.date)
    .bind(&usage.model)
    .bind(usage.input_tokens)
    .bind(usage.output_tokens)
    .bind(usage.cost_usd)
    .execute(pool)
    .await?;
    
    let record = sqlx::query_as::<_, UsageDaily>(
        r#"
        SELECT id, date, model, input_tokens, output_tokens, cost_usd
        FROM usage_daily
        WHERE date = ? AND model = ?
        "#
    )
    .bind(&usage.date)
    .bind(&usage.model)
    .fetch_one(pool)
    .await?;
    
    Ok(record)
}

pub async fn get_usage_stats(
    pool: &SqlitePool,
    start_date: Option<&str>,
    end_date: Option<&str>,
) -> Result<UsageStats> {
    let start = start_date.unwrap_or("2000-01-01");
    let end = end_date.unwrap_or("2099-12-31");
    
    // Get totals
    let row = sqlx::query(
        r#"
        SELECT 
            COALESCE(SUM(input_tokens), 0) as total_input,
            COALESCE(SUM(output_tokens), 0) as total_output,
            COALESCE(SUM(cost_usd), 0.0) as total_cost
        FROM usage_daily
        WHERE date >= ? AND date <= ?
        "#
    )
    .bind(start)
    .bind(end)
    .fetch_one(pool)
    .await?;
    
    let total_input: i64 = row.get("total_input");
    let total_output: i64 = row.get("total_output");
    let total_cost: f64 = row.get("total_cost");
    
    // Get by model
    let rows = sqlx::query(
        r#"
        SELECT 
            model,
            SUM(input_tokens) as input_tokens,
            SUM(output_tokens) as output_tokens,
            SUM(cost_usd) as cost_usd
        FROM usage_daily
        WHERE date >= ? AND date <= ?
        GROUP BY model
        ORDER BY cost_usd DESC
        "#
    )
    .bind(start)
    .bind(end)
    .fetch_all(pool)
    .await?;
    
    let by_model: Vec<ModelUsage> = rows.iter().map(|r| ModelUsage {
        model: r.get("model"),
        input_tokens: r.get::<i64, _>("input_tokens"),
        output_tokens: r.get::<i64, _>("output_tokens"),
        cost_usd: r.get::<f64, _>("cost_usd"),
    }).collect();
    
    Ok(UsageStats {
        total_input_tokens: total_input,
        total_output_tokens: total_output,
        total_cost_usd: total_cost,
        by_model,
    })
}

// ============================================================================
// Search
// ============================================================================

pub async fn search_tasks(pool: &SqlitePool, query: &str) -> Result<Vec<Task>> {
    let pattern = format!("%{}%", query);
    
    let tasks = sqlx::query_as::<_, Task>(
        r#"
        SELECT id, title, description, priority, status, labels, created_by, assigned_to, due_date, created_at, updated_at
        FROM tasks
        WHERE title LIKE ? OR description LIKE ?
        ORDER BY updated_at DESC
        LIMIT 10
        "#
    )
    .bind(&pattern)
    .bind(&pattern)
    .fetch_all(pool)
    .await?;
    
    Ok(tasks)
}

pub async fn search_events(pool: &SqlitePool, query: &str) -> Result<Vec<Event>> {
    let pattern = format!("%{}%", query);
    
    let events = sqlx::query_as::<_, Event>(
        r#"
        SELECT id, event_type, summary, detail, session_id, task_id, metadata, created_at
        FROM events
        WHERE summary LIKE ? OR detail LIKE ?
        ORDER BY created_at DESC
        LIMIT 10
        "#
    )
    .bind(&pattern)
    .bind(&pattern)
    .fetch_all(pool)
    .await?;
    
    Ok(events)
}

pub async fn search_sessions(pool: &SqlitePool, query: &str) -> Result<Vec<Session>> {
    let pattern = format!("%{}%", query);
    
    let sessions = sqlx::query_as::<_, Session>(
        r#"
        SELECT id, session_key, title, session_type, model, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, cost_usd, task_id, parent_session_id, started_at, ended_at
        FROM sessions
        WHERE session_key LIKE ? OR title LIKE ? OR model LIKE ?
        ORDER BY started_at DESC
        LIMIT 10
        "#
    )
    .bind(&pattern)
    .bind(&pattern)
    .bind(&pattern)
    .fetch_all(pool)
    .await?;
    
    Ok(sessions)
}

// ============================================================================
// Daily Costs
// ============================================================================

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, sqlx::FromRow)]
pub struct DailyCostRow {
    pub date: String,
    pub cost_usd: f64,
    pub input_tokens: i64,
    pub output_tokens: i64,
}

pub async fn get_daily_costs(pool: &SqlitePool, days: i32) -> Result<Vec<DailyCostRow>> {
    let costs = sqlx::query_as::<_, DailyCostRow>(
        r#"
        SELECT 
            date,
            SUM(cost_usd) as cost_usd,
            SUM(input_tokens) as input_tokens,
            SUM(output_tokens) as output_tokens
        FROM usage_daily
        WHERE date >= date('now', '-' || ? || ' days')
        GROUP BY date
        ORDER BY date DESC
        "#
    )
    .bind(days)
    .fetch_all(pool)
    .await?;
    
    Ok(costs)
}

// ============================================================================
// Cron Run History
// ============================================================================

pub async fn get_cron_run_history(pool: &SqlitePool, job_id: &str, limit: i32) -> Result<Vec<Event>> {
    let events = sqlx::query_as::<_, Event>(
        r#"
        SELECT id, event_type, summary, detail, session_id, task_id, metadata, created_at
        FROM events
        WHERE event_type IN ('cron', 'cron_run_requested', 'cron_run_completed', 'cron_run_failed')
          AND (metadata LIKE ? OR summary LIKE ?)
        ORDER BY created_at DESC
        LIMIT ?
        "#
    )
    .bind(format!("%{}%", job_id))
    .bind(format!("%{}%", job_id))
    .bind(limit)
    .fetch_all(pool)
    .await?;
    
    Ok(events)
}

// ============================================================================
// Events for Session
// ============================================================================

pub async fn get_events_for_session(pool: &SqlitePool, session_id: &str) -> Result<Vec<Event>> {
    let events = sqlx::query_as::<_, Event>(
        r#"
        SELECT id, event_type, summary, detail, session_id, task_id, metadata, created_at
        FROM events
        WHERE session_id = ?
        ORDER BY created_at ASC
        "#
    )
    .bind(session_id)
    .fetch_all(pool)
    .await?;
    
    Ok(events)
}
