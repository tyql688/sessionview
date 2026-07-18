use anyhow::{anyhow, Context};

use crate::db::Database;
use crate::error::{CommandError, CommandResult};
use crate::services::load_session_meta;
use crate::services::terminal;

use super::AppState;

struct ResumeTarget {
    command: String,
    cwd: Option<String>,
}

pub async fn get_resume_command(session_id: String, state: AppState) -> CommandResult<String> {
    tokio::task::spawn_blocking(move || get_resume_command_for_db(&state.db, &session_id))
        .await
        .context("task join error")?
        .map_err(CommandError::from)
}

/// Sanitize session ID to prevent shell injection — only allow alnum, hyphens, underscores
fn sanitize_session_id(id: &str) -> anyhow::Result<String> {
    if id.is_empty() {
        return Err(anyhow!("session id is empty"));
    }

    if id
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    {
        return Ok(id.to_string());
    }

    Err(anyhow!("session id contains invalid characters: '{id}'"))
}

fn resolve_resume_target(db: &Database, session_id: &str) -> anyhow::Result<ResumeTarget> {
    let safe_id = sanitize_session_id(session_id)?;
    let session = load_session_meta(db, session_id)?;
    let variant_name = session
        .variant_name
        .as_deref()
        .map(sanitize_session_id)
        .transpose()?;

    let command = session
        .provider
        .descriptor()
        .resume_command(&safe_id, variant_name.as_deref())
        .ok_or_else(|| anyhow!("{} session missing variant name", session.provider.key()))?;

    let cwd = (!session.project_path.is_empty()).then_some(session.project_path);

    Ok(ResumeTarget { command, cwd })
}

pub(crate) fn get_resume_command_for_db(db: &Database, session_id: &str) -> anyhow::Result<String> {
    Ok(resolve_resume_target(db, session_id)?.command)
}

/// Resume a session: looks up cwd from DB, builds command, launches terminal
pub async fn resume_session(
    session_id: String,
    terminal_app: String,
    state: AppState,
) -> CommandResult<()> {
    tokio::task::spawn_blocking(move || -> CommandResult<()> {
        let target = resolve_resume_target(&state.db, &session_id)?;
        terminal::launch_terminal(&terminal_app, &target.command, target.cwd.as_deref())?;
        Ok(())
    })
    .await
    .context("task join error")?
}

pub async fn detect_terminal() -> String {
    tokio::task::spawn_blocking(detect_terminal_sync)
        .await
        .unwrap_or_else(|_| "terminal".to_string())
}

fn detect_terminal_sync() -> String {
    // Check $TERM_PROGRAM (set by macOS terminals and some Linux terminals)
    if let Ok(term) = std::env::var("TERM_PROGRAM") {
        match term.to_lowercase().as_str() {
            "iterm.app" => return "iterm2".to_string(),
            "apple_terminal" => return "terminal".to_string(),
            "ghostty" => return "ghostty".to_string(),
            "wezterm-gui" | "wezterm" => return "wezterm".to_string(),
            "warpterm" | "warp" => return "warp".to_string(),
            "kitty" => return "kitty".to_string(),
            "alacritty" => return "alacritty".to_string(),
            _ => {}
        }
    }

    // Windows: check for Windows Terminal
    #[cfg(target_os = "windows")]
    {
        if std::env::var("WT_SESSION").is_ok() {
            return "windows-terminal".to_string();
        }
        "powershell".to_string()
    }

    // Linux: check common terminal indicators
    #[cfg(target_os = "linux")]
    {
        if std::env::var("GNOME_TERMINAL_SERVICE").is_ok()
            || std::env::var("GNOME_TERMINAL_SCREEN").is_ok()
        {
            return "gnome-terminal".to_string();
        }
        if std::env::var("KONSOLE_VERSION").is_ok() {
            return "konsole".to_string();
        }
        // Fallback: probe common terminals in order
        let candidates = [
            "gnome-terminal",
            "konsole",
            "alacritty",
            "kitty",
            "wezterm",
            "xfce4-terminal",
            "xterm",
        ];
        for term in &candidates {
            if std::process::Command::new("which")
                .arg(term)
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
            {
                return term.to_string();
            }
        }
        "xterm".to_string()
    }

    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    "terminal".to_string()
}

#[cfg(test)]
mod tests {
    use super::{detect_terminal_sync, get_resume_command_for_db, sanitize_session_id};
    use crate::db::Database;
    use crate::models::{Provider, SessionMeta};
    use crate::provider::ParsedSession;
    use std::sync::Mutex;
    use tempfile::TempDir;

    // detect_terminal_sync reads a process-global env var; serialize the
    // env-mutating tests so parallel runs don't observe each other's writes.
    static ENV_GUARD: Mutex<()> = Mutex::new(());

    #[test]
    fn sanitize_session_id_accepts_safe_ids() {
        assert_eq!(sanitize_session_id("abc-123_DEF").unwrap(), "abc-123_DEF");
    }

    #[test]
    fn sanitize_session_id_accepts_unicode_ids() {
        assert_eq!(
            sanitize_session_id("会话-123_变体").unwrap(),
            "会话-123_变体"
        );
    }

    #[test]
    fn sanitize_session_id_rejects_invalid_ids() {
        let err = sanitize_session_id("abc;rm").unwrap_err().to_string();
        assert!(err.contains("invalid characters"));
    }

    /// Run `body` with `TERM_PROGRAM` forced to `value` (or unset when None),
    /// restoring the previous value afterwards. Serialized via ENV_GUARD.
    fn with_term_program(value: Option<&str>, body: impl FnOnce()) {
        let _lock = ENV_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        let previous = std::env::var("TERM_PROGRAM").ok();
        match value {
            Some(v) => std::env::set_var("TERM_PROGRAM", v),
            None => std::env::remove_var("TERM_PROGRAM"),
        }
        body();
        match previous {
            Some(v) => std::env::set_var("TERM_PROGRAM", v),
            None => std::env::remove_var("TERM_PROGRAM"),
        }
    }

    #[test]
    fn detect_terminal_sync_maps_known_term_programs() {
        // TERM_PROGRAM matching is case-insensitive on the lowercased value.
        with_term_program(Some("iTerm.app"), || {
            assert_eq!(detect_terminal_sync(), "iterm2");
        });
        with_term_program(Some("Apple_Terminal"), || {
            assert_eq!(detect_terminal_sync(), "terminal");
        });
        with_term_program(Some("ghostty"), || {
            assert_eq!(detect_terminal_sync(), "ghostty");
        });
        with_term_program(Some("WezTerm-gui"), || {
            assert_eq!(detect_terminal_sync(), "wezterm");
        });
        with_term_program(Some("kitty"), || {
            assert_eq!(detect_terminal_sync(), "kitty");
        });
    }

    #[test]
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    fn detect_terminal_sync_falls_back_when_term_program_unknown() {
        // On macOS, an unrecognised TERM_PROGRAM falls through to "terminal".
        with_term_program(Some("some-unknown-terminal"), || {
            assert_eq!(detect_terminal_sync(), "terminal");
        });
        with_term_program(None, || {
            assert_eq!(detect_terminal_sync(), "terminal");
        });
    }

    fn synthetic_meta(id: &str, provider: Provider, project_path: &str) -> SessionMeta {
        SessionMeta {
            id: id.to_string(),
            provider,
            title: "Synthetic session".into(),
            project_path: project_path.to_string(),
            project_name: "proj".into(),
            created_at: 1_767_322_245,
            updated_at: 1_767_322_245,
            message_count: 1,
            file_size_bytes: 0,
            source_path: "/tmp/session.jsonl".into(),
            is_sidechain: false,
            variant_name: None,
            model: None,
            cc_version: None,
            git_branch: None,
            parent_id: None,
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
        }
    }

    fn db_with_session(meta: SessionMeta) -> (TempDir, Database) {
        let dir = TempDir::new().unwrap();
        let db = Database::open(dir.path()).unwrap();
        let provider = meta.provider.clone();
        let parsed = ParsedSession {
            meta,
            messages: Vec::new(),
            content_text: String::new(),
            parse_warning_count: 0,
            child_session_ids: Vec::new(),
            usage_events: Vec::new(),
            source_mtime: 0,
        };
        db.sync_provider_snapshot(&provider, &[parsed], true, &[])
            .unwrap();
        (dir, db)
    }

    #[test]
    fn get_resume_command_for_db_builds_provider_resume_command() {
        let meta = synthetic_meta(
            "11111111-1111-4111-a111-111111111111",
            Provider::Claude,
            "/tmp/proj",
        );
        let (_dir, db) = db_with_session(meta);
        let command =
            get_resume_command_for_db(&db, "11111111-1111-4111-a111-111111111111").unwrap();
        assert_eq!(
            command,
            "claude --resume 11111111-1111-4111-a111-111111111111"
        );
    }

    #[test]
    fn get_resume_command_for_db_uses_opencode_flag() {
        let meta = synthetic_meta(
            "22222222-2222-4222-a222-222222222222",
            Provider::OpenCode,
            "/tmp/proj",
        );
        let (_dir, db) = db_with_session(meta);
        let command =
            get_resume_command_for_db(&db, "22222222-2222-4222-a222-222222222222").unwrap();
        assert_eq!(command, "opencode -s 22222222-2222-4222-a222-222222222222");
    }

    #[test]
    fn get_resume_command_for_db_rejects_unsafe_session_id() {
        let dir = TempDir::new().unwrap();
        let db = Database::open(dir.path()).unwrap();
        // Shell-injection-bearing id is rejected before any DB lookup.
        let err = get_resume_command_for_db(&db, "id; rm -rf /")
            .unwrap_err()
            .to_string();
        assert!(err.contains("invalid characters"));
    }

    #[test]
    fn get_resume_command_for_db_errors_on_missing_session() {
        let dir = TempDir::new().unwrap();
        let db = Database::open(dir.path()).unwrap();
        let err = get_resume_command_for_db(&db, "33333333-3333-4333-a333-333333333333")
            .unwrap_err()
            .to_string();
        assert!(err.contains("session not found"));
    }
}
