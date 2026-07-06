import { cleanup, fireEvent, render, waitFor } from "@testing-library/react";
import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

import type { Message, SessionMeta } from "@/lib/types";
import { SESSION_COMMAND_EVENTS } from "@/lib/session-command-events";
import { processMessages } from "@/components/SessionView/hooks";
import { findFirstMatchingEntryIndex } from "@/components/SessionView/search-utils";

// Minimal synthetic session payloads. The backend is fully mocked: `invoke`
// dispatches on the Tauri command name so the session-load effect resolves
// against in-memory fixtures instead of a real provider.
const META: SessionMeta = {
  id: "11111111-1111-4111-a111-111111111111",
  provider: "claude",
  title: "Smoke session",
  project_name: "smoke",
  is_sidechain: false,
  source_path: "/tmp/smoke/session.jsonl",
  project_path: "/tmp/smoke",
  created_at: 0,
  updated_at: 0,
  message_count: 2,
  file_size_bytes: 0,
  input_tokens: 0,
  output_tokens: 0,
  cache_read_tokens: 0,
  cache_write_tokens: 0,
};

const MESSAGES: Message[] = [
  {
    role: "user",
    content: "Hello there",
    timestamp: "2026-04-11T02:25:16.000Z",
    tool_name: null,
    tool_input: null,
    token_usage: null,
  },
  {
    role: "assistant",
    content: "General Kenobi reply",
    timestamp: "2026-04-11T02:25:17.000Z",
    tool_name: null,
    tool_input: null,
    token_usage: null,
  },
];

function messageAt(index: number, content = `message ${index}`): Message {
  return {
    role: index % 2 === 0 ? "user" : "assistant",
    content,
    timestamp: new Date(Date.UTC(2026, 3, 11, 2, 0, index)).toISOString(),
    tool_name: null,
    tool_input: null,
    token_usage: null,
  };
}

let openWindowMessages = MESSAGES;
let openWindowStart = 0;
let totalMessages = MESSAGES.length;
let messagesWindowMessages = MESSAGES;
const messagesWindowCalls: Array<Record<string, unknown> | undefined> = [];

function tokenTotals() {
  return {
    input_tokens: 0,
    output_tokens: 0,
    cache_read_tokens: 0,
    cache_write_tokens: 0,
  };
}

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async (command: string, args?: Record<string, unknown>) => {
    switch (command) {
      case "get_session_open_window":
        return {
          meta: META,
          window: {
            total: totalMessages,
            start: openWindowStart,
            messages: openWindowMessages,
            parse_warning_count: 0,
            token_totals: tokenTotals(),
          },
        };
      case "get_session_meta":
        return META;
      case "get_session_messages_window":
        messagesWindowCalls.push(args);
        return {
          total: totalMessages,
          start: typeof args?.offset === "number" ? (args.offset as number) : 0,
          messages: messagesWindowMessages,
          parse_warning_count: 0,
          token_totals: tokenTotals(),
        };
      case "get_session_turn_outline":
        return [];
      case "is_favorite":
        return false;
      case "cancel_session_load":
        return undefined;
      default:
        return undefined;
    }
  }),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async () => () => {}),
}));

import { SessionView } from "@/components/SessionView/index";

beforeAll(() => {
  // happy-dom lacks these browser-only APIs that child components touch.
  Element.prototype.scrollIntoView = () => {};
  Object.defineProperty(HTMLElement.prototype, "scrollHeight", {
    configurable: true,
    get() {
      return this.classList?.contains("session-messages") ? 1000 : 0;
    },
  });
  Object.defineProperty(HTMLElement.prototype, "clientHeight", {
    configurable: true,
    get() {
      return this.classList?.contains("session-messages") ? 500 : 0;
    },
  });
  if (!("ResizeObserver" in globalThis)) {
    (globalThis as { ResizeObserver: unknown }).ResizeObserver = class {
      observe() {}
      unobserve() {}
      disconnect() {}
    };
  }
});

beforeEach(() => {
  cleanup();
  openWindowMessages = MESSAGES;
  openWindowStart = 0;
  totalMessages = MESSAGES.length;
  messagesWindowMessages = MESSAGES;
  messagesWindowCalls.length = 0;
});

describe("SessionView smoke", () => {
  it("mounts and renders messages once the load resolves", async () => {
    const { findByText } = render(
      <SessionView
        session={{
          id: META.id,
          provider: "claude",
          title: META.title,
          project_name: "smoke",
          is_sidechain: false,
          source_path: META.source_path,
          project_path: META.project_path,
        }}
        active={true}
        onRefreshTree={() => {}}
        onCloseTab={() => {}}
      />,
    );

    // The async session-load effect resolves the mocked tail; both messages
    // should appear in the rendered timeline.
    expect(await findByText("Hello there")).toBeInTheDocument();
    await waitFor(() =>
      expect(document.querySelector(".session-messages")).not.toBeNull(),
    );
  });

  it("loads older user messages when in-session search misses the initial tail", async () => {
    const olderUserMessage: Message = {
      ...MESSAGES[0],
      content: "我发的旧内容",
      timestamp: "2026-04-11T02:25:15.000Z",
    };
    openWindowMessages = [MESSAGES[1]];
    openWindowStart = 1;
    totalMessages = 2;
    messagesWindowMessages = [olderUserMessage];

    const { findByText } = render(
      <SessionView
        session={{
          id: META.id,
          provider: "claude",
          title: META.title,
          project_name: "smoke",
          is_sidechain: false,
          source_path: META.source_path,
          project_path: META.project_path,
        }}
        active={true}
        onRefreshTree={() => {}}
        onCloseTab={() => {}}
      />,
    );

    expect(await findByText("General Kenobi reply")).toBeInTheDocument();
    document.dispatchEvent(
      new CustomEvent(SESSION_COMMAND_EVENTS.sessionSearch),
    );
    const input = await waitFor(() => {
      const el = document.querySelector<HTMLInputElement>(
        ".session-search-input",
      );
      expect(el).not.toBeNull();
      return el!;
    });

    fireEvent.input(input, { target: { value: "我发的旧内容" } });

    await waitFor(() =>
      expect(messagesWindowCalls).toContainEqual(
        expect.objectContaining({ offset: 0, limit: 1 }),
      ),
    );
    // The match lives outside the initial tail; the search must page it in
    // and reveal it in the rendered timeline (highlighting itself runs on the
    // CSS Highlight API, absent in happy-dom).
    expect(await findByText("我发的旧内容")).toBeInTheDocument();
  });

  it("loads the complete session before choosing the first search match", async () => {
    const oldestUserMessage = messageAt(0, "无常最早是用户提问");
    const middleMessage = messageAt(1, "普通中间消息");
    const newerMessage = messageAt(2, "无常后面又被提到");
    openWindowMessages = [newerMessage];
    openWindowStart = 2;
    totalMessages = 3;
    messagesWindowMessages = [oldestUserMessage, middleMessage];

    const { findByText } = render(
      <SessionView
        session={{
          id: META.id,
          provider: "claude",
          title: META.title,
          project_name: "smoke",
          is_sidechain: false,
          source_path: META.source_path,
          project_path: META.project_path,
        }}
        active={true}
        onRefreshTree={() => {}}
        onCloseTab={() => {}}
      />,
    );

    expect(await findByText("无常后面又被提到")).toBeInTheDocument();
    document.dispatchEvent(
      new CustomEvent(SESSION_COMMAND_EVENTS.sessionSearch),
    );
    const input = await waitFor(() => {
      const el = document.querySelector<HTMLInputElement>(
        ".session-search-input",
      );
      expect(el).not.toBeNull();
      return el!;
    });

    fireEvent.input(input, { target: { value: "无常" } });

    await waitFor(() =>
      expect(messagesWindowCalls).toContainEqual(
        expect.objectContaining({ offset: 0, limit: 2 }),
      ),
    );
    // The FIRST match session-wide is the oldest message; it must be loaded
    // and revealed even though the initial window only held the newest one.
    expect(await findByText("无常最早是用户提问")).toBeInTheDocument();
  });

  it("keeps normal upward scrolling after search reveals an older loaded match", async () => {
    const manyMessages = Array.from({ length: 120 }, (_, index) =>
      messageAt(
        index,
        index === 0
          ? "oldest still above"
          : index === 10
            ? "target after search"
            : `message ${index}`,
      ),
    );
    expect(
      findFirstMatchingEntryIndex(
        processMessages(manyMessages, 0),
        "target after search",
      ),
    ).toBe(10);
    openWindowMessages = manyMessages;
    totalMessages = manyMessages.length;

    const { findByText, queryByText } = render(
      <SessionView
        session={{
          id: META.id,
          provider: "claude",
          title: META.title,
          project_name: "smoke",
          is_sidechain: false,
          source_path: META.source_path,
          project_path: META.project_path,
        }}
        active={true}
        onRefreshTree={() => {}}
        onCloseTab={() => {}}
      />,
    );

    expect(await findByText("message 119")).toBeInTheDocument();
    expect(queryByText("target after search")).not.toBeInTheDocument();
    expect(queryByText("oldest still above")).not.toBeInTheDocument();

    document.dispatchEvent(
      new CustomEvent(SESSION_COMMAND_EVENTS.sessionSearch),
    );
    const input = await waitFor(() => {
      const el = document.querySelector<HTMLInputElement>(
        ".session-search-input",
      );
      expect(el).not.toBeNull();
      return el!;
    });

    fireEvent.input(input, { target: { value: "target after search" } });

    await waitFor(() =>
      expect(queryByText("target after search")).toBeInTheDocument(),
    );
    expect(queryByText("oldest still above")).not.toBeInTheDocument();

    const messagesEl =
      document.querySelector<HTMLDivElement>(".session-messages");
    expect(messagesEl).not.toBeNull();
    messagesEl!.scrollTop = -500;
    fireEvent.scroll(messagesEl!);

    await waitFor(() =>
      expect(queryByText("oldest still above")).toBeInTheDocument(),
    );
  });
});
