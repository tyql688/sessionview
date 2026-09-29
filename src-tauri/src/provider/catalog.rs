use std::path::PathBuf;

use crate::models::Provider;

use super::homes;
use super::{ProviderDescriptor, SessionProvider};

struct ProviderCatalogEntry {
    kind: Provider,
    key: &'static str,
    label: &'static str,
    descriptor: &'static dyn ProviderDescriptor,
    build_runtime: fn() -> Option<Box<dyn SessionProvider>>,
}

/// Wrap one provider's per-home instances into the runtime the app sees.
fn multi_home(
    kind: Provider,
    instances: Vec<Box<dyn SessionProvider>>,
) -> Option<Box<dyn SessionProvider>> {
    if instances.is_empty() {
        // No home resolved (no HOME at all, no data anywhere) — mirror the
        // old "provider unavailable" contract so callers log and skip.
        return None;
    }
    Some(Box::new(homes::MultiHome::new(kind, instances)))
}

fn box_provider<P: SessionProvider + 'static>(provider: P) -> Box<dyn SessionProvider> {
    Box::new(provider)
}

fn build_claude_runtime() -> Option<Box<dyn SessionProvider>> {
    multi_home(
        Provider::Claude,
        homes::candidate_homes()
            .into_iter()
            .map(|home| box_provider(crate::providers::claude::ClaudeProvider::with_home(home)))
            .collect(),
    )
}

fn build_codex_runtime() -> Option<Box<dyn SessionProvider>> {
    multi_home(
        Provider::Codex,
        homes::candidate_homes()
            .into_iter()
            .map(|home| box_provider(crate::providers::codex::CodexProvider::with_home(home)))
            .collect(),
    )
}

fn build_antigravity_runtime() -> Option<Box<dyn SessionProvider>> {
    multi_home(
        Provider::Antigravity,
        homes::candidate_homes()
            .into_iter()
            .map(|home| {
                box_provider(crate::providers::antigravity::AntigravityProvider::with_home(home))
            })
            .collect(),
    )
}

fn build_opencode_runtime() -> Option<Box<dyn SessionProvider>> {
    multi_home(
        Provider::OpenCode,
        crate::providers::opencode::db_candidates(homes::candidate_homes())
            .into_iter()
            .map(|db_path| {
                box_provider(crate::providers::opencode::OpenCodeProvider::with_db_path(
                    db_path,
                ))
            })
            .collect(),
    )
}

fn build_kimi_runtime() -> Option<Box<dyn SessionProvider>> {
    multi_home(
        Provider::Kimi,
        homes::candidate_homes()
            .into_iter()
            .map(|home| {
                box_provider(crate::providers::kimi::KimiProvider::with_root(
                    home.join(".kimi-code"),
                ))
            })
            .collect(),
    )
}

fn build_cursor_runtime() -> Option<Box<dyn SessionProvider>> {
    multi_home(
        Provider::Cursor,
        homes::candidate_homes()
            .into_iter()
            .map(|home| box_provider(crate::providers::cursor::CursorProvider::with_home(home)))
            .collect(),
    )
}

fn build_cc_mirror_runtime() -> Option<Box<dyn SessionProvider>> {
    multi_home(
        Provider::CcMirror,
        homes::candidate_homes()
            .into_iter()
            .map(|home| {
                box_provider(
                    crate::providers::cc_mirror::CcMirrorProvider::with_mirror_root(
                        home.join(".cc-mirror"),
                    ),
                )
            })
            .collect(),
    )
}

fn build_pi_runtime() -> Option<Box<dyn SessionProvider>> {
    multi_home(
        Provider::Pi,
        homes::candidate_homes()
            .into_iter()
            .map(|home| box_provider(crate::providers::pi::PiProvider::with_home(home)))
            .collect(),
    )
}

fn build_grok_runtime() -> Option<Box<dyn SessionProvider>> {
    multi_home(
        Provider::Grok,
        homes::candidate_homes()
            .into_iter()
            .map(|home| {
                box_provider(crate::providers::grok::GrokProvider::with_root(
                    home.join(".grok"),
                ))
            })
            .collect(),
    )
}

fn build_dsh_runtime() -> Option<Box<dyn SessionProvider>> {
    // Explicit override replaces discovery entirely. DSH's constructor is
    // rooted at the tool dir ($DSH_HOME ≈ ~/.dsh), so join it per home.
    let instances: Vec<Box<dyn SessionProvider>> = match std::env::var_os("DSH_HOME") {
        Some(home) if !home.is_empty() => {
            vec![box_provider(crate::providers::dsh::DshProvider::with_home(
                PathBuf::from(home),
            ))]
        }
        _ => homes::candidate_homes()
            .into_iter()
            .map(|home| {
                box_provider(crate::providers::dsh::DshProvider::with_home(
                    home.join(".dsh"),
                ))
            })
            .collect(),
    };
    multi_home(Provider::Dsh, instances)
}

fn build_mcode_runtime() -> Option<Box<dyn SessionProvider>> {
    // Same override rule as DSH: $MINIMAX_DATA_DIR / $MAVIS_DATA_DIR replace
    // home-based discovery (the CLI resolves them before ~/.minimax).
    let instances: Vec<Box<dyn SessionProvider>> = match std::env::var_os("MINIMAX_DATA_DIR")
        .filter(|v| !v.is_empty())
    {
        Some(dir) => vec![box_provider(
            crate::providers::mcode::McodeProvider::with_data_root(PathBuf::from(dir).join("v2")),
        )],
        None => match std::env::var_os("MAVIS_DATA_DIR").filter(|v| !v.is_empty()) {
            Some(dir) => vec![box_provider(
                crate::providers::mcode::McodeProvider::with_data_root(
                    PathBuf::from(dir).join("v2"),
                ),
            )],
            None => homes::candidate_homes()
                .into_iter()
                .map(|home| {
                    box_provider(crate::providers::mcode::McodeProvider::with_data_root(
                        home.join(".minimax").join("v2"),
                    ))
                })
                .collect(),
        },
    };
    multi_home(Provider::Mcode, instances)
}

fn build_copilot_runtime() -> Option<Box<dyn SessionProvider>> {
    // One instance per home pairs that home's `.copilot` with its VS Code
    // transcript trees. An explicit $COPILOT_HOME adds a dedicated instance
    // on top (it replaces only the CLI root it names).
    let mut instances: Vec<Box<dyn SessionProvider>> = Vec::new();
    if let Some(home) = std::env::var_os("COPILOT_HOME").filter(|v| !v.is_empty()) {
        instances.push(box_provider(
            crate::providers::copilot::CopilotProvider::with_roots(PathBuf::from(home), Vec::new()),
        ));
    }
    for home in homes::candidate_homes() {
        let code_user_dirs = homes::home_config_dirs(&home)
            .into_iter()
            .flat_map(|config| ["Code", "Code - Insiders"].map(|app| config.join(app).join("User")))
            .collect();
        instances.push(box_provider(
            crate::providers::copilot::CopilotProvider::with_roots(
                home.join(".copilot"),
                code_user_dirs,
            ),
        ));
    }
    multi_home(Provider::Copilot, instances)
}

fn build_commandcode_runtime() -> Option<Box<dyn SessionProvider>> {
    crate::providers::commandcode::CommandCodeProvider::new()
        .map(|p| Box::new(p) as Box<dyn SessionProvider>)
}

fn provider_entry(provider: &Provider) -> &'static ProviderCatalogEntry {
    // Exhaustive match: a new Provider variant fails to compile until added.
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
        Provider::Pi => &PROVIDER_CATALOG[7],
        Provider::Grok => &PROVIDER_CATALOG[8],
        Provider::Dsh => &PROVIDER_CATALOG[9],
        Provider::Mcode => &PROVIDER_CATALOG[10],
        Provider::Copilot => &PROVIDER_CATALOG[11],
        Provider::CommandCode => &PROVIDER_CATALOG[12],
    }
}

static PROVIDER_KINDS: [Provider; 13] = [
    Provider::Claude,
    Provider::Codex,
    Provider::Antigravity,
    Provider::OpenCode,
    Provider::Kimi,
    Provider::Cursor,
    Provider::CcMirror,
    Provider::Pi,
    Provider::Grok,
    Provider::Dsh,
    Provider::Mcode,
    Provider::Copilot,
    Provider::CommandCode,
];

static PROVIDER_CATALOG: [ProviderCatalogEntry; 13] = [
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
    ProviderCatalogEntry {
        kind: Provider::Pi,
        key: "pi",
        label: "Pi",
        descriptor: &crate::providers::pi::Descriptor,
        build_runtime: build_pi_runtime,
    },
    ProviderCatalogEntry {
        kind: Provider::Grok,
        key: "grok",
        label: "Grok Build",
        descriptor: &crate::providers::grok::Descriptor,
        build_runtime: build_grok_runtime,
    },
    ProviderCatalogEntry {
        kind: Provider::Dsh,
        key: "dsh",
        label: "DSH",
        descriptor: &crate::providers::dsh::Descriptor,
        build_runtime: build_dsh_runtime,
    },
    ProviderCatalogEntry {
        kind: Provider::Mcode,
        key: "mcode",
        label: "MiniMax Code",
        descriptor: &crate::providers::mcode::Descriptor,
        build_runtime: build_mcode_runtime,
    },
    ProviderCatalogEntry {
        kind: Provider::Copilot,
        key: "copilot",
        label: "GitHub Copilot",
        descriptor: &crate::providers::copilot::Descriptor,
        build_runtime: build_copilot_runtime,
    },
    ProviderCatalogEntry {
        kind: Provider::CommandCode,
        key: "commandcode",
        label: "Command Code",
        descriptor: &crate::providers::commandcode::Descriptor,
        build_runtime: build_commandcode_runtime,
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
        PROVIDER_CATALOG
            .iter()
            .find(|entry| entry.key == s)
            .map(|entry| entry.kind.clone())
    }

    pub fn parse_strict(s: &str) -> anyhow::Result<Provider> {
        Self::parse(s).ok_or_else(|| anyhow::anyhow!("unknown provider: '{s}'"))
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

    pub fn require_runtime(&self) -> anyhow::Result<Box<dyn SessionProvider>> {
        self.build_runtime()
            .ok_or_else(|| anyhow::anyhow!("provider unavailable: {}", self.key()))
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

pub fn all_runtimes() -> Vec<Box<dyn SessionProvider>> {
    Provider::all()
        .iter()
        .filter_map(|provider| {
            let runtime = provider.build_runtime();
            if runtime.is_none() {
                log::warn!(
                    "provider {} unavailable: HOME could not be resolved",
                    provider.key()
                );
            }
            runtime
        })
        .collect()
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
