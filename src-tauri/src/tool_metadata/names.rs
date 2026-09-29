use crate::models::{McpToolMetadata, Provider};

pub fn parse_mcp_tool_name(name: &str) -> Option<McpToolMetadata> {
    super::registry::parse_mcp_tool_name(name)
}

pub fn canonical_tool_name(provider: Provider, name: &str) -> String {
    super::registry::canonical_name(provider, name)
}

pub(super) fn tool_category(canonical_name: &str, raw_name: &str) -> String {
    super::registry::category_for(canonical_name, raw_name)
}

pub(super) fn display_tool_name(raw_name: &str, canonical_name: &str) -> String {
    super::registry::display_name(raw_name, canonical_name)
}

#[cfg(test)]
mod tests {
    use super::{canonical_tool_name, parse_mcp_tool_name};
    use crate::models::Provider;

    #[test]
    fn parses_mcp_tool_names() {
        let mcp = parse_mcp_tool_name("mcp__plugin_playwright__browser_snapshot").unwrap();
        assert_eq!(mcp.server, "plugin_playwright");
        assert_eq!(mcp.tool, "browser_snapshot");
        assert_eq!(mcp.display, "browser snapshot");
    }

    #[test]
    fn maps_antigravity_find_by_name_to_file_search() {
        assert_eq!(
            canonical_tool_name(Provider::Antigravity, "find_by_name"),
            "Glob"
        );
    }

    #[test]
    fn maps_pi_find_and_ls_to_glob_without_losing_display_name() {
        assert_eq!(canonical_tool_name(Provider::Pi, "find"), "Glob");
        assert_eq!(canonical_tool_name(Provider::Pi, "ls"), "Glob");
        assert_eq!(super::display_tool_name("find", "Glob"), "find");
        assert_eq!(super::display_tool_name("ls", "Glob"), "ls");
    }

    #[test]
    fn maps_recent_provider_tool_aliases_through_registry() {
        assert_eq!(
            canonical_tool_name(Provider::Claude, "TaskOutput"),
            "TaskOutput"
        );
        assert_eq!(
            canonical_tool_name(Provider::Kimi, "ReadMediaFile"),
            "ReadMediaFile"
        );
        assert_eq!(canonical_tool_name(Provider::Codex, "js"), "JavaScript");
        assert_eq!(
            canonical_tool_name(Provider::CcMirror, "load_workspace_dependencies"),
            "DynamicTool"
        );
        assert_eq!(
            canonical_tool_name(Provider::Antigravity, "invoke_subagent"),
            "Agent"
        );
    }

    #[test]
    fn codex_code_mode_and_current_tools_keep_provider_specific_meaning() {
        assert_eq!(
            canonical_tool_name(Provider::Codex, "exec"),
            "CodeExecution"
        );
        assert_eq!(canonical_tool_name(Provider::Pi, "exec"), "Bash");
        assert_eq!(canonical_tool_name(Provider::Codex, "exec_command"), "Bash");
        assert_eq!(canonical_tool_name(Provider::Codex, "wait"), "Wait");
        assert_eq!(
            canonical_tool_name(Provider::Codex, "request_user_input_async"),
            "AskUserQuestion"
        );
        assert_eq!(
            canonical_tool_name(Provider::Codex, "interrupt_agent"),
            "Agent"
        );
    }
}
