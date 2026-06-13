import { render } from "@solidjs/testing-library";
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
    render(() => <MessageBubble message={message({ content: "**hello**" })} />);

    expect(parseMarkdownDocumentMock).toHaveBeenCalledTimes(1);
    expect(parseMarkdownDocumentMock).toHaveBeenCalledWith("**hello**");
  });

  it("does not parse markdown for tool messages", () => {
    render(() => (
      <MessageBubble
        message={message({
          role: "tool",
          content: "tool output",
          tool_name: "Read",
          tool_input: "{}",
        })}
      />
    ));

    expect(parseMarkdownDocumentMock).not.toHaveBeenCalled();
  });

  it("does not parse markdown for system messages", () => {
    render(() => (
      <MessageBubble
        message={message({
          role: "system",
          content: "[thinking]\ninternal notes",
        })}
      />
    ));

    expect(parseMarkdownDocumentMock).not.toHaveBeenCalled();
  });

  it("does not parse markdown for hidden template content", () => {
    render(() => (
      <MessageBubble
        message={message({
          content: "<environment_context>\nignored\n</environment_context>",
        })}
      />
    ));

    expect(parseMarkdownDocumentMock).not.toHaveBeenCalled();
  });
});
