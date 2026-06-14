import { describe, expect, it } from "vitest";

import {
  formatToolInput,
  formatToolResultMetadata,
  parseMcpToolName,
  toolDisplayName,
  toolIcon,
  toolSummary,
} from "./index";
import type { Message } from "../types";

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
  it("formats canonical input for known tools", () => {
    const detail = formatToolInput({
      ...baseMessage,
      tool_name: "Grep",
      tool_input: JSON.stringify({
        pattern: "fn main",
        path: "/Users/alice/repo/src",
      }),
    });

    expect(detail?.lines).toContainEqual({
      label: "pattern",
      value: "fn main",
    });
    expect(detail?.lines).toContainEqual({
      label: "path",
      value: "~/repo/src",
    });
  });

  it("formats antigravity replace_file_content input as single diff", () => {
    const detail = formatToolInput({
      ...baseMessage,
      tool_name: "Edit",
      tool_input: JSON.stringify({
        TargetFile: "/tmp/project/main.rs",
        TargetContent: "old line",
        ReplacementContent: "new line",
        StartLine: 5,
        EndLine: 5,
      }),
    });

    expect(detail?.lines).toContainEqual({
      label: "file",
      value: "/tmp/project/main.rs",
    });
    expect(detail?.diff).toEqual({ old: "old line", new: "new line" });
  });

  it("formats antigravity multi_replace_file_content input as patch diff", () => {
    const detail = formatToolInput({
      ...baseMessage,
      tool_name: "Edit",
      tool_input: JSON.stringify({
        TargetFile: "/tmp/project/main.rs",
        ReplacementChunks: [
          {
            StartLine: 1,
            EndLine: 1,
            TargetContent: "old A",
            ReplacementContent: "new A",
          },
          {
            StartLine: 10,
            EndLine: 11,
            TargetContent: "old B1\nold B2",
            ReplacementContent: "new B1\nnew B2",
          },
        ],
      }),
    });

    expect(detail?.lines).toEqual([
      { label: "file", value: "/tmp/project/main.rs" },
    ]);
    const types = detail?.patchDiff?.map((line) => line.type);
    // One skip for the *** Update File: header, then per-hunk:
    //  skip (@@), remove(s), add(s). 1+1 then 2+2.
    expect(types).toEqual([
      "skip", // *** Update File: ...
      "skip", // @@ -1,1 +1,1 @@
      "remove",
      "add",
      "skip", // @@ -10,2 +10,2 @@
      "remove",
      "remove",
      "add",
      "add",
    ]);
    expect(detail?.patchDiff?.[0]?.text).toBe(
      "*** Update File: /tmp/project/main.rs",
    );
  });

  it("formats Codex apply_patch input as patch diff rows", () => {
    const detail = formatToolInput({
      ...baseMessage,
      tool_name: "Edit",
      tool_input: JSON.stringify({
        patch: `*** Begin Patch
*** Update File: /Users/alice/project/src/app.ts
@@
-old
+new
*** End Patch
`,
      }),
    });

    expect(detail?.patchDiff?.map((line) => line.type)).toEqual([
      "skip",
      "skip",
      "remove",
      "add",
    ]);
    expect(detail?.lines).toEqual([
      { label: "files", value: "~/project/src/app.ts" },
    ]);
    expect(detail?.patchDiff?.[0]?.text).toBe(
      "*** Update File: ~/project/src/app.ts",
    );
  });
});

describe("tools/result", () => {
  it("formats bash output as stdout", () => {
    const detail = formatToolResultMetadata({
      raw_name: "bash",
      canonical_name: "Bash",
      display_name: "Bash",
      category: "shell",
      status: "success",
      structured: {
        output: "hello from pi",
      },
    });

    expect(detail?.lines).toContainEqual({
      label: "status",
      value: "success",
    });
    expect(detail?.lines).toContainEqual({
      label: "stdout",
      value: "hello from pi",
    });
  });

  it("formats structured edit results as a diff", () => {
    const detail = formatToolResultMetadata({
      raw_name: "Edit",
      canonical_name: "Edit",
      display_name: "Edit",
      category: "file",
      status: "success",
      structured: {
        file_path: "/tmp/App.tsx",
        old_string: "old",
        new_string: "new",
      },
    });

    expect(detail?.lines).toContainEqual({
      label: "file",
      value: "/tmp/App.tsx",
    });
    expect(detail?.diff).toEqual({ old: "old", new: "new" });
  });

  it("formats Claude structuredPatch results as patch diff rows", () => {
    const detail = formatToolResultMetadata({
      raw_name: "Edit",
      canonical_name: "Edit",
      display_name: "Edit",
      category: "file",
      status: "success",
      structured: {
        filePath: "/tmp/App.tsx",
        structuredPatch: [
          {
            oldStart: 4,
            oldLines: 2,
            newStart: 4,
            newLines: 2,
            lines: [" const same = true;", "-old", "+new"],
          },
        ],
      },
    });

    expect(detail?.patchDiff?.map((line) => line.type)).toEqual([
      "skip",
      "context",
      "remove",
      "add",
    ]);
  });

  it("formats task status changes without object stringification", () => {
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

    expect(detail?.lines).toContainEqual({
      label: "statusChange",
      value: "in_progress → completed",
    });
    expect(detail?.lines.some((line) => line.value === "[object Object]")).toBe(
      false,
    );
  });

  it("formats image generation and dynamic tool metadata", () => {
    const imageDetail = formatToolResultMetadata({
      raw_name: "image_generation_call",
      canonical_name: "ImageGeneration",
      display_name: "image generation",
      category: "media",
      status: "completed",
      result_kind: "image",
      structured: {
        savedPath: "/Users/alice/.codex/generated_images/ig_1.png",
        revisedPrompt: "make an icon",
      },
    });

    expect(toolIcon("ImageGeneration")).toBe("🖼️");
    expect(imageDetail?.lines).toContainEqual({
      label: "savedPath",
      value: "~/.codex/generated_images/ig_1.png",
    });

    const dynamicDetail = formatToolResultMetadata({
      raw_name: "load_workspace_dependencies",
      canonical_name: "DynamicTool",
      display_name: "load workspace dependencies",
      category: "tool",
      status: "success",
      structured: {
        tool: "load_workspace_dependencies",
        success: true,
        content: "Workspace dependencies are available",
      },
    });

    expect(toolIcon("DynamicTool")).toBe("🧩");
    expect(dynamicDetail?.lines).toContainEqual({
      label: "tool",
      value: "load_workspace_dependencies",
    });
    expect(dynamicDetail?.lines).toContainEqual({
      label: "result",
      value: "Workspace dependencies are available",
    });
  });

  it("keeps Kimi tool call display metadata in result details", () => {
    const detail = formatToolResultMetadata({
      raw_name: "Bash",
      canonical_name: "Bash",
      display_name: "Bash",
      category: "shell",
      status: "success",
      structured: {
        callDescription: "Run pwd",
        callDisplay: {
          kind: "bash",
          cwd: "/Users/alice/project",
          command: "pwd",
        },
        output: "hello world",
      },
    });

    expect(detail?.lines).toContainEqual({
      label: "description",
      value: "Run pwd",
    });
    expect(detail?.lines).toContainEqual({
      label: "cwd",
      value: "/Users/alice/project",
    });
    expect(detail?.lines).toContainEqual({
      label: "stdout",
      value: "hello world",
    });
  });
});
