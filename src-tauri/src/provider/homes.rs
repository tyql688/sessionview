//! Multi-home session discovery.
//!
//! Providers derive their data roots from `$HOME` (dotdirs like `.claude`,
//! plus platform-spelled trees such as `AppData/Local`). On WSL that means a
//! Linux build only ever sees WSL-side sessions even though the Windows
//! side's trees are reachable under `/mnt/c/Users/<name>`.
//!
//! [`candidate_homes`] answers that: the real `$HOME` first, then every
//! `:`-separated entry of `SESSIONVIEW_EXTRA_HOMES` (so a WSL run can set it
//! to `/mnt/c/Users/me` and index both worlds with one binary). Each
//! provider is instantiated once per home; [`MultiHome`] fans the trait out
//! over those instances so the rest of the app sees a single provider.
//!
//! Explicit per-provider overrides (`DSH_HOME`, `COPILOT_HOME`,
//! `MINIMAX_DATA_DIR`, …) keep replacing their tree outright — they are the
//! escape hatch when data lives somewhere no home spelling reaches.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::models::Provider;

use super::{
    LoadedSession, ParsedSession, ProviderError, ScanOutcome, SessionProvider, SourceState,
};

/// Expand a leading `~` to the given home directory. Anything else passes
/// through unchanged.
fn expand_tilde(value: &str, home: Option<&Path>) -> PathBuf {
    match value.strip_prefix("~") {
        Some(rest) => match home {
            Some(home) => home.join(rest.trim_start_matches('/')),
            None => PathBuf::from(value),
        },
        None => PathBuf::from(value),
    }
}

/// Parse `SESSIONVIEW_EXTRA_HOMES`: `:`-separated, whitespace-tolerated,
/// `~`-expanded, empty and duplicate entries removed.
pub(crate) fn parse_extra_homes(value: &OsStr) -> Vec<PathBuf> {
    parse_extra_homes_with(value, dirs::home_dir().as_deref())
}

fn parse_extra_homes_with(value: &OsStr, home: Option<&Path>) -> Vec<PathBuf> {
    let mut homes = Vec::new();
    for entry in value.to_string_lossy().split(':') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        let home = expand_tilde(entry, home);
        if !home.as_os_str().is_empty() && !homes.contains(&home) {
            homes.push(home);
        }
    }
    homes
}

/// Every home directory providers should scan: the real `$HOME` first (its
/// trees win ties such as identical session ids), then the configured extra
/// homes in declaration order.
pub(crate) fn candidate_homes() -> Vec<PathBuf> {
    let mut homes = Vec::new();
    if let Some(home) = dirs::home_dir() {
        homes.push(home);
    }
    if let Some(extra) = std::env::var_os("SESSIONVIEW_EXTRA_HOMES") {
        for home in parse_extra_homes(&extra) {
            if !homes.contains(&home) {
                homes.push(home);
            }
        }
    }
    homes
}

/// Platform spellings of the per-user *data* directory, relative to any
/// home-like root: Windows (`AppData/Local`), Linux (`.local/share`), macOS
/// (`Library/Application Support`). Only the spelling matching the host OS
/// normally exists; checking all of them is what lets a WSL process read a
/// Windows profile (and vice versa).
pub(crate) fn home_data_dirs(home: &Path) -> Vec<PathBuf> {
    vec![
        home.join("AppData").join("Local"),
        home.join(".local").join("share"),
        home.join("Library").join("Application Support"),
    ]
}

/// Platform spellings of the per-user *config* directory (`AppData/Roaming`,
/// `.config`, `Library/Application Support`).
pub(crate) fn home_config_dirs(home: &Path) -> Vec<PathBuf> {
    vec![
        home.join("AppData").join("Roaming"),
        home.join(".config"),
        home.join("Library").join("Application Support"),
    ]
}

/// Fan one provider trait object out over per-home instances.
///
/// - Scans concatenate; freshness bookkeeping stays per instance (each
///   partitions against the shared known-state map keyed by source path, so
///   instances never disagree about which file is fresh).
/// - Message loads route to the instance whose source root is the longest
///   component-wise prefix of the session's stored `source_path`; when no
///   root matches (deleted mount, moved tree) every instance is tried in
///   order before failing.
pub(crate) struct MultiHome {
    provider_kind: Provider,
    instances: Vec<Box<dyn SessionProvider>>,
}

impl MultiHome {
    pub(crate) fn new(provider_kind: Provider, instances: Vec<Box<dyn SessionProvider>>) -> Self {
        Self {
            provider_kind,
            instances,
        }
    }

    /// Instance whose source root is the longest component-wise prefix of
    /// `source_path`, if any instance claims it.
    fn route(&self, source_path: &str) -> Option<&dyn SessionProvider> {
        let path = Path::new(source_path);
        self.instances
            .iter()
            .flat_map(|instance| {
                instance
                    .source_roots()
                    .into_iter()
                    .filter(|root| path.starts_with(root))
                    .map(move |root| (root.as_os_str().len(), instance))
            })
            .max_by_key(|(depth, _)| *depth)
            .map(|(_, instance)| instance.as_ref())
    }
}

impl SessionProvider for MultiHome {
    fn provider(&self) -> Provider {
        self.provider_kind.clone()
    }

    fn source_roots(&self) -> Vec<PathBuf> {
        let mut roots = Vec::new();
        for instance in &self.instances {
            for root in instance.source_roots() {
                if !roots.contains(&root) {
                    roots.push(root);
                }
            }
        }
        roots
    }

    fn scan_all(&self) -> Result<Vec<ParsedSession>, ProviderError> {
        let mut parsed = Vec::new();
        for instance in &self.instances {
            parsed.extend(instance.scan_all()?);
        }
        Ok(parsed)
    }

    fn scan_incremental(
        &self,
        known: &HashMap<String, SourceState>,
    ) -> Result<ScanOutcome, ProviderError> {
        let mut outcome = ScanOutcome::default();
        for instance in &self.instances {
            let ScanOutcome {
                mut parsed,
                unchanged_source_paths,
            } = instance.scan_incremental(known)?;
            outcome.parsed.append(&mut parsed);
            outcome
                .unchanged_source_paths
                .extend(unchanged_source_paths);
        }
        Ok(outcome)
    }

    fn load_messages(
        &self,
        session_id: &str,
        source_path: &str,
    ) -> Result<LoadedSession, ProviderError> {
        if let Some(instance) = self.route(source_path) {
            return instance.load_messages(session_id, source_path);
        }
        // No root claimed the path (mount gone, tree moved). Try every
        // instance rather than giving up on a loadable session; the last
        // error is the most specific one to surface.
        let mut last_error: Option<ProviderError> = None;
        for instance in &self.instances {
            match instance.load_messages(session_id, source_path) {
                Ok(loaded) => return Ok(loaded),
                Err(error) => last_error = Some(error),
            }
        }
        Err(last_error.unwrap_or_else(|| {
            ProviderError::Parse(format!(
                "no {} instance can serve '{source_path}'",
                self.provider_kind.key()
            ))
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Message, SessionMeta};
    use std::path::Path;

    #[test]
    fn parse_extra_homes_splits_and_dedupes() {
        let homes = parse_extra_homes_with(
            OsStr::new("/mnt/c/Users/a::/mnt/c/Users/b:/mnt/c/Users/a"),
            Some(Path::new("/home/tester")),
        );
        assert_eq!(
            homes,
            vec![
                PathBuf::from("/mnt/c/Users/a"),
                PathBuf::from("/mnt/c/Users/b"),
            ]
        );
        assert!(parse_extra_homes_with(OsStr::new(" : "), Some(Path::new("/h"))).is_empty());
    }

    #[test]
    fn parse_extra_homes_expands_tilde() {
        let homes = parse_extra_homes_with(OsStr::new("~/notes"), Some(Path::new("/home/tester")));
        assert_eq!(homes, vec![PathBuf::from("/home/tester/notes")]);
        // No resolvable home → the raw entry survives untouched.
        let homes = parse_extra_homes_with(OsStr::new("~/notes"), None);
        assert_eq!(homes, vec![PathBuf::from("~/notes")]);
    }

    #[test]
    fn home_data_dirs_cover_all_platform_spellings() {
        let dirs = home_data_dirs(Path::new("/mnt/c/Users/u"));
        assert_eq!(
            dirs,
            vec![
                PathBuf::from("/mnt/c/Users/u/AppData/Local"),
                PathBuf::from("/mnt/c/Users/u/.local/share"),
                PathBuf::from("/mnt/c/Users/u/Library/Application Support"),
            ]
        );
    }

    /// Minimal in-memory provider used to observe MultiHome routing.
    struct FakeProvider {
        kind: Provider,
        roots: Vec<PathBuf>,
        fail_loads: bool,
    }

    impl FakeProvider {
        fn rooted(kind: Provider, root: &Path) -> Self {
            Self {
                kind,
                roots: vec![root.to_path_buf()],
                fail_loads: false,
            }
        }
    }

    impl SessionProvider for FakeProvider {
        fn provider(&self) -> Provider {
            self.kind.clone()
        }
        fn source_roots(&self) -> Vec<PathBuf> {
            self.roots.clone()
        }
        fn scan_all(&self) -> Result<Vec<ParsedSession>, ProviderError> {
            Ok(Vec::new())
        }
        fn load_messages(
            &self,
            _session_id: &str,
            _source_path: &str,
        ) -> Result<LoadedSession, ProviderError> {
            if self.fail_loads {
                return Err(ProviderError::Parse("nope".into()));
            }
            Ok(LoadedSession::new(vec![Message::user("hit")]))
        }
    }

    fn meta_for(kind: &Provider) -> SessionMeta {
        SessionMeta {
            id: String::new(),
            provider: kind.clone(),
            title: String::new(),
            project_path: String::new(),
            project_name: String::new(),
            created_at: 0,
            updated_at: 0,
            message_count: 0,
            file_size_bytes: 0,
            source_path: String::new(),
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

    #[test]
    fn load_routes_to_longest_matching_root() {
        let wsl = tempfile::tempdir().unwrap();
        let win = tempfile::tempdir().unwrap();
        let wsl_sessions = wsl.path().join(".claude").join("projects");
        let win_sessions = win.path().join(".claude").join("projects");
        std::fs::create_dir_all(&wsl_sessions).unwrap();
        std::fs::create_dir_all(&win_sessions).unwrap();

        let wrapper = MultiHome::new(
            Provider::Claude,
            vec![
                Box::new(FakeProvider::rooted(Provider::Claude, &wsl_sessions)),
                Box::new(FakeProvider::rooted(Provider::Claude, &win_sessions)),
            ],
        );

        let deep_win = win_sessions.join("proj").join("x.jsonl");
        let loaded = wrapper
            .load_messages("id", &deep_win.to_string_lossy())
            .expect("routed load");
        assert_eq!(loaded.messages.len(), 1);

        // A sibling prefix must not steal the route (/mnt/… vs /mnt-c trick):
        let other = tempfile::tempdir().unwrap();
        let loaded = wrapper.load_messages(
            "id",
            other
                .path()
                .join("unrelated.jsonl")
                .to_string_lossy()
                .as_ref(),
        );
        assert!(loaded.is_ok(), "fallback tries every instance");
    }

    #[test]
    fn load_fails_only_when_every_instance_fails() {
        let dir = tempfile::tempdir().unwrap();
        let wrapper = MultiHome::new(
            Provider::Dsh,
            vec![Box::new(FakeProvider {
                kind: Provider::Dsh,
                roots: vec![dir.path().to_path_buf()],
                fail_loads: true,
            })],
        );
        let error = wrapper
            .load_messages("id", dir.path().join("x.jsonl").to_string_lossy().as_ref())
            .expect_err("all failed");
        assert!(error.to_string().contains("nope"));
        let _ = meta_for(&Provider::Dsh);
    }

    /// End-to-end over real providers: one DSH home on "WSL", one mounted
    /// "Windows" home; the scan must surface both sides' sessions and a
    /// load must route to the owning side by source-path prefix.
    #[test]
    fn multi_home_scans_and_loads_across_homes() {
        use crate::providers::dsh::DshProvider;

        fn write_session(home: &Path, id: &str, prompt: &str) -> PathBuf {
            let dir = home
                .join(".dsh")
                .join("sessions")
                .join("--tmp-p--")
                .join(id);
            std::fs::create_dir_all(&dir).unwrap();
            let log = format!(
                r#"{{"type":"session","version":0,"id":"{id}","createdAt":1786865077879,"cwd":"/tmp/proj"}}
{{"type":"user/message","seq":1,"time":1786865078000,"data":{{"content":[{{"type":"text","text":"{prompt}"}}],"source":{{"kind":"user"}}}}}}
"#
            );
            let path = dir.join("session.jsonl");
            std::fs::write(&path, log).unwrap();
            path
        }

        let wsl = tempfile::tempdir().unwrap();
        let win = tempfile::tempdir().unwrap();
        let wsl_log = write_session(wsl.path(), "s-wsl", "from wsl");
        let win_log = write_session(win.path(), "s-win", "from windows");

        let wrapper = MultiHome::new(
            Provider::Dsh,
            vec![
                // Mirrors the catalog wiring: instances are rooted at each
                // home's tool directory.
                Box::new(DshProvider::with_home(wsl.path().join(".dsh"))),
                Box::new(DshProvider::with_home(win.path().join(".dsh"))),
            ],
        );

        assert_eq!(
            wrapper.provider(),
            Provider::Dsh,
            "wrapper is transparent about identity"
        );
        let parsed = wrapper.scan_all().expect("both homes scan");
        let prompts: Vec<String> = parsed
            .iter()
            .map(|session| session.meta.id.clone())
            .collect();
        assert_eq!(prompts, vec!["s-wsl".to_string(), "s-win".to_string()]);

        // Incremental: both sides fresh against an empty known-map, then
        // both unchanged when their states are already known.
        let mut known = HashMap::new();
        for session in &parsed {
            known.insert(
                session.meta.source_path.clone(),
                SourceState {
                    size: session.meta.file_size_bytes,
                    mtime: session.source_mtime,
                    title: Some(session.meta.title.clone()),
                },
            );
        }
        let outcome = wrapper.scan_incremental(&known).unwrap();
        assert!(outcome.parsed.is_empty());
        assert_eq!(outcome.unchanged_source_paths.len(), 2);

        let loaded = wrapper
            .load_messages("s-win", win_log.to_string_lossy().as_ref())
            .expect("windows-side load routes to its instance");
        assert_eq!(loaded.messages[0].content, "from windows");
        let loaded = wrapper
            .load_messages("s-wsl", wsl_log.to_string_lossy().as_ref())
            .expect("wsl-side load");
        assert_eq!(loaded.messages[0].content, "from wsl");
        // Roots flatten for snapshot display / asset scoping callers.
        assert_eq!(wrapper.source_roots().len(), 2);
    }
}
