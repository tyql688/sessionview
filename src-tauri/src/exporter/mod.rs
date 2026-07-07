mod format;
pub mod json;
pub mod markdown;

use std::path::Path;

use crate::models::SessionDetail;
use crate::provider_utils::shorten_home_path;

/// Replace home-directory paths with `~` for privacy in exports.
///
/// Keep this as a compatibility wrapper so all Rust display/privacy path
/// handling still goes through `provider_utils::shorten_home_path`.
pub(crate) fn redact_home_path(content: &str) -> String {
    shorten_home_path(content)
}

pub fn export(detail: &SessionDetail, format: &str, output_path: &str) -> anyhow::Result<()> {
    let path = Path::new(output_path);
    match format {
        "json" => json::export_json(detail, path),
        "markdown" | "md" => markdown::export_markdown(detail, path),
        _ => anyhow::bail!("unsupported export format: {format}"),
    }
}
