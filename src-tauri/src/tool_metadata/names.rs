use crate::models::{McpToolMetadata, Provider};

pub fn parse_mcp_tool_name(name: &str) -> Option<McpToolMetadata> {
    let rest = name.strip_prefix("mcp__")?;
    let (server, tool) = rest.split_once("__")?;
    Some(McpToolMetadata {
        server: server.to_string(),
        tool: tool.to_string(),
        display: tool.replace('_', " "),
    })
}

pub fn canonical_tool_name(provider: Provider, name: &str) -> String {
    if provider == Provider::Antigravity && (name.contains("Agent") || name.contains("agent")) {
        return "Agent".to_string();
    }

    match name {
        "Shell" | "shell" | "bash" | "exec_command" | "shell_command" | "run_shell_command"
        | "run_in_terminal" | "write_stdin" | "Monitor" | "LocalShellCall" | "run_command"
        | "AwaitShell" => "Bash",
        "Read" | "read" | "ReadFile" | "read_file" | "view" | "view_file" => "Read",
        "read_mcp_resource" => "ListMcpResourcesTool",
        "Write" | "write" | "WriteFile" | "write_file" | "create" | "write_to_file" => "Write",
        "Edit"
        | "edit"
        | "edit_file"
        | "replace"
        | "StrReplace"
        | "str_replace"
        | "StrReplaceFile"
        | "ApplyPatch"
        | "Apply_patch"
        | "MultiEdit"
        | "str_replace_editor"
        | "apply_patch"
        | "EditNotebook"
        | "replace_file_content"
        | "multi_replace_file_content" => "Edit",
        "Delete" | "delete" => "Delete",
        "Grep"
        | "grep"
        | "rg"
        | "Search"
        | "SemanticSearch"
        | "grep_search"
        | "search_file_content" => "Grep",
        "Glob" | "glob" | "file_search" | "ReadFolder" | "list_directory" | "list" | "list_dir" => {
            "Glob"
        }
        "Task" | "task" | "Subagent" | "agent" | "read_agent" | "spawn_agent" | "wait_agent"
        | "send_input" | "close_agent" | "invoke_subagent" | "define_subagent" => "Agent",
        "send_message" => "SendMessage",
        "followup_task" => "FollowupTask",
        "list_agents" => "ListAgents",
        "update_plan" | "TodoWrite" | "todo" | "todowrite" | "Enter Plan Mode"
        | "EnterPlanMode" | "ExitPlanMode" | "enter_plan_mode" | "exit_plan_mode"
        | "manage_task" => "Plan",
        "request_user_input" | "ask_user" | "question" => "AskUserQuestion",
        "request_permissions" | "ask_permission" | "list_permissions" => "RequestPermissions",
        "ScheduleWakeup" | "schedule" => "ScheduleWakeup",
        "ReadLints" => "Lint",
        "web_fetch" | "webfetch" | "read_url_content" => "WebFetch",
        "web_search" | "web_search_call" | "websearch" | "search_web" => "WebSearch",
        "image_generation_call" | "image_generation_end" => "ImageGeneration",
        "dynamic_tool_call"
        | "dynamic_tool_call_request"
        | "dynamic_tool_call_response"
        | "load_workspace_dependencies"
        | "install_workspace_dependencies" => "DynamicTool",
        "codesearch" => "ToolSearch",
        "list_mcp_resources" | "list_mcp_resource_templates" => "ListMcpResourcesTool",
        "skill" => "Skill",
        "sql" | "SQL" => "SQL",
        other => other,
    }
    .to_string()
}

pub(super) fn tool_category(canonical_name: &str, raw_name: &str) -> String {
    if raw_name.starts_with("mcp__") || raw_name.starts_with("chat-") {
        return "mcp".to_string();
    }

    match canonical_name {
        "Bash" => "shell",
        "Read" | "Write" | "Edit" | "Delete" => "file",
        "Grep" | "Glob" | "ToolSearch" | "ListMcpResourcesTool" => "search",
        "Agent" | "SendMessage" | "ListAgents" => "agent",
        "TaskCreate" | "TaskUpdate" | "TaskList" | "TaskStop" => "task",
        "FollowupTask" => "task",
        "WebSearch" | "WebFetch" => "web",
        "ImageGeneration" => "media",
        "DynamicTool" => "tool",
        "Skill" => "skill",
        "CronCreate" | "CronDelete" | "ScheduleWakeup" => "cron",
        "Plan" => "plan",
        "AskUserQuestion" | "RequestPermissions" => "interaction",
        "SQL" => "database",
        _ => "unknown",
    }
    .to_string()
}

pub(super) fn display_tool_name(raw_name: &str, canonical_name: &str) -> String {
    if let Some(mcp) = parse_mcp_tool_name(raw_name) {
        return mcp.display;
    }
    match raw_name {
        "write_stdin" => "write stdin".to_string(),
        "Monitor" => "monitor".to_string(),
        "ScheduleWakeup" => "schedule wakeup".to_string(),
        "update_plan" => "update plan".to_string(),
        "request_user_input" => "request user input".to_string(),
        "request_permissions" => "request permissions".to_string(),
        "apply_patch" => "apply patch".to_string(),
        "spawn_agent" => "spawn agent".to_string(),
        "wait_agent" => "wait agent".to_string(),
        "send_input" => "send input".to_string(),
        "close_agent" => "close agent".to_string(),
        "send_message" => "send message".to_string(),
        "followup_task" => "followup task".to_string(),
        "list_agents" => "list agents".to_string(),
        "list_mcp_resources" => "list mcp resources".to_string(),
        "list_mcp_resource_templates" => "list mcp resource templates".to_string(),
        "read_mcp_resource" => "read mcp resource".to_string(),
        "todowrite" => "todo write".to_string(),
        "question" => "question".to_string(),
        "webfetch" => "web fetch".to_string(),
        "websearch" => "web search".to_string(),
        "image_generation_call" | "image_generation_end" => "image generation".to_string(),
        "dynamic_tool_call" | "dynamic_tool_call_request" | "dynamic_tool_call_response" => {
            "dynamic tool".to_string()
        }
        "load_workspace_dependencies" => "load workspace dependencies".to_string(),
        "install_workspace_dependencies" => "install workspace dependencies".to_string(),
        "codesearch" => "code search".to_string(),
        "skill" => "skill".to_string(),
        "list" => "list".to_string(),
        _ => canonical_name.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::parse_mcp_tool_name;

    #[test]
    fn parses_mcp_tool_names() {
        let mcp = parse_mcp_tool_name("mcp__plugin_playwright__browser_snapshot").unwrap();
        assert_eq!(mcp.server, "plugin_playwright");
        assert_eq!(mcp.tool, "browser_snapshot");
        assert_eq!(mcp.display, "browser snapshot");
    }
}
