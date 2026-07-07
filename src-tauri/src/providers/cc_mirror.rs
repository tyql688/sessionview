use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use rayon::prelude::*;
use serde::Deserialize;

use crate::models::{Provider, SessionMeta};
use crate::provider::{
    DeletionPlan, LoadedSession, ParsedSession, ProviderError, ScanOutcome, SessionProvider,
    SourceState,
};
use crate::providers::claude::parser;

pub(crate) struct Descriptor;
impl crate::provider::ProviderDescriptor for Descriptor {
    fn owns_source_path(&self, source_path: &str) -> bool {
        let p = source_path.replace('\\', "/");
        p.contains("/.cc-mirror/") && p.contains("/config/projects/")
    }

    fn resume_command(&self, session_id: &str, variant_name: Option<&str>) -> Option<String> {
        variant_name.map(|name| format!("{name} --resume {session_id}"))
    }

    fn display_key(&self, variant_name: Option<&str>) -> String {
        match variant_name {
            Some(name) => format!("cc-mirror:{name}"),
            None => "cc-mirror".into(),
        }
    }

    fn try_parse_display_key(&self, display_key: &str) -> Option<String> {
        display_key
            .strip_prefix("cc-mirror:")
            .map(|v| v.to_string())
    }

    fn sort_order(&self) -> u32 {
        1
    }

    fn color(&self) -> &'static str {
        "#f472b6"
    }

    fn cli_command(&self) -> &'static str {
        ""
    }

    fn avatar_svg(&self) -> &'static str {
        r##"<svg width="24" height="24" viewBox="0 0 24 24" xmlns="http://www.w3.org/2000/svg"><path d="M4.709 15.955l4.72-2.647.08-.23-.08-.128H9.2l-.79-.048-2.698-.073-2.339-.097-2.266-.122-.571-.121L0 11.784l.055-.352.48-.321.686.06 1.52.103 2.278.158 1.652.097 2.449.255h.389l.055-.157-.134-.098-.103-.097-2.358-1.596-2.552-1.688-1.336-.972-.724-.491-.364-.462-.158-1.008.656-.722.881.06.225.061.893.686 1.908 1.476 2.491 1.833.365.304.145-.103.019-.073-.164-.274-1.355-2.446-1.446-2.49-.644-1.032-.17-.619a2.97 2.97 0 01-.104-.729L6.283.134 6.696 0l.996.134.42.364.62 1.414 1.002 2.229 1.555 3.03.456.898.243.832.091.255h.158V9.01l.128-1.706.237-2.095.23-2.695.08-.76.376-.91.747-.492.584.28.48.685-.067.444-.286 1.851-.559 2.903-.364 1.942h.212l.243-.242.985-1.306 1.652-2.064.73-.82.85-.904.547-.431h1.033l.76 1.129-.34 1.166-1.064 1.347-.881 1.142-1.264 1.7-.79 1.36.073.11.188-.02 2.856-.606 1.543-.28 1.841-.315.833.388.091.395-.328.807-1.969.486-2.309.462-3.439.813-.042.03.049.061 1.549.146.662.036h1.622l3.02.225.79.522.474.638-.079.485-1.215.62-1.64-.389-3.829-.91-1.312-.329h-.182v.11l1.093 1.068 2.006 1.81 2.509 2.33.127.578-.322.455-.34-.049-2.205-1.657-.851-.747-1.926-1.62h-.128v.17l.444.649 2.345 3.521.122 1.08-.17.353-.608.213-.668-.122-1.374-1.925-1.415-2.167-1.143-1.943-.14.08-.674 7.254-.316.37-.729.28-.607-.461-.322-.747.322-1.476.389-1.924.315-1.53.286-1.9.17-.632-.012-.042-.14.018-1.434 1.967-2.18 2.945-1.726 1.845-.414.164-.717-.37.067-.662.401-.589 2.388-3.036 1.44-1.882.93-1.086-.006-.158h-.055L4.132 18.56l-1.13.146-.487-.456.061-.746.231-.243 1.908-1.312-.006.006z" fill="#f472b6" fill-rule="nonzero"/></svg>"##
    }
}

#[derive(Debug, Deserialize)]
struct VariantMeta {
    name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Variant {
    command_name: String,
    dir_name: String,
    projects_dir: PathBuf,
    /// Forward-slash normalized `projects_dir` used for path-prefix lookup.
    /// Invariant: disjoint across variants (each lives under a unique `dir_name`
    /// inside `~/.cc-mirror/`), so `starts_with` matches at most one entry.
    normalized_prefix: String,
}

pub(crate) struct CcMirrorProvider {
    variants: Vec<Variant>,
}

fn sanitize_command_name(raw: &str) -> String {
    raw.chars()
        .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
        .collect()
}

fn variant_from_parts(dir_name: &str, meta: Option<VariantMeta>, dir: &Path) -> Option<Variant> {
    let dir_name = sanitize_command_name(dir_name);
    if dir_name.is_empty() {
        return None;
    }

    let command_name = match meta {
        Some(meta) => meta
            .name
            .as_deref()
            .map(sanitize_command_name)
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| dir_name.clone()),
        None => dir_name.clone(),
    };

    let projects_dir = dir.join("config").join("projects");
    let normalized_prefix = projects_dir.to_string_lossy().replace('\\', "/");
    Some(Variant {
        command_name,
        dir_name,
        projects_dir,
        normalized_prefix,
    })
}

fn discover_variants(mirror_root: &Path) -> Vec<Variant> {
    let mut variants = Vec::new();
    let entries = match fs::read_dir(mirror_root) {
        Ok(entries) => entries,
        Err(_) => return variants,
    };

    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }

        let Some(dir_name) = dir.file_name().and_then(|name| name.to_str()) else {
            continue;
        };

        let variant_json = dir.join("variant.json");
        let meta = if variant_json.exists() {
            match fs::read_to_string(&variant_json) {
                Ok(content) => match serde_json::from_str::<VariantMeta>(&content) {
                    Ok(meta) => Some(meta),
                    Err(error) => {
                        log::warn!("failed to parse '{}': {error}", variant_json.display());
                        None
                    }
                },
                Err(_) => None,
            }
        } else {
            None
        };

        if let Some(variant) = variant_from_parts(dir_name, meta, &dir) {
            variants.push(variant);
        }
    }

    variants
}

fn resolve_variant<'a>(
    provider: &'a CcMirrorProvider,
    variant_name: Option<&str>,
    source_path: Option<&str>,
) -> Option<&'a Variant> {
    variant_name
        .and_then(|name| provider.variant_by_command_name(name))
        .or_else(|| source_path.and_then(|path| provider.variant_by_path(path)))
}

fn populate_variant_name_with_provider(
    meta: &mut SessionMeta,
    provider: Option<&CcMirrorProvider>,
) {
    if meta.provider != Provider::CcMirror {
        return;
    }

    let Some(provider) = provider else {
        return;
    };

    let variant = resolve_variant(
        provider,
        meta.variant_name.as_deref(),
        Some(&meta.source_path),
    );

    if let Some(variant) = variant {
        meta.variant_name
            .get_or_insert_with(|| variant.command_name.clone());
    }
}

pub(crate) fn populate_variant_name(meta: &mut SessionMeta) {
    let provider = CcMirrorProvider::new();
    populate_variant_name_with_provider(meta, provider.as_ref());
}

pub(crate) fn hydrate_variant_names(sessions: &mut [SessionMeta]) {
    let provider = CcMirrorProvider::new();
    for session in sessions {
        populate_variant_name_with_provider(session, provider.as_ref());
    }
}

impl CcMirrorProvider {
    pub(crate) fn new() -> Option<Self> {
        let home_dir = dirs::home_dir()?;
        let mirror_root = home_dir.join(".cc-mirror");
        if !mirror_root.exists() {
            return Some(Self {
                variants: Vec::new(),
            });
        }

        Some(Self {
            variants: discover_variants(&mirror_root),
        })
    }

    fn collect_jsonl_files(&self) -> Vec<(PathBuf, Variant)> {
        let mut all_files = Vec::new();
        for variant in &self.variants {
            if !variant.projects_dir.exists() {
                continue;
            }
            let project_dirs = match fs::read_dir(&variant.projects_dir) {
                Ok(dirs) => dirs,
                Err(_) => continue,
            };
            for entry in project_dirs.flatten() {
                let project_dir = entry.path();
                if !project_dir.is_dir() {
                    continue;
                }
                let files = match fs::read_dir(&project_dir) {
                    Ok(files) => files,
                    Err(_) => continue,
                };
                for file_entry in files.flatten() {
                    let file_path = file_entry.path();
                    let is_dir = file_path.is_dir();
                    if file_path.extension().and_then(|ext| ext.to_str()) == Some("jsonl") {
                        all_files.push((file_path, variant.clone()));
                    } else if is_dir {
                        let subagents_dir = file_path.join("subagents");
                        if subagents_dir.is_dir() {
                            for sub_path in
                                crate::provider_utils::collect_subagent_jsonl_files(&subagents_dir)
                            {
                                all_files.push((sub_path, variant.clone()));
                            }
                        }
                    }
                }
            }
        }
        all_files
    }

    fn variant_by_path(&self, source_path: &str) -> Option<&Variant> {
        let normalized = source_path.replace('\\', "/");
        self.variants
            .iter()
            .find(|variant| normalized.starts_with(&variant.normalized_prefix))
    }

    fn variant_by_command_name(&self, command_name: &str) -> Option<&Variant> {
        let candidate = sanitize_command_name(command_name);
        if candidate.is_empty() {
            return None;
        }

        self.variants
            .iter()
            .find(|variant| variant.command_name == candidate || variant.dir_name == candidate)
    }

    fn apply_variant(parsed: &mut ParsedSession, variant: &Variant) {
        parsed.meta.provider = Provider::CcMirror;
        parsed.meta.variant_name = Some(variant.command_name.clone());
    }
}

impl SessionProvider for CcMirrorProvider {
    fn provider(&self) -> Provider {
        Provider::CcMirror
    }

    fn watch_paths(&self) -> Vec<PathBuf> {
        self.variants
            .iter()
            .map(|variant| variant.projects_dir.clone())
            .collect()
    }

    fn scan_all(&self) -> Result<Vec<ParsedSession>, ProviderError> {
        let all_files = self.collect_jsonl_files();
        let sessions: Vec<ParsedSession> = all_files
            .par_iter()
            .filter_map(|(path, variant)| {
                let mut parsed = parser::parse_session_file(path)?;
                Self::apply_variant(&mut parsed, variant);
                Some(parsed)
            })
            .collect();
        Ok(sessions)
    }

    fn scan_incremental(
        &self,
        known: &HashMap<String, SourceState>,
    ) -> Result<ScanOutcome, ProviderError> {
        // cc-mirror keeps `(path, variant)` pairs so we can't use the
        // generic helper directly — variant identity matters for
        // `apply_variant`. Inline the (size, mtime) check here.
        let all_files = self.collect_jsonl_files();
        let mut to_parse: Vec<(PathBuf, Variant)> = Vec::with_capacity(all_files.len());
        let mut unchanged_source_paths: Vec<String> = Vec::new();
        for (path, variant) in all_files {
            let path_str = path.to_string_lossy().to_string();
            match known.get(&path_str) {
                Some(state) if crate::provider::source_state_matches(&path, state) => {
                    unchanged_source_paths.push(path_str);
                }
                _ => to_parse.push((path, variant)),
            }
        }
        let parsed: Vec<ParsedSession> = to_parse
            .par_iter()
            .filter_map(|(path, variant)| {
                let mut parsed = parser::parse_session_file(path)?;
                Self::apply_variant(&mut parsed, variant);
                Some(parsed)
            })
            .collect();
        Ok(ScanOutcome {
            parsed,
            unchanged_source_paths,
        })
    }

    fn scan_source(&self, source_path: &str) -> Result<Vec<ParsedSession>, ProviderError> {
        let path = PathBuf::from(source_path);
        let variant = self.variant_by_path(source_path).cloned();
        let related_paths = crate::provider::jsonl_subagent_related_paths(&path);
        Ok(related_paths
            .par_iter()
            .filter_map(|path| {
                let mut parsed = parser::parse_session_file(path)?;
                parsed.meta.provider = Provider::CcMirror;
                if let Some(variant) = &variant {
                    Self::apply_variant(&mut parsed, variant);
                }
                Some(parsed)
            })
            .collect())
    }

    fn deletion_plan(&self, meta: &SessionMeta, children: &[SessionMeta]) -> DeletionPlan {
        crate::provider::jsonl_subagents_deletion_plan(meta, children)
    }

    fn load_messages(
        &self,
        _session_id: &str,
        source_path: &str,
    ) -> Result<LoadedSession, ProviderError> {
        let path = PathBuf::from(source_path);
        let parsed = parser::parse_session_file(&path).ok_or_else(|| {
            ProviderError::Parse(format!(
                "failed to parse CC-Mirror session file '{}'",
                path.display()
            ))
        })?;

        // Mirror Claude provider: defer persisted-output resolution to the
        // viewer (resolve_persisted_output command). See providers/claude/mod.rs.
        Ok(LoadedSession::from_parsed(parsed))
    }
}

#[cfg(test)]
mod tests {
    use super::discover_variants;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn discover_variants_prefers_variant_json_and_falls_back_to_dir_name() {
        let dir = TempDir::new().unwrap();
        let mirror_root = dir.path();

        let first = mirror_root.join("mirror-alpha");
        fs::create_dir_all(&first).unwrap();
        fs::write(first.join("variant.json"), r#"{"name":"ccalpha"}"#).unwrap();

        let second = mirror_root.join("mirror-beta");
        fs::create_dir_all(&second).unwrap();
        fs::write(second.join("variant.json"), r#"{}"#).unwrap();

        let variants = discover_variants(mirror_root);
        assert_eq!(variants.len(), 2);

        let alpha = variants
            .iter()
            .find(|variant| variant.command_name == "ccalpha")
            .unwrap();
        assert_eq!(alpha.dir_name, "mirror-alpha");

        let beta = variants
            .iter()
            .find(|variant| variant.command_name == "mirror-beta")
            .unwrap();
        assert_eq!(beta.dir_name, "mirror-beta");
    }
}
