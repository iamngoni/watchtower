//! OpenClaw log watcher - tails gateway.log and pushes events to SSE
//!
//! Parses NDJSON log lines from OpenClaw and extracts meaningful events.
//! Improved v0.2.0: Smart categorization based on subsystem and message content.

use crate::models::{CreateEvent, CreateSession};
use crate::sse::Broadcaster;
use crate::db;
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::fs::File;
use tokio::io::{AsyncBufReadExt, AsyncSeekExt, BufReader, SeekFrom};
use tokio::sync::watch;
use tracing::{debug, error, info, warn};

fn log_path() -> String {
    std::env::var("OPENCLAW_LOG_PATH").unwrap_or_else(|_| {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
        format!("{}/.openclaw/logs/gateway.log", home)
    })
}
const POLL_INTERVAL_MS: u64 = 1000;

/// Session state tracking for computing durations
struct SessionTracker {
    active_sessions: HashMap<String, SessionState>,
}

struct SessionState {
    session_key: String,
    model: Option<String>,
    started_at: chrono::DateTime<chrono::Utc>,
}

impl SessionTracker {
    fn new() -> Self {
        Self {
            active_sessions: HashMap::new(),
        }
    }
    
    fn session_started(&mut self, session_id: &str, session_key: &str, model: Option<String>, timestamp: &str) {
        if let Ok(ts) = chrono::DateTime::parse_from_rfc3339(timestamp) {
            self.active_sessions.insert(session_id.to_string(), SessionState {
                session_key: session_key.to_string(),
                model,
                started_at: ts.with_timezone(&chrono::Utc),
            });
        }
    }
    
    fn session_ended(&mut self, session_id: &str, timestamp: &str) -> Option<(String, Option<String>, i64)> {
        if let Some(state) = self.active_sessions.remove(session_id) {
            if let Ok(end_ts) = chrono::DateTime::parse_from_rfc3339(timestamp) {
                let duration_secs = (end_ts.with_timezone(&chrono::Utc) - state.started_at).num_seconds();
                return Some((state.session_key, state.model, duration_secs));
            }
        }
        None
    }
}

/// Start the log watcher background task
pub fn start_log_watcher(pool: SqlitePool, broadcaster: Broadcaster, shutdown: watch::Receiver<bool>) {
    tokio::spawn(async move {
        if let Err(e) = run_log_watcher(pool, broadcaster, shutdown).await {
            error!("Log watcher error: {}", e);
        }
    });
}

async fn run_log_watcher(
    pool: SqlitePool,
    broadcaster: Broadcaster,
    mut shutdown: watch::Receiver<bool>,
) -> anyhow::Result<()> {
    let log_file = PathBuf::from(log_path());
    
    // Wait for log file to exist
    while !log_file.exists() {
        info!("Waiting for OpenClaw log file at {}", log_path());
        tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
        
        if *shutdown.borrow() {
            return Ok(());
        }
    }
    
    info!("Starting log watcher for {}", log_path());
    
    let file = File::open(&log_file).await?;
    let mut reader = BufReader::new(file);
    
    // Seek to end of file to only process new lines
    reader.seek(SeekFrom::End(0)).await?;
    
    let mut line = String::new();
    let mut last_position = reader.stream_position().await?;
    let mut session_tracker = SessionTracker::new();
    
    loop {
        // Check shutdown
        if *shutdown.borrow() {
            info!("Log watcher shutting down");
            return Ok(());
        }
        
        // Try to read a line
        line.clear();
        match reader.read_line(&mut line).await {
            Ok(0) => {
                // No new data, check if file was rotated
                let current_size = tokio::fs::metadata(&log_file)
                    .await
                    .map(|m| m.len())
                    .unwrap_or(0);
                
                if current_size < last_position {
                    // File was truncated/rotated, reopen
                    info!("Log file rotated, reopening");
                    let file = File::open(&log_file).await?;
                    reader = BufReader::new(file);
                    last_position = 0;
                }
                
                // Wait before next poll
                tokio::select! {
                    _ = tokio::time::sleep(tokio::time::Duration::from_millis(POLL_INTERVAL_MS)) => {}
                    _ = shutdown.changed() => {
                        if *shutdown.borrow() {
                            return Ok(());
                        }
                    }
                }
            }
            Ok(_) => {
                last_position = reader.stream_position().await?;
                
                // Parse and process the line
                if let Some(parsed) = parse_log_line(&line, &mut session_tracker) {
                    match parsed {
                        ParsedLogEvent::Event(event) => {
                            // Check for duplicate before inserting
                            match db::insert_event_dedup(&pool, &event).await {
                                Ok(Some(evt)) => {
                                    broadcaster.broadcast("event", serde_json::to_value(&evt).unwrap());
                                    debug!(event_type = %evt.event_type, "Log event broadcast");
                                }
                                Ok(None) => {
                                    // Duplicate, skip
                                    debug!("Skipped duplicate event");
                                }
                                Err(e) => {
                                    warn!("Failed to insert log event: {}", e);
                                }
                            }
                        }
                        ParsedLogEvent::SessionUpdate(session) => {
                            // Upsert session
                            if let Err(e) = db::upsert_session(&pool, &session).await {
                                warn!("Failed to upsert session: {}", e);
                            } else {
                                debug!(session_key = %session.session_key, "Session updated");
                            }
                        }
                    }
                }
            }
            Err(e) => {
                error!("Error reading log line: {}", e);
                tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
            }
        }
    }
}

enum ParsedLogEvent {
    Event(CreateEvent),
    SessionUpdate(CreateSession),
}

/// Parse a log line and extract a meaningful event if applicable
fn parse_log_line(line: &str, session_tracker: &mut SessionTracker) -> Option<ParsedLogEvent> {
    let json: serde_json::Value = serde_json::from_str(line).ok()?;
    
    // Extract metadata
    let meta = json.get("_meta")?;
    let level = meta.get("logLevelName")?.as_str()?;
    let timestamp = json.get("time")?.as_str()?.to_string();
    
    // Get the log name (subsystem info)
    let name_raw = meta.get("name")?.as_str()?;
    let subsystem = parse_subsystem(name_raw);
    
    // Get messages - field "1" is main message, "2" might be additional context
    let message = json.get("1")
        .and_then(|v| {
            if v.is_string() {
                v.as_str().map(|s| s.to_string())
            } else {
                Some(serde_json::to_string(v).unwrap_or_default())
            }
        })
        .unwrap_or_default();
    
    let message_2 = json.get("2")
        .and_then(|v| {
            if v.is_string() {
                v.as_str().map(|s| s.to_string())
            } else {
                Some(serde_json::to_string(v).unwrap_or_default())
            }
        });
    
    // Skip noisy messages
    if should_skip(&message, &message_2, &subsystem) {
        return None;
    }
    
    // Check for session state changes and track them
    if let Some(sub) = &subsystem {
        if sub == "diagnostic" && message.contains("session state:") {
            if let Some(session_event) = process_session_state(&message, &timestamp, session_tracker) {
                return Some(session_event);
            }
        }
    }
    
    // Determine event type and create event
    let (event_type, summary, detail) = categorize_event(&message, &message_2, &subsystem, level)?;
    
    Some(ParsedLogEvent::Event(CreateEvent {
        event_type,
        summary,
        detail: Some(detail),
        session_id: extract_session_id(&message),
        task_id: None,
        metadata: Some(serde_json::json!({
            "source": "openclaw_log",
            "subsystem": subsystem,
            "level": level,
            "timestamp": timestamp,
        })),
    }))
}

fn parse_subsystem(name: &str) -> Option<String> {
    // Name is often JSON like {"subsystem":"agent/embedded"} or {"module":"cron",...}
    if name.starts_with('{') {
        let parsed: serde_json::Value = serde_json::from_str(name).ok()?;
        // Try subsystem first, then module
        parsed.get("subsystem")
            .or_else(|| parsed.get("module"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    } else {
        Some(name.to_string())
    }
}

fn should_skip(message: &str, message_2: &Option<String>, subsystem: &Option<String>) -> bool {
    // Skip cron timer armed (very frequent)
    if let Some(m2) = message_2 {
        if m2.contains("cron: timer armed") {
            return true;
        }
    }
    
    // Skip lane enqueue/dequeue (internal queue management)
    if message.contains("lane enqueue") || message.contains("lane dequeue") {
        return true;
    }
    
    // Skip run cleared (cleanup noise)
    if message.contains("run cleared:") {
        return true;
    }
    
    // Skip native image logs
    if message.contains("Native image:") && !message.contains("error") {
        return true;
    }
    
    // Skip raw telegram update logs (too verbose)
    if let Some(sub) = subsystem {
        if sub.contains("telegram/raw-update") {
            return true;
        }
    }
    
    // Skip lane task done (internal)
    if message.contains("lane task done:") {
        return true;
    }
    
    false
}

fn process_session_state(message: &str, timestamp: &str, tracker: &mut SessionTracker) -> Option<ParsedLogEvent> {
    // Parse: session state: sessionId=X sessionKey=Y prev=Z new=W reason="R"
    let session_id = extract_field(message, "sessionId=")?;
    let session_key = extract_field(message, "sessionKey=")?;
    let new_state = extract_field(message, "new=")?;
    let reason = extract_field(message, "reason=").map(|s| s.trim_matches('"').to_string());
    
    if new_state == "processing" {
        // Session started
        tracker.session_started(&session_id, &session_key, None, timestamp);
        
        // Determine session type from key
        let session_type = determine_session_type(&session_key);
        
        return Some(ParsedLogEvent::SessionUpdate(CreateSession {
            session_key: session_key.clone(),
            title: Some(format_session_title(&session_key, &reason)),
            session_type,
            model: None,
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            cost_usd: 0.0,
            task_id: None,
            parent_session_id: extract_parent_session(&session_key),
        }));
    } else if new_state == "idle" {
        // Session ended - calculate duration
        if let Some((_key, _model, _duration)) = tracker.session_ended(&session_id, timestamp) {
            // Session completion is tracked, but we don't have token usage from logs
            // The session was already created on start, we could update ended_at here
            // but for now just return None since we don't have a separate "end session" API
        }
    }
    
    None
}

fn determine_session_type(session_key: &str) -> String {
    if session_key.contains(":cron:") {
        "cron".to_string()
    } else if session_key.contains(":subagent:") {
        "subagent".to_string()
    } else if session_key.contains(":main:") || session_key.contains("agent:main:") {
        "main".to_string()
    } else {
        "unknown".to_string()
    }
}

fn format_session_title(session_key: &str, reason: &Option<String>) -> String {
    let base = if session_key.contains(":cron:") {
        "Cron job session"
    } else if session_key.contains(":subagent:") {
        "Sub-agent session"
    } else {
        "Main session"
    };
    
    if let Some(r) = reason {
        format!("{} ({})", base, r)
    } else {
        base.to_string()
    }
}

fn extract_parent_session(session_key: &str) -> Option<i64> {
    // For now, we don't track parent sessions by ID
    // This would require looking up the parent session in the DB
    None
}

fn categorize_event(message: &str, message_2: &Option<String>, subsystem: &Option<String>, level: &str) -> Option<(String, String, String)> {
    let msg_lower = message.to_lowercase();
    let sub_lower = subsystem.as_ref().map(|s| s.to_lowercase()).unwrap_or_default();
    
    // Error/Warning events - highest priority
    if level == "ERROR" {
        let summary = clean_error_message(message);
        return Some(("alert".to_string(), summary, message.to_string()));
    }
    
    if level == "WARN" {
        // Check for cron failures
        if let Some(m2) = message_2 {
            if m2.contains("cron: job failed") {
                let job_name = extract_json_field(message, "jobName").unwrap_or_else(|| "Unknown".to_string());
                return Some((
                    "cron".to_string(),
                    format!("⚠️ Cron job '{}' failed", job_name),
                    format!("{}\n{}", message, m2),
                ));
            }
            if m2.contains("cron: applying error backoff") {
                let job_id = extract_json_field(message, "jobId").unwrap_or_else(|| "?".to_string());
                let errors = extract_json_field(message, "consecutiveErrors").unwrap_or_else(|| "?".to_string());
                return Some((
                    "cron".to_string(),
                    format!("Cron job in backoff ({} consecutive errors)", errors),
                    format!("Job ID: {}", job_id),
                ));
            }
        }
        let summary = clean_error_message(message);
        return Some(("warning".to_string(), summary, message.to_string()));
    }
    
    // Cron events
    if sub_lower.contains("cron") || msg_lower.contains("cron:") {
        if let Some(m2) = message_2 {
            // cron: started
            if m2.contains("cron: started") {
                let jobs = extract_json_field(message, "jobs").unwrap_or_else(|| "?".to_string());
                return Some((
                    "cron".to_string(),
                    format!("🕐 Cron scheduler started ({} jobs)", jobs),
                    message.to_string(),
                ));
            }
            // cron: job added
            if m2.contains("cron: job added") {
                let job_name = extract_json_field(message, "jobName").unwrap_or_else(|| "Unknown".to_string());
                return Some((
                    "cron".to_string(),
                    format!("➕ Cron job added: '{}'", job_name),
                    message.to_string(),
                ));
            }
        }
        return None; // Skip other cron messages
    }
    
    // Shell/Exec events
    if sub_lower.contains("exec") || msg_lower.starts_with("elevated command") {
        let cmd = extract_shell_command(message);
        return Some((
            "shell".to_string(),
            format!("🖥️ {}", truncate(&cmd, 80)),
            cmd,
        ));
    }
    
    // Tool execution events
    if msg_lower.contains("embedded run tool start:") {
        let tool = extract_field(message, "tool=").unwrap_or_else(|| "unknown".to_string());
        return Some((
            categorize_tool(&tool),
            format!("🔧 Tool: {}", tool),
            message.to_string(),
        ));
    }
    
    // Skip tool end events (noisy, tool start is enough)
    if msg_lower.contains("embedded run tool end:") {
        return None;
    }
    
    // Agent run start
    if msg_lower.contains("embedded run start:") {
        let model = extract_field(message, "model=").unwrap_or_else(|| "unknown".to_string());
        let channel = extract_field(message, "messageChannel=").unwrap_or_else(|| "unknown".to_string());
        let session_id = extract_field(message, "sessionId=").unwrap_or_else(|| "?".to_string());
        
        // Check if it's a subagent
        if message.contains("announce:v1:agent:main:subagent:") {
            return Some((
                "agent".to_string(),
                format!("🤖 Sub-agent started ({})", model),
                format!("Model: {}, Channel: {}, Session: {}", model, channel, session_id),
            ));
        }
        
        return Some((
            "agent".to_string(),
            format!("🚀 Agent run started ({})", model),
            format!("Model: {}, Channel: {}, Session: {}", model, channel, session_id),
        ));
    }
    
    // Agent run done
    if msg_lower.contains("embedded run done:") {
        let duration = extract_field(message, "durationMs=").unwrap_or_else(|| "?".to_string());
        let aborted = extract_field(message, "aborted=").unwrap_or_else(|| "false".to_string());
        
        if aborted == "true" {
            return Some((
                "agent".to_string(),
                "⚠️ Agent run aborted".to_string(),
                format!("Duration: {}ms", duration),
            ));
        }
        
        return Some((
            "agent".to_string(),
            format!("✅ Agent run completed ({}ms)", duration),
            message.to_string(),
        ));
    }
    
    // Prompt/Context events
    if msg_lower.contains("[context-diag]") {
        let messages = extract_field(message, "messages=").unwrap_or_else(|| "?".to_string());
        let chars = extract_field(message, "historyTextChars=").unwrap_or_else(|| "?".to_string());
        let provider = extract_field(message, "provider=").unwrap_or_else(|| "?".to_string());
        return Some((
            "api".to_string(),
            format!("📊 Context: {} msgs, {} chars ({})", messages, chars, provider),
            message.to_string(),
        ));
    }
    
    // Session state changes
    if msg_lower.contains("session state:") {
        let new_state = extract_field(message, "new=").unwrap_or_else(|| "?".to_string());
        let reason = extract_field(message, "reason=")
            .map(|s| s.trim_matches('"').to_string())
            .unwrap_or_else(|| "".to_string());
        let session_key = extract_field(message, "sessionKey=").unwrap_or_default();
        
        // Determine session type from key for nice emoji
        let emoji = if session_key.contains(":cron:") {
            "🕐"
        } else if session_key.contains(":subagent:") {
            "🤖"
        } else {
            "👤"
        };
        
        let state_desc = match new_state.as_str() {
            "processing" => "working",
            "idle" => "idle",
            _ => &new_state,
        };
        
        return Some((
            "agent".to_string(),
            format!("{} Session {} ({})", emoji, state_desc, reason),
            format!("Key: {}", session_key),
        ));
    }
    
    // Message sending events
    if msg_lower.contains("message") && (msg_lower.contains("send") || msg_lower.contains("telegram")) {
        return Some((
            "message".to_string(),
            "💬 Message sent".to_string(),
            message.to_string(),
        ));
    }
    
    // File operations (web_fetch, read, write)
    if msg_lower.contains("web_fetch") || msg_lower.contains("web_search") {
        return Some((
            "api".to_string(),
            "🌐 Web request".to_string(),
            message.to_string(),
        ));
    }
    
    // WebSocket events
    if sub_lower.contains("gateway/ws") {
        // Check for error responses
        if msg_lower.contains("⇄ res ✗") {
            let error_msg = extract_field(message, "errorMessage=")
                .unwrap_or_else(|| "Unknown error".to_string());
            return Some((
                "alert".to_string(),
                format!("❌ {}", truncate(&error_msg, 60)),
                message.to_string(),
            ));
        }
        return None; // Skip other ws messages
    }
    
    // Skip DEBUG level by default (too verbose)
    if level == "DEBUG" {
        return None;
    }
    
    // Generic INFO events - only if we haven't matched anything specific
    // Return None for truly uninteresting messages
    if message.is_empty() || message.len() < 10 {
        return None;
    }
    
    // For unmatched INFO messages, return a generic event
    Some((
        "info".to_string(),
        truncate(message, 80),
        message.to_string(),
    ))
}

fn categorize_tool(tool: &str) -> String {
    match tool.to_lowercase().as_str() {
        "exec" => "shell".to_string(),
        "read" | "write" | "edit" => "file".to_string(),
        "web_search" | "web_fetch" => "api".to_string(),
        "message" => "message".to_string(),
        "browser" | "canvas" => "api".to_string(),
        "nodes" | "subagents" => "agent".to_string(),
        _ => "info".to_string(),
    }
}

fn extract_shell_command(message: &str) -> String {
    // Extract command from "elevated command X" or similar
    if let Some(idx) = message.find("elevated command ") {
        return message[idx + 17..].trim().to_string();
    }
    if let Some(idx) = message.find("command ") {
        return message[idx + 8..].trim().to_string();
    }
    message.to_string()
}

fn clean_error_message(message: &str) -> String {
    // Remove common prefixes and clean up error messages
    let msg = message
        .trim_start_matches("[tools] ")
        .trim_start_matches("message failed: ");
    
    truncate(msg, 100)
}

fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len])
    }
}

fn extract_field(message: &str, field: &str) -> Option<String> {
    let start = message.find(field)?;
    let rest = &message[start + field.len()..];
    let end = rest.find(|c: char| c.is_whitespace() || c == ',' || c == ')').unwrap_or(rest.len());
    Some(rest[..end].to_string())
}

fn extract_json_field(json_str: &str, field: &str) -> Option<String> {
    // Parse JSON and extract a field value
    let json: serde_json::Value = serde_json::from_str(json_str).ok()?;
    json.get(field).and_then(|v| {
        if v.is_string() {
            v.as_str().map(|s| s.to_string())
        } else {
            Some(v.to_string())
        }
    })
}

fn extract_session_id(message: &str) -> Option<String> {
    extract_field(message, "sessionId=")
}
