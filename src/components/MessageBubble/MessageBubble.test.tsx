import { render, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { Message } from "@/lib/types";

// Message bodies render through the Streamdown-based Markdown component;
// mock it so tests assert what text reaches the renderer without pulling the
// full markdown pipeline (shiki, mermaid) into happy-dom.
const markdownMock = vi.fn((_props: { text: string }) => null);
vi.mock("@/features/session/timeline/Markdown", () => ({
  Markdown: (props: { text: string }) => markdownMock(props),
}));

import { MessageBubble } from "@/components/MessageBubble/index";

function message(overrides: Partial<Message>): Message {
  return {
    role: "user",
    content: "hello",
    timestamp: null,
    tool_name: null,
    tool_input: null,
    token_usage: null,
    ...overrides,
  };
}

describe("MessageBubble", () => {
  beforeEach(() => {
    markdownMock.mockClear();
  });

  it("renders markdown for normal user messages", async () => {
    render(<MessageBubble message={message({ content: "**hello**" })} />);

    // The Markdown module is lazy-loaded; the mock resolves a tick later.
    await waitFor(() => expect(markdownMock).toHaveBeenCalledTimes(1));
    expect(markdownMock).toHaveBeenCalledWith({ text: "**hello**" });
  });

  it("renders command input as a distinct user bubble", async () => {
    const { container } = render(
      <MessageBubble
        message={message({
          message_kind: "command_input",
          content: "/compact now",
        })}
      />,
    );

    expect(container.querySelector(".msg-bubble-command")).toBeTruthy();
    await waitFor(() =>
      expect(markdownMock).toHaveBeenCalledWith({ text: "/compact now" }),
    );
  });

  it("renders command output as a distinct assistant bubble", async () => {
    const { container } = render(
      <MessageBubble
        message={message({
          role: "assistant",
          message_kind: "command_output",
          content: "Reloaded skills",
        })}
      />,
    );

    expect(container.querySelector(".msg-bubble-command")).toBeTruthy();
    await waitFor(() =>
      expect(markdownMock).toHaveBeenCalledWith({ text: "Reloaded skills" }),
    );
  });

  it("strips image placeholders out of the markdown text", async () => {
    render(
      <MessageBubble
        message={message({
          content: "look [Image #1: source: /tmp/a.png] here",
        })}
      />,
    );

    await waitFor(() =>
      expect(markdownMock).toHaveBeenCalledWith({ text: "look  here" }),
    );
  });

  it("does not render markdown for tool messages", () => {
    render(
      <MessageBubble
        message={message({
          role: "tool",
          content: "tool output",
          tool_name: "Read",
          tool_input: "{}",
        })}
      />,
    );

    expect(markdownMock).not.toHaveBeenCalled();
  });

  it("does not render markdown for system messages", () => {
    render(
      <MessageBubble
        message={message({
          role: "system",
          content: "[thinking]\ninternal notes",
        })}
      />,
    );

    expect(markdownMock).not.toHaveBeenCalled();
  });

  it("does not render markdown for hidden template content", () => {
    render(
      <MessageBubble
        message={message({
          content: "<environment_context>\nignored\n</environment_context>",
        })}
      />,
    );

    expect(markdownMock).not.toHaveBeenCalled();
  });
});
