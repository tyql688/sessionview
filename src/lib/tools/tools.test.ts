import { describe, expect, it } from "vitest";

import {
  formatToolInput,
  formatToolResultMetadata,
  parseMcpToolName,
  toolDisplayName,
  toolIcon,
  toolSummary,
} from "@/lib/tools/index";
import type { Message } from "@/lib/types";

const baseMessage: Message = {
  role: "tool",
  content: "",
  timestamp: null,
  tool_name: null,
  tool_input: null,
  token_usage: null,
};

describe("tools/names", () => {
  it("parses and displays MCP tool names", () => {
    const name = "mcp__plugin_playwright_playwright__browser_take_screenshot";

    expect(parseMcpToolName(name)).toEqual({
      server: "plugin_playwright_playwright",
      tool: "browser_take_screenshot",
      display: "browser take screenshot",
    });
    expect(toolIcon(name)).toBe("🔌");
    expect(toolDisplayName(name)).toBe("browser take screenshot");
  });

  it("uses Claude metadata summaries before falling back to input JSON", () => {
    const message: Message = {
      ...baseMessage,
      tool_name: "TaskUpdate",
      tool_input: JSON.stringify({ taskId: "1", status: "in_progress" }),
      tool_metadata: {
        raw_name: "TaskUpdate",
        canonical_name: "TaskUpdate",
        display_name: "TaskUpdate",
        category: "task",
        summary: "Fix Live2D leak",
      },
    };

    expect(toolSummary(message)).toBe("Fix Live2D leak");
  });

  it("summarizes Grep input into /pattern/ path form", () => {
    expect(
      toolSummary({
        ...baseMessage,
        tool_name: "Grep",
        tool_input: JSON.stringify({
          pattern: "fn main",
          path: "/Users/alice/repo/src",
        }),
      }),
    ).toBe("/fn main/ ~/repo/src");
  });

  it("returns image and dynamic tool icons", () => {
    expect(toolIcon("ImageGeneration")).toBe("🖼️");
    expect(toolIcon("DynamicTool")).toBe("🧩");
    expect(toolIcon("JavaScript")).toBe("🟨");
    expect(toolIcon("ComputerUse")).toBe("🖱️");
    expect(toolIcon("StructuredOutput")).toBe("📊");
  });

  it("prefers presentation icons from metadata", () => {
    expect(
      toolIcon("raw_tool", {
        raw_name: "raw_tool",
        canonical_name: "Unknown",
        display_name: "Unknown",
        category: "unknown",
        presentation: {
          icon: "★",
          rawOutputPolicy: "keep",
        },
      }),
    ).toBe("★");
  });

  it("summarizes Kimi-specific tool fallbacks", () => {
    expect(toolIcon("ReadMediaFile")).toBe("🖼️");
    expect(toolIcon("TaskOutput")).toBe("📋");
    expect(toolIcon("CronList")).toBe("⏰");
    expect(toolIcon("SetGoalBudget")).toBe("🎯");
    expect(
      toolSummary({
        ...baseMessage,
        tool_name: "TaskOutput",
        tool_input: JSON.stringify({ task_id: "task-123", block: true }),
      }),
    ).toBe("task-123 · wait");
    expect(
      toolSummary({
        ...baseMessage,
        tool_name: "SetGoalBudget",
        tool_input: JSON.stringify({ value: 3, unit: "turns" }),
      }),
    ).toBe("3 · turns");
  });

  it("summarizes recently observed Claude and Codex tools", () => {
    expect(
      toolSummary({
        ...baseMessage,
        tool_name: "StructuredOutput",
        tool_input: JSON.stringify({
          finding_id: "P1",
          analysis: "unclassified tool",
        }),
      }),
    ).toBe("P1");
    expect(
      toolSummary({
        ...baseMessage,
        tool_name: "Workflow",
        tool_input: JSON.stringify({ script: "cargo test" }),
      }),
    ).toBe("cargo test");
    expect(
      toolSummary({
        ...baseMessage,
        tool_name: "JavaScript",
        tool_input: JSON.stringify({
          title: "Inspect payload",
          code: "await inspect()",
        }),
      }),
    ).toBe("Inspect payload");
    expect(
      toolSummary({
        ...baseMessage,
        tool_name: "ComputerUse",
        tool_input: JSON.stringify({ app: "Codex", key: "Return" }),
      }),
    ).toBe("Codex · Return");
  });
});

describe("tools/input", () => {
  it("returns Rust presentation input detail", () => {
    const detail = formatToolInput({
      ...baseMessage,
      tool_name: "Grep",
      tool_input: JSON.stringify({ pattern: "fn main" }),
      tool_metadata: {
        raw_name: "Grep",
        canonical_name: "Grep",
        display_name: "Grep",
        category: "search",
        presentation: {
          icon: "🔎",
          rawOutputPolicy: "keep",
          inputDetail: {
            lines: [
              { label: "pattern", value: "fn main" },
              { label: "path", value: "~/repo/src" },
            ],
          },
        },
      },
    });

    expect(detail).toEqual({
      lines: [
        { label: "pattern", value: "fn main" },
        { label: "path", value: "~/repo/src" },
      ],
    });
  });

  it("keeps full diff input presentation from metadata", () => {
    const detail = formatToolInput({
      ...baseMessage,
      tool_name: "Edit",
      tool_metadata: {
        raw_name: "Edit",
        canonical_name: "Edit",
        display_name: "Edit",
        category: "file",
        presentation: {
          icon: "✏️",
          rawOutputPolicy: "keep",
          inputDetail: {
            lines: [{ label: "file", value: "/tmp/project/main.rs" }],
            diff: { old: "old line", new: "new line" },
          },
        },
      },
    });

    expect(detail?.lines).toEqual([
      { label: "file", value: "/tmp/project/main.rs" },
    ]);
    expect(detail?.diff).toEqual({ old: "old line", new: "new line" });
  });

  it("does not synthesize legacy input detail without presentation", () => {
    const detail = formatToolInput({
      ...baseMessage,
      tool_name: "Edit",
      tool_input: JSON.stringify({
        file_path: "/tmp/project/main.rs",
        old_string: "old",
        new_string: "new",
      }),
    });

    expect(detail).toBeNull();
  });
});

describe("tools/result", () => {
  it("returns Rust presentation result detail", () => {
    const detail = formatToolResultMetadata({
      raw_name: "bash",
      canonical_name: "Bash",
      display_name: "Bash",
      category: "shell",
      status: "success",
      presentation: {
        icon: "💻",
        rawOutputPolicy: "suppress_terminal",
        resultDetail: {
          lines: [
            { label: "status", value: "success" },
            { label: "stdout", value: "hello from pi" },
          ],
        },
      },
    });

    expect(detail).toEqual({
      lines: [
        { label: "status", value: "success" },
        { label: "stdout", value: "hello from pi" },
      ],
    });
  });

  it("keeps full diff presentation from metadata", () => {
    const detail = formatToolResultMetadata({
      raw_name: "Edit",
      canonical_name: "Edit",
      display_name: "Edit",
      category: "file",
      status: "success",
      presentation: {
        icon: "✏️",
        rawOutputPolicy: "suppress_patch_when_diff_present",
        resultDetail: {
          lines: [{ label: "file", value: "/tmp/App.tsx" }],
          patchDiff: Array.from({ length: 220 }, (_, index) => ({
            type: index % 2 === 0 ? "add" : "remove",
            oldLine: null,
            newLine: index + 1,
            text: `line ${index}`,
          })),
        },
      },
    });

    expect(detail?.patchDiff).toHaveLength(220);
    expect(detail?.patchDiff?.some((line) => line.type === "skip")).toBe(false);
  });

  it("does not synthesize legacy structured results without presentation", () => {
    const detail = formatToolResultMetadata({
      raw_name: "TaskUpdate",
      canonical_name: "TaskUpdate",
      display_name: "TaskUpdate",
      category: "task",
      status: "success",
      structured: {
        taskId: "11",
        statusChange: { from: "in_progress", to: "completed" },
      },
    });

    expect(detail).toBeNull();
  });
});
