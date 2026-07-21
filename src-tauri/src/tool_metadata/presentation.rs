use serde_json::Value;

use super::registry;
use crate::models::{ToolMetadata, ToolPresentation, ToolResultMode};

pub(super) fn refresh_tool_presentation(metadata: &mut ToolMetadata, input: Option<&Value>) {
    let input_detail = input
        .and_then(|value| input_detail_for(metadata, value))
        .or_else(|| {
            metadata
                .presentation
                .as_ref()
                .and_then(|presentation| presentation.input_detail.clone())
        });
    let result_detail = result_detail_for(metadata);
    let has_diff = result_detail
        .as_ref()
        .is_some_and(|detail| detail.diff.is_some() || detail.patch_diff.is_some());
    let failed = matches!(
        metadata.status.as_deref(),
        Some("error" | "failed" | "failure" | "cancelled" | "canceled")
    );
    let result_mode = if metadata.result_raw {
        ToolResultMode::Raw
    } else if metadata.canonical_name == "Bash" {
        ToolResultMode::Terminal
    } else if (metadata.result_kind.as_deref() == Some("file_patch")
        || matches!(metadata.canonical_name.as_str(), "Edit" | "Write"))
        && has_diff
        && !failed
    {
        ToolResultMode::Diff
    } else {
        ToolResultMode::Output
    };

    metadata.presentation = Some(ToolPresentation {
        icon: registry::icon_for(
            &metadata.canonical_name,
            &metadata.category,
            &metadata.raw_name,
        ),
        input_detail,
        result_detail,
        result_mode,
    });
}

mod input;
mod result;
mod util;

#[cfg(test)]
mod tests;

use input::input_detail_for;
use result::result_detail_for;
