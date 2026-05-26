//! Cursor CLI provider.
//!
//! Cursor stores two kinds of agent sessions under `~/.cursor/`:
//!
//! * **CLI** — sessions started via the `agent` binary. The transcript
//!   lives at `~/.cursor/projects/<workdir-key>/agent-transcripts/<id>/<id>.jsonl`
//!   and a sidecar `store.db` exists at `~/.cursor/chats/<md5>/<id>/store.db`.
//! * **IDE** (Composer) — same JSONL layout but no `store.db`.
//!
//! We only surface CLI sessions. The `store.db`'s presence is the hard
//! signal: any session id without a `store.db` is treated as IDE and
//! filtered out entirely.
//!
//! Subagents (`Task` / `Subagent` tool spawns) live under
//! `<sessionId>/subagents/<subagentId>.jsonl`. They're linked back to
//! their parent by directory structure (`parent_id = <sessionId>`) and
//! pick up the parent's workspace path through `store.db`.

mod parser;
mod store_db;
mod tools;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use rayon::prelude::*;

use crate::models::{Provider, SessionMeta};
use crate::provider::{
    ChildPlan, DeletionPlan, FileAction, LoadedSession, ParsedSession, ProviderError,
    SessionProvider,
};
use crate::provider_utils::project_name_from_path;

pub struct Descriptor;
impl crate::provider::ProviderDescriptor for Descriptor {
    fn owns_source_path(&self, source_path: &str) -> bool {
        let p = source_path.replace('\\', "/");
        p.contains("/.cursor/projects/") && p.contains("/agent-transcripts/")
    }
    fn resume_command(&self, session_id: &str, _variant_name: Option<&str>) -> Option<String> {
        // Subagent ids are file stems too — `agent --resume=<id>`
        // accepts any session id the CLI knows about. The bare UUID
        // form is verified locally; quoting isn't needed.
        Some(format!("agent --resume={session_id}"))
    }
    fn display_key(&self, _variant_name: Option<&str>) -> String {
        "cursor".into()
    }
    fn sort_order(&self) -> u32 {
        7
    }
    fn color(&self) -> &'static str {
        "#3b82f6"
    }
    fn cli_command(&self) -> &'static str {
        "agent"
    }
    fn avatar_svg(&self) -> &'static str {
        r#"<svg width="24" height="24" viewBox="0 0 24 24" xmlns="http://www.w3.org/2000/svg"><path d="M22.106 5.68L12.5.135a.998.998 0 00-.998 0L1.893 5.68a.84.84 0 00-.419.726v11.186c0 .3.16.577.42.727l9.607 5.547a.999.999 0 00.998 0l9.608-5.547a.84.84 0 00.42-.727V6.407a.84.84 0 00-.42-.726zm-.603 1.176L12.228 22.92c-.063.108-.228.064-.228-.061V12.34a.59.59 0 00-.295-.51l-9.11-5.26c-.107-.062-.063-.228.062-.228h18.55c.264 0 .428.286.296.514z" fill="currentColor"/></svg>"#
    }
}

pub struct CursorProvider {
    home_dir: PathBuf,
}

impl CursorProvider {
    pub fn new() -> Option<Self> {
        Some(Self {
            home_dir: dirs::home_dir()?,
        })
    }

    /// Test-only constructor that lets fixture tests point the provider
    /// at a TempDir mimicking the user's $HOME layout.
    pub fn with_home(home_dir: PathBuf) -> Self {
        Self { home_dir }
    }

    fn projects_dir(&self) -> PathBuf {
        self.home_dir.join(".cursor").join("projects")
    }

    fn chats_dir(&self) -> PathBuf {
        self.home_dir.join(".cursor").join("chats")
    }

    /// Build the CLI session whitelist by scanning
    /// `~/.cursor/chats/<md5>/<sessionId>/store.db`. Returns
    /// `sessionId → store.db path` so callers can pull the workspace
    /// path out of the same DB later.
    fn collect_cli_store_paths(&self) -> HashMap<String, PathBuf> {
        let chats = self.chats_dir();
        let mut out = HashMap::new();
        let Ok(buckets) = std::fs::read_dir(&chats) else {
            return out;
        };
        for bucket in buckets.flatten() {
            let bucket_dir = bucket.path();
            if !bucket_dir.is_dir() {
                continue;
            }
            let Ok(sessions) = std::fs::read_dir(&bucket_dir) else {
                continue;
            };
            for session in sessions.flatten() {
                let session_dir = session.path();
                let store = session_dir.join("store.db");
                if session_dir.is_dir() && store.is_file() {
                    if let Some(id) = session_dir.file_name().and_then(|n| n.to_str()) {
                        out.insert(id.to_string(), store);
                    }
                }
            }
        }
        out
    }

    /// Walk `~/.cursor/projects/<key>/agent-transcripts/` and return
    /// `(main_transcripts, subagent_transcripts)`.
    fn collect_transcripts(&self) -> (Vec<PathBuf>, Vec<PathBuf>) {
        let projects = self.projects_dir();
        let mut mains = Vec::new();
        let mut subs = Vec::new();
        let Ok(project_entries) = std::fs::read_dir(&projects) else {
            return (mains, subs);
        };
        for project in project_entries.flatten() {
            let transcripts = project.path().join("agent-transcripts");
            if !transcripts.is_dir() {
                continue;
            }
            let Ok(session_entries) = std::fs::read_dir(&transcripts) else {
                continue;
            };
            for session in session_entries.flatten() {
                let session_dir = session.path();
                if !session_dir.is_dir() {
                    continue;
                }
                let dir_name = session_dir
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("");
                let main_path = session_dir.join(format!("{dir_name}.jsonl"));
                if main_path.is_file() {
                    mains.push(main_path);
                }
                subs.extend(parser::subagent_paths_under(&session_dir));
            }
        }
        (mains, subs)
    }

    /// Enrich `session` with everything we can pull from the
    /// matching `store.db`: workspace path (more reliable than the
    /// sanitised dir name), last-used model alias, and inline images
    /// rewritten into shareable `[Image: source: ...]` markers so the
    /// frontend's existing renderer picks them up.
    fn apply_store_metadata(&self, session: &mut ParsedSession, stores: &HashMap<String, PathBuf>) {
        let lookup_id = session
            .meta
            .parent_id
            .as_deref()
            .unwrap_or(&session.meta.id);
        let Some(store) = stores.get(lookup_id) else {
            return;
        };
        let info = store_db::read_store_db(store, &session.meta.id);
        if let Some(path) = info.workspace_path {
            session.meta.project_name = project_name_from_path(&path);
            session.meta.project_path = path;
        }
        if let Some(model) = info.model {
            session.meta.model = Some(model);
        }
        if !info.image_paths.is_empty() {
            substitute_image_placeholders(&mut session.messages, &info.image_paths);
        }
    }
}

/// Walk every user message and replace `[Image #N]` placeholders with
/// For a subagent transcript path of the shape
/// `…/agent-transcripts/<parentId>/subagents/<subId>.jsonl`,
/// return `<parentId>` so callers can look the parent's
/// `store.db` up. Returns None for any other path layout (main
/// transcripts, malformed inputs, etc.).
fn subagent_parent_id_from_path(source_path: &str) -> Option<String> {
    let path = Path::new(source_path);
    let subagents_dir = path.parent()?;
    if subagents_dir.file_name().and_then(|n| n.to_str()) != Some("subagents") {
        return None;
    }
    Some(
        subagents_dir
            .parent()?
            .file_name()?
            .to_string_lossy()
            .to_string(),
    )
}

/// concrete `[Image: source: <cached-path>]` markers from `paths`.
/// Multiple placeholders within one message are handled by repeated
/// substitution. `N` is 1-indexed; out-of-range references are left
/// untouched so the user at least sees the original placeholder.
fn substitute_image_placeholders(messages: &mut [crate::models::Message], paths: &[PathBuf]) {
    if paths.is_empty() {
        return;
    }
    for message in messages.iter_mut() {
        if !message.content.contains("[Image #") {
            continue;
        }
        let mut rewritten = String::with_capacity(message.content.len() + 64);
        let mut remaining = message.content.as_str();
        while let Some(start) = remaining.find("[Image #") {
            rewritten.push_str(&remaining[..start]);
            let after = &remaining[start + "[Image #".len()..];
            let Some(end_rel) = after.find(']') else {
                rewritten.push_str(&remaining[start..]);
                remaining = "";
                break;
            };
            let n_str = &after[..end_rel];
            match n_str.parse::<usize>() {
                Ok(n) if n >= 1 && n <= paths.len() => {
                    let path = paths[n - 1].to_string_lossy();
                    rewritten.push_str(&format!("[Image: source: {path}]"));
                }
                _ => {
                    // Preserve the original placeholder unchanged.
                    rewritten.push_str(&remaining[start..start + "[Image #".len() + end_rel + 1]);
                }
            }
            remaining = &after[end_rel + 1..];
        }
        rewritten.push_str(remaining);
        message.content = rewritten;
    }
}

impl SessionProvider for CursorProvider {
    fn provider(&self) -> Provider {
        Provider::Cursor
    }

    fn watch_paths(&self) -> Vec<PathBuf> {
        let projects = self.projects_dir();
        if !projects.exists() {
            return Vec::new();
        }
        // Watch each project's agent-transcripts subtree so brand-new
        // transcript dirs trigger reindex without scanning the whole
        // ~/.cursor/projects/ tree (which also holds terminals/).
        let mut watched = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&projects) {
            for entry in entries.flatten() {
                let transcripts = entry.path().join("agent-transcripts");
                if transcripts.is_dir() {
                    watched.push(transcripts);
                }
            }
        }
        watched
    }

    fn scan_all(&self) -> Result<Vec<ParsedSession>, ProviderError> {
        let stores = self.collect_cli_store_paths();
        let cli_ids: HashSet<&str> = stores.keys().map(String::as_str).collect();
        let (mains, subs) = self.collect_transcripts();

        let mut sessions: Vec<ParsedSession> = mains
            .par_iter()
            .filter_map(|path| {
                let id = path.file_stem().and_then(|n| n.to_str())?;
                if !cli_ids.contains(id) {
                    return None;
                }
                let mut session = parser::parse_session(path, None)?;
                self.apply_store_metadata(&mut session, &stores);
                Some(session)
            })
            .collect();

        let cli_main_ids: HashSet<String> = sessions.iter().map(|s| s.meta.id.clone()).collect();
        let sub_sessions: Vec<ParsedSession> = subs
            .par_iter()
            .filter_map(|path| {
                let mut session = parser::parse_session(path, None)?;
                let parent_is_cli = session
                    .meta
                    .parent_id
                    .as_deref()
                    .is_some_and(|pid| cli_main_ids.contains(pid));
                if !parent_is_cli {
                    return None; // parent is IDE — drop the child too.
                }
                self.apply_store_metadata(&mut session, &stores);
                Some(session)
            })
            .collect();
        sessions.extend(sub_sessions);
        Ok(sessions)
    }

    fn scan_source(&self, source_path: &str) -> Result<Vec<ParsedSession>, ProviderError> {
        let path = Path::new(source_path);
        if !path.is_file() {
            return Ok(Vec::new());
        }
        let stores = self.collect_cli_store_paths();
        let Some(mut session) = parser::parse_session(path, None) else {
            return Ok(Vec::new());
        };
        let lookup = session
            .meta
            .parent_id
            .as_deref()
            .unwrap_or(&session.meta.id);
        if !stores.contains_key(lookup) {
            return Ok(Vec::new()); // IDE session — skip.
        }
        self.apply_store_metadata(&mut session, &stores);
        Ok(vec![session])
    }

    fn load_messages(
        &self,
        session_id: &str,
        source_path: &str,
    ) -> Result<LoadedSession, ProviderError> {
        let content = std::fs::read_to_string(source_path)
            .map_err(|e| ProviderError::Parse(format!("failed to read transcript: {e}")))?;
        let (mut messages, warnings) = parser::parse_messages(&content, source_path);

        // Reapply store.db image extraction so `[Image #N]` placeholders
        // in the transcript become `[Image: source: <cache-path>]`
        // markers the frontend can render. scan_all/scan_source go
        // through `apply_store_metadata` for the same reason; this is
        // the matching path for the on-demand session-open flow.
        //
        // For a subagent transcript, `session_id` is the subagent's id
        // but its store.db belongs to the parent. Derive the parent id
        // from the path so the lookup still hits.
        let lookup_id =
            subagent_parent_id_from_path(source_path).unwrap_or_else(|| session_id.to_string());
        let stores = self.collect_cli_store_paths();
        if let Some(store) = stores.get(&lookup_id) {
            let info = store_db::read_store_db(store, session_id);
            if !info.image_paths.is_empty() {
                substitute_image_placeholders(&mut messages, &info.image_paths);
            }
        }

        Ok(LoadedSession::from_messages(messages, warnings))
    }

    fn deletion_plan(&self, meta: &SessionMeta, children: &[SessionMeta]) -> DeletionPlan {
        // Subagent: just remove its own jsonl. Its parent owns the
        // session dir, so we don't touch anything else.
        if meta.parent_id.is_some() {
            return DeletionPlan {
                file_action: FileAction::Remove,
                child_plans: Vec::new(),
                cleanup_dirs: Vec::new(),
            };
        }

        // Parent: trash each child's jsonl as its own restorable entry,
        // then clean up the session dir (which only holds subagents/ +
        // the main jsonl after trash). The `store.db` directory is
        // intentionally left alone — `cleanup_on_permanent_delete`
        // removes it on hard-delete, not on trash, so a restored
        // session still resolves as CLI on the next scan.
        let child_plans: Vec<ChildPlan> = children
            .iter()
            .map(|c| ChildPlan {
                id: c.id.clone(),
                source_path: c.source_path.clone(),
                title: c.title.clone(),
                file_action: FileAction::Remove,
            })
            .collect();
        let source = PathBuf::from(&meta.source_path);
        let cleanup_dirs: Vec<PathBuf> = source
            .parent()
            .filter(|d| d.is_dir())
            .map(Path::to_path_buf)
            .into_iter()
            .collect();
        DeletionPlan {
            file_action: FileAction::Remove,
            child_plans,
            cleanup_dirs,
        }
    }

    fn cleanup_on_permanent_delete(&self, session_id: &str) {
        // On permanent delete, also remove the store.db directory so
        // future scans don't keep treating this id as a CLI session.
        let chats = self.chats_dir();
        if let Ok(buckets) = std::fs::read_dir(&chats) {
            for bucket in buckets.flatten() {
                let candidate = bucket.path().join(session_id);
                if candidate.is_dir() {
                    if let Err(error) = std::fs::remove_dir_all(&candidate) {
                        log::warn!(
                            "failed to remove Cursor store.db dir '{}': {error}",
                            candidate.display()
                        );
                    }
                }
            }
        }

        // Drop any inline images we extracted from this session into
        // the shared image cache. Filenames are prefixed with
        // `cursor-<sessionId>-` so we can clean them up surgically.
        if let Some(cache_dir) =
            crate::services::image_cache::image_cache_data_dir().map(|d| d.join("images"))
        {
            if let Ok(entries) = std::fs::read_dir(&cache_dir) {
                let prefix = format!("cursor-{session_id}-");
                for entry in entries.flatten() {
                    let path = entry.path();
                    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    if name.starts_with(&prefix) {
                        if let Err(error) = std::fs::remove_file(&path) {
                            log::warn!(
                                "failed to remove Cursor cached image '{}': {error}",
                                path.display()
                            );
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use serde_json::json;

    fn write_main_transcript(home: &Path, project_key: &str, sid: &str, body: &str) -> PathBuf {
        let dir = home
            .join(".cursor")
            .join("projects")
            .join(project_key)
            .join("agent-transcripts")
            .join(sid);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{sid}.jsonl"));
        std::fs::write(&path, body).unwrap();
        path
    }

    fn write_store_db(home: &Path, sid: &str, workspace: &str) {
        let dir = home.join(".cursor").join("chats").join("hash1").join(sid);
        std::fs::create_dir_all(&dir).unwrap();
        let conn = Connection::open(dir.join("store.db")).unwrap();
        conn.execute("CREATE TABLE blobs (id TEXT PRIMARY KEY, data BLOB)", [])
            .unwrap();
        let blob = serde_json::to_vec(&json!({
            "role": "user",
            "content": format!("<user_info>\nWorkspace Path: {workspace}\n</user_info>"),
        }))
        .unwrap();
        conn.execute("INSERT INTO blobs VALUES (?1, ?2)", ("b1", blob))
            .unwrap();
    }

    #[test]
    fn scan_all_skips_ide_sessions_without_store_db() {
        let dir = tempfile::tempdir().unwrap();
        write_main_transcript(
            dir.path(),
            "TestProj",
            "ide-sid",
            r#"{"role":"user","message":{"content":[{"type":"text","text":"<user_query>hi</user_query>"}]}}
{"role":"assistant","message":{"content":[{"type":"text","text":"hi back"}]}}"#,
        );
        // No store.db → IDE session → expected to be filtered.
        let provider = CursorProvider::with_home(dir.path().to_path_buf());
        assert!(provider.scan_all().unwrap().is_empty());
    }

    #[test]
    fn scan_all_keeps_cli_sessions_and_overlays_store_workspace_path() {
        let dir = tempfile::tempdir().unwrap();
        let sid = "cli-sid";
        // Workspace dir that doesn't match the sanitised project_key, to
        // ensure the store.db override beats the dir-name decode.
        let workspace = dir.path().join("real").join("project-with-dashes");
        std::fs::create_dir_all(&workspace).unwrap();
        write_main_transcript(
            dir.path(),
            "private-tmp-fake-key",
            sid,
            r#"{"role":"user","message":{"content":[{"type":"text","text":"<user_query>hi</user_query>"}]}}
{"role":"assistant","message":{"content":[{"type":"text","text":"hi back"}]}}"#,
        );
        write_store_db(dir.path(), sid, workspace.to_string_lossy().as_ref());

        let provider = CursorProvider::with_home(dir.path().to_path_buf());
        let sessions = provider.scan_all().unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].meta.id, sid);
        assert_eq!(sessions[0].meta.project_path, workspace.to_string_lossy());
        assert_eq!(sessions[0].meta.project_name, "project-with-dashes");
    }

    #[test]
    fn scan_source_filters_ide_session() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_main_transcript(
            dir.path(),
            "TestProj",
            "ide-only",
            r#"{"role":"user","message":{"content":[{"type":"text","text":"<user_query>hi</user_query>"}]}}"#,
        );
        let provider = CursorProvider::with_home(dir.path().to_path_buf());
        let sessions = provider
            .scan_source(path.to_string_lossy().as_ref())
            .unwrap();
        assert!(sessions.is_empty());
    }

    #[test]
    fn scan_source_returns_single_cli_session() {
        let dir = tempfile::tempdir().unwrap();
        let sid = "cli-source";
        let workspace = dir.path().join("ws");
        std::fs::create_dir_all(&workspace).unwrap();
        write_store_db(dir.path(), sid, workspace.to_string_lossy().as_ref());
        let path = write_main_transcript(
            dir.path(),
            "TestProj",
            sid,
            r#"{"role":"user","message":{"content":[{"type":"text","text":"<user_query>hi</user_query>"}]}}
{"role":"assistant","message":{"content":[{"type":"text","text":"ok"}]}}"#,
        );
        let provider = CursorProvider::with_home(dir.path().to_path_buf());
        let sessions = provider
            .scan_source(path.to_string_lossy().as_ref())
            .unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].meta.id, sid);
    }

    #[test]
    fn deletion_plan_for_subagent_only_removes_its_file() {
        use crate::models::SessionMeta;
        let provider = CursorProvider::with_home(PathBuf::from("/tmp/unused"));
        let meta = SessionMeta {
            id: "sub-1".into(),
            provider: Provider::Cursor,
            title: "sub".into(),
            project_path: String::new(),
            project_name: String::new(),
            created_at: 0,
            updated_at: 0,
            message_count: 1,
            file_size_bytes: 0,
            source_path: "/tmp/x.jsonl".into(),
            is_sidechain: true,
            variant_name: None,
            model: None,
            cc_version: None,
            git_branch: None,
            parent_id: Some("parent-1".into()),
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
        };
        let plan = provider.deletion_plan(&meta, &[]);
        assert_eq!(plan.file_action, FileAction::Remove);
        assert!(plan.child_plans.is_empty());
        assert!(plan.cleanup_dirs.is_empty());
    }
}
