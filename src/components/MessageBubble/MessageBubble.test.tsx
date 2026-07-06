import { render } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { Message } from "../../lib/types";

vi.mock("./MarkdownRenderer", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./MarkdownRenderer")>();
  return {
    ...actual,
    parseMarkdownDocument: vi.fn(actual.parseMarkdownDocument),
  };
});

import { parseMarkdownDocument } from "./MarkdownRenderer";
import { MessageBubble } from "./index";

const parseMarkdownDocumentMock = vi.mocked(parseMarkdownDocument);

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
    parseMarkdownDocumentMock.mockClear();
  });

  it("parses markdown for normal user messages", () => {
    render(<MessageBubble message={message({ content: "**hello**" })} />);

    expect(parseMarkdownDocumentMock).toHaveBeenCalledTimes(1);
    expect(parseMarkdownDocumentMock).toHaveBeenCalledWith("**hello**");
  });

  it("renders command input as a distinct user bubble", () => {
    const { container } = render(
      <MessageBubble
        message={message({
          message_kind: "command_input",
          content: "/compact now",
        })}
      />,
    );

    expect(container.querySelector(".msg-bubble-command")).toBeTruthy();
    expect(parseMarkdownDocumentMock).toHaveBeenCalledWith("/compact now");
  });

  it("renders command output as a distinct assistant bubble", () => {
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
    expect(parseMarkdownDocumentMock).toHaveBeenCalledWith("Reloaded skills");
  });

  it("does not parse markdown for tool messages", () => {
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

    expect(parseMarkdownDocumentMock).not.toHaveBeenCalled();
  });

  it("does not parse markdown for system messages", () => {
    render(
      <MessageBubble
        message={message({
          role: "system",
          content: "[thinking]\ninternal notes",
        })}
      />,
    );

    expect(parseMarkdownDocumentMock).not.toHaveBeenCalled();
  });

  it("does not parse markdown for hidden template content", () => {
    render(
      <MessageBubble
        message={message({
          content: "<environment_context>\nignored\n</environment_context>",
        })}
      />,
    );

    expect(parseMarkdownDocumentMock).not.toHaveBeenCalled();
  });
});
