//! Command Code session provider.
//!
//! Current Command Code stores one append-only v3 transcript per session:
//!
//! ```text
//! ~/.commandcode/projects/<project-slug>/<session-id>.jsonl
//! ~/.commandcode/projects/<project-slug>/<session-id>.meta.json
//! ```
//!
//! The transcript header carries the session id, creation time, and working
//! directory. Subsequent entries form a tree through `id` / `parentId`; the
//! final entry is the active leaf. The sidecar supplies mutable title, model,
//! and git-branch metadata, so its size/mtime participates in incremental
//! freshness. `.checkpoints.jsonl` and `.prompts.jsonl` are sidecars, not
//! conversations. The `agent` / `agent_output` tools run subagents inside the
//! parent process and persist their inputs and final text only in the parent's
//! transcript. SessionView exposes each typed call as a limited inline child
//! keyed by its tool-call id; internal child turns, tools, model, and disjoint
//! usage remain unavailable because Command Code did not persist them.
//! Fork/clone lineage is deliberately not treated as a subagent relationship.

pub(crate) mod parser;
mod types;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rayon::prelude::*;
use walkdir::WalkDir;

use crate::models::Provider;
use crate::provider::{
    LoadedSession, ParsedSession, ProviderError, ScanOutcome, SessionProvider, SourceState,
};

pub(crate) struct Descriptor;

impl crate::provider::ProviderDescriptor for Descriptor {
    fn resume_command(&self, session_id: &str, _variant_name: Option<&str>) -> Option<String> {
        let root_id = session_id
            .split_once(':')
            .map_or(session_id, |(root_id, _)| root_id);
        Some(format!("commandcode --session {root_id}"))
    }

    fn display_key(&self, _variant_name: Option<&str>) -> String {
        "commandcode".into()
    }

    fn sort_order(&self) -> u32 {
        15
    }

    fn color(&self) -> &'static str {
        "#18181b"
    }

    fn cli_command(&self) -> &'static str {
        "commandcode"
    }
}

pub(crate) struct CommandCodeProvider {
    commandcode_home: PathBuf,
}

impl CommandCodeProvider {
    pub(crate) fn new() -> Option<Self> {
        dirs::home_dir().map(|home| Self::with_root(home.join(".commandcode")))
    }

    pub(crate) fn with_root(commandcode_home: PathBuf) -> Self {
        Self { commandcode_home }
    }

    fn projects_dir(&self) -> PathBuf {
        self.commandcode_home.join("projects")
    }

    fn collect_session_files(&self) -> Vec<PathBuf> {
        let root = self.projects_dir();
        if !root.is_dir() {
            return Vec::new();
        }
        let mut files = Vec::new();
        // Command Code catalogs one directory per project slug and session
        // transcripts directly inside it. Keep the same depth boundary so an
        // unrelated nested JSONL file is never mistaken for a conversation.
        for entry in WalkDir::new(&root).min_depth(2).max_depth(2) {
            match entry {
                Ok(entry) if entry.file_type().is_file() && is_transcript_file(entry.path()) => {
                    files.push(entry.into_path());
                }
                Ok(_) => {}
                Err(error) => log::warn!("failed to scan Command Code sessions: {error}"),
            }
        }
        files.sort();
        files
    }
}

fn is_transcript_file(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    name.ends_with(".jsonl")
        && !name.contains(".checkpoints.")
        && !name.contains(".prompts.")
        && !name.contains(".v2.bak")
}

impl SessionProvider for CommandCodeProvider {
    fn provider(&self) -> Provider {
        Provider::CommandCode
    }

    fn source_roots(&self) -> Vec<PathBuf> {
        let root = self.projects_dir();
        if root.is_dir() {
            vec![root]
        } else {
            Vec::new()
        }
    }

    fn scan_all(&self) -> Result<Vec<ParsedSession>, ProviderError> {
        Ok(self
            .collect_session_files()
            .par_iter()
            .flat_map(|path| parser::parse_session_file(path))
            .collect())
    }

    fn scan_incremental(
        &self,
        known: &HashMap<String, SourceState>,
    ) -> Result<ScanOutcome, ProviderError> {
        let mut fresh = Vec::new();
        let mut unchanged_source_paths = Vec::new();
        for file in self.collect_session_files() {
            let path = file.to_string_lossy().to_string();
            match (known.get(&path), parser::source_state(&file)) {
                (Some(known), Some(current))
                    if known.size == current.size && known.mtime == current.mtime =>
                {
                    unchanged_source_paths.push(path);
                }
                _ => fresh.push(file),
            }
        }
        let parsed = fresh
            .par_iter()
            .flat_map(|path| parser::parse_session_file(path))
            .collect();
        Ok(ScanOutcome {
            parsed,
            unchanged_source_paths,
        })
    }

    fn load_messages(
        &self,
        session_id: &str,
        source_path: &str,
    ) -> Result<LoadedSession, ProviderError> {
        let path = PathBuf::from(source_path);
        if !path.is_file() {
            return Err(ProviderError::Parse(format!(
                "Command Code session file not found: {source_path}"
            )));
        }
        let parsed = parser::parse_session_file(&path)
            .into_iter()
            .find(|parsed| parsed.meta.id == session_id)
            .ok_or_else(|| {
                ProviderError::Parse(format!(
                    "session '{session_id}' not found in Command Code transcript '{source_path}'"
                ))
            })?;
        Ok(LoadedSession::from_parsed(parsed))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::provider::ProviderDescriptor;

    #[test]
    fn descriptor_uses_documented_resume_form() {
        let descriptor = Descriptor;
        assert_eq!(
            descriptor.resume_command("11111111-1111-4111-a111-111111111111", None),
            Some("commandcode --session 11111111-1111-4111-a111-111111111111".to_string())
        );
        assert_eq!(
            descriptor.resume_command(
                "11111111-1111-4111-a111-111111111111:tool-synthetic",
                Some("general")
            ),
            Some("commandcode --session 11111111-1111-4111-a111-111111111111".to_string())
        );
        assert_eq!(descriptor.display_key(None), "commandcode");
        assert_eq!(descriptor.sort_order(), 15);
        assert_eq!(descriptor.cli_command(), "commandcode");
    }

    #[test]
    fn collect_session_files_excludes_jsonl_sidecars() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("projects/demo");
        std::fs::create_dir_all(&project).unwrap();
        let session = project.join("11111111-1111-4111-a111-111111111111.jsonl");
        std::fs::write(&session, "").unwrap();
        std::fs::write(
            project.join("11111111-1111-4111-a111-111111111111.checkpoints.jsonl"),
            "",
        )
        .unwrap();
        std::fs::write(
            project.join("11111111-1111-4111-a111-111111111111.prompts.jsonl"),
            "",
        )
        .unwrap();
        std::fs::write(project.join("ignored.checkpoints.backup.jsonl"), "").unwrap();
        std::fs::write(project.join("ignored.prompts.backup.jsonl"), "").unwrap();
        std::fs::write(project.join("ignored.v2.bak.jsonl"), "").unwrap();
        let nested = project.join("nested/22222222-2222-4222-a222-222222222222.jsonl");
        std::fs::create_dir_all(nested.parent().unwrap()).unwrap();
        std::fs::write(nested, "").unwrap();

        let provider = CommandCodeProvider::with_root(root.path().to_path_buf());
        assert_eq!(provider.collect_session_files(), vec![session]);
    }

    #[test]
    fn load_messages_rejects_mismatched_session_id() {
        let root = tempfile::tempdir().unwrap();
        let path = root
            .path()
            .join("projects/demo/11111111-1111-4111-a111-111111111111.jsonl");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let records = [
            json!({
                "type": "session", "version": 3,
                "id": "11111111-1111-4111-a111-111111111111",
                "timestamp": "2026-09-01T10:00:00Z", "cwd": "/tmp/demo"
            }),
            json!({
                "type": "message", "id": "m1", "parentId": null,
                "timestamp": "2026-09-01T10:00:01Z",
                "message": {"role": "user", "content": [{"type": "text", "text": "hello"}]}
            }),
            json!({
                "type": "message", "id": "m2", "parentId": "m1",
                "timestamp": "2026-09-01T10:00:02Z", "model": "model-a",
                "message": {"role": "assistant", "content": [{"type": "text", "text": "hi"}]}
            }),
        ];
        std::fs::write(
            &path,
            format!(
                "{}\n",
                records
                    .iter()
                    .map(serde_json::Value::to_string)
                    .collect::<Vec<_>>()
                    .join("\n")
            ),
        )
        .unwrap();
        let provider = CommandCodeProvider::with_root(root.path().to_path_buf());

        let error = provider
            .load_messages(
                "22222222-2222-4222-a222-222222222222",
                path.to_str().unwrap(),
            )
            .unwrap_err();
        assert!(error.to_string().contains("not found"));
    }

    #[test]
    fn load_messages_selects_inline_subagent_from_shared_transcript() {
        let root = tempfile::tempdir().unwrap();
        let path = root
            .path()
            .join("projects/demo/11111111-1111-4111-a111-111111111111.jsonl");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let records = [
            json!({
                "type": "session", "version": 3,
                "id": "11111111-1111-4111-a111-111111111111",
                "timestamp": "2026-09-01T10:00:00Z", "cwd": "/tmp/demo"
            }),
            json!({
                "type": "message", "id": "m1", "parentId": null,
                "timestamp": "2026-09-01T10:00:01Z",
                "message": {"role": "user", "content": [{"type": "text", "text": "delegate"}]}
            }),
            json!({
                "type": "message", "id": "m2", "parentId": "m1",
                "timestamp": "2026-09-01T10:00:02Z", "model": "model-a",
                "message": {"role": "assistant", "content": [{
                    "type": "tool_use", "id": "tool-child", "name": "agent", "input": {
                        "description": "Child", "prompt": "Return child"
                    }
                }]}
            }),
            json!({
                "type": "message", "id": "m3", "parentId": "m2",
                "timestamp": "2026-09-01T10:00:03Z",
                "message": {"role": "user", "content": [{
                    "type": "tool_result", "tool_use_id": "tool-child",
                    "content": [{"type": "text", "text": "child result"}]
                }]}
            }),
        ];
        std::fs::write(
            &path,
            format!(
                "{}\n",
                records
                    .iter()
                    .map(serde_json::Value::to_string)
                    .collect::<Vec<_>>()
                    .join("\n")
            ),
        )
        .unwrap();
        let provider = CommandCodeProvider::with_root(root.path().to_path_buf());

        let loaded = provider
            .load_messages(
                "11111111-1111-4111-a111-111111111111:tool-child",
                path.to_str().unwrap(),
            )
            .unwrap();

        assert_eq!(loaded.messages.len(), 2);
        assert_eq!(loaded.messages[0].content, "Return child");
        assert_eq!(loaded.messages[1].content, "child result");
    }

    #[test]
    fn incremental_scan_preserves_shared_source_and_reparses_root_with_child() {
        let root = tempfile::tempdir().unwrap();
        let path = root
            .path()
            .join("projects/demo/11111111-1111-4111-a111-111111111111.jsonl");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let records = [
            json!({
                "type": "session", "version": 3,
                "id": "11111111-1111-4111-a111-111111111111",
                "timestamp": "2026-09-01T10:00:00Z", "cwd": "/tmp/demo"
            }),
            json!({
                "type": "message", "id": "m1", "parentId": null,
                "timestamp": "2026-09-01T10:00:01Z",
                "message": {"role": "user", "content": [{"type": "text", "text": "delegate"}]}
            }),
            json!({
                "type": "message", "id": "m2", "parentId": "m1",
                "timestamp": "2026-09-01T10:00:02Z", "model": "model-a",
                "message": {"role": "assistant", "content": [{
                    "type": "tool_use", "id": "tool-child", "name": "agent", "input": {
                        "description": "Child", "prompt": "Return child"
                    }
                }]}
            }),
            json!({
                "type": "message", "id": "m3", "parentId": "m2",
                "timestamp": "2026-09-01T10:00:03Z",
                "message": {"role": "user", "content": [{
                    "type": "tool_result", "tool_use_id": "tool-child",
                    "content": [{"type": "text", "text": "child result"}]
                }]}
            }),
        ];
        std::fs::write(
            &path,
            format!(
                "{}\n",
                records
                    .iter()
                    .map(serde_json::Value::to_string)
                    .collect::<Vec<_>>()
                    .join("\n")
            ),
        )
        .unwrap();
        let provider = CommandCodeProvider::with_root(root.path().to_path_buf());
        let first = provider.scan_all().unwrap();
        assert_eq!(first.len(), 2);
        let source_path = path.to_string_lossy().to_string();
        let known = HashMap::from([(source_path.clone(), parser::source_state(&path).unwrap())]);

        let unchanged = provider.scan_incremental(&known).unwrap();
        assert!(unchanged.parsed.is_empty());
        assert_eq!(unchanged.unchanged_source_paths, [source_path]);

        std::fs::write(
            path.with_extension("meta.json"),
            r#"{"title":"Changed title"}"#,
        )
        .unwrap();
        let changed = provider.scan_incremental(&known).unwrap();
        assert_eq!(changed.parsed.len(), 2);
        assert_eq!(changed.parsed[0].meta.title, "Changed title");
        assert_eq!(
            changed.parsed[1].meta.parent_id.as_deref(),
            Some("11111111-1111-4111-a111-111111111111")
        );
    }

    #[test]
    #[ignore = "requires local Command Code session data"]
    fn scan_real_local_sessions() {
        let provider = CommandCodeProvider::new().expect("home directory");
        let sessions = provider.scan_all().expect("scan Command Code sessions");
        assert!(!sessions.is_empty(), "expected local Command Code sessions");
        let ids = sessions
            .iter()
            .map(|session| session.meta.id.as_str())
            .collect::<std::collections::HashSet<_>>();
        let mut agent_tool_count = 0usize;
        let mut child_count = 0usize;
        for session in &sessions {
            assert_eq!(session.meta.provider, Provider::CommandCode);
            assert!(!session.meta.id.is_empty());
            assert!(!session.meta.source_path.is_empty());
            assert_eq!(session.meta.message_count as usize, session.messages.len());
            assert!(
                session
                    .usage_events
                    .iter()
                    .all(|event| !event.model.trim().is_empty())
            );
            if session.meta.is_sidechain {
                child_count += 1;
                let parent_id = session.meta.parent_id.as_deref().expect("child parent");
                assert!(ids.contains(parent_id));
                assert!(session.meta.id.starts_with(&format!("{parent_id}:")));
            } else {
                assert_eq!(
                    Path::new(&session.meta.source_path)
                        .file_stem()
                        .and_then(|id| id.to_str()),
                    Some(session.meta.id.as_str())
                );
                agent_tool_count += session
                    .messages
                    .iter()
                    .filter(|message| {
                        message
                            .tool_metadata
                            .as_ref()
                            .is_some_and(|metadata| metadata.raw_name == "agent")
                    })
                    .count();
                assert!(
                    session
                        .child_session_ids
                        .iter()
                        .all(|child_id| ids.contains(child_id.as_str()))
                );
            }
            let loaded = provider
                .load_messages(&session.meta.id, &session.meta.source_path)
                .expect("load indexed Command Code session");
            assert_eq!(loaded.messages.len(), session.messages.len());
            assert_eq!(loaded.parse_warning_count, session.parse_warning_count);
        }
        assert_eq!(child_count, agent_tool_count);
    }
}
