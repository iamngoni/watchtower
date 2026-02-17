use crate::db;
use crate::models::*;
use crate::sse::Broadcaster;
use actix_web::{delete, get, patch, post, web, HttpRequest, HttpResponse, Responder};
use sqlx::SqlitePool;
use tracing::{error, info};

/// Check API token authentication
fn check_auth(req: &HttpRequest, expected_token: &str) -> bool {
    if expected_token.is_empty() {
        return true; // No auth required if token is empty
    }
    
    req.headers()
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.strip_prefix("Bearer ").unwrap_or(v) == expected_token)
        .unwrap_or(false)
}

macro_rules! require_auth {
    ($req:expr, $config:expr) => {
        if !check_auth($req, &$config.api_token) {
            return HttpResponse::Unauthorized().json(serde_json::json!({
                "error": "Invalid or missing API token"
            }));
        }
    };
}

// ============================================================================
// Health Check
// ============================================================================

#[get("/health")]
pub async fn health_check() -> impl Responder {
    HttpResponse::Ok().json(serde_json::json!({
        "status": "ok",
        "service": "watchtower",
        "version": env!("CARGO_PKG_VERSION")
    }))
}

// ============================================================================
// Events API
// ============================================================================

#[derive(Debug, serde::Deserialize)]
pub struct EventsQuery {
    #[serde(rename = "type")]
    event_type: Option<String>,
    #[serde(default = "default_limit")]
    limit: i64,
    #[serde(default)]
    offset: i64,
}

fn default_limit() -> i64 { 50 }

#[get("/api/events")]
pub async fn list_events(
    req: HttpRequest,
    pool: web::Data<SqlitePool>,
    config: web::Data<crate::Config>,
    query: web::Query<EventsQuery>,
) -> impl Responder {
    require_auth!(&req, config);
    
    match db::list_events(&pool, query.event_type.as_deref(), query.limit, query.offset).await {
        Ok(events) => HttpResponse::Ok().json(events),
        Err(e) => {
            error!("Failed to list events: {}", e);
            HttpResponse::InternalServerError().json(serde_json::json!({
                "error": e.to_string()
            }))
        }
    }
}

#[post("/api/events")]
pub async fn create_event(
    req: HttpRequest,
    pool: web::Data<SqlitePool>,
    config: web::Data<crate::Config>,
    broadcaster: web::Data<Broadcaster>,
    body: web::Json<CreateEvent>,
) -> impl Responder {
    require_auth!(&req, config);
    
    match db::insert_event(&pool, &body).await {
        Ok(event) => {
            // Broadcast rendered HTML via SSE for HTMX
            if let Some(html) = crate::web::render_event_html(&event) {
                broadcaster.broadcast_html("event", html);
            } else {
                broadcaster.broadcast("event", serde_json::to_value(&event).unwrap());
            }
            info!(event_type = %event.event_type, "Event created and broadcast");
            HttpResponse::Created().json(event)
        }
        Err(e) => {
            error!("Failed to create event: {}", e);
            HttpResponse::InternalServerError().json(serde_json::json!({
                "error": e.to_string()
            }))
        }
    }
}

#[get("/events/stream")]
pub async fn events_stream(
    broadcaster: web::Data<Broadcaster>,
) -> impl Responder {
    let client = broadcaster.subscribe();
    
    HttpResponse::Ok()
        .content_type("text/event-stream")
        .insert_header(("Cache-Control", "no-cache"))
        .insert_header(("Connection", "keep-alive"))
        .insert_header(("X-Accel-Buffering", "no"))
        .streaming(client)
}

// ============================================================================
// Tasks API
// ============================================================================

#[derive(Debug, serde::Deserialize)]
pub struct TasksQuery {
    status: Option<String>,
    priority: Option<String>,
    assigned_to: Option<String>,
}

#[get("/api/tasks")]
pub async fn list_tasks(
    req: HttpRequest,
    pool: web::Data<SqlitePool>,
    config: web::Data<crate::Config>,
    query: web::Query<TasksQuery>,
) -> impl Responder {
    require_auth!(&req, config);
    
    match db::list_tasks(
        &pool,
        query.status.as_deref(),
        query.priority.as_deref(),
        query.assigned_to.as_deref(),
    ).await {
        Ok(tasks) => HttpResponse::Ok().json(tasks),
        Err(e) => {
            error!("Failed to list tasks: {}", e);
            HttpResponse::InternalServerError().json(serde_json::json!({
                "error": e.to_string()
            }))
        }
    }
}

#[post("/api/tasks")]
pub async fn create_task(
    req: HttpRequest,
    pool: web::Data<SqlitePool>,
    config: web::Data<crate::Config>,
    broadcaster: web::Data<Broadcaster>,
    body: web::Json<CreateTask>,
) -> impl Responder {
    require_auth!(&req, config);
    
    match db::insert_task(&pool, &body).await {
        Ok(task) => {
            // Broadcast via SSE
            broadcaster.broadcast("task_created", serde_json::to_value(&task).unwrap());
            info!(task_id = task.id, title = %task.title, "Task created");
            HttpResponse::Created().json(task)
        }
        Err(e) => {
            error!("Failed to create task: {}", e);
            HttpResponse::InternalServerError().json(serde_json::json!({
                "error": e.to_string()
            }))
        }
    }
}

#[get("/api/tasks/{id}")]
pub async fn get_task(
    req: HttpRequest,
    pool: web::Data<SqlitePool>,
    config: web::Data<crate::Config>,
    path: web::Path<i64>,
) -> impl Responder {
    require_auth!(&req, config);
    
    let id = path.into_inner();
    
    match db::get_task(&pool, id).await {
        Ok(Some(task)) => HttpResponse::Ok().json(task),
        Ok(None) => HttpResponse::NotFound().json(serde_json::json!({
            "error": "Task not found"
        })),
        Err(e) => {
            error!("Failed to get task {}: {}", id, e);
            HttpResponse::InternalServerError().json(serde_json::json!({
                "error": e.to_string()
            }))
        }
    }
}

#[derive(Debug, serde::Deserialize)]
pub struct UpdateTaskBody {
    #[serde(flatten)]
    update: UpdateTask,
    #[serde(default = "default_changed_by")]
    changed_by: String,
}

fn default_changed_by() -> String { "human".to_string() }

#[patch("/api/tasks/{id}")]
pub async fn update_task(
    req: HttpRequest,
    pool: web::Data<SqlitePool>,
    config: web::Data<crate::Config>,
    broadcaster: web::Data<Broadcaster>,
    path: web::Path<i64>,
    body: web::Json<UpdateTaskBody>,
) -> impl Responder {
    require_auth!(&req, config);
    
    let id = path.into_inner();
    
    match db::update_task(&pool, id, &body.update, &body.changed_by).await {
        Ok(Some(task)) => {
            // Broadcast via SSE
            broadcaster.broadcast("task_updated", serde_json::to_value(&task).unwrap());
            info!(task_id = task.id, "Task updated");
            HttpResponse::Ok().json(task)
        }
        Ok(None) => HttpResponse::NotFound().json(serde_json::json!({
            "error": "Task not found"
        })),
        Err(e) => {
            error!("Failed to update task {}: {}", id, e);
            HttpResponse::InternalServerError().json(serde_json::json!({
                "error": e.to_string()
            }))
        }
    }
}

#[delete("/api/tasks/{id}")]
pub async fn delete_task(
    req: HttpRequest,
    pool: web::Data<SqlitePool>,
    config: web::Data<crate::Config>,
    broadcaster: web::Data<Broadcaster>,
    path: web::Path<i64>,
) -> impl Responder {
    require_auth!(&req, config);
    
    let id = path.into_inner();
    
    match db::delete_task(&pool, id).await {
        Ok(true) => {
            broadcaster.broadcast("task_deleted", serde_json::json!({ "id": id }));
            info!(task_id = id, "Task deleted");
            HttpResponse::NoContent().finish()
        }
        Ok(false) => HttpResponse::NotFound().json(serde_json::json!({
            "error": "Task not found"
        })),
        Err(e) => {
            error!("Failed to delete task {}: {}", id, e);
            HttpResponse::InternalServerError().json(serde_json::json!({
                "error": e.to_string()
            }))
        }
    }
}

// ============================================================================
// Task Comments API
// ============================================================================

#[post("/api/tasks/{id}/comments")]
pub async fn add_task_comment(
    req: HttpRequest,
    pool: web::Data<SqlitePool>,
    config: web::Data<crate::Config>,
    path: web::Path<i64>,
    body: web::Json<CreateComment>,
) -> impl Responder {
    require_auth!(&req, config);
    
    let task_id = path.into_inner();
    
    // Check task exists
    match db::get_task(&pool, task_id).await {
        Ok(None) => return HttpResponse::NotFound().json(serde_json::json!({
            "error": "Task not found"
        })),
        Err(e) => return HttpResponse::InternalServerError().json(serde_json::json!({
            "error": e.to_string()
        })),
        _ => {}
    }
    
    match db::add_comment(&pool, task_id, &body).await {
        Ok(comment) => {
            info!(task_id = task_id, comment_id = comment.id, "Comment added");
            HttpResponse::Created().json(comment)
        }
        Err(e) => {
            error!("Failed to add comment to task {}: {}", task_id, e);
            HttpResponse::InternalServerError().json(serde_json::json!({
                "error": e.to_string()
            }))
        }
    }
}

#[get("/api/tasks/{id}/comments")]
pub async fn get_task_comments(
    req: HttpRequest,
    pool: web::Data<SqlitePool>,
    config: web::Data<crate::Config>,
    path: web::Path<i64>,
) -> impl Responder {
    require_auth!(&req, config);
    
    let task_id = path.into_inner();
    
    match db::get_task_comments(&pool, task_id).await {
        Ok(comments) => HttpResponse::Ok().json(comments),
        Err(e) => {
            error!("Failed to get comments for task {}: {}", task_id, e);
            HttpResponse::InternalServerError().json(serde_json::json!({
                "error": e.to_string()
            }))
        }
    }
}

// ============================================================================
// Task History API
// ============================================================================

#[get("/api/tasks/{id}/history")]
pub async fn get_task_history(
    req: HttpRequest,
    pool: web::Data<SqlitePool>,
    config: web::Data<crate::Config>,
    path: web::Path<i64>,
) -> impl Responder {
    require_auth!(&req, config);
    
    let task_id = path.into_inner();
    
    match db::get_task_history(&pool, task_id).await {
        Ok(history) => HttpResponse::Ok().json(history),
        Err(e) => {
            error!("Failed to get history for task {}: {}", task_id, e);
            HttpResponse::InternalServerError().json(serde_json::json!({
                "error": e.to_string()
            }))
        }
    }
}

// ============================================================================
// Task Events API (Activity linked to task)
// ============================================================================

#[get("/api/tasks/{id}/events")]
pub async fn get_task_events(
    req: HttpRequest,
    pool: web::Data<SqlitePool>,
    config: web::Data<crate::Config>,
    path: web::Path<i64>,
) -> impl Responder {
    require_auth!(&req, config);
    
    let task_id = path.into_inner();
    
    match db::get_events_for_task(&pool, task_id).await {
        Ok(events) => HttpResponse::Ok().json(events),
        Err(e) => {
            error!("Failed to get events for task {}: {}", task_id, e);
            HttpResponse::InternalServerError().json(serde_json::json!({
                "error": e.to_string()
            }))
        }
    }
}

// ============================================================================
// Sessions API
// ============================================================================

#[derive(Debug, serde::Deserialize)]
pub struct SessionsQuery {
    #[serde(rename = "type")]
    session_type: Option<String>,
    #[serde(default = "default_limit")]
    limit: i64,
    #[serde(default)]
    offset: i64,
}

#[get("/api/sessions")]
pub async fn list_sessions(
    req: HttpRequest,
    pool: web::Data<SqlitePool>,
    config: web::Data<crate::Config>,
    query: web::Query<SessionsQuery>,
) -> impl Responder {
    require_auth!(&req, config);
    
    match db::list_sessions(&pool, query.session_type.as_deref(), query.limit, query.offset).await {
        Ok(sessions) => HttpResponse::Ok().json(sessions),
        Err(e) => {
            error!("Failed to list sessions: {}", e);
            HttpResponse::InternalServerError().json(serde_json::json!({
                "error": e.to_string()
            }))
        }
    }
}

#[post("/api/sessions")]
pub async fn create_session(
    req: HttpRequest,
    pool: web::Data<SqlitePool>,
    config: web::Data<crate::Config>,
    body: web::Json<CreateSession>,
) -> impl Responder {
    require_auth!(&req, config);
    
    match db::upsert_session(&pool, &body).await {
        Ok(session) => {
            info!(session_key = %session.session_key, "Session upserted");
            HttpResponse::Created().json(session)
        }
        Err(e) => {
            error!("Failed to create/update session: {}", e);
            HttpResponse::InternalServerError().json(serde_json::json!({
                "error": e.to_string()
            }))
        }
    }
}

#[get("/api/sessions/{id}")]
pub async fn get_session(
    req: HttpRequest,
    pool: web::Data<SqlitePool>,
    config: web::Data<crate::Config>,
    path: web::Path<i64>,
) -> impl Responder {
    require_auth!(&req, config);
    
    let id = path.into_inner();
    
    match db::get_session(&pool, id).await {
        Ok(Some(session)) => HttpResponse::Ok().json(session),
        Ok(None) => HttpResponse::NotFound().json(serde_json::json!({
            "error": "Session not found"
        })),
        Err(e) => {
            error!("Failed to get session {}: {}", id, e);
            HttpResponse::InternalServerError().json(serde_json::json!({
                "error": e.to_string()
            }))
        }
    }
}

// ============================================================================
// Cron API
// ============================================================================

#[get("/api/cron")]
pub async fn list_cron_jobs(
    req: HttpRequest,
    pool: web::Data<SqlitePool>,
    config: web::Data<crate::Config>,
) -> impl Responder {
    require_auth!(&req, config);
    
    match db::list_cron_jobs(&pool).await {
        Ok(jobs) => HttpResponse::Ok().json(jobs),
        Err(e) => {
            error!("Failed to list cron jobs: {}", e);
            HttpResponse::InternalServerError().json(serde_json::json!({
                "error": e.to_string()
            }))
        }
    }
}

#[derive(Debug, serde::Deserialize)]
pub struct UpdateCronJobBody {
    #[serde(default)]
    enabled: Option<bool>,
}

#[patch("/api/cron/{job_id}")]
pub async fn update_cron_job(
    req: HttpRequest,
    pool: web::Data<SqlitePool>,
    config: web::Data<crate::Config>,
    path: web::Path<String>,
    body: web::Json<UpdateCronJobBody>,
) -> impl Responder {
    require_auth!(&req, config);
    
    let job_id = path.into_inner();
    
    if let Some(enabled) = body.enabled {
        match db::update_cron_job_enabled(&pool, &job_id, enabled).await {
            Ok(Some(job)) => {
                info!(job_id = %job_id, enabled = enabled, "Cron job updated");
                HttpResponse::Ok().json(job)
            }
            Ok(None) => HttpResponse::NotFound().json(serde_json::json!({
                "error": "Cron job not found"
            })),
            Err(e) => {
                error!("Failed to update cron job {}: {}", job_id, e);
                HttpResponse::InternalServerError().json(serde_json::json!({
                    "error": e.to_string()
                }))
            }
        }
    } else {
        HttpResponse::BadRequest().json(serde_json::json!({
            "error": "No fields to update"
        }))
    }
}

#[post("/api/cron/{job_id}/run")]
pub async fn run_cron_job(
    req: HttpRequest,
    pool: web::Data<SqlitePool>,
    config: web::Data<crate::Config>,
    broadcaster: web::Data<Broadcaster>,
    path: web::Path<String>,
) -> impl Responder {
    require_auth!(&req, config);
    
    let job_id = path.into_inner();
    
    // Check job exists
    match db::get_cron_job(&pool, &job_id).await {
        Ok(Some(job)) => {
            // Record a "run requested" event
            let event = CreateEvent {
                event_type: "cron_run_requested".to_string(),
                summary: format!("Manual run requested for cron job: {}", job.name),
                detail: Some(format!("Job ID: {}", job_id)),
                session_id: None,
                task_id: None,
                metadata: Some(serde_json::json!({
                    "job_id": job_id,
                    "job_name": job.name
                })),
            };
            
            match db::insert_event(&pool, &event).await {
                Ok(evt) => {
                    broadcaster.broadcast("event", serde_json::to_value(&evt).unwrap());
                    info!(job_id = %job_id, "Cron job run requested");
                    HttpResponse::Ok().json(serde_json::json!({
                        "status": "run_requested",
                        "job_id": job_id,
                        "message": "Run request recorded. The agent will pick this up on next check."
                    }))
                }
                Err(e) => {
                    error!("Failed to record run request for cron job {}: {}", job_id, e);
                    HttpResponse::InternalServerError().json(serde_json::json!({
                        "error": e.to_string()
                    }))
                }
            }
        }
        Ok(None) => HttpResponse::NotFound().json(serde_json::json!({
            "error": "Cron job not found"
        })),
        Err(e) => {
            error!("Failed to get cron job {}: {}", job_id, e);
            HttpResponse::InternalServerError().json(serde_json::json!({
                "error": e.to_string()
            }))
        }
    }
}

#[derive(Debug, serde::Deserialize)]
pub struct SyncCronRequest {
    jobs: Vec<SyncCronJob>,
}

#[post("/api/cron/sync")]
pub async fn sync_cron_jobs(
    req: HttpRequest,
    pool: web::Data<SqlitePool>,
    config: web::Data<crate::Config>,
    body: web::Json<SyncCronRequest>,
) -> impl Responder {
    require_auth!(&req, config);
    
    let mut synced = Vec::new();
    
    for job in &body.jobs {
        match db::sync_cron_job(&pool, job).await {
            Ok(j) => synced.push(j),
            Err(e) => {
                error!("Failed to sync cron job {}: {}", job.job_id, e);
                return HttpResponse::InternalServerError().json(serde_json::json!({
                    "error": e.to_string()
                }));
            }
        }
    }
    
    info!(count = synced.len(), "Cron jobs synced");
    HttpResponse::Ok().json(synced)
}

// ============================================================================
// Usage API
// ============================================================================

#[derive(Debug, serde::Deserialize)]
pub struct UsageQuery {
    start_date: Option<String>,
    end_date: Option<String>,
}

#[get("/api/usage")]
pub async fn get_usage(
    req: HttpRequest,
    pool: web::Data<SqlitePool>,
    config: web::Data<crate::Config>,
    query: web::Query<UsageQuery>,
) -> impl Responder {
    require_auth!(&req, config);
    
    match db::get_usage_stats(&pool, query.start_date.as_deref(), query.end_date.as_deref()).await {
        Ok(stats) => HttpResponse::Ok().json(stats),
        Err(e) => {
            error!("Failed to get usage stats: {}", e);
            HttpResponse::InternalServerError().json(serde_json::json!({
                "error": e.to_string()
            }))
        }
    }
}

#[post("/api/usage/report")]
pub async fn report_usage(
    req: HttpRequest,
    pool: web::Data<SqlitePool>,
    config: web::Data<crate::Config>,
    body: web::Json<ReportUsage>,
) -> impl Responder {
    require_auth!(&req, config);
    
    match db::report_usage(&pool, &body).await {
        Ok(usage) => {
            info!(date = %usage.date, model = %usage.model, "Usage reported");
            HttpResponse::Created().json(usage)
        }
        Err(e) => {
            error!("Failed to report usage: {}", e);
            HttpResponse::InternalServerError().json(serde_json::json!({
                "error": e.to_string()
            }))
        }
    }
}

// ============================================================================
// Search API
// ============================================================================

#[derive(Debug, serde::Deserialize)]
pub struct SearchQuery {
    q: String,
}

#[derive(Debug, serde::Serialize)]
pub struct SearchResults {
    tasks: Vec<Task>,
    events: Vec<Event>,
    sessions: Vec<Session>,
}

#[get("/api/search")]
pub async fn search_api(
    req: HttpRequest,
    pool: web::Data<SqlitePool>,
    config: web::Data<crate::Config>,
    query: web::Query<SearchQuery>,
) -> impl Responder {
    require_auth!(&req, config);
    
    let q = &query.q;
    
    // Search tasks
    let tasks = db::search_tasks(&pool, q).await.unwrap_or_default();
    
    // Search events
    let events = db::search_events(&pool, q).await.unwrap_or_default();
    
    // Search sessions
    let sessions = db::search_sessions(&pool, q).await.unwrap_or_default();
    
    HttpResponse::Ok().json(SearchResults {
        tasks,
        events,
        sessions,
    })
}

// ============================================================================
// Daily Costs API
// ============================================================================

#[derive(Debug, serde::Serialize)]
pub struct DailyCost {
    pub date: String,
    pub cost_usd: f64,
    pub input_tokens: i64,
    pub output_tokens: i64,
}

#[get("/api/costs/daily")]
pub async fn get_daily_costs(
    req: HttpRequest,
    pool: web::Data<SqlitePool>,
    config: web::Data<crate::Config>,
) -> impl Responder {
    require_auth!(&req, config);
    
    match db::get_daily_costs(&pool, 14).await {
        Ok(costs) => HttpResponse::Ok().json(costs),
        Err(e) => {
            error!("Failed to get daily costs: {}", e);
            HttpResponse::InternalServerError().json(serde_json::json!({
                "error": e.to_string()
            }))
        }
    }
}

// ============================================================================
// Agent Status API (OpenClaw-focused)
// ============================================================================

#[derive(Debug, serde::Serialize)]
pub struct AgentStatus {
    pub is_active: bool,
    pub current_session: Option<String>,
    pub current_model: Option<String>,
    pub last_activity: Option<i64>,
    pub active_subagents: Vec<Session>,
}

#[get("/api/agent/status")]
pub async fn get_agent_status(
    req: HttpRequest,
    pool: web::Data<SqlitePool>,
    config: web::Data<crate::Config>,
) -> impl Responder {
    require_auth!(&req, config);
    
    // Check for recent events (last 5 minutes)
    let five_min_ago = chrono::Utc::now().timestamp() - 300;
    let recent_events = db::list_events(&pool, None, 1, 0).await.unwrap_or_default();
    
    let is_active = recent_events.first()
        .map(|e| e.created_at > five_min_ago)
        .unwrap_or(false);
    
    let last_activity = recent_events.first().map(|e| e.created_at);
    
    // Get most recent active session
    let sessions = db::list_sessions(&pool, None, 5, 0).await.unwrap_or_default();
    let current_session = sessions.first()
        .filter(|s| s.ended_at.is_none())
        .map(|s| s.session_key.clone());
    let current_model = sessions.first()
        .filter(|s| s.ended_at.is_none())
        .and_then(|s| s.model.clone());
    
    // Get active sub-agents (sessions with "subagent" in session_type that haven't ended)
    let active_subagents: Vec<Session> = sessions.into_iter()
        .filter(|s| s.session_type.contains("sub") && s.ended_at.is_none())
        .collect();
    
    HttpResponse::Ok().json(AgentStatus {
        is_active,
        current_session,
        current_model,
        last_activity,
        active_subagents,
    })
}

// ============================================================================
// Activity Summary API (OpenClaw-focused)
// ============================================================================

#[derive(Debug, serde::Serialize)]
pub struct ActivitySummary {
    pub shell_commands: i64,
    pub file_ops: i64,
    pub api_calls: i64,
    pub messages: i64,
    pub total_events: i64,
}

#[get("/api/activity/summary")]
pub async fn get_activity_summary(
    req: HttpRequest,
    pool: web::Data<SqlitePool>,
    config: web::Data<crate::Config>,
) -> impl Responder {
    require_auth!(&req, config);
    
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    
    match db::get_activity_summary(&pool, &today).await {
        Ok(summary) => HttpResponse::Ok().json(summary),
        Err(e) => {
            error!("Failed to get activity summary: {}", e);
            HttpResponse::InternalServerError().json(serde_json::json!({
                "error": e.to_string()
            }))
        }
    }
}

// ============================================================================
// Admin API (Data Management)
// ============================================================================

#[post("/api/admin/clear-events")]
pub async fn clear_events(
    req: HttpRequest,
    pool: web::Data<SqlitePool>,
    config: web::Data<crate::Config>,
) -> impl Responder {
    require_auth!(&req, config);
    
    match db::clear_all_events(&pool).await {
        Ok(count) => {
            info!(count = count, "Cleared all events");
            HttpResponse::Ok().json(serde_json::json!({
                "status": "ok",
                "deleted": count
            }))
        }
        Err(e) => {
            error!("Failed to clear events: {}", e);
            HttpResponse::InternalServerError().json(serde_json::json!({
                "error": e.to_string()
            }))
        }
    }
}

#[post("/api/admin/reset-database")]
pub async fn reset_database(
    req: HttpRequest,
    pool: web::Data<SqlitePool>,
    config: web::Data<crate::Config>,
) -> impl Responder {
    require_auth!(&req, config);
    
    match db::reset_all_data(&pool).await {
        Ok(_) => {
            info!("Database reset");
            HttpResponse::Ok().json(serde_json::json!({
                "status": "ok",
                "message": "All data cleared"
            }))
        }
        Err(e) => {
            error!("Failed to reset database: {}", e);
            HttpResponse::InternalServerError().json(serde_json::json!({
                "error": e.to_string()
            }))
        }
    }
}
