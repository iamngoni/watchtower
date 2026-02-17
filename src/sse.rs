use crate::models::SseMessage;
use actix_web::web::Bytes;
use futures::Stream;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use tracing::debug;

const CHANNEL_CAPACITY: usize = 100;

/// SSE broadcast state
pub struct SseBroadcaster {
    sender: broadcast::Sender<SseMessage>,
}

impl SseBroadcaster {
    pub fn new() -> Self {
        let (sender, _) = broadcast::channel(CHANNEL_CAPACITY);
        Self { sender }
    }

    /// Broadcast a message to all connected clients
    pub fn broadcast(&self, event: &str, data: serde_json::Value) {
        let msg = SseMessage {
            event: event.to_string(),
            data,
            raw_html: None,
        };
        // Ignore errors - means no subscribers
        let _ = self.sender.send(msg);
        debug!(event = %event, "Broadcast SSE message");
    }

    /// Broadcast a pre-rendered HTML fragment (for HTMX sse-swap)
    pub fn broadcast_html(&self, event: &str, html: String) {
        let msg = SseMessage {
            event: event.to_string(),
            data: serde_json::Value::Null,
            raw_html: Some(html),
        };
        let _ = self.sender.send(msg);
        debug!(event = %event, "Broadcast SSE HTML message");
    }

    /// Subscribe to the broadcast channel
    pub fn subscribe(&self) -> SseClient {
        SseClient::new(self.sender.subscribe())
    }
}

impl Default for SseBroadcaster {
    fn default() -> Self {
        Self::new()
    }
}

/// SSE client stream
pub struct SseClient {
    inner: BroadcastStream<SseMessage>,
}

impl SseClient {
    pub fn new(receiver: broadcast::Receiver<SseMessage>) -> Self {
        Self {
            inner: BroadcastStream::new(receiver),
        }
    }
}

impl Stream for SseClient {
    type Item = Result<Bytes, actix_web::error::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match Pin::new(&mut self.inner).poll_next(cx) {
            Poll::Ready(Some(Ok(msg))) => {
                let payload = if let Some(html) = &msg.raw_html {
                    html.clone()
                } else {
                    serde_json::to_string(&msg.data).unwrap_or_else(|_| "{}".to_string())
                };
                // SSE spec: multi-line data needs each line prefixed with "data: "
                let data_lines: String = payload.lines()
                    .map(|line| format!("data: {}", line))
                    .collect::<Vec<_>>()
                    .join("\n");
                let data = format!("event: {}\n{}\n\n", msg.event, data_lines);
                Poll::Ready(Some(Ok(Bytes::from(data))))
            }
            Poll::Ready(Some(Err(_))) => {
                // Skip lagged messages and continue
                cx.waker().wake_by_ref();
                Poll::Pending
            }
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

/// Wrapper type for web::Data
pub type Broadcaster = Arc<SseBroadcaster>;

/// Create a new broadcaster
pub fn new_broadcaster() -> Broadcaster {
    Arc::new(SseBroadcaster::new())
}
