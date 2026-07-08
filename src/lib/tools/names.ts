import type { Message, ToolMetadata } from "@/lib/types";
import { shortenHomePath } from "@/lib/formatters";
import { firstString } from "@/lib/tools/types";

/** Parse MCP tool name: mcp__server__tool → { server, tool, display } */
export function parseMcpToolName(name: string): { server: string; tool: string; display: string } | null {
  if (!name.startsWith("mcp__")) return null;
  const parts = name.slice(5).split("__");
  if (parts.length < 2) return null;
  const tool = parts.slice(1).join("__");
  return { server: parts[0], tool, display: tool.replace(/_/g, " ") };
}

function formatMcpLabel(name: string): string {
  const mcp = parseMcpToolName(name);
  return mcp ? mcp.display : name;
}

export function toolDisplayName(name: string, metadata?: ToolMetadata): string {
  if (metadata?.display_name) return metadata.display_name;
  return formatMcpLabel(name);
}

function joinParts(parts: string[]): string {
  return parts.filter((part) => part.length > 0).join(" · ");
}

function optionalNumber(obj: Record<string, unknown>, key: string): string {
  const value = obj[key];
  return typeof value === "number" ? value.toLocaleString() : "";
}

function parseToolInput(raw: string): Record<string, unknown> | null {
  try {
    const parsed: unknown = JSON.parse(raw);
    return parsed !== null && typeof parsed === "object" && !Array.isArray(parsed)
      ? (parsed as Record<string, unknown>)
      : null;
  } catch {
    return null;
  }
}

/** Extract a human-readable summary from tool input JSON. */
export function toolSummary(message: Message): string {
  const name = message.tool_name || "";
  if (message.tool_metadata?.summary) return message.tool_metadata.summary;
  const inputJson = message.tool_input;
  if (!inputJson) return "";

  const obj = parseToolInput(inputJson);
  if (obj === null) {
    if (name === "Agent") {
      const m = inputJson.match(/"description"\s*:\s*"([^"]+)"/);
      if (m) return m[1];
    }
    return "";
  }

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
      return firstString(obj, ["description", "command", "cmd", "CommandLine"]);
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
        typeof obj.active_only === "boolean" ? (obj.active_only ? "active" : "all") : "",
        optionalNumber(obj, "limit") ? `limit ${optionalNumber(obj, "limit")}` : "",
      ]);
    case "TaskOutput":
      return joinParts([firstString(obj, ["task_id", "taskId"]), obj.block === true ? "wait" : ""]);
    case "TaskStop":
      return joinParts([firstString(obj, ["task_id", "taskId"]), firstString(obj, ["reason"])]);
    case "Workflow":
      return firstString(obj, ["name", "description", "script"]);
    case "StructuredOutput":
      return firstString(obj, ["finding_id", "title", "analysis", "summary", "corrected_root_cause", "minimal_fix"]);
    case "CronCreate":
      return joinParts([firstString(obj, ["cron"]), firstString(obj, ["prompt"])]);
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
      return firstString(obj, ["title", "code"]);
    case "ComputerUse":
      return joinParts([firstString(obj, ["app"]), firstString(obj, ["key", "direction", "element_index", "action"])]);
    case "AskUserQuestion": {
      const questions = Array.isArray(obj.questions) ? `${obj.questions.length} question(s)` : "";
      return joinParts([questions, obj.background === true ? "background" : ""]);
    }
    case "CreateGoal":
      return firstString(obj, ["objective"]);
    case "SetGoalBudget":
      return joinParts([optionalNumber(obj, "value"), firstString(obj, ["unit"])]);
    case "UpdateGoal":
      return firstString(obj, ["status"]);
    default: {
      const first = Object.values(obj).find((v) => typeof v === "string" && (v as string).length > 0);
      return first ? String(first) : "";
    }
  }
}
