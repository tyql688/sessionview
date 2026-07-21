import { fireEvent, render, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { Message } from "@/lib/types";

// Message bodies render through the Markdown component; mock it so tests assert
// what text reaches the renderer without pulling shiki/mermaid into happy-dom.
const markdownMock = vi.fn((_props: { text: string }) => null);
vi.mock("@/features/session/timeline/Markdown", () => ({
  Markdown: (props: { text: string }) => markdownMock(props),
}));

vi.mock("@/features/session/MessageBubble/ImagePreview", () => ({
  ImagePreview: () => null,
  LocalImage: () => null,
  RemoteImage: () => null,
  isLocalPath: (src: string) => src.startsWith("/"),
}));

import { MessageBubble } from "@/features/session/MessageBubble/index";

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

  it.each([
    ["[turn_duration] 1.5s, 6 messages", "1.5s, 6 messages", "1.5s, 6 messages"],
    [
      "[stop_hook_summary] 2 hooks: lint (120ms), test (340ms)",
      "2 hooks: lint (120ms), test (340ms)",
      "2 hooks: lint (120ms), test (340ms)",
    ],
    [
      "[task_status_error] failed · bash-demo1234\n<notification>failed</notification>",
      "failed · bash-demo1234",
      "failed · bash-demo1234\n<notification>failed</notification>",
    ],
    [
      "[subagent_task] Inspect the parser\nUse structured origins.",
      "Inspect the parser",
      "Inspect the parser\nUse structured origins.",
    ],
  ])("renders %s as a collapsed system disclosure", (content, summary, detail) => {
    const { container } = render(
      <MessageBubble
        message={message({
          role: "system",
          content,
        })}
      />,
    );

    const toggle = container.querySelector<HTMLButtonElement>(".msg-system-toggle");
    expect(toggle).not.toBeNull();
    if (!toggle) throw new Error("missing system disclosure toggle");
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    expect(toggle).toHaveTextContent(summary);
    expect(container.querySelector(".msg-system-body")).toBeNull();

    fireEvent.click(toggle);

    expect(toggle).toHaveAttribute("aria-expanded", "true");
    expect(container.querySelector(".msg-system-body")?.textContent).toBe(detail);
    expect(markdownMock).not.toHaveBeenCalled();
  });

  it("collapses Grok/Pi [Compaction] summaries like context_compacted", () => {
    const detail = "This session is being continued from a previous conversation. Summary: built the demo.";
    const { container } = render(
      <MessageBubble
        message={message({
          role: "system",
          content: `[Compaction] ${detail}`,
        })}
      />,
    );

    const toggle = container.querySelector<HTMLButtonElement>(".msg-system-toggle");
    expect(toggle).not.toBeNull();
    if (!toggle) throw new Error("missing compaction toggle");
    expect(toggle).not.toHaveTextContent(detail);

    fireEvent.click(toggle);

    expect(container.querySelector(".msg-system-body")?.textContent).toBe(detail);
  });

  it("hides context compacted content until expanded", () => {
    const detail = "very long compacted context\nwith more retained conversation details";
    const { container } = render(
      <MessageBubble
        message={message({
          role: "system",
          content: `[context_compacted]\n${detail}`,
        })}
      />,
    );

    const toggle = container.querySelector<HTMLButtonElement>(".msg-system-toggle");
    expect(toggle).not.toBeNull();
    if (!toggle) throw new Error("missing context compacted toggle");
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    expect(toggle).not.toHaveTextContent(detail);
    expect(container.querySelector(".msg-system-body")).toBeNull();

    fireEvent.click(toggle);

    expect(toggle).toHaveAttribute("aria-expanded", "true");
    expect(container.querySelector(".msg-system-body")?.textContent).toBe(detail);
    expect(markdownMock).not.toHaveBeenCalled();
  });

  it("keeps parsed system context visible even when it contains template markers", () => {
    const { container } = render(
      <MessageBubble
        message={message({
          role: "system",
          content: "[subagent_task] Inspect context\n<environment_context>synthetic</environment_context>",
        })}
      />,
    );

    expect(container.querySelector(".msg-system-toggle")).not.toBeNull();
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
