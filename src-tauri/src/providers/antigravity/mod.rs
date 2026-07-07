use rayon::prelude::*;
use std::collections::HashMap;
use std::path::PathBuf;
use walkdir::WalkDir;

use crate::models::{Provider, SessionMeta};
use crate::provider::{
    ChildPlan, DeletionPlan, FileAction, LoadedSession, ParsedSession, ProviderDescriptor,
    ProviderError, SessionProvider, WatchStrategy,
};

struct ParentInfo {
    parent_id: String,
    project_path: String,
    project_name: String,
}

pub(crate) struct Descriptor;

impl ProviderDescriptor for Descriptor {
    fn owns_source_path(&self, source_path: &str) -> bool {
        let p = source_path.replace('\\', "/");
        p.contains("/.gemini/antigravity-cli/brain/") && p.ends_with("/transcript.jsonl")
    }

    fn resume_command(&self, session_id: &str, _variant_name: Option<&str>) -> Option<String> {
        Some(format!("agy --conversation {session_id}"))
    }

    fn display_key(&self, _variant_name: Option<&str>) -> String {
        "antigravity".into()
    }

    fn sort_order(&self) -> u32 {
        3
    }

    fn color(&self) -> &'static str {
        "#4f46e5"
    }

    fn cli_command(&self) -> &'static str {
        "agy"
    }

    fn avatar_svg(&self) -> &'static str {
        r#"<svg width="24" height="24" viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg"><circle cx="12" cy="12" r="10" stroke="currentColor" stroke-width="2"/><path d="M12 6L16 10H14V14H10V10H8L12 6Z" fill="currentColor"/><circle cx="12" cy="17" r="1.5" fill="currentColor"/></svg>"#
    }

    fn watch_strategy(&self) -> WatchStrategy {
        WatchStrategy::Fs
    }
}

pub struct AntigravityProvider {
    home_dir: PathBuf,
}

impl AntigravityProvider {
    pub fn new() -> Option<Self> {
        let home_dir = dirs::home_dir()?;
        Some(Self { home_dir })
    }

    fn brain_dir(&self) -> PathBuf {
        self.home_dir
            .join(".gemini")
            .join("antigravity-cli")
            .join("brain")
    }
}

impl SessionProvider for AntigravityProvider {
    fn provider(&self) -> Provider {
        Provider::Antigravity
    }

    fn watch_paths(&self) -> Vec<PathBuf> {
        vec![self.brain_dir()]
    }

    fn scan_all(&self) -> Result<Vec<ParsedSession>, ProviderError> {
        let root = self.brain_dir();
        if !root.is_dir() {
            return Ok(Vec::new());
        }

        let mut all_files = Vec::new();
        for entry in WalkDir::new(&root).max_depth(4) {
            let Ok(entry) = entry else {
                continue;
            };
            let path = entry.path();
            if path.is_file() && path.file_name().is_some_and(|n| n == "transcript.jsonl") {
                all_files.push(path.to_path_buf());
            }
        }

        let mut sessions: Vec<ParsedSession> = all_files
            .par_iter()
            .filter_map(|p| parser::parse_session_file(p))
            .collect();

        // Walk the explicit parent → children links the parser extracted from
        // each transcript's INVOKE_SUBAGENT steps and back-fill child rows.
        // The child's parser may already have set parent_id from its own
        // send_message Recipient — we prefer that, and only fill from the
        // parent-side link when it's missing.
        let mut child_parents: HashMap<String, ParentInfo> = HashMap::new();
        for parent in &sessions {
            for child_id in &parent.child_session_ids {
                child_parents
                    .entry(child_id.clone())
                    .or_insert_with(|| ParentInfo {
                        parent_id: parent.meta.id.clone(),
                        project_path: parent.meta.project_path.clone(),
                        project_name: parent.meta.project_name.clone(),
                    });
            }
        }

        for session in &mut sessions {
            if let Some(info) = child_parents.get(&session.meta.id) {
                if session.meta.parent_id.is_none() {
                    session.meta.parent_id = Some(info.parent_id.clone());
                }
                session.meta.is_sidechain = true;
                if session.meta.project_path.is_empty() {
                    session.meta.project_path = info.project_path.clone();
                }
                if session.meta.project_name.is_empty()
                    || session.meta.project_name == "Unknown Project"
                {
                    session.meta.project_name = info.project_name.clone();
                }
            }
        }

        Ok(sessions)
    }

    fn scan_source(&self, source_path: &str) -> Result<Vec<ParsedSession>, ProviderError> {
        let path = PathBuf::from(source_path);
        let parsed = parser::parse_session_file(&path).ok_or_else(|| {
            ProviderError::Parse(format!(
                "failed to parse Antigravity session file '{}'",
                path.display()
            ))
        })?;
        Ok(vec![parsed])
    }

    fn deletion_plan(&self, meta: &SessionMeta, children: &[SessionMeta]) -> DeletionPlan {
        if meta.parent_id.is_some() {
            return DeletionPlan {
                file_action: FileAction::Remove,
                child_plans: Vec::new(),
                cleanup_dirs: Vec::new(),
            };
        }

        let child_plans = children
            .iter()
            .map(|c| ChildPlan {
                id: c.id.clone(),
                source_path: c.source_path.clone(),
                title: c.title.clone(),
                file_action: FileAction::Remove,
            })
            .collect();

        let source = PathBuf::from(&meta.source_path);
        let mut cleanup_dirs = Vec::new();
        if let Some(conversation_dir) = source
            .parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.parent())
        {
            if conversation_dir.is_dir() {
                cleanup_dirs.push(conversation_dir.to_path_buf());
            }
        }

        DeletionPlan {
            file_action: FileAction::Remove,
            child_plans,
            cleanup_dirs,
        }
    }

    fn load_messages(
        &self,
        _session_id: &str,
        source_path: &str,
    ) -> Result<LoadedSession, ProviderError> {
        let path = PathBuf::from(source_path);
        let parsed = parser::parse_session_file(&path).ok_or_else(|| {
            ProviderError::Parse(format!(
                "failed to parse Antigravity session file '{}'",
                path.display()
            ))
        })?;
        Ok(LoadedSession::from_parsed(parsed))
    }
}

pub mod parser;
