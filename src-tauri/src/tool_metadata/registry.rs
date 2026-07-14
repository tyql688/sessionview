use crate::models::{McpToolMetadata, Provider};

#[derive(Debug, Clone, Copy)]
pub(super) struct ToolDescriptor {
    pub canonical_name: &'static str,
    pub category: &'static str,
    pub icon: &'static str,
    pub display_name: &'static str,
    aliases: &'static [&'static str],
}

impl ToolDescriptor {
    fn matches_raw(self, raw_name: &str) -> bool {
        self.canonical_name == raw_name || self.aliases.contains(&raw_name)
    }
}

const DESCRIPTORS: &[ToolDescriptor] = &[
    descriptor(
        "Bash",
        "shell",
        "💻",
        "Bash",
        &[
            "Shell",
            "shell",
            "bash",
            "exec_command",
            "shell_command",
            "run_shell_command",
            "run_in_terminal",
            "run_terminal_command",
            "run_terminal_cmd",
            "write_stdin",
            "Monitor",
            "LocalShellCall",
            "run_command",
            "AwaitShell",
        ],
    ),
    descriptor(
        "Read",
        "file",
        "📄",
        "Read",
        &["read", "ReadFile", "read_file", "view", "view_file"],
    ),
    descriptor(
        "Write",
        "file",
        "📝",
        "Write",
        &[
            "write",
            "WriteFile",
            "write_file",
            "create",
            "write_to_file",
        ],
    ),
    descriptor(
        "Edit",
        "file",
        "✏️",
        "Edit",
        &[
            "edit",
            "edit_file",
            "replace",
            "StrReplace",
            "str_replace",
            "search_replace",
            "StrReplaceFile",
            "ApplyPatch",
            "Apply_patch",
            "MultiEdit",
            "str_replace_editor",
            "apply_patch",
            "EditNotebook",
            "replace_file_content",
            "multi_replace_file_content",
        ],
    ),
    descriptor("Delete", "file", "🗑️", "Delete", &["delete"]),
    descriptor(
        "Grep",
        "search",
        "🔎",
        "Grep",
        &[
            "grep",
            "rg",
            "Search",
            "SemanticSearch",
            "grep_search",
            "search_file_content",
        ],
    ),
    descriptor(
        "Glob",
        "search",
        "🔍",
        "Glob",
        &[
            "glob",
            "file_search",
            "ReadFolder",
            "list_directory",
            "list",
            "list_dir",
            "ls",
            "find",
            "find_by_name",
        ],
    ),
    descriptor(
        "Agent",
        "agent",
        "🤖",
        "Agent",
        &[
            "Task",
            "task",
            "Subagent",
            "subagent",
            "AgentSwarm",
            "agent",
            "read_agent",
            "spawn_agent",
            "spawn_subagent",
            "wait_agent",
            "send_input",
            "close_agent",
            "invoke_subagent",
            "define_subagent",
        ],
    ),
    descriptor(
        "SendMessage",
        "agent",
        "✉️",
        "send message",
        &["send_message"],
    ),
    descriptor(
        "FollowupTask",
        "task",
        "📋",
        "followup task",
        &["followup_task"],
    ),
    descriptor("ListAgents", "agent", "🤖", "list agents", &["list_agents"]),
    descriptor("TaskCreate", "task", "📋", "task create", &[]),
    descriptor("TaskUpdate", "task", "📋", "task update", &[]),
    descriptor("TaskList", "task", "📋", "task list", &[]),
    descriptor(
        "TaskOutput",
        "task",
        "📋",
        "task output",
        &["get_task_output", "get_command_or_subagent_output"],
    ),
    descriptor("TaskStop", "task", "🛑", "task stop", &["kill_task"]),
    descriptor("Workflow", "tool", "🔁", "workflow", &[]),
    descriptor("StructuredOutput", "tool", "📊", "structured output", &[]),
    descriptor(
        "Plan",
        "plan",
        "📋",
        "Plan",
        &[
            "update_plan",
            "TodoWrite",
            "TodoList",
            "todo",
            "todowrite",
            "todo_write",
            "Enter Plan Mode",
            "EnterPlanMode",
            "ExitPlanMode",
            "enter_plan_mode",
            "exit_plan_mode",
            "manage_task",
        ],
    ),
    descriptor(
        "AskUserQuestion",
        "interaction",
        "❓",
        "ask user",
        &["request_user_input", "ask_user", "question"],
    ),
    descriptor(
        "RequestPermissions",
        "interaction",
        "🔐",
        "request permissions",
        &["request_permissions", "ask_permission", "list_permissions"],
    ),
    descriptor(
        "ScheduleWakeup",
        "cron",
        "⏰",
        "schedule wakeup",
        &["schedule"],
    ),
    descriptor("CronCreate", "cron", "⏰", "cron create", &[]),
    descriptor("CronList", "cron", "⏰", "cron list", &[]),
    descriptor("CronDelete", "cron", "⏰", "cron delete", &[]),
    descriptor("Lint", "tool", "🧹", "Lint", &["ReadLints"]),
    descriptor(
        "WebFetch",
        "web",
        "🌐",
        "web fetch",
        &["web_fetch", "webfetch", "FetchURL", "read_url_content"],
    ),
    descriptor(
        "WebSearch",
        "web",
        "🌐",
        "web search",
        &["web_search", "web_search_call", "websearch", "search_web"],
    ),
    descriptor(
        "ImageGeneration",
        "media",
        "🖼️",
        "image generation",
        &["image_generation_call", "image_generation_end"],
    ),
    descriptor(
        "DynamicTool",
        "tool",
        "🧩",
        "dynamic tool",
        &[
            "dynamic_tool_call",
            "dynamic_tool_call_request",
            "dynamic_tool_call_response",
            "load_workspace_dependencies",
            "install_workspace_dependencies",
        ],
    ),
    descriptor(
        "JavaScript",
        "tool",
        "🟨",
        "node repl",
        &["js", "js_add_node_module_dir", "js_reset"],
    ),
    descriptor(
        "ReadMediaFile",
        "media",
        "🖼️",
        "read media file",
        &["view_image"],
    ),
    descriptor(
        "ComputerUse",
        "tool",
        "🖱️",
        "computer use",
        &[
            "get_app_state",
            "list_apps",
            "click",
            "press_key",
            "scroll",
            "drag",
            "type_text",
            "set_value",
            "select_text",
            "perform_secondary_action",
        ],
    ),
    descriptor("CreateGoal", "goal", "🎯", "create goal", &["create_goal"]),
    descriptor("GetGoal", "goal", "🎯", "get goal", &["get_goal"]),
    descriptor(
        "SetGoalBudget",
        "goal",
        "🎯",
        "set goal budget",
        &["set_goal_budget"],
    ),
    descriptor("UpdateGoal", "goal", "🎯", "update goal", &["update_goal"]),
    descriptor("ToolSearch", "search", "🧰", "tool search", &["codesearch"]),
    descriptor(
        "ListMcpResourcesTool",
        "search",
        "🔌",
        "list mcp resources",
        &[
            "list_mcp_resources",
            "list_mcp_resource_templates",
            "read_mcp_resource",
        ],
    ),
    descriptor("Skill", "skill", "⚡", "skill", &["skill"]),
    descriptor("SQL", "database", "🗄️", "SQL", &["sql"]),
];

const fn descriptor(
    canonical_name: &'static str,
    category: &'static str,
    icon: &'static str,
    display_name: &'static str,
    aliases: &'static [&'static str],
) -> ToolDescriptor {
    ToolDescriptor {
        canonical_name,
        category,
        icon,
        display_name,
        aliases,
    }
}

pub(super) fn parse_mcp_tool_name(name: &str) -> Option<McpToolMetadata> {
    let rest = name.strip_prefix("mcp__")?;
    let (server, tool) = rest.split_once("__")?;
    Some(McpToolMetadata {
        server: server.to_string(),
        tool: tool.to_string(),
        display: tool.replace('_', " "),
    })
}

pub(super) fn descriptor_for(provider: Provider, raw_name: &str) -> Option<ToolDescriptor> {
    if provider == Provider::Antigravity
        && (raw_name.contains("Agent") || raw_name.contains("agent"))
    {
        return descriptor_for_canonical("Agent");
    }

    DESCRIPTORS
        .iter()
        .copied()
        .find(|descriptor| descriptor.matches_raw(raw_name))
}

pub(super) fn descriptor_for_canonical(canonical_name: &str) -> Option<ToolDescriptor> {
    DESCRIPTORS
        .iter()
        .copied()
        .find(|descriptor| descriptor.canonical_name == canonical_name)
}

pub(super) fn canonical_name(provider: Provider, raw_name: &str) -> String {
    descriptor_for(provider, raw_name)
        .map(|descriptor| descriptor.canonical_name)
        .unwrap_or(raw_name)
        .to_string()
}

pub(super) fn category_for(canonical_name: &str, raw_name: &str) -> String {
    if raw_name.starts_with("mcp__") || raw_name.starts_with("chat-") {
        return "mcp".to_string();
    }

    descriptor_for_canonical(canonical_name)
        .map(|descriptor| descriptor.category)
        .unwrap_or("unknown")
        .to_string()
}

pub(super) fn icon_for(canonical_name: &str, category: &str, raw_name: &str) -> String {
    if category == "mcp" || raw_name.starts_with("mcp__") {
        return "🔌".to_string();
    }

    descriptor_for_canonical(canonical_name)
        .map(|descriptor| descriptor.icon)
        .unwrap_or("⚙")
        .to_string()
}

pub(super) fn display_name(raw_name: &str, canonical_name: &str) -> String {
    if let Some(mcp) = parse_mcp_tool_name(raw_name) {
        return mcp.display;
    }

    raw_display_name(raw_name)
        .or_else(|| {
            descriptor_for_canonical(canonical_name).map(|descriptor| descriptor.display_name)
        })
        .unwrap_or(canonical_name)
        .to_string()
}

fn raw_display_name(raw_name: &str) -> Option<&'static str> {
    Some(match raw_name {
        "write_stdin" => "write stdin",
        "Monitor" => "monitor",
        "ScheduleWakeup" => "schedule wakeup",
        "SendMessage" => "send message",
        "update_plan" => "update plan",
        "request_user_input" => "request user input",
        "request_permissions" => "request permissions",
        "apply_patch" => "apply patch",
        "AgentSwarm" => "agent swarm",
        "spawn_agent" => "spawn agent",
        "wait_agent" => "wait agent",
        "send_input" => "send input",
        "close_agent" => "close agent",
        "send_message" => "send message",
        "followup_task" => "followup task",
        "list_agents" => "list agents",
        "list_mcp_resources" => "list mcp resources",
        "list_mcp_resource_templates" => "list mcp resource templates",
        "read_mcp_resource" => "read mcp resource",
        "todowrite" => "todo write",
        "TodoList" => "todo list",
        "question" => "question",
        "TaskCreate" => "task create",
        "TaskUpdate" => "task update",
        "TaskList" => "task list",
        "TaskOutput" => "task output",
        "TaskStop" => "task stop",
        "Workflow" => "workflow",
        "StructuredOutput" => "structured output",
        "ToolSearch" => "tool search",
        "CronCreate" => "cron create",
        "CronList" => "cron list",
        "CronDelete" => "cron delete",
        "ReadMediaFile" => "read media file",
        "view_image" => "view image",
        "FetchURL" => "fetch URL",
        "WebFetch" => "web fetch",
        "WebSearch" => "web search",
        "AskUserQuestion" => "ask user",
        "EnterPlanMode" => "enter plan mode",
        "ExitPlanMode" => "exit plan mode",
        "CreateGoal" => "create goal",
        "GetGoal" => "get goal",
        "SetGoalBudget" => "set goal budget",
        "UpdateGoal" => "update goal",
        "webfetch" => "web fetch",
        "websearch" => "web search",
        "image_generation_call" | "image_generation_end" => "image generation",
        "dynamic_tool_call" | "dynamic_tool_call_request" | "dynamic_tool_call_response" => {
            "dynamic tool"
        }
        "load_workspace_dependencies" => "load workspace dependencies",
        "install_workspace_dependencies" => "install workspace dependencies",
        "js" => "node repl",
        "js_add_node_module_dir" => "add node module dir",
        "js_reset" => "reset node repl",
        "get_app_state" => "get app state",
        "list_apps" => "list apps",
        "click" => "click",
        "scroll" => "scroll",
        "drag" => "drag",
        "press_key" => "press key",
        "type_text" => "type text",
        "set_value" => "set value",
        "select_text" => "select text",
        "perform_secondary_action" => "perform secondary action",
        "create_goal" => "create goal",
        "get_goal" => "get goal",
        "set_goal_budget" => "set goal budget",
        "update_goal" => "update goal",
        "codesearch" => "code search",
        "skill" => "skill",
        "find" => "find",
        "ls" => "ls",
        "list" => "list",
        "find_by_name" => "find by name",
        _ => return None,
    })
}
