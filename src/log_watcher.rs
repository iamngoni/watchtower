//! OpenClaw log watcher - tails gateway.log and pushes events to SSE
//!
//! Parses NDJSON log lines from OpenClaw and extracts meaningful events.

use crate::models::CreateEvent;
use crate::sse::Broadcaster;
use crate::db;
use sqlx::SqlitePool;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::fs::File;
use tokio::io::{AsyncBufReadExt, AsyncSeekExt, BufReader, SeekFrom};
use tokio::sync::watch;
use tracing::{debug, error, info, warn};

const LOG_PATH: &str = "/home/iamngoni/.openclaw/logs/gateway.log";
const POLL_INTERVAL_MS: u64 = 1000;

/// Parsed log entry from OpenClaw
#[derive(Debug)]
struct LogEntry {
    subsystem: Option<String>,
    message: String,
    level: String,
    timestamp: String,
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
    let log_path = PathBuf::from(LOG_PATH);
    
    // Wait for log file to exist
    while !log_path.exists() {
        info!("Waiting for OpenClaw log file at {}", LOG_PATH);
        tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
        
        if *shutdown.borrow() {
            return Ok(());
        }
    }
    
    info!("Starting log watcher for {}", LOG_PATH);
    
    let file = File::open(&log_path).await?;
    let mut reader = BufReader::new(file);
    
    // Seek to end of file to only process new lines
    reader.seek(SeekFrom::End(0)).await?;
    
    let mut line = String::new();
    let mut last_position = reader.stream_position().await?;
    
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
                let current_size = tokio::fs::metadata(&log_path)
                    .await
                    .map(|m| m.len())
                    .unwrap_or(0);
                
                if current_size < last_position {
                    // File was truncated/rotated, reopen
                    info!("Log file rotated, reopening");
                    let file = File::open(&log_path).await?;
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
                if let Some(event) = parse_log_line(&line) {
                    // Insert into database
                    match db::insert_event(&pool, &event).await {
                        Ok(evt) => {
                            broadcaster.broadcast("event", serde_json::to_value(&evt).unwrap());
                            debug!(event_type = %evt.event_type, "Log event broadcast");
                        }
                        Err(e) => {
                            warn!("Failed to insert log event: {}", e);
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

/// Parse a log line and extract a meaningful event if applicable
fn parse_log_line(line: &str) -> Option<CreateEvent> {
    let json: serde_json::Value = serde_json::from_str(line).ok()?;
    
    // Extract metadata
    let meta = json.get("_meta")?;
    let level = meta.get("logLevelName")?.as_str()?;
    let timestamp = json.get("time")?.as_str()?.to_string();
    
    // Get the log name (subsystem info)
    let name_raw = meta.get("name")?.as_str()?;
    let subsystem = parse_subsystem(name_raw);
    
    // Get the main message - it's in field "1"
    let message = json.get("1")
        .and_then(|v| {
            if v.is_string() {
                v.as_str().map(|s| s.to_string())
            } else {
                Some(serde_json::to_string(v).unwrap_or_default())
            }
        })
        .unwrap_or_default();
    
    // Skip noisy messages
    if should_skip(&message, &subsystem) {
        return None;
    }
    
    // Determine event type and create event
    let (event_type, summary, detail) = categorize_event(&message, &subsystem, level)?;
    
    Some(CreateEvent {
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
    })
}

fn parse_subsystem(name: &str) -> Option<String> {
    // Name is often JSON like {"subsystem":"agent/embedded"}
    if name.starts_with('{') {
        let parsed: serde_json::Value = serde_json::from_str(name).ok()?;
        parsed.get("subsystem").and_then(|v| v.as_str()).map(|s| s.to_string())
    } else {
        Some(name.to_string())
    }
}

fn should_skip(message: &str, subsystem: &Option<String>) -> bool {
    // Skip noisy diagnostic messages
    if message.contains("cron: timer armed") {
        return true;
    }
    if message.contains("lane enqueue") || message.contains("lane dequeue") {
        return true;
    }
    if message.contains("Native image:") && !message.contains("error") {
        return true;
    }
    
    // Skip raw telegram update logs (too verbose)
    if let Some(sub) = subsystem {
        if sub.contains("telegram/raw-update") {
            return true;
        }
    }
    
    false
}

fn categorize_event(message: &str, subsystem: &Option<String>, level: &str) -> Option<(String, String, String)> {
    let msg_lower = message.to_lowercase();
    
    // Error/Warning events
    if level == "ERROR" || level == "WARN" {
        let event_type = if level == "ERROR" { "alert" } else { "warning" };
        let summary = if message.len() > 100 {
            format!("{}...", &message[..100])
        } else {
            message.to_string()
        };
        return Some((event_type.to_string(), summary, message.to_string()));
    }
    
    // Session events
    if msg_lower.contains("embedded run start") {
        // Extract model and session info
        let model = extract_field(message, "model=");
        let session_id = extract_field(message, "sessionId=");
        return Some((
            "agent".to_string(),
            format!("Agent session started ({})", model.unwrap_or("unknown".to_string())),
            format!("Session: {}", session_id.unwrap_or("unknown".to_string())),
        ));
    }
    
    if msg_lower.contains("embedded run done") || msg_lower.contains("embedded run agent end") {
        let duration = extract_field(message, "durationMs=");
        return Some((
            "agent".to_string(),
            format!("Agent session completed ({}ms)", duration.unwrap_or("?".to_string())),
            message.to_string(),
        ));
    }
    
    // Cron events
    if msg_lower.contains("cron") && !msg_lower.contains("timer armed") {
        if msg_lower.contains("run") || msg_lower.contains("execute") {
            return Some((
                "cron".to_string(),
                "Cron job executed".to_string(),
                message.to_string(),
            ));
        }
    }
    
    // Context/prompt events (shows token usage)
    if msg_lower.contains("[context-diag]") {
        let messages = extract_field(message, "messages=");
        let chars = extract_field(message, "historyTextChars=");
        return Some((
            "api".to_string(),
            format!("Context prepared ({} messages, {} chars)", 
                messages.unwrap_or("?".to_string()),
                chars.unwrap_or("?".to_string())
            ),
            message.to_string(),
        ));
    }
    
    // Telegram message received (not raw update, but the processed one)
    if let Some(sub) = subsystem {
        if sub.contains("telegram") && msg_lower.contains("telegram update:") {
            // This is handled by raw-update filter
            return None;
        }
    }
    
    // Session state changes
    if msg_lower.contains("session state:") {
        let new_state = extract_field(message, "new=");
        let reason = extract_field(message, "reason=");
        if let Some(state) = new_state {
            let reason_str = reason.unwrap_or_default();
            return Some((
                "agent".to_string(),
                format!("Session state: {} ({})", state, reason_str.trim_matches('"')),
                message.to_string(),
            ));
        }
    }
    
    // Skip everything else at DEBUG level
    if level == "DEBUG" {
        return None;
    }
    
    // Generic INFO events
    Some((
        "info".to_string(),
        if message.len() > 80 {
            format!("{}...", &message[..80])
        } else {
            message.to_string()
        },
        message.to_string(),
    ))
}

fn extract_field(message: &str, field: &str) -> Option<String> {
    let start = message.find(field)?;
    let rest = &message[start + field.len()..];
    let end = rest.find(|c: char| c.is_whitespace() || c == ',' || c == ')').unwrap_or(rest.len());
    Some(rest[..end].to_string())
}

fn extract_session_id(message: &str) -> Option<String> {
    extract_field(message, "sessionId=")
}
