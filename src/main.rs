mod db;
mod handlers;
mod log_watcher;
mod models;
mod session_watcher;
mod sse;
mod web;

use actix_files as fs;
use actix_web::{middleware::Logger, web as aweb, App, HttpServer};
use std::env;
use tracing::{info, Level};
use tracing_subscriber::FmtSubscriber;

/// Application configuration
#[derive(Clone)]
pub struct Config {
    pub host: String,
    pub port: u16,
    pub database_url: String,
    pub api_token: String,
    pub web_user: String,
    pub web_pass: String,
    pub telegram_bot_token: Option<String>,
    pub telegram_chat_id: Option<String>,
}

impl Config {
    fn from_env() -> Self {
        Self {
            host: env::var("HOST").unwrap_or_else(|_| "localhost".to_string()),
            port: env::var("PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(3002),
            database_url: env::var("DATABASE_URL")
                .unwrap_or_else(|_| "sqlite:data/watchtower.db".to_string()),
            api_token: env::var("WATCHTOWER_API_TOKEN").unwrap_or_default(),
            web_user: env::var("WATCHTOWER_USER").unwrap_or_default(),
            web_pass: env::var("WATCHTOWER_PASS").unwrap_or_default(),
            telegram_bot_token: env::var("TELEGRAM_BOT_TOKEN").ok(),
            telegram_chat_id: env::var("TELEGRAM_CHAT_ID").ok(),
        }
    }
}

#[actix_web::main]
async fn main() -> anyhow::Result<()> {
    // Initialize logging
    FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .with_target(false)
        .compact()
        .init();

    info!("=== Watchtower Starting ===");
    info!("Version: {}", env!("CARGO_PKG_VERSION"));

    // Load configuration
    let config = Config::from_env();
    info!(port = config.port, db = %config.database_url, "Configuration loaded");

    // Ensure data directory exists
    if config.database_url.starts_with("sqlite:") {
        let db_path = config.database_url.strip_prefix("sqlite:").unwrap();
        if let Some(parent) = std::path::Path::new(db_path).parent() {
            std::fs::create_dir_all(parent)?;
        }
    }

    // Initialize database with migrations
    info!("Initializing database...");
    let pool = db::init_pool(&config.database_url).await?;
    info!("Database initialized successfully");

    // Create SSE broadcaster
    let broadcaster = sse::new_broadcaster();
    info!("SSE broadcaster initialized");

    // Start log watcher
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    log_watcher::start_log_watcher(pool.clone(), broadcaster.clone(), shutdown_rx.clone());
    info!("Log watcher started");

    // Start session JSONL watcher (for tool results)
    session_watcher::start_session_watcher(pool.clone(), broadcaster.clone(), shutdown_rx);
    info!("Session watcher started");

    // Clone for HTTP server
    let config_clone = config.clone();
    let pool_clone = pool.clone();
    let broadcaster_clone = broadcaster.clone();

    info!(bind = %format!("0.0.0.0:{}", config.port), "Starting HTTP server");

    // Start HTTP server
    HttpServer::new(move || {
        App::new()
            .app_data(aweb::Data::new(pool_clone.clone()))
            .app_data(aweb::Data::new(config_clone.clone()))
            .app_data(aweb::Data::new(broadcaster_clone.clone()))
            .wrap(Logger::new("%a %r %s %b %Dms"))
            // Health check
            .service(handlers::health_check)
            // SSE Stream
            .service(handlers::events_stream)
            // API routes - Events
            .service(handlers::list_events)
            .service(handlers::create_event)
            // API routes - Tasks
            .service(handlers::list_tasks)
            .service(handlers::create_task)
            .service(handlers::get_task)
            .service(handlers::update_task)
            .service(handlers::delete_task)
            .service(handlers::add_task_comment)
            .service(handlers::get_task_comments)
            .service(handlers::get_task_history)
            .service(handlers::get_task_events)
            // API routes - Sessions
            .service(handlers::list_sessions)
            .service(handlers::create_session)
            .service(handlers::get_session)
            // API routes - Cron
            .service(handlers::list_cron_jobs)
            .service(handlers::sync_cron_jobs)
            .service(handlers::update_cron_job)
            .service(handlers::run_cron_job)
            // API routes - Usage
            .service(handlers::get_usage)
            .service(handlers::report_usage)
            // API routes - Search & Stats
            .service(handlers::search_api)
            .service(handlers::get_daily_costs)
            .service(handlers::get_agent_status)
            .service(handlers::get_activity_summary)
            // API routes - Admin
            .service(handlers::clear_events)
            .service(handlers::reset_database)
            // Web UI routes
            .service(web::index)
            .service(web::feed_page)
            .service(web::board_page)
            .service(web::costs_page)
            .service(web::cron_page)
            .service(web::sessions_page)
            .service(web::session_detail_page)
            .service(web::settings_page)
            .service(web::favicon)
            // HTMX partials
            .service(web::events_partial)
            .service(web::sessions_partial)
            .service(web::task_detail_partial)
            .service(web::tasks_partial)
            .service(web::costs_chart_partial)
            .service(web::cron_history_partial)
            .service(web::board_swimlane_partial)
            // Static files
            .service(fs::Files::new("/static", "static").show_files_listing())
    })
    .bind(("0.0.0.0", config.port))?
    .run()
    .await?;

    // Signal shutdown to log watcher
    let _ = shutdown_tx.send(true);

    info!("Watchtower shutdown complete");
    Ok(())
}
