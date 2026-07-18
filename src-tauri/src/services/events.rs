use serde_json::Value;

/// Transport-agnostic backend→frontend event channel.
///
/// Command cores emit UI events (`maintenance-status`, `export-progress`)
/// through this trait instead of a `tauri::AppHandle`, so the same logic
/// serves both the Tauri shell (which forwards to the webview) and the
/// headless HTTP shell (which forwards to SSE subscribers).
pub trait EventBus: Send + Sync {
    fn emit(&self, event: &str, payload: Value);
}

/// Drops every event. Used by tests and contexts with no UI attached.
pub struct NullEventBus;

impl EventBus for NullEventBus {
    fn emit(&self, _event: &str, _payload: Value) {}
}
