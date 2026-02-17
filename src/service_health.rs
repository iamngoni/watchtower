//! Service health checker - checks the status of homelab services

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{debug, warn};

const CACHE_DURATION_SECS: u64 = 30;
const REQUEST_TIMEOUT_SECS: u64 = 5;

/// Service health status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceHealth {
    pub name: String,
    pub url: String,
    pub healthy: bool,
    pub response_time_ms: Option<u64>,
    pub error: Option<String>,
}

/// All services health response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServicesHealth {
    pub services: Vec<ServiceHealth>,
    pub checked_at: i64,
}

/// Cached health state
struct HealthCache {
    health: Option<ServicesHealth>,
    last_check: Option<Instant>,
}

/// Service health checker with caching
pub struct HealthChecker {
    cache: RwLock<HealthCache>,
    client: reqwest::Client,
}

impl HealthChecker {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .build()
            .unwrap_or_default();
        
        Self {
            cache: RwLock::new(HealthCache {
                health: None,
                last_check: None,
            }),
            client,
        }
    }
    
    /// Get cached health or check if cache expired
    pub async fn get_health(&self) -> ServicesHealth {
        // Check cache
        {
            let cache = self.cache.read().await;
            if let (Some(health), Some(last_check)) = (&cache.health, cache.last_check) {
                if last_check.elapsed() < Duration::from_secs(CACHE_DURATION_SECS) {
                    return health.clone();
                }
            }
        }
        
        // Cache expired or empty, refresh
        self.refresh_health().await
    }
    
    /// Force refresh health checks
    pub async fn refresh_health(&self) -> ServicesHealth {
        let services = self.check_all_services().await;
        let health = ServicesHealth {
            services,
            checked_at: chrono::Utc::now().timestamp(),
        };
        
        // Update cache
        {
            let mut cache = self.cache.write().await;
            cache.health = Some(health.clone());
            cache.last_check = Some(Instant::now());
        }
        
        health
    }
    
    async fn check_all_services(&self) -> Vec<ServiceHealth> {
        let services_to_check = vec![
            ("Watchtower", "http://localhost:3002/health", false, None),
            ("Kompressor", "http://localhost:8078/health", false, None),
            ("Jellyfin", "http://localhost:8096/health", false, None),
            ("Homepage", "http://localhost:3000", false, None),
            ("qBittorrent", "http://localhost:8080", false, None),
            ("Home Assistant", "http://localhost:8123", false, None),
            ("Sonarr", "http://localhost:8989/api/v3/health", true, Some("f679bfcb53dc43c392abe4fb70f4e75f")),
            ("Radarr", "http://localhost:7878/api/v3/health", true, Some("8ff6bbce0b6648e0a715edfa68fa262a")),
        ];
        
        let mut handles = Vec::new();
        
        for (name, url, needs_api_key, api_key) in services_to_check {
            let client = self.client.clone();
            let name = name.to_string();
            let url = url.to_string();
            let api_key = api_key.map(|s| s.to_string());
            
            let handle = tokio::spawn(async move {
                check_service(&client, &name, &url, needs_api_key, api_key.as_deref()).await
            });
            handles.push(handle);
        }
        
        let mut results = Vec::new();
        for handle in handles {
            match handle.await {
                Ok(health) => results.push(health),
                Err(e) => {
                    warn!("Health check task failed: {}", e);
                }
            }
        }
        
        results
    }
}

impl Default for HealthChecker {
    fn default() -> Self {
        Self::new()
    }
}

async fn check_service(
    client: &reqwest::Client,
    name: &str,
    url: &str,
    needs_api_key: bool,
    api_key: Option<&str>,
) -> ServiceHealth {
    let start = Instant::now();
    
    let mut request = client.get(url);
    
    if needs_api_key {
        if let Some(key) = api_key {
            request = request.query(&[("apikey", key)]);
        }
    }
    
    match request.send().await {
        Ok(response) => {
            let elapsed = start.elapsed().as_millis() as u64;
            let status = response.status();
            
            if status.is_success() || status.as_u16() == 401 || status.as_u16() == 403 {
                // Consider auth errors as "up" since the service responded
                ServiceHealth {
                    name: name.to_string(),
                    url: url.to_string(),
                    healthy: status.is_success(),
                    response_time_ms: Some(elapsed),
                    error: if !status.is_success() {
                        Some(format!("HTTP {}", status.as_u16()))
                    } else {
                        None
                    },
                }
            } else {
                ServiceHealth {
                    name: name.to_string(),
                    url: url.to_string(),
                    healthy: false,
                    response_time_ms: Some(elapsed),
                    error: Some(format!("HTTP {}", status.as_u16())),
                }
            }
        }
        Err(e) => {
            let elapsed = start.elapsed().as_millis() as u64;
            let error_msg = if e.is_connect() {
                "Connection refused".to_string()
            } else if e.is_timeout() {
                "Timeout".to_string()
            } else {
                e.to_string()
            };
            
            debug!("Health check failed for {}: {}", name, error_msg);
            
            ServiceHealth {
                name: name.to_string(),
                url: url.to_string(),
                healthy: false,
                response_time_ms: Some(elapsed),
                error: Some(error_msg),
            }
        }
    }
}

/// Kompressor stats (parsed from HTML response)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct KompressorStats {
    pub discovered_files: u64,
    pub space_saved_gb: f64,
    pub space_before_gb: f64,
    pub space_after_gb: f64,
    pub processed: u64,
    pub saved_count: u64,
    pub skipped_count: u64,
    pub pending: u64,
    pub active: u64,
    pub failed: u64,
    pub cancelled: u64,
    pub encoder: String,
    pub available: bool,
}

/// Fetch and parse Kompressor stats
pub async fn get_kompressor_stats() -> KompressorStats {
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build() {
            Ok(c) => c,
            Err(_) => return KompressorStats::default(),
        };
    
    let mut stats = KompressorStats::default();
    
    // Fetch main stats
    match client.get("http://localhost:8078/api/stats").send().await {
        Ok(resp) => {
            match resp.text().await {
                Ok(html) => {
                    stats.available = true;
                    parse_main_stats(&html, &mut stats);
                }
                Err(_) => {
                    stats.available = false;
                    return stats;
                }
            }
        }
        Err(_) => {
            stats.available = false;
            return stats;
        }
    }
    
    // Fetch queue stats
    if let Ok(resp) = client.get("http://localhost:8078/api/queue/stats").send().await {
        if let Ok(html) = resp.text().await {
            parse_queue_stats(&html, &mut stats);
        }
    }
    
    stats
}

fn parse_main_stats(html: &str, stats: &mut KompressorStats) {
    // Parse discovered files
    if let Some(val) = extract_metric_value(html, "DISCOVERED FILES") {
        stats.discovered_files = val.parse().unwrap_or(0);
    }
    
    // Parse space saved (e.g., "27.68 GB")
    if let Some(space_section) = html.find("SPACE SAVED") {
        let rest = &html[space_section..];
        if let Some(val_start) = rest.find("metric-value\">") {
            let val_rest = &rest[val_start + 14..];
            if let Some(val_end) = val_rest.find('<') {
                let val_str = &val_rest[..val_end].trim();
                // Parse "27.68 GB" -> 27.68
                if let Some(num_str) = val_str.split_whitespace().next() {
                    stats.space_saved_gb = num_str.parse().unwrap_or(0.0);
                }
            }
        }
        // Parse subtext "75.01 GB → 47.33 GB"
        if let Some(subtext_start) = rest.find("metric-subtext\">") {
            let sub_rest = &rest[subtext_start + 16..];
            if let Some(sub_end) = sub_rest.find('<') {
                let subtext = &sub_rest[..sub_end];
                // Parse before and after
                if subtext.contains('→') {
                    let parts: Vec<&str> = subtext.split('→').collect();
                    if parts.len() == 2 {
                        if let Some(before) = parts[0].trim().split_whitespace().next() {
                            stats.space_before_gb = before.parse().unwrap_or(0.0);
                        }
                        if let Some(after) = parts[1].trim().split_whitespace().next() {
                            stats.space_after_gb = after.parse().unwrap_or(0.0);
                        }
                    }
                }
            }
        }
    }
    
    // Parse processed
    if let Some(val) = extract_metric_value(html, "PROCESSED") {
        stats.processed = val.parse().unwrap_or(0);
    }
    
    // Parse encoder
    if let Some(encoder_section) = html.find("ENCODER") {
        let rest = &html[encoder_section..];
        if let Some(subtext_start) = rest.find("metric-subtext\">") {
            let sub_rest = &rest[subtext_start + 16..];
            if let Some(sub_end) = sub_rest.find('<') {
                stats.encoder = sub_rest[..sub_end].trim().to_string();
            }
        }
    }
}

fn parse_queue_stats(html: &str, stats: &mut KompressorStats) {
    // Parse pending
    if let Some(pending_section) = html.find("Pending") {
        let rest = &html[pending_section..];
        if let Some(val_start) = rest.find("metric-value\">") {
            let val_rest = &rest[val_start + 14..];
            if let Some(val_end) = val_rest.find('<') {
                stats.pending = val_rest[..val_end].trim().parse().unwrap_or(0);
            }
        }
    }
    
    // Parse Processing (active)
    if let Some(section) = html.find("Processing") {
        let rest = &html[section..];
        if let Some(val_start) = rest.find("metric-value\"") {
            let val_rest = &rest[val_start..];
            if let Some(gt_pos) = val_rest.find('>') {
                let inner = &val_rest[gt_pos + 1..];
                if let Some(lt_pos) = inner.find('<') {
                    stats.active = inner[..lt_pos].trim().parse().unwrap_or(0);
                }
            }
        }
    }
    
    // Parse Processed with subtext
    if let Some(section) = html.find(">Processed<") {
        let rest = &html[section..];
        if let Some(subtext) = rest.find("metric-subtext\"") {
            let sub_rest = &rest[subtext..];
            if let Some(gt_pos) = sub_rest.find('>') {
                let inner = &sub_rest[gt_pos + 1..];
                if let Some(lt_pos) = inner.find('<') {
                    let text = &inner[..lt_pos];
                    // Parse "941 saved · 3 skipped"
                    for part in text.split('·') {
                        let part = part.trim();
                        if part.contains("saved") {
                            if let Some(num) = part.split_whitespace().next() {
                                stats.saved_count = num.parse().unwrap_or(0);
                            }
                        } else if part.contains("skipped") {
                            if let Some(num) = part.split_whitespace().next() {
                                stats.skipped_count = num.parse().unwrap_or(0);
                            }
                        }
                    }
                }
            }
        }
    }
    
    // Parse Failed
    if let Some(section) = html.find(">Failed<") {
        let rest = &html[section..];
        if let Some(val_start) = rest.find("metric-value\"") {
            let val_rest = &rest[val_start..];
            if let Some(gt_pos) = val_rest.find('>') {
                let inner = &val_rest[gt_pos + 1..];
                if let Some(lt_pos) = inner.find('<') {
                    stats.failed = inner[..lt_pos].trim().parse().unwrap_or(0);
                }
            }
        }
    }
    
    // Parse Cancelled
    if let Some(section) = html.find(">Cancelled<") {
        let rest = &html[section..];
        if let Some(val_start) = rest.find("metric-value\"") {
            let val_rest = &rest[val_start..];
            if let Some(gt_pos) = val_rest.find('>') {
                let inner = &val_rest[gt_pos + 1..];
                if let Some(lt_pos) = inner.find('<') {
                    stats.cancelled = inner[..lt_pos].trim().parse().unwrap_or(0);
                }
            }
        }
    }
}

fn extract_metric_value(html: &str, label: &str) -> Option<String> {
    let section = html.find(label)?;
    let rest = &html[section..];
    let val_start = rest.find("metric-value\">")?;
    let val_rest = &rest[val_start + 14..];
    let val_end = val_rest.find('<')?;
    Some(val_rest[..val_end].trim().to_string())
}
