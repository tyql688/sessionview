use serde_json::Value;

use super::registry;
use crate::models::{RawOutputPolicy, ToolMetadata, ToolPresentation};

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
    let raw_output_policy = raw_output_policy(metadata.result_kind.as_deref());

    metadata.presentation = Some(ToolPresentation {
        icon: registry::icon_for(
            &metadata.canonical_name,
            &metadata.category,
            &metadata.raw_name,
        ),
        input_detail,
        result_detail,
        raw_output_policy,
    });
}

fn raw_output_policy(result_kind: Option<&str>) -> RawOutputPolicy {
    match result_kind {
        Some("terminal_output") => RawOutputPolicy::SuppressTerminal,
        Some("file_patch") => RawOutputPolicy::SuppressPatchWhenDiffPresent,
        _ => RawOutputPolicy::Keep,
    }
}

mod input;
mod result;
mod util;

#[cfg(test)]
mod tests;

use input::input_detail_for;
use result::result_detail_for;
