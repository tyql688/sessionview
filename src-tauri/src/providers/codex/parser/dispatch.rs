//! Record dispatch and inherited-context boundaries.

use std::path::Path;

use serde_json::Value;

use super::{CodexLine, CodexScanAccum};

impl CodexScanAccum {
    pub(super) fn scan_line(&mut self, entry: &CodexLine, path: &Path) {
        if let Some(ref ts) = entry.timestamp {
            if self.first_timestamp.is_none() {
                self.first_timestamp = Some(ts.clone());
            }
            self.last_timestamp = Some(ts.clone());
        }

        let payload = match entry.payload {
            Some(ref p) => p,
            None => return,
        };

        if entry.line_type == "session_meta" {
            self.handle_session_meta(entry, payload);
            return;
        }
        if self.target_is_subagent && !self.segment_owns_usage {
            // Retained parent snapshots seed the child's cumulative baseline;
            // their messages and usage events stay owned by the parent.
            if entry.line_type == "event_msg"
                && payload.get("type").and_then(Value::as_str) == Some("token_count")
            {
                self.handle_token_count(entry, payload, path);
            }
            return;
        }
        if let Some(start) = self.transcript_start_ordinal {
            let Some(ordinal) = entry.ordinal else {
                log::warn!(
                    "skipping Codex record without required ordinal in '{}' at {:?}",
                    path.display(),
                    entry.timestamp
                );
                self.parse_warning_count = self.parse_warning_count.saturating_add(1);
                return;
            };
            if ordinal < start {
                if entry.line_type == "event_msg"
                    && payload.get("type").and_then(Value::as_str) == Some("token_count")
                {
                    self.handle_token_count(entry, payload, path);
                }
                return;
            }
            self.skipping_fork_context = false;
            self.replay_usage_skip = false;
        }
        if matches!(
            entry.line_type.as_str(),
            "inter_agent_communication_metadata" | "inter_agent_communication"
        ) && payload.get("trigger_turn").and_then(Value::as_bool) == Some(true)
        {
            self.skipping_fork_context = false;
            self.replay_usage_skip = false;
            return;
        }

        // Skip forked parent context in subagent files. Clear the flag on
        // the first subagent-owned `task_started` event (its `started_at`
        // matches the subagent session's creation time). Older transcripts
        // don't carry that marker — fall back to the textual
        // `newly spawned agent` cue still present in their function-call
        // output.
        if self.skipping_fork_context {
            // Usage is deduped by the replay-burst check inside
            // handle_token_count, not by the transcript skip: files whose
            // skip marker never fires must still count their own turns.
            if entry.line_type == "event_msg"
                && payload.get("type").and_then(|v| v.as_str()) == Some("token_count")
            {
                self.handle_token_count(entry, payload, path);
                return;
            }
            // The forked parent context is not this session's transcript,
            // but its turn_context still names the model that every later
            // token_count needs for cost attribution — harvest it without
            // emitting messages.
            if entry.line_type == "turn_context" {
                if let Some(model) = payload
                    .get("model")
                    .and_then(|v| v.as_str())
                    .filter(|model| !model.is_empty())
                {
                    self.current_model = Some(model.to_string());
                    self.models_seen.insert(model.to_string());
                    if self.model.is_none() {
                        self.model = Some(model.to_string());
                    }
                }
                return;
            }
            if entry.line_type == "event_msg"
                && payload.get("type").and_then(|v| v.as_str()) == Some("task_started")
            {
                if let (Some(started_at), Some(sub_sec)) = (
                    payload.get("started_at").and_then(|v| v.as_i64()),
                    self.subagent_start_seconds,
                ) && started_at >= sub_sec
                {
                    self.skipping_fork_context = false;
                    self.replay_usage_skip = false;
                    self.handle_event_msg(entry, payload, path);
                    return;
                }
            } else if entry.line_type == "response_item"
                && payload.get("type").and_then(|v| v.as_str()) == Some("function_call_output")
            {
                let output = payload.get("output").and_then(|v| v.as_str()).unwrap_or("");
                if output.contains("newly spawned agent") {
                    self.skipping_fork_context = false;
                    self.replay_usage_skip = false;
                }
            }
            return;
        }

        match entry.line_type.as_str() {
            "session_meta" => {}
            "compacted" => self.handle_compacted(entry, payload),
            "response_item" => self.handle_response_item(entry, payload, path),
            "turn_context" => self.handle_turn_context(payload),
            "event_msg" => self.handle_event_msg(entry, payload, path),
            "token_usage_record" => self.handle_token_usage_record(entry, payload, path),
            // Environment snapshots and agent-team turn bookkeeping carry no
            // transcript or usage data.
            "world_state" | "inter_agent_communication_metadata" | "inter_agent_communication" => {}
            unknown => {
                log::warn!("skipping unknown Codex record type '{unknown}'");
                self.parse_warning_count = self.parse_warning_count.saturating_add(1);
            }
        }
    }
}
