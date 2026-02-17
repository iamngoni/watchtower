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
            // Broadcast via SSE
            broadcaster.broadcast("event", serde_json::to_value(&event).unwrap());
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
