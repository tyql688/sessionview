use serde_json::Value;
use tokio::sync::broadcast;

use crate::services::EventBus;

/// One backend event as delivered to SSE subscribers.
#[derive(Clone, Debug)]
pub struct BackendEvent {
    pub name: String,
    pub payload: Value,
}

/// Fan-out event bus for the headless shell: command cores emit into a
/// broadcast channel and every connected `/api/events` SSE stream receives
/// a copy. Emitting with zero subscribers is a no-op, mirroring how Tauri
/// emits are fire-and-forget.
pub struct BroadcastEventBus {
    tx: broadcast::Sender<BackendEvent>,
}

impl BroadcastEventBus {
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self { tx }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<BackendEvent> {
        self.tx.subscribe()
    }
}

impl EventBus for BroadcastEventBus {
    fn emit(&self, event: &str, payload: Value) {
        let _ = self.tx.send(BackendEvent {
            name: event.to_string(),
            payload,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emit_without_subscribers_is_a_noop() {
        let bus = BroadcastEventBus::new(4);
        bus.emit(
            "maintenance-status",
            serde_json::json!({"phase": "started"}),
        );
    }

    #[test]
    fn subscribers_receive_emitted_events() {
        let bus = BroadcastEventBus::new(4);
        let mut rx = bus.subscribe();
        bus.emit(
            "export-progress",
            serde_json::json!({"current": 1, "total": 2}),
        );
        let event = rx.try_recv().unwrap();
        assert_eq!(event.name, "export-progress");
        assert_eq!(event.payload["total"], 2);
    }
}
