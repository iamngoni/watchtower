//! Session JSONL watcher - tails active session files to capture tool results
//! 
//! Complements log_watcher by reading the actual tool outputs from session files.

use crate::db;
use crate::models::CreateEvent;
use crate::sse::Broadcaster;
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::fs::File;
use tokio::io::{AsyncBufReadExt, AsyncSeekExt, BufReader, SeekFrom};
use tokio::sync::watch;
use tracing::{debug, error, info, warn};

const POLL_INTERVAL_MS: u64 = 1500;
const SESSION_SCAN_INTERVAL_SECS: u64 = 10;
const MAX_OUTPUT_PREVIEW: usize = 500;

fn sessions_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    let custom = std::env::var("OPENCLAW_SESSIONS_DIR").ok();
    PathBuf::from(custom.unwrap_or_else(|| format!("{}/.openclaw/agents/main/sessions", home)))
}

/// Tracks a single session file being tailed
struct SessionFileTailer {
    path: PathBuf,
    reader: BufReader<File>,
    position: u64,
    /// Track pending tool calls: toolCallId -> (tool_name, command/args summary)
    pending_tools: HashMap<String, PendingTool>,
}

struct PendingTool {
    tool_name: String,
    args_summary: String,
}

/// Start the session watcher background task
pub fn start_session_watcher(pool: SqlitePool, broadcaster: Broadcaster, shutdown: watch::Receiver<bool>) {
    tokio::spawn(async move {
        if let Err(e) = run_session_watcher(pool, broadcaster, shutdown).await {
            error!("Session watcher error: {}", e);
        }
    });
}

async fn run_session_watcher(
    pool: SqlitePool,
    broadcaster: Broadcaster,
    mut shutdown: watch::Receiver<bool>,
) -> anyhow::Result<()> {
    let dir = sessions_dir();
    
    if !dir.exists() {
        info!("Sessions directory not found at {:?}, waiting...", dir);
        while !dir.exists() {
            tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
            if *shutdown.borrow() { return Ok(()); }
        }
    }
    
    info!("Starting session watcher for {:?}", dir);
    
    // Track active session files by path
    let mut tailers: HashMap<PathBuf, SessionFileTailer> = HashMap::new();
    let mut last_scan = std::time::Instant::now();
    
    loop {
        if *shutdown.borrow() { return Ok(()); }
        
        // Periodically scan for new/changed session files
        if last_scan.elapsed().as_secs() >= SESSION_SCAN_INTERVAL_SECS {
            scan_sessions(&dir, &mut tailers).await;
            last_scan = std::time::Instant::now();
        }
        
        // Read new lines from all active tailers
        let mut any_data = false;
        let paths: Vec<PathBuf> = tailers.keys().cloned().collect();
        
        for path in paths {
            if let Some(tailer) = tailers.get_mut(&path) {
                match read_new_lines(tailer, &pool, &broadcaster).await {
                    Ok(true) => any_data = true,
                    Ok(false) => {}
                    Err(e) => {
                        warn!("Error reading session file {:?}: {}", path, e);
                        tailers.remove(&path);
                    }
                }
            }
        }
        
        if !any_data {
            tokio::select! {
                _ = tokio::time::sleep(tokio::time::Duration::from_millis(POLL_INTERVAL_MS)) => {}
                _ = shutdown.changed() => {
                    if *shutdown.borrow() { return Ok(()); }
                }
            }
        }
    }
}

/// Scan sessions dir for recently modified files
async fn scan_sessions(dir: &PathBuf, tailers: &mut HashMap<PathBuf, SessionFileTailer>) {
    let mut entries = match tokio::fs::read_dir(dir).await {
        Ok(e) => e,
        Err(_) => return,
    };
    
    let now = std::time::SystemTime::now();
    
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        
        // Only watch files modified in the last 5 minutes
        if let Ok(meta) = entry.metadata().await {
            if let Ok(modified) = meta.modified() {
                let age = now.duration_since(modified).unwrap_or_default();
                if age.as_secs() > 300 {
                    // Remove stale tailers
                    tailers.remove(&path);
                    continue;
                }
                
                // Add new tailer if not already tracking
                if !tailers.contains_key(&path) {
                    match open_tailer(&path).await {
                        Ok(tailer) => {
                            debug!("Now tailing session file: {:?}", path.file_name());
                            tailers.insert(path, tailer);
                        }
                        Err(e) => {
                            warn!("Failed to open session file {:?}: {}", path, e);
                        }
                    }
                }
            }
        }
    }
}

async fn open_tailer(path: &PathBuf) -> anyhow::Result<SessionFileTailer> {
    let file = File::open(path).await?;
    let mut reader = BufReader::new(file);
    // Seek to end — only process new lines
    let pos = reader.seek(SeekFrom::End(0)).await?;
    
    Ok(SessionFileTailer {
        path: path.clone(),
        reader,
        position: pos,
        pending_tools: HashMap::new(),
    })
}

async fn read_new_lines(
    tailer: &mut SessionFileTailer,
    pool: &SqlitePool,
    broadcaster: &Broadcaster,
) -> anyhow::Result<bool> {
    let mut line = String::new();
    let mut had_data = false;
    
    loop {
        line.clear();
        match tailer.reader.read_line(&mut line).await? {
            0 => {
                // Check for file truncation
                let current_size = tokio::fs::metadata(&tailer.path)
                    .await
                    .map(|m| m.len())
                    .unwrap_or(0);
                if current_size < tailer.position {
                    let file = File::open(&tailer.path).await?;
                    tailer.reader = BufReader::new(file);
                    tailer.position = 0;
                    tailer.pending_tools.clear();
                }
                break;
            }
            _ => {
                had_data = true;
                tailer.position = tailer.reader.stream_position().await?;
                
                if let Some(event) = parse_session_line(&line, &mut tailer.pending_tools) {
                    match db::insert_event_dedup(pool, &event).await {
                        Ok(Some(evt)) => {
                            if let Some(html) = crate::web::render_event_html(&evt) {
                                broadcaster.broadcast_html("event", html);
                            }
                            debug!(event_type = %evt.event_type, "Session event broadcast");
                        }
                        Ok(None) => {} // duplicate
                        Err(e) => warn!("Failed to insert session event: {}", e),
                    }
                }
            }
        }
    }
    
    Ok(had_data)
}

fn parse_session_line(line: &str, pending_tools: &mut HashMap<String, PendingTool>) -> Option<CreateEvent> {
    let json: serde_json::Value = serde_json::from_str(line).ok()?;
    let msg = json.get("message")?;
    let role = msg.get("role")?.as_str()?;
    
    match role {
        "assistant" => {
            // Track tool calls for pairing with results
            let content = msg.get("content")?;
            if let Some(arr) = content.as_array() {
                for block in arr {
                    if block.get("type")?.as_str()? == "toolCall" {
                        let id = block.get("id")?.as_str()?;
                        let name = block.get("name")?.as_str()?.to_string();
                        let args = block.get("arguments").cloned().unwrap_or_default();
                        let summary = summarize_tool_args(&name, &args);
                        pending_tools.insert(id.to_string(), PendingTool {
                            tool_name: name,
                            args_summary: summary,
                        });
                    }
                }
            }
            None
        }
        "toolResult" => {
            let tool_call_id = msg.get("toolCallId")?.as_str()?;
            let tool_name = msg.get("toolName").and_then(|v| v.as_str()).unwrap_or("unknown");
            let details = msg.get("details");
            
            // Get status and duration from details
            let status = details
                .and_then(|d| d.get("status"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let exit_code = details
                .and_then(|d| d.get("exitCode"))
                .and_then(|v| v.as_i64());
            let duration_ms = details
                .and_then(|d| d.get("durationMs"))
                .and_then(|v| v.as_u64());
            
            // Extract output text
            let output = extract_tool_output(msg);
            
            // Get the original args from pending
            let pending = pending_tools.remove(tool_call_id);
            let args_summary = pending.as_ref().map(|p| p.args_summary.clone()).unwrap_or_default();
            
            // Build summary
            let duration_str = duration_ms
                .map(|ms| if ms >= 1000 { format!("{:.1}s", ms as f64 / 1000.0) } else { format!("{}ms", ms) })
                .unwrap_or_default();
            
            let exit_str = exit_code
                .map(|c| if c == 0 { "✓".to_string() } else { format!("exit {}", c) })
                .unwrap_or_default();
            
            let status_icon = match status {
                "completed" => if exit_code.unwrap_or(0) == 0 { "✅" } else { "⚠️" },
                "error" => "❌",
                _ => "📋",
            };
            
            let summary = format!(
                "{} {} result ({} {})",
                status_icon, tool_name, exit_str, duration_str
            ).trim().to_string();
            
            // Build detail with args + output preview
            let output_preview = truncate_output(&output, MAX_OUTPUT_PREVIEW);
            let detail = if args_summary.is_empty() {
                output_preview
            } else {
                format!("$ {}\n{}", args_summary, output_preview)
            };
            
            Some(CreateEvent {
                event_type: categorize_tool_result(tool_name),
                summary,
                detail: Some(detail),
                session_id: None,
                task_id: None,
                metadata: Some(serde_json::json!({
                    "source": "session_jsonl",
                    "tool": tool_name,
                    "status": status,
                    "exit_code": exit_code,
                    "duration_ms": duration_ms,
                })),
            })
        }
        _ => None,
    }
}

fn summarize_tool_args(tool_name: &str, args: &serde_json::Value) -> String {
    match tool_name {
        "exec" => {
            args.get("command")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        }
        "Read" | "read" => {
            let path = args.get("path")
                .or_else(|| args.get("file_path"))
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            format!("read {}", path)
        }
        "Write" | "write" => {
            let path = args.get("path")
                .or_else(|| args.get("file_path"))
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            format!("write {}", path)
        }
        "Edit" | "edit" => {
            let path = args.get("path")
                .or_else(|| args.get("file_path"))
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            format!("edit {}", path)
        }
        "web_search" => {
            let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("?");
            format!("search: {}", query)
        }
        "web_fetch" => {
            let url = args.get("url").and_then(|v| v.as_str()).unwrap_or("?");
            format!("fetch {}", url)
        }
        "message" => {
            let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("?");
            format!("message {}", action)
        }
        _ => {
            // Generic: just show first string arg
            if let Some(obj) = args.as_object() {
                for (k, v) in obj.iter().take(1) {
                    if let Some(s) = v.as_str() {
                        return format!("{}: {}", k, truncate_str(s, 80));
                    }
                }
            }
            String::new()
        }
    }
}

fn extract_tool_output(msg: &serde_json::Value) -> String {
    let content = match msg.get("content") {
        Some(c) => c,
        None => return String::new(),
    };
    
    if let Some(arr) = content.as_array() {
        let mut parts = Vec::new();
        for block in arr {
            if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
                parts.push(text.to_string());
            }
        }
        parts.join("\n")
    } else if let Some(s) = content.as_str() {
        s.to_string()
    } else {
        String::new()
    }
}

fn categorize_tool_result(tool: &str) -> String {
    match tool.to_lowercase().as_str() {
        "exec" => "shell_result".to_string(),
        "read" => "file_result".to_string(),
        "write" | "edit" => "file_result".to_string(),
        "web_search" | "web_fetch" => "api_result".to_string(),
        "message" => "message_result".to_string(),
        "browser" | "canvas" => "api_result".to_string(),
        _ => "tool_result".to_string(),
    }
}

fn truncate_output(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}… ({} more chars)", &s[..max], s.len() - max)
    }
}

fn truncate_str(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max])
    }
}
