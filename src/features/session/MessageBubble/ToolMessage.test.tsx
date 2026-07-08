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
  it("renders raw output after expansion", () => {
    const { container } = render(<ToolMessage message={bashOutputMessage} />);

    expect(container.querySelector(".msg-tool-output")).toBeNull();
    const header = container.querySelector(".terminal-tool-toggle");
    if (!header) throw new Error("expected tool header");

    fireEvent.click(header);

    expect(container.querySelector(".msg-tool-output pre")?.textContent).toBe(
      "line one\nline two",
    );
  });

  it("uses presentation raw output policy to suppress terminal output", () => {
    const { container } = render(
      <ToolMessage
        message={{
          ...bashOutputMessage,
          tool_metadata: {
            raw_name: "Bash",
            canonical_name: "Bash",
            display_name: "Bash",
            category: "shell",
            presentation: {
              icon: "💻",
              rawOutputPolicy: "suppress_terminal",
              resultDetail: {
                lines: [{ label: "stdout", value: "line one\nline two" }],
              },
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
    expect(container.querySelector(".msg-tool-output pre")?.textContent).toBe(
      "line one\nline two",
    );
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

  it("does not suppress ordinary output when presentation policy is keep", () => {
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
              rawOutputPolicy: "keep",
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
