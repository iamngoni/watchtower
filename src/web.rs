use crate::db;
use crate::gateway_client;
use crate::models::*;
use actix_web::{get, web, HttpRequest, HttpResponse, Responder};
use askama::Template;
use chrono::Datelike;
use serde::Serialize;
use sqlx::SqlitePool;
use tracing::{error, warn};

// ============================================================================
// Gateway Data Converters
// ============================================================================

/// Convert gateway sessions JSON to our Session model
fn gateway_sessions_to_models(data: &serde_json::Value) -> Vec<Session> {
    let sessions = data.get("sessions").and_then(|v| v.as_array());
    let Some(sessions) = sessions else { return vec![] };
    
    sessions.iter().enumerate().map(|(i, s)| {
        let key = s.get("key").and_then(|v| v.as_str()).unwrap_or("unknown").to_string();
        let model = s.get("model").and_then(|v| v.as_str()).map(String::from);
        let provider = s.get("modelProvider").and_then(|v| v.as_str()).unwrap_or("");
        let display_model = model.as_ref().map(|m| {
            if provider.is_empty() { m.clone() } else { format!("{}/{}", provider, m) }
        });
        
        let kind = s.get("kind").and_then(|v| v.as_str()).unwrap_or("unknown");
        let session_type = if key.contains(":cron:") { "cron" }
            else if key.contains(":subagent:") { "subagent" }
            else if kind == "direct" { "main" }
            else { "unknown" };
        
        let updated_at = s.get("updatedAt").and_then(|v| v.as_i64()).unwrap_or(0);
        let input = s.get("inputTokens").and_then(|v| v.as_i64()).unwrap_or(0);
        let output = s.get("outputTokens").and_then(|v| v.as_i64()).unwrap_or(0);
        let total = s.get("totalTokens").and_then(|v| v.as_i64()).unwrap_or(0);
        let channel = s.get("channel").and_then(|v| v.as_str()).unwrap_or("");
        
        let title = s.get("displayName").and_then(|v| v.as_str())
            .or_else(|| s.get("origin").and_then(|o| o.get("label")).and_then(|v| v.as_str()))
            .map(String::from)
            .or_else(|| Some(format!("{} ({})", key, channel)));
        
        Session {
            id: (i + 1) as i64,
            session_key: key,
            title,
            session_type: session_type.to_string(),
            model: display_model,
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            cost_usd: 0.0,
            task_id: None,
            parent_session_id: None,
            started_at: updated_at / 1000,
            ended_at: None,
        }
    }).collect()
}

/// Convert gateway cron JSON to our CronJob model
fn gateway_cron_to_models(data: &serde_json::Value) -> Vec<CronJob> {
    let jobs = data.get("jobs").and_then(|v| v.as_array());
    let Some(jobs) = jobs else { return vec![] };
    
    jobs.iter().enumerate().map(|(i, j)| {
        let state = j.get("state");
        let schedule = j.get("schedule");
        let schedule_str = schedule.and_then(|s| {
            let kind = s.get("kind").and_then(|v| v.as_str()).unwrap_or("?");
            match kind {
                "cron" => s.get("expr").and_then(|v| v.as_str()).map(|e| {
                    let tz = s.get("tz").and_then(|v| v.as_str()).unwrap_or("");
                    if tz.is_empty() { e.to_string() } else { format!("{} ({})", e, tz) }
                }),
                "every" => s.get("everyMs").and_then(|v| v.as_u64()).map(|ms| {
                    if ms >= 3_600_000 { format!("every {}h", ms / 3_600_000) }
                    else if ms >= 60_000 { format!("every {}m", ms / 60_000) }
                    else { format!("every {}s", ms / 1000) }
                }),
                "at" => s.get("at").and_then(|v| v.as_str()).map(|a| format!("at {}", a)),
                _ => Some("?".to_string()),
            }
        }).unwrap_or_else(|| "?".to_string());
        
        CronJob {
            id: (i + 1) as i64,
            job_id: j.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            name: j.get("name").and_then(|v| v.as_str()).unwrap_or("Unnamed").to_string(),
            schedule: schedule_str,
            enabled: if j.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false) { 1 } else { 0 },
            last_status: state.and_then(|s| s.get("lastStatus")).and_then(|v| v.as_str()).map(String::from),
            last_run_at: state.and_then(|s| s.get("lastRunAtMs")).and_then(|v| v.as_i64()).map(|ms| ms / 1000),
            next_run_at: state.and_then(|s| s.get("nextRunAtMs")).and_then(|v| v.as_i64()).map(|ms| ms / 1000),
            consecutive_errors: state.and_then(|s| s.get("consecutiveErrors")).and_then(|v| v.as_i64()).unwrap_or(0),
            updated_at: j.get("updatedAtMs").and_then(|v| v.as_i64()).unwrap_or(0) / 1000,
        }
    }).collect()
}

/// Extract today's cost from gateway cost data
fn gateway_today_cost(data: &serde_json::Value) -> (f64, f64) {
    let daily = data.get("daily").and_then(|v| v.as_array());
    let Some(daily) = daily else { return (0.0, 0.0) };
    
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let yesterday = (chrono::Utc::now() - chrono::Duration::days(1)).format("%Y-%m-%d").to_string();
    
    let today_cost = daily.iter()
        .find(|d| d.get("date").and_then(|v| v.as_str()) == Some(&today))
        .and_then(|d| d.get("totalCost").and_then(|v| v.as_f64()))
        .unwrap_or(0.0);
    
    let yesterday_cost = daily.iter()
        .find(|d| d.get("date").and_then(|v| v.as_str()) == Some(&yesterday))
        .and_then(|d| d.get("totalCost").and_then(|v| v.as_f64()))
        .unwrap_or(0.0);
    
    (today_cost, yesterday_cost)
}

/// Convert gateway cost data to UsageStats for costs page
fn gateway_costs_to_stats(data: &serde_json::Value) -> UsageStats {
    let daily = data.get("daily").and_then(|v| v.as_array());
    let Some(daily) = daily else {
        return UsageStats { total_input_tokens: 0, total_output_tokens: 0, total_cost_usd: 0.0, by_model: vec![] };
    };
    
    let mut total_input: i64 = 0;
    let mut total_output: i64 = 0;
    let mut total_cost: f64 = 0.0;
    let mut total_cache_read: i64 = 0;
    let mut total_cache_write: i64 = 0;
    
    for day in daily {
        total_input += day.get("input").and_then(|v| v.as_i64()).unwrap_or(0);
        total_output += day.get("output").and_then(|v| v.as_i64()).unwrap_or(0);
        total_cost += day.get("totalCost").and_then(|v| v.as_f64()).unwrap_or(0.0);
        total_cache_read += day.get("cacheRead").and_then(|v| v.as_i64()).unwrap_or(0);
        total_cache_write += day.get("cacheWrite").and_then(|v| v.as_i64()).unwrap_or(0);
    }
    
    // Build a cost breakdown by category (input/output/cache) since we don't have per-model data
    let mut by_model = vec![];
    
    let mut input_cost: f64 = 0.0;
    let mut output_cost: f64 = 0.0;
    let mut cache_read_cost: f64 = 0.0;
    let mut cache_write_cost: f64 = 0.0;
    for day in daily {
        input_cost += day.get("inputCost").and_then(|v| v.as_f64()).unwrap_or(0.0);
        output_cost += day.get("outputCost").and_then(|v| v.as_f64()).unwrap_or(0.0);
        cache_read_cost += day.get("cacheReadCost").and_then(|v| v.as_f64()).unwrap_or(0.0);
        cache_write_cost += day.get("cacheWriteCost").and_then(|v| v.as_f64()).unwrap_or(0.0);
    }
    
    if cache_write_cost > 0.0 {
        by_model.push(ModelUsage { model: "Cache Write".to_string(), input_tokens: total_cache_write, output_tokens: 0, cost_usd: cache_write_cost });
    }
    if cache_read_cost > 0.0 {
        by_model.push(ModelUsage { model: "Cache Read".to_string(), input_tokens: total_cache_read, output_tokens: 0, cost_usd: cache_read_cost });
    }
    if output_cost > 0.0 {
        by_model.push(ModelUsage { model: "Output Tokens".to_string(), input_tokens: 0, output_tokens: total_output, cost_usd: output_cost });
    }
    if input_cost > 0.0 {
        by_model.push(ModelUsage { model: "Input Tokens".to_string(), input_tokens: total_input, output_tokens: 0, cost_usd: input_cost });
    }
    
    UsageStats {
        total_input_tokens: total_input,
        total_output_tokens: total_output,
        total_cost_usd: total_cost,
        by_model,
    }
}

// ============================================================================
// Weekly Cost Trends
// ============================================================================

#[derive(Debug, Clone, Serialize)]
pub struct WeeklyCostTrend {
    pub week_label: String,
    pub week_start: String,
    pub total_cost: f64,
    pub total_tokens: i64,
    pub days: usize,
    pub days_label: String,
    pub avg_daily_cost: f64,
    pub wow_change_pct: Option<f64>,
    pub wow_display: String,       // Pre-formatted "+12.3%" or "-5.1%" or "—"
    pub wow_color: String,         // "red", "green", or "neutral"
}

fn gateway_weekly_trends(data: &serde_json::Value) -> Vec<WeeklyCostTrend> {
    let daily = data.get("daily").and_then(|v| v.as_array());
    let Some(daily) = daily else { return vec![] };
    
    // Group by ISO week (Monday start)
    let mut weeks: std::collections::BTreeMap<String, (f64, i64, usize, String, String)> = std::collections::BTreeMap::new();
    
    for d in daily {
        let date_str = d.get("date").and_then(|v| v.as_str()).unwrap_or("");
        let cost = d.get("totalCost").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let tokens = d.get("totalTokens").and_then(|v| v.as_i64()).unwrap_or(0);
        
        if let Ok(date) = chrono::NaiveDate::parse_from_str(date_str, "%Y-%m-%d") {
            let week_start = date - chrono::Duration::days(date.weekday().num_days_from_monday() as i64);
            let week_end = week_start + chrono::Duration::days(6);
            let key = week_start.format("%Y-%m-%d").to_string();
            let label = format!("{} – {}", week_start.format("%b %d"), week_end.format("%b %d"));
            
            let entry = weeks.entry(key.clone()).or_insert((0.0, 0, 0, label, key));
            entry.0 += cost;
            entry.1 += tokens;
            entry.2 += 1;
        }
    }
    
    let mut result: Vec<WeeklyCostTrend> = Vec::new();
    let mut prev_cost: Option<f64> = None;
    
    for (_key, (cost, tokens, days, label, start)) in &weeks {
        let wow = prev_cost.map(|prev| {
            if prev > 0.0 { ((cost - prev) / prev) * 100.0 } else { 0.0 }
        });
        
        let (wow_display, wow_color) = match wow {
            Some(pct) if pct > 5.0 => (format!("+{:.1}%", pct), "red".to_string()),
            Some(pct) if pct < -5.0 => (format!("{:.1}%", pct), "green".to_string()),
            Some(pct) => (format!("{:+.1}%", pct), "neutral".to_string()),
            None => ("—".to_string(), "neutral".to_string()),
        };
        
        result.push(WeeklyCostTrend {
            week_label: label.clone(),
            week_start: start.clone(),
            total_cost: *cost,
            total_tokens: *tokens,
            days_label: if *days == 1 { "1 day".to_string() } else { format!("{} days", days) },
            days: *days,
            avg_daily_cost: if *days > 0 { cost / *days as f64 } else { 0.0 },
            wow_change_pct: wow,
            wow_display,
            wow_color,
        });
        
        // Only use full weeks (7 days) for WoW comparison
        if *days >= 5 {
            prev_cost = Some(*cost);
        }
    }
    
    result
}

// ============================================================================
// Agent Performance Metrics
// ============================================================================

#[derive(Debug, Clone, Serialize, Default)]
pub struct AgentPerformanceMetrics {
    pub total_runs: i64,
    pub avg_response_ms: f64,
    pub median_response_ms: f64,
    pub p95_response_ms: f64,
    pub min_response_ms: f64,
    pub max_response_ms: f64,
    pub total_events: i64,
    pub error_count: i64,
    pub warning_count: i64,
    pub error_rate_pct: f64,
    pub tool_calls: i64,
    pub tool_success_rate_pct: f64,
    pub shell_commands: i64,
    pub file_operations: i64,
    pub api_calls: i64,
    pub messages_sent: i64,
    // Per-day breakdown for sparkline
    pub daily_runs: Vec<(String, i64)>,
    pub daily_errors: Vec<(String, i64)>,
    pub daily_response_ms: Vec<(String, f64)>,
}

async fn compute_performance_metrics(pool: &SqlitePool) -> AgentPerformanceMetrics {
    let mut metrics = AgentPerformanceMetrics::default();
    
    // Get agent run durations
    let durations: Vec<f64> = sqlx::query_scalar::<_, f64>(
        r#"SELECT CAST(json_extract(metadata, '$.duration_ms') AS REAL)
           FROM events 
           WHERE event_type = 'shell_result' 
             AND json_extract(metadata, '$.duration_ms') IS NOT NULL
             AND json_extract(metadata, '$.duration_ms') > 0
           ORDER BY json_extract(metadata, '$.duration_ms')"#
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    
    // Also get agent run completions with duration from summary
    let agent_durations: Vec<f64> = sqlx::query_scalar::<_, String>(
        r#"SELECT detail FROM events 
           WHERE event_type = 'agent' 
             AND summary LIKE '%completed%'"#
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default()
    .iter()
    .filter_map(|detail| {
        // Extract durationMs= from detail
        detail.split("durationMs=").nth(1)
            .and_then(|s| s.split_whitespace().next())
            .and_then(|s| s.parse::<f64>().ok())
    })
    .filter(|d| *d > 0.0)
    .collect();
    
    if !agent_durations.is_empty() {
        let mut sorted = agent_durations.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        
        metrics.total_runs = sorted.len() as i64;
        metrics.avg_response_ms = sorted.iter().sum::<f64>() / sorted.len() as f64;
        metrics.min_response_ms = sorted.first().copied().unwrap_or(0.0);
        metrics.max_response_ms = sorted.last().copied().unwrap_or(0.0);
        metrics.median_response_ms = sorted[sorted.len() / 2];
        metrics.p95_response_ms = sorted[(sorted.len() as f64 * 0.95) as usize].min(metrics.max_response_ms);
    }
    
    // Event counts by type
    let type_counts: Vec<(String, i64)> = sqlx::query_as::<_, (String, i64)>(
        "SELECT event_type, count(*) FROM events GROUP BY event_type"
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    
    for (etype, count) in &type_counts {
        metrics.total_events += count;
        match etype.as_str() {
            "alert" => metrics.error_count += count,
            "warning" => metrics.warning_count += count,
            "shell" | "shell_result" => metrics.shell_commands += count,
            "file" | "file_result" => metrics.file_operations += count,
            "api" | "api_result" => metrics.api_calls += count,
            "message" | "message_result" => metrics.messages_sent += count,
            _ => {}
        }
    }
    
    if metrics.total_events > 0 {
        metrics.error_rate_pct = (metrics.error_count as f64 / metrics.total_events as f64) * 100.0;
    }
    
    // Tool success rate
    let tool_results: Vec<(String, i64)> = sqlx::query_as::<_, (String, i64)>(
        r#"SELECT json_extract(metadata, '$.status') as status, count(*) 
           FROM events 
           WHERE event_type LIKE '%_result' 
             AND json_extract(metadata, '$.status') IS NOT NULL
           GROUP BY status"#
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    
    let total_tool_results: i64 = tool_results.iter().map(|(_, c)| c).sum();
    let successful_tool_results: i64 = tool_results.iter()
        .filter(|(s, _)| s == "completed")
        .map(|(_, c)| *c)
        .sum();
    metrics.tool_calls = total_tool_results;
    if total_tool_results > 0 {
        metrics.tool_success_rate_pct = (successful_tool_results as f64 / total_tool_results as f64) * 100.0;
    }
    
    // Daily breakdown (last 7 days)
    let daily_stats: Vec<(String, i64, i64, f64)> = sqlx::query_as::<_, (String, i64, i64, f64)>(
        r#"SELECT 
             date(created_at, 'unixepoch') as day,
             count(*) as events,
             sum(CASE WHEN event_type IN ('alert', 'warning') THEN 1 ELSE 0 END) as errors,
             COALESCE(avg(CASE WHEN event_type = 'shell_result' AND json_extract(metadata, '$.duration_ms') > 0 
                THEN json_extract(metadata, '$.duration_ms') END), 0) as avg_ms
           FROM events 
           GROUP BY day 
           ORDER BY day DESC 
           LIMIT 7"#
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    
    for (day, events, errors, avg_ms) in daily_stats.into_iter().rev() {
        metrics.daily_runs.push((day.clone(), events));
        metrics.daily_errors.push((day.clone(), errors));
        metrics.daily_response_ms.push((day, avg_ms));
    }
    
    metrics
}

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
    cost_trend_up: bool,
    cost_trend_down: bool,
    // Current task (if in progress)
    current_task_title: String,
    // Agent Status (OpenClaw-focused)
    is_agent_active: bool,
    current_session: Option<String>,
    current_model: Option<String>,
    last_activity: Option<i64>,
    // Activity Summary
    shell_commands: i64,
    file_ops: i64,
    api_calls: i64,
    messages: i64,
    total_events_today: i64,
    // Active Sub-agents
    active_subagents: Vec<Session>,
    // Recent Completions
    recent_completions: Vec<Task>,
    // Lists
    recent_events: Vec<Event>,
    blocked_tasks: Vec<Task>,
    failed_cron_jobs: Vec<CronJob>,
    // Performance metrics
    perf: AgentPerformanceMetrics,
}

#[derive(Template)]
#[template(path = "settings.html")]
struct SettingsTemplate {
    title: String,
    active_page: String,
    version: String,
    start_time: i64,
    api_token_masked: String,
    api_endpoint: String,
    event_count: i64,
    session_count: i64,
    task_count: i64,
    db_size: String,
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
    weekly_trends: Vec<WeeklyCostTrend>,
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

#[derive(Template)]
#[template(path = "session_detail.html")]
struct SessionDetailTemplate {
    title: String,
    active_page: String,
    session: Session,
    events: Vec<Event>,
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
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    
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
    
    // Get today's cost - try gateway first, fall back to DB
    let (today_cost_val, yesterday_cost_val) = if let Some(gw) = gateway_client::get_gateway_client() {
        match gw.get_costs().await {
            Ok(data) => gateway_today_cost(&data),
            Err(e) => {
                warn!("Gateway costs unavailable: {}, falling back to DB", e);
                (0.0, 0.0)
            }
        }
    } else {
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        let yesterday = (chrono::Utc::now() - chrono::Duration::days(1)).format("%Y-%m-%d").to_string();
        let s = db::get_usage_stats(&pool, Some(&today), Some(&today)).await.unwrap_or_else(|_| UsageStats { total_input_tokens: 0, total_output_tokens: 0, total_cost_usd: 0.0, by_model: vec![] });
        let y = db::get_usage_stats(&pool, Some(&yesterday), Some(&yesterday)).await.unwrap_or_else(|_| UsageStats { total_input_tokens: 0, total_output_tokens: 0, total_cost_usd: 0.0, by_model: vec![] });
        (s.total_cost_usd, y.total_cost_usd)
    };
    
    let cost_trend_up = today_cost_val > yesterday_cost_val && yesterday_cost_val > 0.0;
    let cost_trend_down = today_cost_val < yesterday_cost_val && yesterday_cost_val > 0.0;
    
    // Get current in-progress task title
    let current_task_title = all_tasks.iter()
        .find(|t| t.status == "in_progress")
        .map(|t| t.title.clone())
        .unwrap_or_default();
    
    // Get recent events
    let recent_events = db::list_events(&pool, None, 5, 0)
        .await
        .unwrap_or_default();
    
    // Get cron jobs with stats - try gateway first
    let all_jobs = if let Some(gw) = gateway_client::get_gateway_client() {
        match gw.list_cron_jobs().await {
            Ok(data) => gateway_cron_to_models(&data),
            Err(e) => {
                warn!("Gateway cron unavailable: {}, falling back to DB", e);
                db::list_cron_jobs(&pool).await.unwrap_or_default()
            }
        }
    } else {
        db::list_cron_jobs(&pool).await.unwrap_or_default()
    };
    let total_cron_jobs = all_jobs.len();
    let enabled_cron_jobs = all_jobs.iter().filter(|j| j.enabled != 0).count();
    let disabled_cron_jobs = total_cron_jobs - enabled_cron_jobs;
    let error_cron_jobs = all_jobs.iter().filter(|j| j.consecutive_errors > 0 || j.last_status.as_deref() == Some("error")).count();
    let failed_cron_jobs: Vec<CronJob> = all_jobs.into_iter()
        .filter(|j| j.consecutive_errors > 0 || j.last_status.as_deref() == Some("error"))
        .collect();
    
    // Agent Status (OpenClaw-focused)
    let five_min_ago = chrono::Utc::now().timestamp() - 300;
    let is_agent_active = recent_events.first()
        .map(|e| e.created_at > five_min_ago)
        .unwrap_or(false);
    let last_activity = recent_events.first().map(|e| e.created_at);
    
    // Get sessions from gateway or DB
    let sessions = if let Some(gw) = gateway_client::get_gateway_client() {
        match gw.list_sessions().await {
            Ok(data) => gateway_sessions_to_models(&data),
            Err(e) => {
                warn!("Gateway sessions unavailable: {}, falling back to DB", e);
                db::list_sessions(&pool, None, 10, 0).await.unwrap_or_default()
            }
        }
    } else {
        db::list_sessions(&pool, None, 10, 0).await.unwrap_or_default()
    };
    
    // Main session is the first one (most recently updated)
    let current_session = sessions.first().map(|s| s.session_key.clone());
    let current_model = sessions.first().and_then(|s| s.model.clone());
    
    // Get active sub-agents
    let active_subagents: Vec<Session> = sessions.iter()
        .filter(|s| s.session_type.contains("sub"))
        .cloned()
        .collect();
    
    // Activity Summary
    let activity = db::get_activity_summary(&pool, &today).await.unwrap_or_else(|_| db::ActivitySummary {
        shell_commands: 0,
        file_ops: 0,
        api_calls: 0,
        messages: 0,
        total_events: 0,
    });
    
    // Recent Completions
    let recent_completions = db::get_recent_completions(&pool, 3).await.unwrap_or_default();
    
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
        today_cost: today_cost_val,
        cost_trend_up,
        cost_trend_down,
        current_task_title,
        is_agent_active,
        current_session,
        current_model,
        last_activity,
        shell_commands: activity.shell_commands,
        file_ops: activity.file_ops,
        api_calls: activity.api_calls,
        messages: activity.messages,
        total_events_today: activity.total_events,
        active_subagents,
        recent_completions,
        recent_events,
        blocked_tasks,
        failed_cron_jobs,
        perf: compute_performance_metrics(&pool).await,
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
    
    let (stats, weekly_trends) = if let Some(gw) = gateway_client::get_gateway_client() {
        match gw.get_costs().await {
            Ok(data) => (gateway_costs_to_stats(&data), gateway_weekly_trends(&data)),
            Err(e) => {
                warn!("Gateway costs unavailable: {}", e);
                (db::get_usage_stats(&pool, None, None).await.unwrap_or_else(|_| UsageStats { total_input_tokens: 0, total_output_tokens: 0, total_cost_usd: 0.0, by_model: vec![] }), vec![])
            }
        }
    } else {
        (db::get_usage_stats(&pool, None, None).await.unwrap_or_else(|_| UsageStats { total_input_tokens: 0, total_output_tokens: 0, total_cost_usd: 0.0, by_model: vec![] }), vec![])
    };
    
    let template = CostsTemplate {
        title: "Usage & Costs".to_string(),
        active_page: "costs".to_string(),
        stats,
        weekly_trends,
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
    
    let jobs = if let Some(gw) = gateway_client::get_gateway_client() {
        match gw.list_cron_jobs().await {
            Ok(data) => gateway_cron_to_models(&data),
            Err(e) => {
                warn!("Gateway cron unavailable: {}", e);
                db::list_cron_jobs(&pool).await.unwrap_or_default()
            }
        }
    } else {
        db::list_cron_jobs(&pool).await.unwrap_or_default()
    };
    
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
    
    let sessions = if let Some(gw) = gateway_client::get_gateway_client() {
        match gw.list_sessions().await {
            Ok(data) => gateway_sessions_to_models(&data),
            Err(e) => {
                warn!("Gateway sessions unavailable: {}", e);
                db::list_sessions(&pool, None, 100, 0).await.unwrap_or_default()
            }
        }
    } else {
        db::list_sessions(&pool, None, 100, 0).await.unwrap_or_default()
    };
    
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

#[get("/sessions/{id}")]
pub async fn session_detail_page(
    req: HttpRequest,
    pool: web::Data<SqlitePool>,
    config: web::Data<crate::Config>,
    path: web::Path<i64>,
) -> impl Responder {
    if let Some(resp) = require_web_auth(&req, &config) {
        return resp;
    }
    
    let id = path.into_inner();
    
    let session = match db::get_session(&pool, id).await {
        Ok(Some(s)) => s,
        Ok(None) => return HttpResponse::NotFound().body("Session not found"),
        Err(e) => {
            error!("Failed to get session: {}", e);
            return HttpResponse::InternalServerError().body("Database error");
        }
    };
    
    // Get events for this session
    let events = db::get_events_for_session(&pool, &session.session_key)
        .await
        .unwrap_or_default();
    
    let template = SessionDetailTemplate {
        title: format!("Session: {}", session.session_key),
        active_page: "sessions".to_string(),
        session,
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

// ============================================================================
// Settings Page
// ============================================================================

// Static start time for uptime calculation
static START_TIME: std::sync::OnceLock<i64> = std::sync::OnceLock::new();

fn get_start_time() -> i64 {
    *START_TIME.get_or_init(|| chrono::Utc::now().timestamp())
}

#[get("/settings")]
pub async fn settings_page(
    req: HttpRequest,
    pool: web::Data<SqlitePool>,
    config: web::Data<crate::Config>,
) -> impl Responder {
    if let Some(resp) = require_web_auth(&req, &config) {
        return resp;
    }
    
    // Get counts
    let tasks = db::list_tasks(&pool, None, None, None).await.unwrap_or_default();
    let event_count = db::count_events(&pool).await.unwrap_or(0);
    let session_count = db::count_sessions(&pool).await.unwrap_or(0);
    let task_count = tasks.len() as i64;
    
    // Mask API token
    let api_token_masked = if config.api_token.len() > 8 {
        format!("{}...{}", &config.api_token[..4], &config.api_token[config.api_token.len()-4..])
    } else if !config.api_token.is_empty() {
        "****".to_string()
    } else {
        String::new()
    };
    
    // Get database size
    let db_path_str = config.database_url.replace("sqlite:", "").replace("?mode=rwc", "");
    let db_path = std::path::Path::new(&db_path_str);
    let db_size = if db_path.exists() {
        let size = std::fs::metadata(db_path).map(|m| m.len()).unwrap_or(0);
        if size > 1_000_000 {
            format!("{:.1} MB", size as f64 / 1_000_000.0)
        } else {
            format!("{:.0} KB", size as f64 / 1_000.0)
        }
    } else {
        "Unknown".to_string()
    };
    
    let template = SettingsTemplate {
        title: "Settings".to_string(),
        active_page: "settings".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        start_time: get_start_time(),
        api_token_masked,
        api_endpoint: format!("http://{}:{}/api", config.host, config.port),
        event_count,
        session_count,
        task_count,
        db_size,
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

/// Render an event as an HTML fragment (for SSE broadcasting)
pub fn render_event_html(event: &Event) -> Option<String> {
    let template = EventItemTemplate { event: event.clone() };
    template.render().ok()
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
    events: Vec<Event>,
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
    
    // Get events linked to this task
    let events = db::get_events_for_task(&pool, task_id)
        .await
        .unwrap_or_default();
    
    let template = TaskDetailTemplate {
        task,
        labels,
        comments,
        history,
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

// ============================================================================
// Costs Chart Partial
// ============================================================================

#[derive(Template)]
#[template(path = "partials/costs_chart.html")]
struct CostsChartTemplate {
    costs: Vec<db::DailyCostRow>,
    max_cost: f64,
}

#[get("/partials/costs-chart")]
pub async fn costs_chart_partial(
    req: HttpRequest,
    pool: web::Data<SqlitePool>,
    config: web::Data<crate::Config>,
) -> impl Responder {
    if let Some(resp) = require_web_auth(&req, &config) {
        return resp;
    }
    
    let costs = if let Some(gw) = gateway_client::get_gateway_client() {
        match gw.get_costs().await {
            Ok(data) => {
                let daily = data.get("daily").and_then(|v| v.as_array());
                daily.map(|arr| {
                    arr.iter().rev().take(14).map(|d| {
                        db::DailyCostRow {
                            date: d.get("date").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                            cost_usd: d.get("totalCost").and_then(|v| v.as_f64()).unwrap_or(0.0),
                            input_tokens: d.get("input").and_then(|v| v.as_i64()).unwrap_or(0),
                            output_tokens: d.get("output").and_then(|v| v.as_i64()).unwrap_or(0),
                        }
                    }).collect::<Vec<_>>()
                }).unwrap_or_default()
            }
            Err(_) => db::get_daily_costs(&pool, 14).await.unwrap_or_default()
        }
    } else {
        db::get_daily_costs(&pool, 14).await.unwrap_or_default()
    };
    let max_cost = costs.iter().map(|c| c.cost_usd).fold(0.0_f64, |a, b| a.max(b));
    
    let template = CostsChartTemplate { 
        costs,
        max_cost: if max_cost == 0.0 { 1.0 } else { max_cost },
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
// Cron History Partial
// ============================================================================

#[derive(Template)]
#[template(path = "partials/cron_history.html")]
struct CronHistoryTemplate {
    job_id: String,
    events: Vec<Event>,
}

#[get("/partials/cron/{job_id}/history")]
pub async fn cron_history_partial(
    req: HttpRequest,
    pool: web::Data<SqlitePool>,
    config: web::Data<crate::Config>,
    path: web::Path<String>,
) -> impl Responder {
    if let Some(resp) = require_web_auth(&req, &config) {
        return resp;
    }
    
    let job_id = path.into_inner();
    let events = db::get_cron_run_history(&pool, &job_id, 5).await.unwrap_or_default();
    
    let template = CronHistoryTemplate { job_id, events };
    
    match template.render() {
        Ok(html) => HttpResponse::Ok().content_type("text/html").body(html),
        Err(e) => {
            error!("Template error: {}", e);
            HttpResponse::InternalServerError().body("Template error")
        }
    }
}

// ============================================================================
// Board Swimlane Partial (Assignee View)
// ============================================================================

#[derive(Template)]
#[template(path = "partials/board_swimlane.html")]
struct BoardSwimlaneTemplate {
    agent_backlog: Vec<Task>,
    agent_todo: Vec<Task>,
    agent_in_progress: Vec<Task>,
    agent_blocked: Vec<Task>,
    agent_in_review: Vec<Task>,
    agent_done: Vec<Task>,
    human_backlog: Vec<Task>,
    human_todo: Vec<Task>,
    human_in_progress: Vec<Task>,
    human_blocked: Vec<Task>,
    human_in_review: Vec<Task>,
    human_done: Vec<Task>,
}

#[get("/partials/board/swimlane")]
pub async fn board_swimlane_partial(
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
    
    // Split by assignee
    let agent_tasks: Vec<Task> = all_tasks.iter()
        .filter(|t| t.assigned_to == "agent")
        .cloned()
        .collect();
    let human_tasks: Vec<Task> = all_tasks.iter()
        .filter(|t| t.assigned_to != "agent")
        .cloned()
        .collect();
    
    let template = BoardSwimlaneTemplate {
        agent_backlog: agent_tasks.iter().filter(|t| t.status == "backlog").cloned().collect(),
        agent_todo: agent_tasks.iter().filter(|t| t.status == "todo").cloned().collect(),
        agent_in_progress: agent_tasks.iter().filter(|t| t.status == "in_progress").cloned().collect(),
        agent_blocked: agent_tasks.iter().filter(|t| t.status == "blocked").cloned().collect(),
        agent_in_review: agent_tasks.iter().filter(|t| t.status == "in_review").cloned().collect(),
        agent_done: agent_tasks.iter().filter(|t| t.status == "done").cloned().collect(),
        human_backlog: human_tasks.iter().filter(|t| t.status == "backlog").cloned().collect(),
        human_todo: human_tasks.iter().filter(|t| t.status == "todo").cloned().collect(),
        human_in_progress: human_tasks.iter().filter(|t| t.status == "in_progress").cloned().collect(),
        human_blocked: human_tasks.iter().filter(|t| t.status == "blocked").cloned().collect(),
        human_in_review: human_tasks.iter().filter(|t| t.status == "in_review").cloned().collect(),
        human_done: human_tasks.iter().filter(|t| t.status == "done").cloned().collect(),
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
