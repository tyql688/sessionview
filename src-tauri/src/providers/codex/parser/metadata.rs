//! Canonical session metadata and copied-parent context boundaries.

use serde_json::Value;

use crate::provider::util::parse_rfc3339_timestamp;

use super::{CodexLine, CodexScanAccum};

impl CodexScanAccum {
    pub(super) fn handle_session_meta(&mut self, entry: &CodexLine, payload: &Value) {
        // Validated segment headers carry canonical identity and metadata.
        let is_segment_header = payload.get("id").and_then(Value::as_str).is_some_and(|id| {
            self.history_segment_headers
                .remove(&(id.to_string(), entry.ordinal.unwrap_or(0)))
        });
        if self.session_id.is_some() && is_segment_header {
            // Cumulative usage belongs to the retained logical history and
            // continues across physical segment headers.
            self.begin_usage_turn(None);
            self.replay_last_timestamp_ms = None;
            self.replay_usage_skip = false;
            self.skipping_fork_context = false;
            self.is_sidechain = false;
            self.parent_id = None;
            self.agent_nickname = None;
            self.cwd = None;
            self.cc_version = None;
            self.model_provider = None;
            self.git_branch = None;
            self.current_model = None;
        } else if self.session_id.is_some() {
            // 2nd session_meta = start of forked parent context
            if self.is_sidechain {
                self.skipping_fork_context = true;
                self.replay_usage_skip = true;
            }
            return;
        }
        if let Some(id) = payload.get("id").and_then(|v| v.as_str()) {
            self.session_id = Some(id.to_string());
            self.segment_owns_usage = self
                .owned_session_id
                .as_deref()
                .is_none_or(|owner| owner == id);
        }
        if let Some(c) = payload.get("cwd").and_then(|v| v.as_str()) {
            self.cwd = Some(c.to_string());
        }
        if let Some(v) = payload.get("cli_version").and_then(|v| v.as_str())
            && !v.is_empty()
        {
            self.cc_version = Some(v.to_string());
        }
        if let Some(m) = payload.get("model_provider").and_then(|v| v.as_str())
            && !m.is_empty()
        {
            self.model_provider = Some(m.to_string());
        }
        if let Some(b) = payload
            .get("git")
            .and_then(|g| g.get("branch"))
            .and_then(|v| v.as_str())
            && !b.is_empty()
            && b != "HEAD"
        {
            self.git_branch = Some(b.to_string());
        }
        // A fresh spawned agent has no inherited usage. Only an explicit fork
        // or the second session_meta above proves that a parent replay exists.
        if payload
            .get("forked_from_id")
            .and_then(Value::as_str)
            .is_some_and(|id| !id.is_empty())
            && payload.get("history_base").is_none_or(Value::is_null)
            && self.usage_start_ordinal.is_none()
        {
            self.replay_usage_skip = true;
        }
        // Detect subagent sessions: source.subagent.thread_spawn
        if let Some(spawn) = payload
            .get("source")
            .and_then(|s| s.get("subagent"))
            .and_then(|a| a.get("thread_spawn"))
        {
            self.is_sidechain = true;
            self.parent_id = spawn
                .get("parent_thread_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            self.agent_nickname = payload
                .get("agent_nickname")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let sub_ts = parse_rfc3339_timestamp(
                payload
                    .get("timestamp")
                    .and_then(|v| v.as_str())
                    .or(entry.timestamp.as_deref()),
            );
            if sub_ts > 0 {
                self.subagent_start_seconds = Some(sub_ts);
            }
        } else if matches!(
            payload.get("thread_source").and_then(Value::as_str),
            Some("subagent" | "guardian_review")
        ) {
            self.is_sidechain = true;
            self.parent_id = payload
                .get("parent_thread_id")
                .and_then(Value::as_str)
                .map(str::to_string);
            self.agent_nickname = payload
                .get("agent_nickname")
                .and_then(Value::as_str)
                .map(str::to_string);
        } else if self.parent_id.is_none() {
            // Regular forks (source: "vscode" etc.) also carry
            // a `forked_from_id` we can use as the parent. We
            // intentionally leave is_sidechain=false so the
            // forked session shows in the main list, just with
            // provenance back to its origin.
            if let Some(id) = payload
                .get("forked_from_id")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
            {
                self.parent_id = Some(id.to_string());
            }
        }
    }
}
