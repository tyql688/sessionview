import { fireEvent, render } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { Message } from "@/lib/types";
import { ToolMessage } from "@/features/session/MessageBubble/ToolMessage";

const agentMessage: Message = {
  role: "tool",
  content: "",
  timestamp: null,
  tool_name: "Agent",
  tool_input: null,
  token_usage: null,
  tool_metadata: {
    raw_name: "invoke_subagent",
    canonical_name: "Agent",
    display_name: "Agent",
    category: "agent",
    structured: {
      childConversationIds: ["child-conversation-1"],
      childPrompts: ["inspect the provider"],
    },
  },
};

const defineSubagentMessage: Message = {
  ...agentMessage,
  tool_input: JSON.stringify({
    description: "Defines a reusable agy analyzer but does not spawn it",
  }),
  tool_metadata: {
    raw_name: "define_subagent",
    canonical_name: "Agent",
    display_name: "Agent",
    category: "agent",
  },
};

const promptOnlyInvokeMessage: Message = {
  ...agentMessage,
  tool_metadata: {
    raw_name: "invoke_subagent",
    canonical_name: "Agent",
    display_name: "Agent",
    category: "agent",
    structured: {
      childPrompts: ["prompt-only child task"],
    },
  },
};

const kimiSwarmMessage: Message = {
  ...agentMessage,
  tool_name: "AgentSwarm",
  tool_metadata: {
    raw_name: "AgentSwarm",
    canonical_name: "Agent",
    display_name: "AgentSwarm",
    category: "agent",
    structured: {
      childConversationIds: ["agent-0", "agent-1"],
      childPrompts: [
        "apps/desktop/src/App.vue, apps/desktop/src/main.ts",
        "packages/core/src/index.ts",
      ],
    },
  },
};

const bashOutputMessage: Message = {
  role: "tool",
  content: "line one\nline two",
  timestamp: null,
  tool_name: "Bash",
  tool_input: JSON.stringify({ command: "printf 'line one\\nline two'" }),
  token_usage: null,
};

const readNoDetailMessage: Message = {
  role: "tool",
  content: "",
  timestamp: null,
  tool_name: "Read",
  tool_input: null,
  token_usage: null,
  tool_metadata: {
    raw_name: "Read",
    canonical_name: "Read",
    display_name: "Read",
    category: "file",
  },
};

describe("ToolMessage", () => {
  it("renders terminal output after expansion", () => {
    const { container } = render(<ToolMessage message={bashOutputMessage} />);

    expect(container.querySelector(".msg-tool-output")).toBeNull();
    expect(container.querySelector(".terminal-tool-window-dots")).toBeNull();
    const header = container.querySelector(".terminal-tool-toggle");
    if (!header) throw new Error("expected tool header");

    fireEvent.click(header);

    expect(container.querySelector(".terminal-tool-window-dots")).toBeNull();
    expect(container.querySelector(".msg-tool-output pre")?.textContent).toBe(
      "line one\nline two",
    );
  });

  it("renders a plain Codex exec input as the terminal command", () => {
    const input = 'const result = await tools.exec_command({ cmd: "pwd" });\ntext(result.output);';
    const { container } = render(
      <ToolMessage
        message={{
          ...bashOutputMessage,
          tool_input: input,
          tool_metadata: {
            raw_name: "exec",
            canonical_name: "Bash",
            display_name: "Bash",
            category: "shell",
            presentation: { icon: "💻", resultMode: "terminal" },
          },
        }}
      />,
    );

    const header = container.querySelector(".terminal-tool-toggle");
    if (!header) throw new Error("expected terminal tool header");
    fireEvent.click(header);

    expect(container.querySelector(".terminal-tool-command-text")?.textContent).toBe(input);
  });

  it("keeps the provider terminal body when structured stdout omits a warning", () => {
    const providerOutput =
      "line one\nline two\n[This command modified 2 files. Call Read before editing.]";
    const { container } = render(
      <ToolMessage
        message={{
          ...bashOutputMessage,
          content: providerOutput,
          tool_metadata: {
            raw_name: "Bash",
            canonical_name: "Bash",
            display_name: "Bash",
            category: "shell",
            presentation: {
              icon: "💻",
              resultMode: "terminal",
              resultDetail: {
                lines: [{ label: "exit", value: "0" }],
              },
            },
            structured: {
              stdout: "line one\nline two",
              staleReadFileStateHint: "[This command modified 2 files. Call Read before editing.]",
            },
          },
        }}
      />,
    );

    const header = container.querySelector(".terminal-tool-toggle");
    if (!header) throw new Error("expected tool header");
    fireEvent.click(header);

    expect(container.querySelector(".terminal-tool")).not.toBeNull();
    expect(container.querySelector(".msg-tool-result-detail")).toBeNull();
    expect(container.querySelector(".msg-tool-output pre")?.textContent).toBe(providerOutput);
    expect(container.textContent?.match(/Call Read before editing/g)).toHaveLength(1);
  });

  it("does not warn for bracket-prefixed terminal text output", () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => undefined);
    try {
      render(
        <ToolMessage
          message={{
            ...bashOutputMessage,
            content: "[Image: source: C:/tmp/example.png]",
          }}
        />,
      );

      expect(
        warn.mock.calls.some((call) => String(call[0]).includes("failed to parse terminal tool content JSON")),
      ).toBe(false);
    } finally {
      warn.mockRestore();
    }
  });

  it("renders ordinary provider output through the default output mode", () => {
    const { container } = render(
      <ToolMessage
        message={{
          ...bashOutputMessage,
          tool_metadata: {
            raw_name: "CustomTool",
            canonical_name: "CustomTool",
            display_name: "CustomTool",
            category: "unknown",
            presentation: {
              icon: "⚙",
              resultMode: "output",
              resultDetail: {
                lines: [{ label: "status", value: "success" }],
              },
            },
          },
        }}
      />,
    );

    const header = container.querySelector(".msg-tool-toggle-row");
    if (!header) throw new Error("expected tool header");
    fireEvent.click(header);

    expect(container.querySelector(".msg-tool-output pre")?.textContent).toBe(
      "line one\nline two",
    );
  });

  it("renders normalized Kimi output once without displaying its result envelope", () => {
    const output = "1\tfirst line\n2\tsecond line";
    const { container } = render(
      <ToolMessage
        message={{
          ...readNoDetailMessage,
          content: output,
          tool_metadata: {
            raw_name: "Read",
            canonical_name: "Read",
            display_name: "Read",
            category: "file",
            structured: {
              note: "<system>2 lines read.</system>",
              output,
            },
            presentation: {
              icon: "📄",
              resultMode: "output",
            },
          },
        }}
      />,
    );

    const header = container.querySelector(".msg-tool-toggle-row");
    if (!header) throw new Error("expected tool header");
    fireEvent.click(header);

    expect(container.querySelector(".msg-tool-result-detail")).toBeNull();
    expect(container.querySelector(".msg-tool-output pre")?.textContent).toBe(output);
    expect(container.textContent?.match(/first line/g)).toHaveLength(1);
    expect(container.textContent).not.toContain("<system>");
  });

  it("hydrates Codex typed media inside the single output renderer", () => {
    const { container } = render(
      <ToolMessage
        message={{
          ...readNoDetailMessage,
          tool_name: "JavaScript",
          content: "captured\n[Image]",
          tool_metadata: {
            raw_name: "js",
            canonical_name: "JavaScript",
            display_name: "JavaScript",
            category: "tool",
            presentation: {
              icon: "🟨",
              resultMode: "output",
              resultDetail: { lines: [], media: ["data:image/png;base64,AAAA"] },
            },
          },
        }}
      />,
    );

    const header = container.querySelector(".msg-tool-toggle-row");
    if (!header) throw new Error("expected tool header");
    fireEvent.click(header);

    expect(container.querySelectorAll(".msg-tool-output")).toHaveLength(1);
    expect(container.querySelectorAll(".msg-image-wrap")).toHaveLength(1);
    expect(container.querySelector(".msg-tool-output")?.textContent).not.toContain("[Image]");
    expect(container.querySelector(".msg-tool-output-label")).toBeNull();
  });

  it("keeps typed media inert when the result mode is raw", () => {
    const { container } = render(
      <ToolMessage
        message={{
          ...readNoDetailMessage,
          tool_name: "FutureTool",
          content: "[Image]",
          tool_metadata: {
            raw_name: "FutureTool",
            canonical_name: "FutureTool",
            display_name: "FutureTool",
            category: "unknown",
            presentation: { icon: "⚙", resultMode: "raw" },
            structured: {
              output: [{ type: "input_image", image_url: "data:image/png;base64,AAAA" }],
            },
          },
        }}
      />,
    );

    const header = container.querySelector(".msg-tool-toggle-row");
    if (!header) throw new Error("expected tool header");
    fireEvent.click(header);

    expect(container.querySelectorAll(".msg-tool-output")).toHaveLength(1);
    expect(container.querySelector(".msg-image-wrap")).toBeNull();
    expect(container.querySelector(".msg-tool-raw-output pre")?.textContent).toBe("[Image]");
  });

  it("renders terminal text and typed media in one terminal output stream", () => {
    const { container } = render(
      <ToolMessage
        message={{
          ...bashOutputMessage,
          content: "Script completed\n[Image]",
          tool_metadata: {
            raw_name: "exec_command",
            canonical_name: "Bash",
            display_name: "Bash",
            category: "shell",
            presentation: {
              icon: "💻",
              resultMode: "terminal",
              resultDetail: { lines: [], media: ["data:image/png;base64,AAAA"] },
            },
          },
        }}
      />,
    );

    const header = container.querySelector(".terminal-tool-toggle");
    if (!header) throw new Error("expected terminal tool header");
    fireEvent.click(header);

    expect(container.querySelectorAll(".terminal-tool")).toHaveLength(1);
    expect(container.querySelectorAll(".msg-tool-output")).toHaveLength(1);
    expect(container.querySelectorAll(".msg-image-wrap")).toHaveLength(1);
    expect(container.querySelector(".msg-tool-output")?.textContent).not.toContain("[Image]");
    expect(container.querySelector(".msg-tool-output-label")).toBeNull();
  });

  it("keeps provider-rendered output instead of structured file content", () => {
    const { container } = render(
      <ToolMessage
        message={{
          ...readNoDetailMessage,
          tool_name: "Write",
          content: "File created successfully.",
          tool_metadata: {
            raw_name: "Write",
            canonical_name: "Write",
            display_name: "Write",
            category: "file",
            presentation: {
              icon: "📝",
              resultMode: "output",
            },
            structured: { content: "fn main() {}" },
          },
        }}
      />,
    );

    const header = container.querySelector(".msg-tool-toggle-row");
    if (!header) throw new Error("expected tool header");
    fireEvent.click(header);

    expect(container.querySelector(".msg-tool-output pre")?.textContent).toBe("File created successfully.");
    expect(container.textContent).not.toContain("fn main()");
  });

  it("uses raw as an explicit mutually exclusive fallback", () => {
    const raw = JSON.stringify([{ type: "future_media", payload: "keep" }]);
    const { container } = render(
      <ToolMessage
        message={{
          ...readNoDetailMessage,
          tool_name: "FutureTool",
          content: raw,
          tool_metadata: {
            raw_name: "FutureTool",
            canonical_name: "FutureTool",
            display_name: "FutureTool",
            category: "unknown",
            presentation: { icon: "⚙", resultMode: "raw" },
          },
        }}
      />,
    );

    const header = container.querySelector(".msg-tool-toggle-row");
    if (!header) throw new Error("expected tool header");
    fireEvent.click(header);

    expect(container.querySelectorAll(".msg-tool-output")).toHaveLength(1);
    expect(container.querySelector(".msg-tool-raw-output pre")?.textContent).toBe(raw);
    expect(container.querySelector(".msg-tool-output-label")?.textContent).toBe("raw");
  });

  it("keeps raw bytes literal instead of interpreting content markers", () => {
    const raw = '[{"payload":"[Image: source: /tmp/not-an-image.png]\\n```json\\n{\\"keep\\":true}\\n```"}]';
    const { container } = render(
      <ToolMessage
        message={{
          ...readNoDetailMessage,
          tool_name: "FutureTool",
          content: raw,
          tool_metadata: {
            raw_name: "FutureTool",
            canonical_name: "FutureTool",
            display_name: "FutureTool",
            category: "unknown",
            presentation: { icon: "⚙", resultMode: "raw" },
          },
        }}
      />,
    );

    const header = container.querySelector(".msg-tool-toggle-row");
    if (!header) throw new Error("expected tool header");
    fireEvent.click(header);

    expect(container.querySelectorAll(".msg-tool-raw-output pre")).toHaveLength(1);
    expect(container.querySelector(".msg-tool-raw-output pre")?.textContent).toBe(raw);
    expect(container.querySelector(".msg-tool-raw-output img")).toBeNull();
  });

  it("keeps persisted-output markers literal in raw mode", () => {
    const raw =
      '[{"type":"future_content","payload":"<persisted-output>\\nFull output saved to: /tmp/raw.txt\\n</persisted-output>"}]';
    const { container } = render(
      <ToolMessage
        message={{
          ...readNoDetailMessage,
          tool_name: "FutureTool",
          content: raw,
          tool_metadata: {
            raw_name: "FutureTool",
            canonical_name: "FutureTool",
            display_name: "FutureTool",
            category: "unknown",
            structured: { persistedOutputPath: "/tmp/raw.txt" },
            presentation: { icon: "⚙", resultMode: "raw" },
          },
        }}
      />,
    );

    const header = container.querySelector(".msg-tool-toggle-row");
    if (!header) throw new Error("expected tool header");
    fireEvent.click(header);

    expect(container.querySelector(".msg-tool-raw-output pre")?.textContent).toBe(raw);
    expect(container.textContent).not.toContain("Load full result");
  });

  it("uses the raw renderer instead of the terminal renderer for an unknown Bash payload", () => {
    const raw = JSON.stringify([{ type: "future_content", payload: { keep: true } }]);
    const { container } = render(
      <ToolMessage
        message={{
          ...bashOutputMessage,
          content: raw,
          tool_metadata: {
            raw_name: "exec_command",
            canonical_name: "Bash",
            display_name: "Bash",
            category: "shell",
            presentation: { icon: "💻", resultMode: "raw" },
          },
        }}
      />,
    );

    expect(container.querySelector(".terminal-tool")).toBeNull();
    const header = container.querySelector(".msg-tool-toggle-row");
    if (!header) throw new Error("expected tool header");
    fireEvent.click(header);

    expect(container.querySelectorAll(".msg-tool-output")).toHaveLength(1);
    expect(container.querySelector(".msg-tool-raw-output pre")?.textContent).toBe(raw);
    expect(container.querySelector(".msg-tool-output-label")?.textContent).toBe("raw");
  });

  it("keeps a JSON file as ordinary output", () => {
    const jsonFile = '{"32360":"cursor"}';
    const { container } = render(
      <ToolMessage
        message={{
          ...readNoDetailMessage,
          content: jsonFile,
          tool_metadata: {
            ...readNoDetailMessage.tool_metadata!,
            presentation: { icon: "📄", resultMode: "output" },
          },
        }}
      />,
    );

    const header = container.querySelector(".msg-tool-toggle-row");
    if (!header) throw new Error("expected tool header");
    fireEvent.click(header);

    expect(container.querySelector(".msg-tool-output pre")?.textContent).toBe('{\n  "32360": "cursor"\n}');
    expect(container.querySelector(".msg-tool-output-label")).toBeNull();
  });

  it("renders tools with no expandable detail as static rows", () => {
    const { container, queryByRole } = render(<ToolMessage message={readNoDetailMessage} />);

    expect(container.querySelector(".msg-tool-toggle-row-static")).not.toBeNull();
    expect(queryByRole("button", { name: /Read/ })).toBeNull();
  });

  it("includes the source parent session id when opening an antigravity child", () => {
    let detail:
      | {
          description?: string;
          agentId?: string;
          parentSessionId?: string;
        }
      | undefined;
    const listener = (event: Event) => {
      detail = (
        event as CustomEvent<{
          description?: string;
          agentId?: string;
          parentSessionId?: string;
        }>
      ).detail;
    };
    window.addEventListener("open-subagent", listener);

    const { getByRole } = render(
      <ToolMessage
        message={agentMessage}
        provider="antigravity"
        parentSessionId="parent-conversation-1"
      />,
    );

    fireEvent.click(getByRole("button", { name: /Open/ }));
    window.removeEventListener("open-subagent", listener);

    expect(detail).toEqual({
      description: "inspect the provider",
      agentId: "child-conversation-1",
      parentSessionId: "parent-conversation-1",
    });
  });

  it("does not show an open button for antigravity define_subagent", () => {
    const { queryByRole } = render(
      <ToolMessage
        message={defineSubagentMessage}
        provider="antigravity"
        parentSessionId="parent-conversation-1"
      />,
    );

    expect(queryByRole("button", { name: /Open/ })).toBeNull();
  });

  it("opens antigravity child by prompt when childConversationIds are absent", () => {
    let detail:
      | {
          description?: string;
          agentId?: string;
          parentSessionId?: string;
        }
      | undefined;
    const listener = (event: Event) => {
      detail = (
        event as CustomEvent<{
          description?: string;
          agentId?: string;
          parentSessionId?: string;
        }>
      ).detail;
    };
    window.addEventListener("open-subagent", listener);

    const { getByRole } = render(
      <ToolMessage
        message={promptOnlyInvokeMessage}
        provider="antigravity"
        parentSessionId="parent-conversation-1"
      />,
    );

    fireEvent.click(getByRole("button", { name: /Open/ }));
    window.removeEventListener("open-subagent", listener);

    expect(detail).toEqual({
      description: "prompt-only child task",
      agentId: undefined,
      parentSessionId: "parent-conversation-1",
    });
  });

  it("labels kimi swarm child buttons with prompt identity", () => {
    let detail:
      | {
          description?: string;
          agentId?: string;
          parentSessionId?: string;
        }
      | undefined;
    const listener = (event: Event) => {
      detail = (
        event as CustomEvent<{
          description?: string;
          agentId?: string;
          parentSessionId?: string;
        }>
      ).detail;
    };
    window.addEventListener("open-subagent", listener);

    const { getByRole } = render(
      <ToolMessage
        message={kimiSwarmMessage}
        provider="kimi"
        parentSessionId="session_parent"
      />,
    );

    expect(
      getByRole("button", { name: /Open apps\/desktop\/src\/App\.vue/ }),
    ).toBeTruthy();
    const second = getByRole("button", {
      name: /Open packages\/core\/src\/index\.ts/,
    });
    fireEvent.click(second);
    window.removeEventListener("open-subagent", listener);

    expect(detail).toEqual({
      description: "packages/core/src/index.ts",
      agentId: "agent-1",
      parentSessionId: "session_parent",
    });
  });
});
