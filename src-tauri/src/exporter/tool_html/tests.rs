use super::*;
use crate::models::{RawOutputPolicy, ToolDetail, ToolLine, ToolMetadata, ToolPresentation};
use serde_json::json;

fn metadata(canonical: &str, category: &str) -> ToolMetadata {
    ToolMetadata {
        raw_name: canonical.to_string(),
        canonical_name: canonical.to_string(),
        display_name: canonical.to_string(),
        category: category.to_string(),
        summary: None,
        status: None,
        ids: Default::default(),
        mcp: None,
        result_kind: None,
        structured: None,
        presentation: None,
    }
}

#[test]
fn html_escape_replaces_all_reserved_characters() {
    assert_eq!(
        html_escape(r#"<a href="x">&'</a>"#),
        "&lt;a href=&quot;x&quot;&gt;&amp;&#39;&lt;/a&gt;"
    );
}

#[test]
fn truncate_char_boundary_does_not_split_multibyte() {
    // "café" is 5 bytes (é is 2). Truncating at 4 must back off to 3
    // so we never slice mid-codepoint.
    assert_eq!(truncate_char_boundary("café", 4), "caf");
    assert_eq!(truncate_char_boundary("ab", 10), "ab");
}

#[test]
fn is_path_label_matches_path_suffixes() {
    assert!(is_path_label("file"));
    assert!(is_path_label("path"));
    assert!(is_path_label("filePath"));
    assert!(is_path_label("FILE"));
    assert!(!is_path_label("command"));
}

#[test]
fn render_field_escapes_label_and_value() {
    let html = render_field("name", "a<b>&c");
    assert!(html.contains(r#"<span class="tool-field-label">name</span>"#));
    assert!(html.contains("a&lt;b&gt;&amp;c"));
}

#[test]
fn render_line_diff_marks_added_and_removed_lines() {
    let html = render_line_diff("one\ntwo\n", "one\nTWO\n");
    // The unchanged line is context, the changed line shows as remove+add.
    assert!(html.contains(r#"<div class="tool-line-diff">"#));
    assert!(html.contains(r#"tool-diff-line context"#));
    assert!(html.contains(r#"tool-diff-line remove"#));
    assert!(html.contains(r#"tool-diff-line add"#));
    assert!(html.contains(">two<"));
    assert!(html.contains(">TWO<"));
}

#[test]
fn render_patch_diff_classifies_marker_lines() {
    let patch = "*** Begin Patch\n*** Update File: a.txt\n@@ ctx @@\n+added line\n-removed line\n unchanged\n*** End Patch";
    let html = render_patch_diff(patch);
    // Begin/End patch sentinels are dropped; +/-/space prefixes map to
    // add/remove/context, and @@/*** headers become "skip".
    assert!(!html.contains("Begin Patch"));
    assert!(!html.contains("End Patch"));
    assert!(html.contains(r#"tool-diff-line skip"#)); // the @@ / *** Update header
    assert!(html.contains("added line"));
    assert!(html.contains("removed line"));
    assert!(html.contains("unchanged"));
}

#[test]
fn tool_icon_uses_canonical_name_then_falls_back() {
    assert_eq!(tool_icon("Read", None), "📄");
    assert_eq!(tool_icon("Bash", None), "💻");
    assert_eq!(tool_icon("ReadMediaFile", None), "🖼️");
    assert_eq!(tool_icon("JavaScript", None), "🟨");
    assert_eq!(tool_icon("ComputerUse", None), "🖱️");
    assert_eq!(tool_icon("StructuredOutput", None), "📊");
    assert_eq!(tool_icon("Workflow", None), "🔁");
    assert_eq!(tool_icon("TaskOutput", None), "📋");
    assert_eq!(tool_icon("CronList", None), "⏰");
    assert_eq!(tool_icon("SetGoalBudget", None), "🎯");
    // Unknown tool → default gear.
    assert_eq!(tool_icon("Frobnicate", None), "⚙");
    // mcp category / mcp__ prefix → plug.
    assert_eq!(tool_icon("mcp__server__do", None), "🔌");
    assert_eq!(tool_icon("anything", Some(&metadata("Read", "mcp"))), "🔌");
    // metadata canonical name wins over the raw name.
    assert_eq!(tool_icon("raw", Some(&metadata("Grep", "search"))), "🔎");
}

#[test]
fn tool_display_name_prefers_metadata() {
    assert_eq!(tool_display_name("raw_tool", None), "raw_tool");
    let mut md = metadata("Read", "file");
    md.display_name = "Read File".into();
    assert_eq!(tool_display_name("raw_tool", Some(&md)), "Read File");
}

#[test]
fn tool_summary_prefers_metadata_summary() {
    let mut md = metadata("Bash", "shell");
    md.summary = Some("run the build".into());
    assert_eq!(
        tool_summary("Bash", r#"{"command":"make"}"#, Some(&md)),
        "run the build"
    );
}

#[test]
fn tool_summary_extracts_file_path_for_read() {
    // /srv/... is outside any home dir, so shorten_home_path leaves it
    // intact and we can assert the raw extracted file path.
    let summary = tool_summary("Read", r#"{"file_path":"/srv/proj/a.rs"}"#, None);
    assert_eq!(summary, "/srv/proj/a.rs");
}

#[test]
fn tool_summary_truncates_long_bash_command() {
    let long = "x".repeat(100);
    let input = json!({ "command": long }).to_string();
    let summary = tool_summary("Bash", &input, None);
    assert!(summary.ends_with("..."));
    // 57 chars kept + "..." == 60.
    assert_eq!(summary.chars().count(), 60);
}

#[test]
fn tool_summary_reads_apply_patch_file_line() {
    let input = "*** Begin Patch\n*** Update File: /srv/proj/x.rs\n+new\n*** End Patch";
    assert_eq!(tool_summary("Apply_patch", input, None), "/srv/proj/x.rs");
}

#[test]
fn tool_summary_handles_kimi_specific_tools() {
    assert_eq!(
        tool_summary("TaskOutput", r#"{"task_id":"task-123","block":true}"#, None),
        "task-123 · wait"
    );
    assert_eq!(
        tool_summary("SetGoalBudget", r#"{"value":3,"unit":"turns"}"#, None),
        "3 · turns"
    );
    assert_eq!(
        tool_summary(
            "ReadMediaFile",
            r#"{"path":"/Users/alice/image.png"}"#,
            None
        ),
        "~/image.png"
    );
}

#[test]
fn render_tool_input_detail_renders_bash_command_block() {
    let html = render_tool_input_detail("Bash", r#"{"command":"echo hi"}"#);
    assert!(html.contains(r#"<pre class="tool-cmd">echo hi</pre>"#));
    assert!(html.contains(r#"<span class="tool-field-label">$</span>"#));
}

#[test]
fn render_tool_input_detail_renders_edit_line_diff() {
    let input = json!({
        "file_path": "/srv/proj/a.rs",
        "old_string": "let x = 1;",
        "new_string": "let x = 2;",
    })
    .to_string();
    let html = render_tool_input_detail("Edit", &input);
    assert!(html.contains("/srv/proj/a.rs"));
    assert!(html.contains(r#"<div class="tool-line-diff">"#));
    assert!(html.contains("let x = 1;"));
    assert!(html.contains("let x = 2;"));
}

#[test]
fn render_tool_input_detail_falls_back_to_raw_for_non_object() {
    let html = render_tool_input_detail("Unknown", "not json at all");
    assert_eq!(html, r#"<pre class="tool-raw">not json at all</pre>"#);
}

#[test]
fn render_tool_result_detail_empty_without_structured() {
    assert_eq!(render_tool_result_detail(None), "");
    assert_eq!(
        render_tool_result_detail(Some(&metadata("Bash", "shell"))),
        ""
    );
}

#[test]
fn render_tool_result_detail_prefers_presentation_without_structured() {
    let mut md = metadata("Bash", "shell");
    md.presentation = Some(ToolPresentation {
        icon: "💻".to_string(),
        input_detail: None,
        result_detail: Some(ToolDetail {
            lines: vec![ToolLine {
                label: "stdout".to_string(),
                value: "hello from presentation".to_string(),
            }],
            diff: None,
            patch_diff: None,
            persisted_output_path: None,
        }),
        raw_output_policy: RawOutputPolicy::SuppressTerminal,
    });

    let html = render_tool_result_detail(Some(&md));
    assert!(html.contains("hello from presentation"));
    assert!(suppress_raw_output(Some(&md), false));
}

#[test]
fn suppress_raw_output_honours_presentation_policy() {
    let mut md = metadata("Bash", "shell");
    md.presentation = Some(ToolPresentation {
        icon: "💻".to_string(),
        input_detail: None,
        result_detail: None,
        raw_output_policy: RawOutputPolicy::SuppressTerminal,
    });
    assert!(suppress_raw_output(Some(&md), false));

    md.presentation = Some(ToolPresentation {
        icon: "✏️".to_string(),
        input_detail: None,
        result_detail: None,
        raw_output_policy: RawOutputPolicy::SuppressPatchWhenDiffPresent,
    });
    assert!(suppress_raw_output(Some(&md), true));
    assert!(!suppress_raw_output(Some(&md), false));

    md.presentation = Some(ToolPresentation {
        icon: "⚙".to_string(),
        input_detail: None,
        result_detail: None,
        raw_output_policy: RawOutputPolicy::Keep,
    });
    assert!(!suppress_raw_output(Some(&md), true));
    assert!(!suppress_raw_output(None, true));
}

#[test]
fn should_skip_tool_only_skips_unresolved_toolu_ids() {
    assert!(should_skip_tool("toolu_01ABC", None));
    assert!(!should_skip_tool(
        "toolu_01ABC",
        Some(&metadata("Read", "file"))
    ));
    assert!(!should_skip_tool("Read", None));
}
