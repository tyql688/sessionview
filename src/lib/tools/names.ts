import type { Message, ToolMetadata } from "../types";
import { shortenHomePath } from "../formatters";
import { firstString } from "./types";

const TOOL_ICONS: Record<string, string> = {
  Read: "📄",
  Edit: "✏️",
  Apply_patch: "✏️",
  Plan: "📋",
  Write: "📝",
  Bash: "💻",
  Glob: "🔍",
  Grep: "🔎",
  Agent: "🤖",
  WebSearch: "🌐",
  WebFetch: "🌐",
  ImageGeneration: "🖼️",
  DynamicTool: "🧩",
  JavaScript: "🟨",
  ComputerUse: "🖱️",
  TaskCreate: "📋",
  TaskUpdate: "📋",
  TaskList: "📋",
  TaskOutput: "📋",
  TaskStop: "🛑",
  Workflow: "🔁",
  StructuredOutput: "📊",
  ToolSearch: "🧰",
  Skill: "⚡",
  AskUserQuestion: "❓",
  CronCreate: "⏰",
  CronList: "⏰",
  CronDelete: "⏰",
  ReadMediaFile: "🖼️",
  EnterPlanMode: "🧭",
  ExitPlanMode: "🧭",
  CreateGoal: "🎯",
  GetGoal: "🎯",
  SetGoalBudget: "🎯",
  UpdateGoal: "🎯",
  SendMessage: "✉️",
  FollowupTask: "📋",
  ListAgents: "🤖",
  RequestPermissions: "🔐",
  ListMcpResourcesTool: "🔌",
  mcp: "🔌",
};

/** Parse MCP tool name: mcp__server__tool → { server, tool, display } */
export function parseMcpToolName(
  name: string,
): { server: string; tool: string; display: string } | null {
  if (!name.startsWith("mcp__")) return null;
  const parts = name.slice(5).split("__");
  if (parts.length < 2) return null;
  const tool = parts.slice(1).join("__");
  return { server: parts[0], tool, display: tool.replace(/_/g, " ") };
}

export function formatMcpLabel(name: string): string {
  const mcp = parseMcpToolName(name);
  return mcp ? mcp.display : name;
}

export function toolDisplayName(name: string, metadata?: ToolMetadata): string {
  if (metadata?.display_name) return metadata.display_name;
  return formatMcpLabel(name);
}

export function toolIcon(name: string, metadata?: ToolMetadata): string {
  if (metadata?.category === "mcp" || name.startsWith("mcp__")) {
    return TOOL_ICONS.mcp;
  }
  return (
    TOOL_ICONS[metadata?.canonical_name ?? name] || TOOL_ICONS[name] || "⚙"
  );
}

function joinParts(parts: string[]): string {
  return parts.filter((part) => part.length > 0).join(" · ");
}

function optionalNumber(obj: Record<string, unknown>, key: string): string {
  const value = obj[key];
  return typeof value === "number" ? value.toLocaleString() : "";
}

/** Extract a human-readable summary from tool input JSON. */
export function toolSummary(message: Message): string {
  const name = message.tool_name || "";
  if (message.tool_metadata?.summary) return message.tool_metadata.summary;
  const inputJson = message.tool_input;
  if (!inputJson) return "";

  try {
    const obj = JSON.parse(inputJson) as Record<string, unknown>;
    switch (name) {
      case "Read":
      case "Edit":
      case "Write":
        return shortenHomePath(
          firstString(obj, [
            "file_path",
            "filePath",
            "path",
            // Antigravity PascalCase aliases
            "AbsolutePath",
            "TargetFile",
          ]),
        );
      case "Bash":
        return firstString(obj, [
          "description",
          "command",
          "cmd",
          "CommandLine",
        ]).slice(0, 80);
      case "Glob":
        return firstString(obj, ["pattern", "DirectoryPath"]);
      case "Grep": {
        const pattern = firstString(obj, ["pattern", "query", "Query"]);
        const path = firstString(obj, ["path", "SearchPath"]);
        return `/${pattern}/${path ? ` ${shortenHomePath(path)}` : ""}`;
      }
      case "Agent":
        return firstString(obj, ["description", "prompt"]);
      case "TaskList":
        return joinParts([
          typeof obj.active_only === "boolean"
            ? obj.active_only
              ? "active"
              : "all"
            : "",
          optionalNumber(obj, "limit")
            ? `limit ${optionalNumber(obj, "limit")}`
            : "",
        ]);
      case "TaskOutput":
        return joinParts([
          firstString(obj, ["task_id", "taskId"]),
          obj.block === true ? "wait" : "",
        ]);
      case "TaskStop":
        return joinParts([
          firstString(obj, ["task_id", "taskId"]),
          firstString(obj, ["reason"]),
        ]);
      case "Workflow":
        return firstString(obj, ["name", "description", "script"]).slice(0, 80);
      case "StructuredOutput":
        return firstString(obj, [
          "finding_id",
          "title",
          "analysis",
          "summary",
          "corrected_root_cause",
          "minimal_fix",
        ]).slice(0, 80);
      case "CronCreate":
        return joinParts([
          firstString(obj, ["cron"]),
          firstString(obj, ["prompt"]).slice(0, 80),
        ]);
      case "CronDelete":
        return firstString(obj, ["id"]);
      case "Skill":
        return firstString(obj, ["skill"]);
      case "ToolSearch":
      case "WebSearch":
        return firstString(obj, ["query", "Query"]);
      case "WebFetch":
        return firstString(obj, ["url", "Url"]);
      case "ReadMediaFile":
        return shortenHomePath(firstString(obj, ["path"]));
      case "JavaScript":
        return firstString(obj, ["title", "code"]).slice(0, 80);
      case "ComputerUse":
        return joinParts([
          firstString(obj, ["app"]),
          firstString(obj, ["key", "direction", "element_index", "action"]),
        ]);
      case "AskUserQuestion": {
        const questions = Array.isArray(obj.questions)
          ? `${obj.questions.length} question(s)`
          : "";
        return joinParts([
          questions,
          obj.background === true ? "background" : "",
        ]);
      }
      case "CreateGoal":
        return firstString(obj, ["objective"]).slice(0, 80);
      case "SetGoalBudget":
        return joinParts([
          optionalNumber(obj, "value"),
          firstString(obj, ["unit"]),
        ]);
      case "UpdateGoal":
        return firstString(obj, ["status"]);
      default: {
        const first = Object.values(obj).find(
          (v) => typeof v === "string" && (v as string).length > 0,
        );
        return first ? String(first).slice(0, 80) : "";
      }
    }
  } catch (error) {
    console.warn(`Failed to summarize tool input for ${name}:`, error);
    if (name === "Agent") {
      const m = inputJson.match(/"description"\s*:\s*"([^"]+)"/);
      if (m) return m[1];
    }
    return "";
  }
}
