//! OpenClaw Gateway WebSocket Client
//!
//! Connects to the OpenClaw Gateway via WebSocket, handles authentication
//! with Ed25519 device identity signing, and provides API methods for
//! interacting with the gateway.

use crate::models::CreateEvent;
use crate::sse::Broadcaster;
use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use ed25519_dalek::{SecretKey, Signer, SigningKey};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot, watch, Mutex, RwLock};
use tokio::time::{sleep, timeout};
use tokio_tungstenite::{
    connect_async, tungstenite::protocol::Message, MaybeTlsStream, WebSocketStream,
};
use tracing::{debug, error, info, warn};

/// Gateway configuration
#[derive(Clone, Debug)]
pub struct GatewayConfig {
    pub url: String,
    pub password: String,
    pub device_id: String,
    pub private_key: SecretKey,
    pub public_key_b64: String,
    pub auth_token: String,
}

impl GatewayConfig {
    /// Load configuration from environment variables and OpenClaw files
    pub fn load() -> Result<Self> {
        let url = std::env::var("OPENCLAW_GATEWAY_URL")
            .unwrap_or_else(|_| "ws://127.0.0.1:18789".to_string());

        let home = std::env::var("HOME").context("HOME not set")?;
        let openclaw_dir = PathBuf::from(&home).join(".openclaw");

        // Load gateway password from openclaw.json
        let password = std::env::var("OPENCLAW_GATEWAY_PASSWORD").unwrap_or_else(|_| {
            let config_path = openclaw_dir.join("openclaw.json");
            if let Ok(content) = std::fs::read_to_string(&config_path) {
                if let Ok(json) = serde_json::from_str::<Value>(&content) {
                    if let Some(pwd) = json
                        .get("gateway")
                        .and_then(|g| g.get("auth"))
                        .and_then(|a| a.get("password"))
                        .and_then(|p| p.as_str())
                    {
                        return pwd.to_string();
                    }
                }
            }
            String::new()
        });

        // Load device identity from device.json
        let device_path = openclaw_dir.join("identity/device.json");
        let device_json: DeviceJson = serde_json::from_str(
            &std::fs::read_to_string(&device_path)
                .context("Failed to read device.json")?,
        )
        .context("Failed to parse device.json")?;

        // Load auth token from device-auth.json
        let auth_path = openclaw_dir.join("identity/device-auth.json");
        let auth_json: DeviceAuthJson = serde_json::from_str(
            &std::fs::read_to_string(&auth_path)
                .context("Failed to read device-auth.json")?,
        )
        .context("Failed to parse device-auth.json")?;

        let auth_token = auth_json
            .tokens
            .get("operator")
            .map(|t| t.token.clone())
            .unwrap_or_default();

        // Parse the private key from PEM format
        let private_key = parse_ed25519_private_key(&device_json.private_key_pem)?;

        // Derive public key and encode it
        let signing_key = SigningKey::from_bytes(&private_key);
        let public_key = signing_key.verifying_key();
        let public_key_b64 = URL_SAFE_NO_PAD.encode(public_key.as_bytes());

        Ok(Self {
            url,
            password,
            device_id: device_json.device_id,
            private_key,
            public_key_b64,
            auth_token,
        })
    }
}

#[derive(Debug, Deserialize)]
struct DeviceJson {
    #[serde(rename = "deviceId")]
    device_id: String,
    #[serde(rename = "privateKeyPem")]
    private_key_pem: String,
}

#[derive(Debug, Deserialize)]
struct DeviceAuthJson {
    tokens: HashMap<String, TokenInfo>,
}

#[derive(Debug, Deserialize)]
struct TokenInfo {
    token: String,
}

/// Parse an Ed25519 private key from PEM format
fn parse_ed25519_private_key(pem: &str) -> Result<SecretKey> {
    // Remove PEM headers and decode base64
    let b64 = pem
        .lines()
        .filter(|line| !line.starts_with("-----"))
        .collect::<String>();

    let der = base64::engine::general_purpose::STANDARD
        .decode(&b64)
        .context("Failed to decode private key base64")?;

    // PKCS#8 Ed25519 private key structure:
    // 0x30 len 0x02 0x01 0x00 0x30 0x05 0x06 0x03 0x2b 0x65 0x70 0x04 0x22 0x04 0x20 [32 bytes key]
    // The actual 32-byte key is at the end after the ASN.1 wrapper
    if der.len() < 48 {
        return Err(anyhow!("Private key DER too short"));
    }

    // The 32-byte private key is at offset 16 in the DER encoding
    let key_bytes: [u8; 32] = der[16..48]
        .try_into()
        .context("Failed to extract 32-byte key")?;

    Ok(key_bytes)
}

/// Pending request tracker
type PendingMap = Arc<Mutex<HashMap<String, oneshot::Sender<Result<Value>>>>>;

/// Gateway client state
enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
}

/// Commands to send to the WebSocket writer task
enum WsCommand {
    Send(String),
    Shutdown,
}

/// Gateway WebSocket client
pub struct GatewayClient {
    config: GatewayConfig,
    pool: SqlitePool,
    broadcaster: Broadcaster,
    state: Arc<RwLock<ConnectionState>>,
    pending: PendingMap,
    command_tx: Arc<Mutex<Option<mpsc::Sender<WsCommand>>>>,
    shutdown: watch::Sender<bool>,
}

impl GatewayClient {
    /// Create a new gateway client
    pub fn new(
        config: GatewayConfig,
        pool: SqlitePool,
        broadcaster: Broadcaster,
    ) -> Arc<Self> {
        let (shutdown_tx, _) = watch::channel(false);

        Arc::new(Self {
            config,
            pool,
            broadcaster,
            state: Arc::new(RwLock::new(ConnectionState::Disconnected)),
            pending: Arc::new(Mutex::new(HashMap::new())),
            command_tx: Arc::new(Mutex::new(None)),
            shutdown: shutdown_tx,
        })
    }

    /// Start the gateway client (connects in background, handles reconnection)
    pub fn start(self: &Arc<Self>, mut shutdown_rx: watch::Receiver<bool>) {
        let client = Arc::clone(self);
        tokio::spawn(async move {
            let mut backoff = Duration::from_secs(1);
            let max_backoff = Duration::from_secs(60);

            loop {
                // Check shutdown
                if *shutdown_rx.borrow() {
                    info!("Gateway client shutdown requested");
                    break;
                }

                info!(url = %client.config.url, "Connecting to OpenClaw Gateway...");
                *client.state.write().await = ConnectionState::Connecting;

                match client.connect_and_run().await {
                    Ok(()) => {
                        info!("Gateway connection closed gracefully");
                        backoff = Duration::from_secs(1); // Reset backoff
                    }
                    Err(e) => {
                        warn!("Gateway connection error: {}", e);
                    }
                }

                *client.state.write().await = ConnectionState::Disconnected;

                // Wait before reconnecting
                info!("Reconnecting in {:?}...", backoff);
                tokio::select! {
                    _ = sleep(backoff) => {}
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            break;
                        }
                    }
                }

                // Exponential backoff
                backoff = std::cmp::min(backoff * 2, max_backoff);
            }

            info!("Gateway client stopped");
        });
    }

    /// Connect to the gateway and run the message loop
    async fn connect_and_run(self: &Arc<Self>) -> Result<()> {
        let (ws_stream, _) = connect_async(&self.config.url)
            .await
            .context("Failed to connect to gateway")?;

        info!("WebSocket connected, awaiting challenge...");

        let (write, read) = ws_stream.split();

        // Create command channel
        let (cmd_tx, cmd_rx) = mpsc::channel::<WsCommand>(100);
        *self.command_tx.lock().await = Some(cmd_tx.clone());

        // Spawn writer task
        let writer_handle = tokio::spawn(Self::writer_task(write, cmd_rx));

        // Run reader loop (handles challenge, auth, and messages)
        let result = self.reader_loop(read, cmd_tx.clone()).await;

        // Signal writer to shutdown
        let _ = cmd_tx.send(WsCommand::Shutdown).await;
        let _ = writer_handle.await;

        *self.command_tx.lock().await = None;
        result
    }

    /// Writer task - sends messages from command channel
    async fn writer_task(
        mut write: futures_util::stream::SplitSink<
            WebSocketStream<MaybeTlsStream<TcpStream>>,
            Message,
        >,
        mut cmd_rx: mpsc::Receiver<WsCommand>,
    ) {
        while let Some(cmd) = cmd_rx.recv().await {
            match cmd {
                WsCommand::Send(msg) => {
                    if let Err(e) = write.send(Message::Text(msg.into())).await {
                        error!("Failed to send WebSocket message: {}", e);
                        break;
                    }
                }
                WsCommand::Shutdown => {
                    debug!("Writer task shutdown");
                    break;
                }
            }
        }
    }

    /// Reader loop - processes incoming messages
    async fn reader_loop(
        self: &Arc<Self>,
        mut read: futures_util::stream::SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>,
        cmd_tx: mpsc::Sender<WsCommand>,
    ) -> Result<()> {
        // First message should be the challenge
        let challenge_msg = timeout(Duration::from_secs(10), read.next())
            .await
            .context("Timeout waiting for challenge")?
            .ok_or_else(|| anyhow!("Connection closed before challenge"))?
            .context("Failed to read challenge")?;

        let nonce = self.handle_challenge(challenge_msg)?;
        debug!("Received challenge, nonce: {}", nonce);

        // Send connect request
        let connect_req = self.build_connect_request(&nonce)?;
        cmd_tx
            .send(WsCommand::Send(connect_req))
            .await
            .context("Failed to send connect request")?;

        // Wait for connect response
        let connect_res = timeout(Duration::from_secs(10), read.next())
            .await
            .context("Timeout waiting for connect response")?
            .ok_or_else(|| anyhow!("Connection closed before connect response"))?
            .context("Failed to read connect response")?;

        self.handle_connect_response(connect_res)?;

        info!("Gateway authenticated successfully!");
        *self.state.write().await = ConnectionState::Connected;

        // Main message loop
        while let Some(msg_result) = read.next().await {
            match msg_result {
                Ok(Message::Text(text)) => {
                    if let Err(e) = self.handle_message(&text).await {
                        warn!("Error handling message: {}", e);
                    }
                }
                Ok(Message::Ping(_)) => {
                    // Ping is handled automatically by tungstenite
                }
                Ok(Message::Close(_)) => {
                    info!("Gateway sent close frame");
                    break;
                }
                Err(e) => {
                    error!("WebSocket read error: {}", e);
                    break;
                }
                _ => {}
            }
        }

        Ok(())
    }

    /// Handle the challenge event
    fn handle_challenge(&self, msg: Message) -> Result<String> {
        let text = match msg {
            Message::Text(t) => t.to_string(),
            _ => return Err(anyhow!("Expected text message for challenge")),
        };

        let json: Value = serde_json::from_str(&text).context("Failed to parse challenge")?;

        if json.get("type").and_then(|t| t.as_str()) != Some("event")
            || json.get("event").and_then(|e| e.as_str()) != Some("connect.challenge")
        {
            return Err(anyhow!("Expected connect.challenge event"));
        }

        let nonce = json
            .get("payload")
            .and_then(|p| p.get("nonce"))
            .and_then(|n| n.as_str())
            .ok_or_else(|| anyhow!("Missing nonce in challenge"))?
            .to_string();

        Ok(nonce)
    }

    /// Build the connect request with device signature
    fn build_connect_request(&self, nonce: &str) -> Result<String> {
        let signed_at = chrono::Utc::now().timestamp_millis();
        let client_id = "cli";
        let client_mode = "cli";
        let role = "operator";
        let scopes = "operator.admin";

        // Build the message to sign:
        // v2|<deviceId>|<clientId>|<clientMode>|<role>|<scopes>|<signedAtMs>|<token>|<nonce>
        let sign_message = format!(
            "v2|{}|{}|{}|{}|{}|{}|{}|{}",
            self.config.device_id,
            client_id,
            client_mode,
            role,
            scopes,
            signed_at,
            self.config.auth_token,
            nonce
        );

        debug!("Signing message: {}", sign_message);

        // Sign with Ed25519
        let signing_key = SigningKey::from_bytes(&self.config.private_key);
        let signature = signing_key.sign(sign_message.as_bytes());
        let signature_b64 = URL_SAFE_NO_PAD.encode(signature.to_bytes());

        let request = json!({
            "type": "req",
            "id": "connect-1",
            "method": "connect",
            "params": {
                "minProtocol": 3,
                "maxProtocol": 3,
                "client": {
                    "id": client_id,
                    "version": env!("CARGO_PKG_VERSION"),
                    "platform": std::env::consts::OS,
                    "mode": client_mode,
                    "displayName": "Watchtower Dashboard"
                },
                "auth": {
                    "token": self.config.auth_token,
                    "password": self.config.password
                },
                "role": role,
                "scopes": ["operator.admin"],
                "device": {
                    "id": self.config.device_id,
                    "publicKey": self.config.public_key_b64,
                    "signature": signature_b64,
                    "signedAt": signed_at,
                    "nonce": nonce
                },
                "caps": ["tool-events"]
            }
        });

        Ok(serde_json::to_string(&request)?)
    }

    /// Handle the connect response
    fn handle_connect_response(&self, msg: Message) -> Result<()> {
        let text = match msg {
            Message::Text(t) => t.to_string(),
            _ => return Err(anyhow!("Expected text message for connect response")),
        };

        let json: Value =
            serde_json::from_str(&text).context("Failed to parse connect response")?;

        if json.get("type").and_then(|t| t.as_str()) != Some("res") {
            return Err(anyhow!("Expected response type"));
        }

        let ok = json.get("ok").and_then(|o| o.as_bool()).unwrap_or(false);
        if !ok {
            let error = json
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or("Unknown error");
            return Err(anyhow!("Connect failed: {}", error));
        }

        Ok(())
    }

    /// Handle an incoming message (event or response)
    async fn handle_message(&self, text: &str) -> Result<()> {
        let json: Value = serde_json::from_str(text)?;
        let msg_type = json.get("type").and_then(|t| t.as_str()).unwrap_or("");

        match msg_type {
            "event" => self.handle_event(&json).await,
            "res" => self.handle_response(&json).await,
            _ => {
                debug!("Unknown message type: {}", msg_type);
                Ok(())
            }
        }
    }

    /// Handle an event from the gateway
    async fn handle_event(&self, json: &Value) -> Result<()> {
        let event_name = json
            .get("event")
            .and_then(|e| e.as_str())
            .unwrap_or("unknown");
        let payload = json.get("payload").cloned().unwrap_or(Value::Null);

        // Skip tick events (keepalive)
        if event_name == "tick" {
            return Ok(());
        }

        debug!("Gateway event: {} {:?}", event_name, payload);

        // Convert gateway events to our event format and broadcast
        if let Some(event) = self.convert_gateway_event(event_name, &payload) {
            // Insert into database (with dedup)
            match crate::db::insert_event_dedup(&self.pool, &event).await {
                Ok(Some(evt)) => {
                    // Broadcast via SSE
                    if let Some(html) = crate::web::render_event_html(&evt) {
                        self.broadcaster.broadcast_html("event", html);
                    } else {
                        self.broadcaster
                            .broadcast("event", serde_json::to_value(&evt)?);
                    }
                    debug!(event_type = %evt.event_type, "Gateway event broadcast");
                }
                Ok(None) => {
                    // Duplicate, skip
                }
                Err(e) => {
                    warn!("Failed to insert gateway event: {}", e);
                }
            }
        }

        Ok(())
    }

    /// Convert a gateway event to our CreateEvent format
    fn convert_gateway_event(&self, event_name: &str, payload: &Value) -> Option<CreateEvent> {
        let (event_type, summary, detail) = match event_name {
            "agent" => {
                let state = payload
                    .get("state")
                    .and_then(|s| s.as_str())
                    .unwrap_or("unknown");
                let session_key = payload
                    .get("sessionKey")
                    .and_then(|s| s.as_str())
                    .unwrap_or("");

                let emoji = if session_key.contains(":cron:") {
                    "🕐"
                } else if session_key.contains(":subagent:") {
                    "🤖"
                } else {
                    "👤"
                };

                (
                    "agent".to_string(),
                    format!("{} Agent state: {}", emoji, state),
                    Some(format!("Session: {}", session_key)),
                )
            }
            "chat" => {
                let role = payload
                    .get("role")
                    .and_then(|r| r.as_str())
                    .unwrap_or("unknown");
                (
                    "message".to_string(),
                    format!("💬 Chat message ({})", role),
                    payload.get("content").and_then(|c| c.as_str()).map(|s| {
                        if s.len() > 100 {
                            format!("{}...", &s[..100])
                        } else {
                            s.to_string()
                        }
                    }),
                )
            }
            "cron" => {
                let action = payload
                    .get("action")
                    .and_then(|a| a.as_str())
                    .unwrap_or("unknown");
                let job_name = payload
                    .get("jobName")
                    .and_then(|n| n.as_str())
                    .unwrap_or("unknown");
                (
                    "cron".to_string(),
                    format!("🕐 Cron {}: {}", action, job_name),
                    Some(serde_json::to_string_pretty(payload).unwrap_or_default()),
                )
            }
            "health" => {
                let status = payload
                    .get("status")
                    .and_then(|s| s.as_str())
                    .unwrap_or("unknown");
                (
                    "info".to_string(),
                    format!("💚 Health: {}", status),
                    None,
                )
            }
            "presence" => {
                let session_key = payload
                    .get("sessionKey")
                    .and_then(|s| s.as_str())
                    .unwrap_or("");
                let state = payload
                    .get("state")
                    .and_then(|s| s.as_str())
                    .unwrap_or("unknown");
                (
                    "agent".to_string(),
                    format!("👁️ Presence: {} ({})", session_key, state),
                    None,
                )
            }
            _ => {
                // Unknown event type, still log it
                (
                    "info".to_string(),
                    format!("📡 Gateway: {}", event_name),
                    Some(serde_json::to_string_pretty(payload).unwrap_or_default()),
                )
            }
        };

        Some(CreateEvent {
            event_type,
            summary,
            detail,
            session_id: payload
                .get("sessionKey")
                .and_then(|s| s.as_str())
                .map(|s| s.to_string()),
            task_id: None,
            metadata: Some(json!({
                "source": "gateway",
                "event": event_name,
                "payload": payload
            })),
        })
    }

    /// Handle a response to a request
    async fn handle_response(&self, json: &Value) -> Result<()> {
        let id = json
            .get("id")
            .and_then(|i| i.as_str())
            .unwrap_or("")
            .to_string();

        let mut pending = self.pending.lock().await;
        if let Some(tx) = pending.remove(&id) {
            let ok = json.get("ok").and_then(|o| o.as_bool()).unwrap_or(false);
            let result = if ok {
                Ok(json.get("payload").cloned().unwrap_or(Value::Null))
            } else {
                let error = json
                    .get("error")
                    .and_then(|e| e.get("message"))
                    .and_then(|m| m.as_str())
                    .unwrap_or("Unknown error");
                Err(anyhow!("{}", error))
            };
            let _ = tx.send(result);
        }

        Ok(())
    }

    /// Send a request and wait for response
    async fn request(&self, method: &str, params: Value) -> Result<Value> {
        let id = uuid::Uuid::new_v4().to_string();

        let request = json!({
            "type": "req",
            "id": &id,
            "method": method,
            "params": params
        });

        // Set up response channel
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().await;
            pending.insert(id.clone(), tx);
        }

        // Send request
        let cmd_tx = self.command_tx.lock().await;
        let cmd_tx = cmd_tx
            .as_ref()
            .ok_or_else(|| anyhow!("Not connected to gateway"))?;
        cmd_tx
            .send(WsCommand::Send(serde_json::to_string(&request)?))
            .await
            .context("Failed to send request")?;

        // Wait for response with timeout
        let result = timeout(Duration::from_secs(30), rx)
            .await
            .context("Request timeout")?
            .context("Response channel closed")?;

        result
    }

    /// Check if connected to gateway
    pub async fn is_connected(&self) -> bool {
        matches!(*self.state.read().await, ConnectionState::Connected)
    }

    // ========================================================================
    // Public API Methods
    // ========================================================================

    /// List sessions from gateway
    pub async fn list_sessions(&self) -> Result<Value> {
        self.request("sessions.list", json!({})).await
    }

    /// Get usage costs from gateway
    pub async fn get_costs(&self) -> Result<Value> {
        self.request("usage.cost", json!({})).await
    }

    /// Get usage status from gateway
    pub async fn get_usage_status(&self) -> Result<Value> {
        self.request("usage.status", json!({})).await
    }

    /// List cron jobs from gateway
    pub async fn list_cron_jobs(&self) -> Result<Value> {
        self.request("cron.list", json!({})).await
    }

    /// Run a cron job
    pub async fn run_cron_job(&self, job_id: &str) -> Result<Value> {
        self.request("cron.run", json!({ "jobId": job_id })).await
    }

    /// Send a message to a session
    pub async fn send_message(&self, session_key: &str, message: &str) -> Result<Value> {
        self.request(
            "chat.send",
            json!({
                "sessionKey": session_key,
                "message": message
            }),
        )
        .await
    }

    /// Abort a session
    pub async fn abort_session(&self, session_key: &str) -> Result<Value> {
        self.request("chat.abort", json!({ "sessionKey": session_key }))
            .await
    }

    /// Get gateway status
    pub async fn get_status(&self) -> Result<Value> {
        self.request("status", json!({})).await
    }
}

/// Global gateway client instance
static GATEWAY_CLIENT: tokio::sync::OnceCell<Arc<GatewayClient>> = tokio::sync::OnceCell::const_new();

/// Initialize the global gateway client
pub fn init_gateway_client(
    pool: SqlitePool,
    broadcaster: Broadcaster,
    shutdown_rx: watch::Receiver<bool>,
) -> Result<Arc<GatewayClient>> {
    let config = match GatewayConfig::load() {
        Ok(c) => c,
        Err(e) => {
            warn!("Failed to load gateway config, gateway client disabled: {}", e);
            return Err(e);
        }
    };

    info!(
        url = %config.url,
        device_id = %config.device_id,
        "Gateway config loaded"
    );

    let client = GatewayClient::new(config, pool, broadcaster);
    client.start(shutdown_rx);

    let _ = GATEWAY_CLIENT.set(client.clone());

    Ok(client)
}

/// Get the global gateway client (if initialized)
pub fn get_gateway_client() -> Option<Arc<GatewayClient>> {
    GATEWAY_CLIENT.get().cloned()
}
