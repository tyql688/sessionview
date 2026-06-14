import { fireEvent, render, waitFor } from "@solidjs/testing-library";
import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

import type { Message, SessionMeta } from "../../lib/types";
import { SESSION_COMMAND_EVENTS } from "../../lib/session-command-events";

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

import { SessionView } from "./index";

beforeAll(() => {
  // happy-dom lacks these browser-only APIs that child components touch.
  Element.prototype.scrollIntoView = () => {};
  if (!("ResizeObserver" in globalThis)) {
    (globalThis as { ResizeObserver: unknown }).ResizeObserver = class {
      observe() {}
      unobserve() {}
      disconnect() {}
    };
  }
});

beforeEach(() => {
  openWindowMessages = MESSAGES;
  openWindowStart = 0;
  totalMessages = MESSAGES.length;
  messagesWindowMessages = MESSAGES;
  messagesWindowCalls.length = 0;
});

describe("SessionView smoke", () => {
  it("mounts and renders messages once the load resolves", async () => {
    const { findByText } = render(() => (
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
      />
    ));

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

    const { findByText } = render(() => (
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
      />
    ));

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
    await waitFor(() =>
      expect(
        document.querySelector(".msg-row-user mark.search-highlight")
          ?.textContent,
      ).toBe("我发的旧内容"),
    );
  });
});
