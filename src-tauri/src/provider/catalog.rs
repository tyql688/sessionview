use crate::models::Provider;

use super::{ProviderDescriptor, SessionProvider};

struct ProviderCatalogEntry {
    kind: Provider,
    key: &'static str,
    label: &'static str,
    descriptor: &'static dyn ProviderDescriptor,
    build_runtime: fn() -> Option<Box<dyn SessionProvider>>,
}

fn build_claude_runtime() -> Option<Box<dyn SessionProvider>> {
    crate::providers::claude::ClaudeProvider::new().map(|p| Box::new(p) as Box<dyn SessionProvider>)
}

fn build_codex_runtime() -> Option<Box<dyn SessionProvider>> {
    crate::providers::codex::CodexProvider::new().map(|p| Box::new(p) as Box<dyn SessionProvider>)
}

fn build_antigravity_runtime() -> Option<Box<dyn SessionProvider>> {
    crate::providers::antigravity::AntigravityProvider::new()
        .map(|p| Box::new(p) as Box<dyn SessionProvider>)
}

fn build_opencode_runtime() -> Option<Box<dyn SessionProvider>> {
    crate::providers::opencode::OpenCodeProvider::new()
        .map(|p| Box::new(p) as Box<dyn SessionProvider>)
}

fn build_kimi_runtime() -> Option<Box<dyn SessionProvider>> {
    crate::providers::kimi::KimiProvider::new().map(|p| Box::new(p) as Box<dyn SessionProvider>)
}

fn build_cursor_runtime() -> Option<Box<dyn SessionProvider>> {
    crate::providers::cursor::CursorProvider::new().map(|p| Box::new(p) as Box<dyn SessionProvider>)
}

fn build_cc_mirror_runtime() -> Option<Box<dyn SessionProvider>> {
    crate::providers::cc_mirror::CcMirrorProvider::new()
        .map(|p| Box::new(p) as Box<dyn SessionProvider>)
}

fn provider_catalog() -> &'static [ProviderCatalogEntry] {
    &PROVIDER_CATALOG
}

fn provider_entry(provider: &Provider) -> &'static ProviderCatalogEntry {
    // Exhaustive match — adding a new Provider variant forces this to be updated
    // at compile time, replacing the previous runtime .expect() panic risk.
    // Indices must stay in lock-step with PROVIDER_CATALOG; enforced by
    // `provider_entry_indices_match_catalog` below.
    match provider {
        Provider::Claude => &PROVIDER_CATALOG[0],
        Provider::Codex => &PROVIDER_CATALOG[1],
        Provider::Antigravity => &PROVIDER_CATALOG[2],
        Provider::OpenCode => &PROVIDER_CATALOG[3],
        Provider::Kimi => &PROVIDER_CATALOG[4],
        Provider::Cursor => &PROVIDER_CATALOG[5],
        Provider::CcMirror => &PROVIDER_CATALOG[6],
    }
}

fn provider_entry_for_key(key: &str) -> Option<&'static ProviderCatalogEntry> {
    provider_catalog().iter().find(|entry| entry.key == key)
}

static PROVIDER_KINDS: [Provider; 7] = [
    Provider::Claude,
    Provider::Codex,
    Provider::Antigravity,
    Provider::OpenCode,
    Provider::Kimi,
    Provider::Cursor,
    Provider::CcMirror,
];

static PROVIDER_CATALOG: [ProviderCatalogEntry; 7] = [
    ProviderCatalogEntry {
        kind: Provider::Claude,
        key: "claude",
        label: "Claude Code",
        descriptor: &crate::providers::claude::Descriptor,
        build_runtime: build_claude_runtime,
    },
    ProviderCatalogEntry {
        kind: Provider::Codex,
        key: "codex",
        label: "Codex",
        descriptor: &crate::providers::codex::Descriptor,
        build_runtime: build_codex_runtime,
    },
    ProviderCatalogEntry {
        kind: Provider::Antigravity,
        key: "antigravity",
        label: "Antigravity",
        descriptor: &crate::providers::antigravity::Descriptor,
        build_runtime: build_antigravity_runtime,
    },
    ProviderCatalogEntry {
        kind: Provider::OpenCode,
        key: "opencode",
        label: "OpenCode",
        descriptor: &crate::providers::opencode::Descriptor,
        build_runtime: build_opencode_runtime,
    },
    ProviderCatalogEntry {
        kind: Provider::Kimi,
        key: "kimi",
        label: "Kimi Code",
        descriptor: &crate::providers::kimi::Descriptor,
        build_runtime: build_kimi_runtime,
    },
    ProviderCatalogEntry {
        kind: Provider::Cursor,
        key: "cursor",
        label: "Cursor CLI",
        descriptor: &crate::providers::cursor::Descriptor,
        build_runtime: build_cursor_runtime,
    },
    ProviderCatalogEntry {
        kind: Provider::CcMirror,
        key: "cc-mirror",
        label: "CC-Mirror",
        descriptor: &crate::providers::cc_mirror::Descriptor,
        build_runtime: build_cc_mirror_runtime,
    },
];

impl Provider {
    pub fn label(&self) -> &'static str {
        provider_entry(self).label
    }

    pub fn key(&self) -> &'static str {
        provider_entry(self).key
    }

    pub fn parse(s: &str) -> Option<Provider> {
        provider_entry_for_key(s).map(|entry| entry.kind.clone())
    }

    pub fn parse_strict(s: &str) -> Result<Provider, String> {
        Self::parse(s).ok_or_else(|| format!("unknown provider: '{s}'"))
    }

    pub fn all() -> &'static [Provider] {
        &PROVIDER_KINDS
    }

    /// Get the descriptor for this provider (static metadata).
    pub fn descriptor(&self) -> &'static dyn ProviderDescriptor {
        provider_entry(self).descriptor
    }

    pub fn build_runtime(&self) -> Option<Box<dyn SessionProvider>> {
        (provider_entry(self).build_runtime)()
    }

    pub fn require_runtime(&self) -> Result<Box<dyn SessionProvider>, String> {
        self.build_runtime()
            .ok_or_else(|| format!("provider unavailable: {}", self.key()))
    }

    /// Identify which provider owns a source path.
    pub fn from_source_path(source_path: &str) -> Option<Provider> {
        Provider::all()
            .iter()
            .find(|p| p.descriptor().owns_source_path(source_path))
            .cloned()
    }

    /// Parse a display key (as produced by `descriptor().display_key()`) back to a provider and label.
    /// Handles cc-mirror variants like "cc-mirror:cczai" → (CcMirror, "cczai").
    pub fn parse_display_key(display_key: &str) -> Option<(Provider, String)> {
        // Direct match: covers most providers
        if let Some(p) = Provider::parse(display_key) {
            let label = p.label().to_string();
            return Some((p, label));
        }
        // Custom formats: e.g. "cc-mirror:variant"
        for p in Provider::all() {
            if let Some(label) = p.descriptor().try_parse_display_key(display_key) {
                return Some((p.clone(), label));
            }
        }
        None
    }
}

/// Create a provider instance by enum variant. Returns None if HOME is unavailable.
pub fn make_provider(provider: &Provider) -> Option<Box<dyn SessionProvider>> {
    provider.build_runtime()
}

/// Create all provider instances, silently skipping any that cannot resolve HOME.
pub fn all_providers() -> Vec<Box<dyn SessionProvider>> {
    Provider::all().iter().filter_map(make_provider).collect()
}

pub fn all_runtimes() -> Vec<Box<dyn SessionProvider>> {
    all_providers()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_entry_indices_match_catalog() {
        // Guards against reordering `PROVIDER_CATALOG` without updating the
        // exhaustive match in `provider_entry` (and vice versa).
        for kind in Provider::all() {
            let entry = provider_entry(kind);
            assert_eq!(
                &entry.kind, kind,
                "provider_entry({kind:?}) returned entry with kind {:?}",
                entry.kind
            );
        }
    }

    #[test]
    fn test_from_source_path() {
        let cases = [
            (
                "/home/user/.claude/projects/foo/abc.jsonl",
                Some(Provider::Claude),
            ),
            (
                "/home/user/.codex/sessions/xyz.jsonl",
                Some(Provider::Codex),
            ),
            (
                "/home/user/.gemini/antigravity-cli/brain/abc/.system_generated/logs/transcript.jsonl",
                Some(Provider::Antigravity),
            ),
            (
                "/home/user/.local/share/opencode/opencode.db",
                Some(Provider::OpenCode),
            ),
            (
                "/home/user/.kimi-code/sessions/wd_proj_abc/session_uuid/agents/main/wire.jsonl",
                Some(Provider::Kimi),
            ),
            (
                "/home/user/.cc-mirror/variant/config/projects/foo/abc.jsonl",
                Some(Provider::CcMirror),
            ),
            ("/home/user/random/file.txt", None),
            // cc-mirror path should NOT match claude
            (
                "/home/user/.cc-mirror/cczai/config/projects/foo/abc.jsonl",
                Some(Provider::CcMirror),
            ),
        ];
        for (path, expected) in &cases {
            assert_eq!(
                Provider::from_source_path(path).as_ref(),
                expected.as_ref(),
                "from_source_path({path})"
            );
        }
    }

    #[test]
    fn test_parse_display_key() {
        // Regular providers
        assert_eq!(
            Provider::parse_display_key("claude"),
            Some((Provider::Claude, "Claude Code".to_string()))
        );
        assert_eq!(
            Provider::parse_display_key("codex"),
            Some((Provider::Codex, "Codex".to_string()))
        );
        // CC-Mirror variants
        assert_eq!(
            Provider::parse_display_key("cc-mirror:cczai"),
            Some((Provider::CcMirror, "cczai".to_string()))
        );
        // Unknown
        assert_eq!(Provider::parse_display_key("unknown"), None);
    }

    #[test]
    fn test_display_key_roundtrip() {
        // Regular providers roundtrip through parse_display_key
        for p in Provider::all() {
            if *p == Provider::CcMirror {
                continue;
            }
            let key = p.descriptor().display_key(None);
            let parsed = Provider::parse_display_key(&key);
            assert!(parsed.is_some(), "display_key roundtrip failed for {:?}", p);
            assert_eq!(parsed.unwrap().0, *p);
        }
        let key = Provider::CcMirror.descriptor().display_key(Some("cczai"));
        let parsed = Provider::parse_display_key(&key);
        assert_eq!(parsed, Some((Provider::CcMirror, "cczai".to_string())));
    }

    #[test]
    fn test_descriptor_sort_order_unique() {
        let mut orders: Vec<u32> = Provider::all()
            .iter()
            .map(|p| p.descriptor().sort_order())
            .collect();
        orders.sort();
        orders.dedup();
        assert_eq!(
            orders.len(),
            Provider::all().len(),
            "sort_order values must be unique"
        );
    }
}
