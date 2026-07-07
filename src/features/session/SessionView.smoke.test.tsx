import { cleanup, fireEvent, render, waitFor } from "@testing-library/react";
import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

import type { Message, SessionMeta } from "@/lib/types";
import { SESSION_COMMAND_EVENTS } from "@/lib/session-command-events";
import { processMessages } from "@/features/session/hooks";
import { findFirstMatchingEntryIndex } from "@/features/session/search-utils";

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
let outlineEntries: Array<Record<string, unknown>> = [];
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
        return outlineEntries;
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

import { SessionView } from "@/features/session/index";

beforeAll(() => {
  // happy-dom lacks these browser-only APIs that child components touch.
  Element.prototype.scrollIntoView = () => {};
  // The virtualizer reads offsetWidth/offsetHeight for both the scroll
  // container's viewport and each row's measured size; happy-dom reports 0
  // for all of them, which renders zero rows. Give the container a viewport
  // and every row a fixed height so windowing math behaves like a real
  // browser (120 rows × 100px, 800px viewport → ~8 visible + overscan).
  Object.defineProperty(HTMLElement.prototype, "offsetHeight", {
    configurable: true,
    get() {
      if (this.classList?.contains("session-messages")) return 800;
      if (this.classList?.contains("session-entry")) return 100;
      return 0;
    },
  });
  Object.defineProperty(HTMLElement.prototype, "offsetWidth", {
    configurable: true,
    get() {
      return this.classList?.contains("session-messages") ? 800 : 0;
    },
  });
  // scrollToIndex clamps against scrollHeight - clientHeight; derive
  // scrollHeight from the virtualizer's spacer so the max offset tracks the
  // loaded content like a real browser.
  Object.defineProperty(HTMLElement.prototype, "clientHeight", {
    configurable: true,
    get() {
      return this.classList?.contains("session-messages") ? 800 : 0;
    },
  });
  Object.defineProperty(HTMLElement.prototype, "scrollHeight", {
    configurable: true,
    get() {
      if (!this.classList?.contains("session-messages")) return 0;
      const inner = this.querySelector<HTMLElement>(".session-messages-inner");
      const height = inner ? Number.parseInt(inner.style.height, 10) : 0;
      return Number.isFinite(height) ? height : 0;
    },
  });
  // happy-dom's scrollTo doesn't notify; the virtualizer relies on the
  // scroll event to track its offset.
  Element.prototype.scrollTo = function (
    options?: ScrollToOptions | number,
    y?: number,
  ) {
    const top = typeof options === "object" ? (options?.top ?? 0) : (y ?? 0);
    this.scrollTop = top;
    this.dispatchEvent(new Event("scroll"));
  };
  // happy-dom ships a ResizeObserver that reports 0×0 for everything, which
  // would overwrite the stubbed row heights the moment it fires — replace it
  // with a silent one so measurements come from the offsetHeight stub above.
  (globalThis as { ResizeObserver: unknown }).ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
});

beforeEach(() => {
  cleanup();
  openWindowMessages = MESSAGES;
  openWindowStart = 0;
  totalMessages = MESSAGES.length;
  messagesWindowMessages = MESSAGES;
  outlineEntries = [];
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

    // Opening lands at the newest messages; older rows are outside the
    // virtualizer's window.
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

    // After the reveal, plain scrolling must still work: driving the scroll
    // container to the very top brings the oldest row into the virtualizer's
    // window.
    const messagesEl =
      document.querySelector<HTMLDivElement>(".session-messages");
    expect(messagesEl).not.toBeNull();
    messagesEl!.scrollTop = 0;
    fireEvent.scroll(messagesEl!);

    await waitFor(() =>
      expect(queryByText("oldest still above")).toBeInTheDocument(),
    );
  });

  it("re-centers the window on a far minimap jump instead of loading the gap", async () => {
    // Open a 5000-message session at its newest tail, then jump to the first
    // turn via the minimap. The jump must fetch a small window around the
    // target — NOT the ~4700 messages in between.
    const all = Array.from({ length: 5000 }, (_, index) => messageAt(index));
    openWindowMessages = all.slice(4700);
    openWindowStart = 4700;
    totalMessages = 5000;
    messagesWindowMessages = all.slice(0, 300);
    outlineEntries = [
      { ordinal: 0, message_index: 0, user_text: "first turn", reply_text: "" },
      {
        ordinal: 1,
        message_index: 4800,
        user_text: "last turn",
        reply_text: "",
      },
    ];

    const { findByText, getByLabelText } = render(
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

    expect(await findByText("message 4999")).toBeInTheDocument();
    const firstTick = await waitFor(() => getByLabelText("first turn"));

    fireEvent.click(firstTick);

    await waitFor(() =>
      expect(messagesWindowCalls).toContainEqual(
        expect.objectContaining({ offset: 0, limit: 300 }),
      ),
    );
    // No bulk fetch of the gap between the tail and the target.
    for (const call of messagesWindowCalls) {
      expect(call?.limit).toBeLessThanOrEqual(600);
    }
    expect(await findByText("message 0")).toBeInTheDocument();
  });
});
