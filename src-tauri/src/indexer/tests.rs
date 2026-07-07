use crate::db::Database;
use crate::models::{Message, MessageRole, Provider, SessionMeta, TokenUsage};
use crate::pricing::PricingCatalog;
use crate::provider::{
    default_compute_token_stats_from_messages, DeletionPlan, FileAction, LoadedSession,
    ParsedSession, ProviderError, SessionProvider, TokenStatRow,
};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::TempDir;

struct DefaultStatsProvider;

impl SessionProvider for DefaultStatsProvider {
    fn provider(&self) -> Provider {
        Provider::Claude
    }

    fn watch_paths(&self) -> Vec<PathBuf> {
        Vec::new()
    }

    fn scan_all(&self) -> Result<Vec<ParsedSession>, ProviderError> {
        Ok(Vec::new())
    }

    fn load_messages(
        &self,
        _session_id: &str,
        _source_path: &str,
    ) -> Result<LoadedSession, ProviderError> {
        Ok(LoadedSession::new(Vec::new()))
    }

    fn deletion_plan(&self, _meta: &SessionMeta, _children: &[SessionMeta]) -> DeletionPlan {
        DeletionPlan {
            file_action: FileAction::Skip,
            child_plans: Vec::new(),
            cleanup_dirs: Vec::new(),
        }
    }
}

/// Drives the default per-message aggregation path that all
/// non-Codex providers use through the trait. Tests stay focused on
/// the dedup/timestamp/model logic without dragging in a real
/// provider runtime.
fn compute_token_stats(parsed: &ParsedSession) -> Vec<TokenStatRow> {
    default_compute_token_stats_from_messages(parsed, None, None)
}

fn compute_token_stats_with_catalog_dedup(
    parsed: &ParsedSession,
    pricing_catalog: Option<&PricingCatalog>,
    seen_hashes: &mut HashSet<String>,
) -> Vec<TokenStatRow> {
    default_compute_token_stats_from_messages(parsed, pricing_catalog, Some(seen_hashes))
}

fn make_session(meta_model: Option<&str>, messages: Vec<Message>) -> ParsedSession {
    ParsedSession {
        meta: SessionMeta {
            id: "session-1".into(),
            provider: Provider::Claude,
            title: "Test".into(),
            project_path: "/tmp/project".into(),
            project_name: "project".into(),
            created_at: 1_775_635_200,
            updated_at: 1_775_635_200,
            message_count: messages.len() as u32,
            file_size_bytes: 0,
            source_path: "/tmp/source.jsonl".into(),
            is_sidechain: false,
            variant_name: None,
            model: meta_model.map(str::to_string),
            cc_version: None,
            git_branch: None,
            parent_id: None,
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
        },
        messages,
        content_text: String::new(),
        parse_warning_count: 0,
        child_session_ids: Vec::new(),
        usage_events: Vec::new(),
        source_mtime: 0,
    }
}

fn token_usage(input: u32, output: u32) -> Option<TokenUsage> {
    Some(TokenUsage {
        input_tokens: input,
        output_tokens: output,
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: 0,
    })
}

fn usage_message(hash: &str) -> Message {
    Message {
        role: MessageRole::Assistant,
        message_kind: None,
        content: String::new(),
        timestamp: Some("2026-04-09T12:00:00Z".into()),
        tool_name: None,
        tool_input: None,
        token_usage: token_usage(100, 50),
        model: Some("claude-opus-4-6".into()),
        usage_hash: Some(hash.into()),
        tool_metadata: None,
    }
}

fn tree_session(session_id: &str, updated_at: i64, created_at: i64) -> ParsedSession {
    let mut parsed = make_session(None, Vec::new());
    parsed.meta.id = session_id.into();
    parsed.meta.title = session_id.into();
    parsed.meta.created_at = created_at;
    parsed.meta.updated_at = updated_at;
    parsed.meta.message_count = 0;
    parsed.meta.source_path = format!("/tmp/{session_id}.jsonl");
    parsed
}

#[test]
fn build_tree_preserves_session_child_and_orphan_order() {
    let dir = TempDir::new().unwrap();
    let db = Arc::new(Database::open(dir.path()).unwrap());

    let parent_new = tree_session("parent-new", 300, 100);
    let parent_old = tree_session("parent-old", 200, 90);

    let mut child_late = tree_session("child-late", 500, 30);
    child_late.meta.parent_id = Some(parent_new.meta.id.clone());
    child_late.meta.is_sidechain = true;

    let mut orphan = tree_session("orphan", 250, 20);
    orphan.meta.parent_id = Some("missing-parent".into());
    orphan.meta.is_sidechain = true;

    let mut child_early = tree_session("child-early", 100, 10);
    child_early.meta.parent_id = Some(parent_new.meta.id.clone());
    child_early.meta.is_sidechain = true;

    db.sync_provider_snapshot(
        &Provider::Claude,
        &[child_late, parent_old, orphan, child_early, parent_new],
        true,
        &[],
    )
    .unwrap();

    let indexer = super::Indexer::new(db, Vec::new(), dir.path().join("data"));
    let tree = indexer.build_tree().unwrap();
    let project_node = &tree[0].children[0];
    let session_ids: Vec<&str> = project_node
        .children
        .iter()
        .map(|node| node.id.as_str())
        .collect();

    assert_eq!(session_ids, vec!["parent-new", "parent-old", "orphan"]);

    let parent_new_node = &project_node.children[0];
    let child_ids: Vec<&str> = parent_new_node
        .children
        .iter()
        .map(|node| node.id.as_str())
        .collect();

    assert_eq!(child_ids, vec!["child-early", "child-late"]);
    assert!(parent_new_node
        .children
        .iter()
        .all(|child| child.is_sidechain));
    assert!(project_node.children[2].is_sidechain);
    assert_eq!(project_node.count, 3);
}

#[test]
fn epoch_millis_rejects_times_before_unix_epoch() {
    let error = super::epoch_millis(std::time::UNIX_EPOCH - std::time::Duration::from_millis(1))
        .expect_err("pre-epoch time must be an explicit error");

    assert!(error.to_string().contains("system clock is before"));
}

#[test]
fn build_token_stats_batch_processes_parents_before_children_for_dedup() {
    let mut parent = make_session(Some("claude-opus-4-6"), vec![usage_message("usage-1")]);
    parent.meta.id = "parent".into();
    let mut child = make_session(Some("claude-opus-4-6"), vec![usage_message("usage-1")]);
    child.meta.id = "child".into();
    child.meta.parent_id = Some(parent.meta.id.clone());
    child.meta.is_sidechain = true;

    let provider = DefaultStatsProvider;
    let batch = super::build_token_stats_batch(&provider, &[child, parent], None);

    assert_eq!(batch.len(), 2);
    assert_eq!(batch[0].0, "parent");
    assert_eq!(batch[0].1.len(), 1);
    assert_eq!(batch[1].0, "child");
    assert!(batch[1].1.is_empty());
}

#[test]
fn compute_token_stats_skips_usage_without_message_model() {
    let parsed = make_session(
        Some("claude-opus-4-6"),
        vec![Message {
            role: MessageRole::Assistant,
            message_kind: None,
            content: String::new(),
            timestamp: Some("2026-04-09T12:00:00Z".into()),
            tool_name: None,
            tool_input: None,
            token_usage: token_usage(100, 50),
            model: None,
            usage_hash: None,
            tool_metadata: None,
        }],
    );

    let rows = compute_token_stats(&parsed);
    assert!(rows.is_empty());
}

#[test]
fn compute_token_stats_skips_usage_without_message_timestamp() {
    let parsed = make_session(
        Some("gpt-5.4"),
        vec![Message {
            role: MessageRole::Assistant,
            message_kind: None,
            content: String::new(),
            timestamp: None,
            tool_name: None,
            tool_input: None,
            token_usage: token_usage(25, 10),
            model: None,
            usage_hash: None,
            tool_metadata: None,
        }],
    );

    let rows = compute_token_stats(&parsed);
    assert!(rows.is_empty());
}

#[test]
fn compute_token_stats_skips_tool_usage_without_explicit_message_model() {
    let parsed = make_session(
        Some("claude-haiku-4-5-20251001"),
        vec![
            Message {
                role: MessageRole::Assistant,
                message_kind: None,
                content: String::new(),
                timestamp: Some("2026-04-09T12:00:00Z".into()),
                tool_name: None,
                tool_input: None,
                token_usage: None,
                model: Some("claude-opus-4-6".into()),
                usage_hash: None,
                tool_metadata: None,
            },
            Message {
                role: MessageRole::Tool,
                message_kind: None,
                content: String::new(),
                timestamp: Some("2026-04-09T12:00:01Z".into()),
                tool_name: Some("Bash".into()),
                tool_input: None,
                token_usage: token_usage(100, 50),
                model: None,
                usage_hash: None,
                tool_metadata: None,
            },
        ],
    );

    let rows = compute_token_stats(&parsed);
    assert!(rows.is_empty());
}

#[test]
fn compute_token_stats_groups_dates_in_local_timezone() {
    let ts = "2026-04-08T16:30:00Z";
    let expected_date = chrono::DateTime::parse_from_rfc3339(ts)
        .unwrap()
        .with_timezone(&chrono::Local)
        .format("%Y-%m-%d")
        .to_string();

    let parsed = make_session(
        Some("claude-opus-4-6"),
        vec![Message {
            role: MessageRole::Assistant,
            message_kind: None,
            content: String::new(),
            timestamp: Some(ts.into()),
            tool_name: None,
            tool_input: None,
            token_usage: token_usage(10, 5),
            model: Some("claude-opus-4-6".into()),
            usage_hash: None,
            tool_metadata: None,
        }],
    );

    let rows = compute_token_stats(&parsed);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].date, expected_date);
}

#[test]
fn compute_token_stats_dedups_same_usage_hash_across_sessions() {
    let make_message = || Message {
        role: MessageRole::Assistant,
        message_kind: None,
        content: String::new(),
        timestamp: Some("2026-04-09T12:00:00Z".into()),
        tool_name: None,
        tool_input: None,
        token_usage: token_usage(100, 50),
        model: Some("claude-opus-4-6".into()),
        usage_hash: Some("msg-1:req-1".into()),
        tool_metadata: None,
    };

    let first = make_session(Some("claude-opus-4-6"), vec![make_message()]);
    let second = make_session(Some("claude-opus-4-6"), vec![make_message()]);
    let mut seen_hashes = HashSet::new();

    let first_rows = compute_token_stats_with_catalog_dedup(&first, None, &mut seen_hashes);
    let second_rows = compute_token_stats_with_catalog_dedup(&second, None, &mut seen_hashes);

    assert_eq!(first_rows.len(), 1);
    assert!(second_rows.is_empty());
}

#[test]
fn compute_token_stats_keeps_max_cumulative_usage_per_hash() {
    // Claude Code streams cumulative usage: the JSONL lines of one API
    // call share a messageId:requestId and output_tokens grows line
    // over line. Counting the first line undercounts the call; the
    // largest entry is its final total.
    let make_message = |output: u32| Message {
        role: MessageRole::Assistant,
        message_kind: None,
        content: String::new(),
        timestamp: Some("2026-06-07T12:00:00Z".into()),
        tool_name: None,
        tool_input: None,
        token_usage: token_usage(100, output),
        model: Some("claude-opus-4-8".into()),
        usage_hash: Some("msg-1:req-1".into()),
        tool_metadata: None,
    };

    let parsed = make_session(
        Some("claude-opus-4-8"),
        vec![make_message(5), make_message(480), make_message(60)],
    );

    let rows = compute_token_stats(&parsed);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].turn_count, 1);
    assert_eq!(rows[0].input_tokens, 100);
    assert_eq!(rows[0].output_tokens, 480);
}

#[test]
fn compute_token_stats_skips_synthetic_model() {
    // Claude emits usage entries with model="<synthetic>" as internal
    // placeholders (continuation stubs, retry shells, etc.). They
    // don't represent real API calls and must be excluded from the
    // per-date aggregates.
    let parsed = make_session(
        Some("<synthetic>"),
        vec![Message {
            role: MessageRole::Assistant,
            message_kind: None,
            content: String::new(),
            timestamp: Some("2026-04-09T12:00:00Z".into()),
            tool_name: None,
            tool_input: None,
            token_usage: token_usage(500, 200),
            model: Some("<synthetic>".into()),
            usage_hash: Some("msg-x:req-x".into()),
            tool_metadata: None,
        }],
    );

    let rows = compute_token_stats(&parsed);
    assert!(
        rows.is_empty(),
        "<synthetic> entries must not contribute to token stats"
    );
}
