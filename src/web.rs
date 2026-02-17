use crate::db;
use crate::models::*;
use actix_web::{get, web, HttpRequest, HttpResponse, Responder};
use askama::Template;
use sqlx::SqlitePool;
use tracing::error;

/// Check session cookie authentication for web UI
fn check_web_auth(req: &HttpRequest, config: &crate::Config) -> bool {
    if config.web_user.is_empty() || config.web_pass.is_empty() {
        return true; // No auth required
    }
    
    // Check for basic auth header
    if let Some(auth) = req.headers().get("Authorization") {
        if let Ok(auth_str) = auth.to_str() {
            if auth_str.starts_with("Basic ") {
                if let Ok(decoded) = base64_decode(&auth_str[6..]) {
                    let expected = format!("{}:{}", config.web_user, config.web_pass);
                    return decoded == expected;
                }
            }
        }
    }
    
    false
}

fn base64_decode(input: &str) -> Result<String, ()> {
    let bytes = base64_decode_bytes(input)?;
    String::from_utf8(bytes).map_err(|_| ())
}

fn base64_decode_bytes(input: &str) -> Result<Vec<u8>, ()> {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    
    let input = input.trim_end_matches('=');
    let mut output = Vec::with_capacity(input.len() * 3 / 4);
    let mut buffer = 0u32;
    let mut bits = 0;
    
    for c in input.bytes() {
        let val = CHARS.iter().position(|&x| x == c).ok_or(())? as u32;
        buffer = (buffer << 6) | val;
        bits += 6;
        
        if bits >= 8 {
            bits -= 8;
            output.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    
    Ok(output)
}

fn require_web_auth(req: &HttpRequest, config: &crate::Config) -> Option<HttpResponse> {
    if config.web_user.is_empty() || config.web_pass.is_empty() {
        return None;
    }
    
    if !check_web_auth(req, config) {
        return Some(HttpResponse::Unauthorized()
            .insert_header(("WWW-Authenticate", "Basic realm=\"Watchtower\""))
            .body("Unauthorized"));
    }
    
    None
}

// ============================================================================
// Templates
// ============================================================================

#[derive(Template)]
#[template(path = "dashboard.html")]
struct DashboardTemplate {
    title: String,
    active_page: String,
    // Task stats
    total_tasks: usize,
    active_tasks: usize,
    pending_tasks: usize,
    backlog_tasks: usize,
    blocked_tasks_count: usize,
    done_tasks: usize,
    // Cron stats
    total_cron_jobs: usize,
    enabled_cron_jobs: usize,
    disabled_cron_jobs: usize,
    error_cron_jobs: usize,
    // Cost
    today_cost: f64,
    // Lists
    recent_events: Vec<Event>,
    blocked_tasks: Vec<Task>,
    failed_cron_jobs: Vec<CronJob>,
}

#[derive(Template)]
#[template(path = "feed.html")]
struct FeedTemplate {
    title: String,
    active_page: String,
    events: Vec<Event>,
}

#[derive(Template)]
#[template(path = "board.html")]
struct BoardTemplate {
    title: String,
    active_page: String,
    tasks_backlog: Vec<Task>,
    tasks_todo: Vec<Task>,
    tasks_in_progress: Vec<Task>,
    tasks_blocked: Vec<Task>,
    tasks_in_review: Vec<Task>,
    tasks_done: Vec<Task>,
}

#[derive(Template)]
#[template(path = "costs.html")]
struct CostsTemplate {
    title: String,
    active_page: String,
    stats: UsageStats,
}

#[derive(Template)]
#[template(path = "cron.html")]
struct CronTemplate {
    title: String,
    active_page: String,
    jobs: Vec<CronJob>,
}

#[derive(Template)]
#[template(path = "sessions.html")]
struct SessionsTemplate {
    title: String,
    active_page: String,
    sessions: Vec<Session>,
}

// ============================================================================
// Page Handlers
// ============================================================================

#[get("/")]
pub async fn index(
    req: HttpRequest,
    pool: web::Data<SqlitePool>,
    config: web::Data<crate::Config>,
) -> impl Responder {
    if let Some(resp) = require_web_auth(&req, &config) {
        return resp;
    }
    
    // Get all tasks for stats
    let all_tasks = db::list_tasks(&pool, None, None, None)
        .await
        .unwrap_or_default();
    
    let total_tasks = all_tasks.len();
    let active_tasks = all_tasks.iter().filter(|t| t.status == "in_progress").count();
    let pending_tasks = all_tasks.iter().filter(|t| t.status == "todo").count();
    let backlog_tasks = all_tasks.iter().filter(|t| t.status == "backlog").count();
    let blocked_tasks_count = all_tasks.iter().filter(|t| t.status == "blocked").count();
    let done_tasks = all_tasks.iter().filter(|t| t.status == "done").count();
    let blocked_tasks: Vec<Task> = all_tasks.iter().filter(|t| t.status == "blocked").cloned().collect();
    
    // Get today's cost
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let stats = db::get_usage_stats(&pool, Some(&today), Some(&today))
        .await
        .unwrap_or_else(|_| UsageStats {
            total_input_tokens: 0,
            total_output_tokens: 0,
            total_cost_usd: 0.0,
            by_model: vec![],
        });
    
    // Get recent events
    let recent_events = db::list_events(&pool, None, 5, 0)
        .await
        .unwrap_or_default();
    
    // Get cron jobs with stats
    let all_jobs = db::list_cron_jobs(&pool).await.unwrap_or_default();
    let total_cron_jobs = all_jobs.len();
    let enabled_cron_jobs = all_jobs.iter().filter(|j| j.enabled != 0).count();
    let disabled_cron_jobs = total_cron_jobs - enabled_cron_jobs;
    let error_cron_jobs = all_jobs.iter().filter(|j| j.consecutive_errors > 0 || j.last_status.as_deref() == Some("error")).count();
    let failed_cron_jobs: Vec<CronJob> = all_jobs.into_iter()
        .filter(|j| j.consecutive_errors > 0 || j.last_status.as_deref() == Some("error"))
        .collect();
    
    let template = DashboardTemplate {
        title: "Dashboard".to_string(),
        active_page: "dashboard".to_string(),
        total_tasks,
        active_tasks,
        pending_tasks,
        backlog_tasks,
        blocked_tasks_count,
        done_tasks,
        total_cron_jobs,
        enabled_cron_jobs,
        disabled_cron_jobs,
        error_cron_jobs,
        today_cost: stats.total_cost_usd,
        recent_events,
        blocked_tasks,
        failed_cron_jobs,
    };
    
    match template.render() {
        Ok(html) => HttpResponse::Ok().content_type("text/html").body(html),
        Err(e) => {
            error!("Template error: {}", e);
            HttpResponse::InternalServerError().body("Template error")
        }
    }
}

#[get("/feed")]
pub async fn feed_page(
    req: HttpRequest,
    pool: web::Data<SqlitePool>,
    config: web::Data<crate::Config>,
) -> impl Responder {
    if let Some(resp) = require_web_auth(&req, &config) {
        return resp;
    }
    
    let events = db::list_events(&pool, None, 100, 0)
        .await
        .unwrap_or_default();
    
    let template = FeedTemplate {
        title: "Live Activity Feed".to_string(),
        active_page: "feed".to_string(),
        events,
    };
    
    match template.render() {
        Ok(html) => HttpResponse::Ok().content_type("text/html").body(html),
        Err(e) => {
            error!("Template error: {}", e);
            HttpResponse::InternalServerError().body("Template error")
        }
    }
}

#[get("/board")]
pub async fn board_page(
    req: HttpRequest,
    pool: web::Data<SqlitePool>,
    config: web::Data<crate::Config>,
) -> impl Responder {
    if let Some(resp) = require_web_auth(&req, &config) {
        return resp;
    }
    
    let all_tasks = db::list_tasks(&pool, None, None, None)
        .await
        .unwrap_or_default();
    
    let tasks_backlog: Vec<Task> = all_tasks.iter().filter(|t| t.status == "backlog").cloned().collect();
    let tasks_todo: Vec<Task> = all_tasks.iter().filter(|t| t.status == "todo").cloned().collect();
    let tasks_in_progress: Vec<Task> = all_tasks.iter().filter(|t| t.status == "in_progress").cloned().collect();
    let tasks_blocked: Vec<Task> = all_tasks.iter().filter(|t| t.status == "blocked").cloned().collect();
    let tasks_in_review: Vec<Task> = all_tasks.iter().filter(|t| t.status == "in_review").cloned().collect();
    let tasks_done: Vec<Task> = all_tasks.iter().filter(|t| t.status == "done").cloned().collect();
    
    let template = BoardTemplate {
        title: "Kanban Board".to_string(),
        active_page: "board".to_string(),
        tasks_backlog,
        tasks_todo,
        tasks_in_progress,
        tasks_blocked,
        tasks_in_review,
        tasks_done,
    };
    
    match template.render() {
        Ok(html) => HttpResponse::Ok().content_type("text/html").body(html),
        Err(e) => {
            error!("Template error: {}", e);
            HttpResponse::InternalServerError().body("Template error")
        }
    }
}

#[get("/costs")]
pub async fn costs_page(
    req: HttpRequest,
    pool: web::Data<SqlitePool>,
    config: web::Data<crate::Config>,
) -> impl Responder {
    if let Some(resp) = require_web_auth(&req, &config) {
        return resp;
    }
    
    let stats = db::get_usage_stats(&pool, None, None)
        .await
        .unwrap_or_else(|_| UsageStats {
            total_input_tokens: 0,
            total_output_tokens: 0,
            total_cost_usd: 0.0,
            by_model: vec![],
        });
    
    let template = CostsTemplate {
        title: "Usage & Costs".to_string(),
        active_page: "costs".to_string(),
        stats,
    };
    
    match template.render() {
        Ok(html) => HttpResponse::Ok().content_type("text/html").body(html),
        Err(e) => {
            error!("Template error: {}", e);
            HttpResponse::InternalServerError().body("Template error")
        }
    }
}

#[get("/cron")]
pub async fn cron_page(
    req: HttpRequest,
    pool: web::Data<SqlitePool>,
    config: web::Data<crate::Config>,
) -> impl Responder {
    if let Some(resp) = require_web_auth(&req, &config) {
        return resp;
    }
    
    let jobs = db::list_cron_jobs(&pool)
        .await
        .unwrap_or_default();
    
    let template = CronTemplate {
        title: "Cron Jobs".to_string(),
        active_page: "cron".to_string(),
        jobs,
    };
    
    match template.render() {
        Ok(html) => HttpResponse::Ok().content_type("text/html").body(html),
        Err(e) => {
            error!("Template error: {}", e);
            HttpResponse::InternalServerError().body("Template error")
        }
    }
}

#[get("/sessions")]
pub async fn sessions_page(
    req: HttpRequest,
    pool: web::Data<SqlitePool>,
    config: web::Data<crate::Config>,
) -> impl Responder {
    if let Some(resp) = require_web_auth(&req, &config) {
        return resp;
    }
    
    let sessions = db::list_sessions(&pool, None, 100, 0)
        .await
        .unwrap_or_default();
    
    let template = SessionsTemplate {
        title: "Sessions".to_string(),
        active_page: "sessions".to_string(),
        sessions,
    };
    
    match template.render() {
        Ok(html) => HttpResponse::Ok().content_type("text/html").body(html),
        Err(e) => {
            error!("Template error: {}", e);
            HttpResponse::InternalServerError().body("Template error")
        }
    }
}

// ============================================================================
// HTMX Partials
// ============================================================================

#[derive(Template)]
#[template(path = "partials/event_item.html")]
struct EventItemTemplate {
    event: Event,
}

#[derive(Template)]
#[template(path = "partials/task_card.html")]
struct TaskCardTemplate {
    task: Task,
}

#[derive(Template)]
#[template(path = "partials/task_detail.html")]
struct TaskDetailTemplate {
    task: Task,
    labels: Vec<String>,
    comments: Vec<TaskComment>,
    history: Vec<TaskHistory>,
}

#[get("/partials/events")]
pub async fn events_partial(
    req: HttpRequest,
    pool: web::Data<SqlitePool>,
    config: web::Data<crate::Config>,
) -> impl Responder {
    if let Some(resp) = require_web_auth(&req, &config) {
        return resp;
    }
    
    let events = db::list_events(&pool, None, 50, 0)
        .await
        .unwrap_or_default();
    
    let mut html = String::new();
    for event in events {
        let template = EventItemTemplate { event };
        if let Ok(rendered) = template.render() {
            html.push_str(&rendered);
        }
    }
    
    HttpResponse::Ok().content_type("text/html").body(html)
}

#[get("/partials/tasks/{status}")]
pub async fn tasks_partial(
    req: HttpRequest,
    pool: web::Data<SqlitePool>,
    config: web::Data<crate::Config>,
    path: web::Path<String>,
) -> impl Responder {
    if let Some(resp) = require_web_auth(&req, &config) {
        return resp;
    }
    
    let status = path.into_inner();
    let tasks = db::list_tasks(&pool, Some(&status), None, None)
        .await
        .unwrap_or_default();
    
    let mut html = String::new();
    for task in tasks {
        let template = TaskCardTemplate { task };
        if let Ok(rendered) = template.render() {
            html.push_str(&rendered);
        }
    }
    
    HttpResponse::Ok().content_type("text/html").body(html)
}

#[derive(Debug, serde::Deserialize)]
pub struct SessionsPartialQuery {
    #[serde(rename = "type")]
    session_type: Option<String>,
}

#[derive(Template)]
#[template(path = "partials/session_item.html")]
struct SessionItemTemplate {
    session: Session,
}

#[get("/partials/sessions")]
pub async fn sessions_partial(
    req: HttpRequest,
    pool: web::Data<SqlitePool>,
    config: web::Data<crate::Config>,
    query: web::Query<SessionsPartialQuery>,
) -> impl Responder {
    if let Some(resp) = require_web_auth(&req, &config) {
        return resp;
    }
    
    let session_type = query.session_type.as_deref().filter(|s| !s.is_empty());
    
    let sessions = db::list_sessions(&pool, session_type, 100, 0)
        .await
        .unwrap_or_default();
    
    if sessions.is_empty() {
        return HttpResponse::Ok().content_type("text/html").body(r#"
            <div class="flex flex-col items-center justify-center py-16 text-center px-4">
                <i data-lucide="history" class="w-12 h-12 text-wt-text-tertiary mb-3"></i>
                <p class="text-sm text-wt-text-secondary">No sessions found</p>
            </div>
            <script>lucide.createIcons();</script>
        "#);
    }
    
    let mut html = String::new();
    for session in sessions {
        let template = SessionItemTemplate { session };
        if let Ok(rendered) = template.render() {
            html.push_str(&rendered);
        }
    }
    html.push_str("<script>formatAllTimestamps(); formatAllNumbers(); lucide.createIcons();</script>");
    
    HttpResponse::Ok().content_type("text/html").body(html)
}

#[get("/partials/tasks/{id}/detail")]
pub async fn task_detail_partial(
    req: HttpRequest,
    pool: web::Data<SqlitePool>,
    config: web::Data<crate::Config>,
    path: web::Path<i64>,
) -> impl Responder {
    if let Some(resp) = require_web_auth(&req, &config) {
        return resp;
    }
    
    let task_id = path.into_inner();
    
    // Get the task
    let task = match db::get_task(&pool, task_id).await {
        Ok(Some(t)) => t,
        Ok(None) => return HttpResponse::NotFound().body("Task not found"),
        Err(e) => {
            error!("Failed to get task: {}", e);
            return HttpResponse::InternalServerError().body("Database error");
        }
    };
    
    // Parse labels from JSON string
    let labels: Vec<String> = serde_json::from_str(&task.labels).unwrap_or_default();
    
    // Get comments
    let comments = db::get_task_comments(&pool, task_id)
        .await
        .unwrap_or_default();
    
    // Get history
    let history = db::get_task_history(&pool, task_id)
        .await
        .unwrap_or_default();
    
    let template = TaskDetailTemplate {
        task,
        labels,
        comments,
        history,
    };
    
    match template.render() {
        Ok(html) => HttpResponse::Ok().content_type("text/html").body(html),
        Err(e) => {
            error!("Template error: {}", e);
            HttpResponse::InternalServerError().body("Template error")
        }
    }
}

// ============================================================================
// Favicon Redirect
// ============================================================================

#[get("/favicon.ico")]
pub async fn favicon() -> impl Responder {
    HttpResponse::PermanentRedirect()
        .insert_header(("Location", "/static/favicon.svg"))
        .finish()
}
