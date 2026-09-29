import { cleanup, fireEvent, render, waitFor } from "@testing-library/react";
import "@testing-library/jest-dom/vitest";
import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { StrictMode } from "react";

import type { Message, SessionMeta } from "../../lib/types";
import { SESSION_COMMAND_EVENTS } from "../../lib/session-command-events";
import { setPendingSessionSearch } from "../search/search";
import { SESSION_SEARCH_DEBOUNCE_MS } from "./search-utils";

const LOAD_CANCELED_SENTINEL = "__sessionview_load_canceled__";

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
// When set, window requests slice this complete session so indices stay
// consistent for sessions larger than one page; otherwise they return
// `messagesWindowMessages`.
let windowSource: Message[] | null = null;
let messagesWindowGate: Promise<void> | null = null;
let outlineEntries: Array<Record<string, unknown>> = [];
// The whole session's searchable dialogue, at absolute message indices.
let searchTextMessages: Array<{ message_index: number; role: string; content: string }> = [];
let searchTextGate: Promise<void> | null = null;
const searchTextCalls: Array<Record<string, unknown> | undefined> = [];
const openWindowCalls: Array<Record<string, unknown> | undefined> = [];
const messagesWindowCalls: Array<Record<string, unknown> | undefined> = [];
const cancelSessionLoadCalls: Array<{
  sessionId: string;
  requestId?: string;
}> = [];
const canceledOpenRequestIds = new Set<string>();
const latestOpenRequestBySession = new Map<string, string>();
// When > 0, the next open-window calls fail with the cancel sentinel —
// simulates a lost backend cancel race against the CURRENT request.
let cancelNextOpenCalls = 0;

function searchTextOf(indexed: Array<[number, Message]>) {
  return indexed
    .filter(([, m]) => (m.role === "user" || m.role === "assistant") && m.content.trim().length > 0)
    .map(([message_index, m]) => ({ message_index, role: m.role, content: m.content }));
}

function tokenTotals() {
  return {
    input_tokens: 0,
    output_tokens: 0,
    cache_read_tokens: 0,
    cache_write_tokens: 0,
  };
}

vi.mock("@/lib/runtime", () => ({
  isTauriRuntime: true,
  backendToken: () => null,
  withBackendToken: (path: string) => path,
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async (command: string, args?: Record<string, unknown>) => {
    switch (command) {
      case "get_session_open_window":
        openWindowCalls.push(args);
        {
          const sessionId = String(args?.sessionId ?? "");
          const requestId =
            typeof args?.requestId === "string" ? args.requestId : undefined;
          if (requestId) latestOpenRequestBySession.set(sessionId, requestId);
          await new Promise((resolve) => setTimeout(resolve, 5));
          if (requestId && canceledOpenRequestIds.has(requestId)) {
            throw LOAD_CANCELED_SENTINEL;
          }
          if (cancelNextOpenCalls > 0) {
            cancelNextOpenCalls -= 1;
            throw LOAD_CANCELED_SENTINEL;
          }
        }
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
      case "get_session_messages_window": {
        messagesWindowCalls.push(args);
        if (messagesWindowGate) await messagesWindowGate;
        const offset = typeof args?.offset === "number" ? (args.offset as number) : 0;
        const limit = typeof args?.limit === "number" ? (args.limit as number) : 0;
        return {
          total: totalMessages,
          start: offset,
          messages: windowSource ? windowSource.slice(offset, offset + limit) : messagesWindowMessages,
          parse_warning_count: 0,
          token_totals: tokenTotals(),
        };
      }
      case "get_session_search_text":
        searchTextCalls.push(args);
        if (searchTextGate) await searchTextGate;
        return { total: totalMessages, messages: searchTextMessages };
      case "get_session_turn_outline":
        return {
          turns: outlineEntries,
          role_counts: { user: 1, assistant: 1, tool: 0, system: 0 },
        };
      case "is_favorite":
        return false;
      case "cancel_session_load":
        {
          const sessionId = String(args?.sessionId ?? "");
          const requestId =
            typeof args?.requestId === "string" ? args.requestId : undefined;
          cancelSessionLoadCalls.push({
            sessionId,
            ...(requestId ? { requestId } : {}),
          });
          await new Promise((resolve) => setTimeout(resolve));
          if (requestId) {
            canceledOpenRequestIds.add(requestId);
          } else {
            const latestRequestId = latestOpenRequestBySession.get(sessionId);
            if (latestRequestId) canceledOpenRequestIds.add(latestRequestId);
          }
        }
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
  // Give the scroll container and rows realistic dimensions for pagination
  // and visibility calculations; happy-dom reports 0 for all of them.
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
  // Derive a stable scroll viewport for the content-visibility timeline.
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
      return this.querySelectorAll(".session-entry").length * 100;
    },
  });
  // happy-dom's scrollTo doesn't notify; the timeline relies on the scroll
  // event to track its offset.
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

beforeEach(async () => {
  cleanup();
  await new Promise((resolve) => setTimeout(resolve));
  openWindowMessages = MESSAGES;
  openWindowStart = 0;
  totalMessages = MESSAGES.length;
  messagesWindowMessages = MESSAGES;
  messagesWindowGate = null;
  windowSource = null;
  outlineEntries = [];
  searchTextMessages = searchTextOf(MESSAGES.map((m, i) => [i, m]));
  searchTextGate = null;
  searchTextCalls.length = 0;
  setPendingSessionSearch(null);
  openWindowCalls.length = 0;
  messagesWindowCalls.length = 0;
  cancelSessionLoadCalls.length = 0;
  canceledOpenRequestIds.clear();
  latestOpenRequestBySession.clear();
  cancelNextOpenCalls = 0;
});

describe("SessionView smoke", () => {
  it("mounts and renders messages once the load resolves", async () => {
    const scrollTo = vi.spyOn(Element.prototype, "scrollTo");
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
      />,
    );

    // The async session-load effect resolves the mocked tail; both messages
    // should appear in the rendered timeline.
    expect(await findByText("Hello there")).toBeInTheDocument();
    const messages = await waitFor(() => {
      const element = document.querySelector<HTMLDivElement>(".session-messages");
      expect(element).not.toBeNull();
      return element!;
    });
    // Rows live inside display:contents block wrappers — walk them by class.
    const rows = messages.querySelectorAll(".session-entry");
    expect(rows).toHaveLength(2);
    // Opening lands at the newest message: scrollTop 0 in the column-reverse scroller.
    await waitFor(() => expect(scrollTo.mock.instances).toContain(messages));
    scrollTo.mockRestore();
  });

  it("does not cancel the current load during StrictMode remount", async () => {
    const { findByText } = render(
      <StrictMode>
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
        />
      </StrictMode>,
    );

    expect(await findByText("Hello there")).toBeInTheDocument();
    await waitFor(() =>
      expect(openWindowCalls.length).toBeGreaterThanOrEqual(2),
    );
    const latestRequestId =
      openWindowCalls[openWindowCalls.length - 1]?.requestId;
    expect(typeof latestRequestId).toBe("string");
    expect(cancelSessionLoadCalls.length).toBeGreaterThan(0);
    expect(cancelSessionLoadCalls.every((call) => call.requestId)).toBe(true);
    expect(
      cancelSessionLoadCalls.some((call) => call.requestId === latestRequestId),
    ).toBe(false);
  });

  it("retries once instead of blanking when the current open is canceled", async () => {
    const warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});
    cancelNextOpenCalls = 1;

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
      />,
    );

    // The lost cancel race must not strand an empty "no messages" view —
    // the retry hydrates the timeline.
    expect(await findByText("Hello there")).toBeInTheDocument();
    expect(openWindowCalls.length).toBe(2);
    expect(String(openWindowCalls[1]?.requestId)).toContain(":retry");
    warnSpy.mockRestore();
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
    searchTextMessages = searchTextOf([
      [0, olderUserMessage],
      [1, MESSAGES[1]],
    ]);

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

    await waitFor(() => expect(searchTextCalls).toEqual([{ sessionId: META.id }]));
    // The match lives outside the initial tail; the search must bring it into
    // the window and reveal it in the rendered timeline (highlighting itself
    // runs on the CSS Highlight API, absent in happy-dom).
    expect(await findByText("我发的旧内容")).toBeInTheDocument();
  });

  it("reveals the session-wide first match without loading the whole session", async () => {
    const session = Array.from({ length: 1000 }, (_, index) =>
      messageAt(index, index === 0 ? "无常最早是用户提问" : index === 999 ? "无常后面又被提到" : `message ${index}`),
    );
    openWindowMessages = [session[999]];
    openWindowStart = 999;
    totalMessages = session.length;
    windowSource = session;
    searchTextMessages = searchTextOf(session.map((m, i) => [i, m]));

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
        expect.objectContaining({ offset: 0, limit: 300 }),
      ),
    );
    // Matches are counted on the searchable text, so every window request
    // stays page-sized (at most one 600-message batch) — the history before
    // the loaded window is never pulled in wholesale.
    expect(messagesWindowCalls.every((call) => Number(call?.limit) <= 600)).toBe(true);
    // The FIRST match session-wide is the oldest message; it must be loaded
    // and revealed even though the initial window only held the newest one.
    expect(await findByText("无常最早是用户提问")).toBeInTheDocument();
  });

  it("applies a global search handed to an open session whose search text is held", async () => {
    const session = Array.from({ length: 40 }, (_, index) =>
      messageAt(
        index,
        index === 3
          ? "alpha only here"
          : index === 4
            ? "<system-reminder>\nalpha stays hidden\n</system-reminder>"
            : index === 5
              ? "beta first"
              : index === 7
                ? "beta second"
                : `message ${index}`,
      ),
    );
    openWindowMessages = session;
    totalMessages = session.length;
    searchTextMessages = searchTextOf(session.map((m, i) => [i, m]));

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
      />,
    );

    expect(await findByText("message 39")).toBeInTheDocument();
    document.dispatchEvent(new CustomEvent(SESSION_COMMAND_EVENTS.sessionSearch));
    const input = await waitFor(() => {
      const el = document.querySelector<HTMLInputElement>(".session-search-input");
      expect(el).not.toBeNull();
      return el!;
    });
    fireEvent.input(input, { target: { value: "alpha" } });
    const count = () => document.querySelector(".session-search-count")?.textContent;
    // The reminder-only message is hidden in the timeline, so it is not a match.
    await waitFor(() => expect(count()).toBe("1/1"));

    const scrolled: string[] = [];
    const scrollIntoView = vi.spyOn(Element.prototype, "scrollIntoView").mockImplementation(function (this: Element) {
      scrolled.push(this.textContent?.trim() ?? "");
    });

    // The global overlay hands this open session a different query; the held
    // search text answers it without another fetch.
    setPendingSessionSearch({ sessionId: META.id, query: "beta" });

    await waitFor(() => expect(count()).toBe("1/2"));
    expect(input.value).toBe("beta");
    expect(searchTextCalls).toHaveLength(1);
    await waitFor(() => expect(scrolled.at(-1)).toBe("beta first"));

    fireEvent.click(document.querySelectorAll(".session-search-nav")[1]);
    await waitFor(() => expect(scrolled.at(-1)).toBe("beta second"));

    // Handing over the same query again starts over at its first match.
    setPendingSessionSearch({ sessionId: META.id, query: "beta" });
    await waitFor(() => expect(scrolled.at(-1)).toBe("beta first"));
    expect(count()).toBe("1/2");
    scrollIntoView.mockRestore();
  });

  it("shares one search-text fetch across queries committed while it loads", async () => {
    let releaseSearchText = () => {};
    searchTextGate = new Promise((resolve) => {
      releaseSearchText = resolve;
    });
    const session = Array.from({ length: 40 }, (_, index) =>
      messageAt(index, index === 5 ? "alpha beta" : index === 9 ? "alpha only" : `message ${index}`),
    );
    openWindowMessages = session;
    totalMessages = session.length;
    searchTextMessages = searchTextOf(session.map((m, i) => [i, m]));

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
      />,
    );

    expect(await findByText("message 39")).toBeInTheDocument();
    document.dispatchEvent(new CustomEvent(SESSION_COMMAND_EVENTS.sessionSearch));
    const input = await waitFor(() => {
      const el = document.querySelector<HTMLInputElement>(".session-search-input");
      expect(el).not.toBeNull();
      return el!;
    });

    // Typing on: the second query commits while the first fetch still loads.
    fireEvent.input(input, { target: { value: "alpha" } });
    await waitFor(() => expect(searchTextCalls).toHaveLength(1));
    fireEvent.input(input, { target: { value: "alpha b" } });
    await new Promise((resolve) => setTimeout(resolve, SESSION_SEARCH_DEBOUNCE_MS + 50));
    releaseSearchText();

    const count = () => document.querySelector(".session-search-count")?.textContent;
    await waitFor(() => expect(count()).toBe("1/1"));
    expect(searchTextCalls).toHaveLength(1);
  });

  it("re-centers a far search match until it stays on screen", async () => {
    const box = (top: number, bottom: number) => ({
      x: 0,
      y: top,
      width: 0,
      height: bottom - top,
      top,
      right: 0,
      bottom,
      left: 0,
      toJSON: () => ({}),
    });
    const inMatchRow = (node: Node | null) =>
      !!(node instanceof Element ? node : node?.parentElement)?.closest('[data-entry-key^="msg-0-"]');
    let matchScrolls = 0;
    const scrollIntoView = vi.spyOn(Element.prototype, "scrollIntoView").mockImplementation(function (this: Element) {
      if (!this.classList.contains("session-entry") && inMatchRow(this)) matchScrolls += 1;
    });
    const nativeElementRect = Element.prototype.getBoundingClientRect;
    const viewportGeometry = vi
      .spyOn(Element.prototype, "getBoundingClientRect")
      .mockImplementation(function (this: Element) {
        return this.classList.contains("session-messages") ? box(0, 800) : nativeElementRect.call(this);
      });
    // The first centering lands on estimated row heights; real heights then
    // push the match below the viewport until a second pass re-centers it.
    const nativeRangeRect = Range.prototype.getBoundingClientRect;
    const matchGeometry = vi.spyOn(Range.prototype, "getBoundingClientRect").mockImplementation(function (this: Range) {
      if (!inMatchRow(this.startContainer)) return nativeRangeRect.call(this);
      const top = matchScrolls < 2 ? 900 : 400;
      return box(top, top + 20);
    });
    const session = Array.from({ length: 1000 }, (_, index) =>
      messageAt(index, index === 0 ? "无常最早是用户提问" : `message ${index}`),
    );
    openWindowMessages = [session[999]];
    openWindowStart = 999;
    totalMessages = session.length;
    windowSource = session;
    searchTextMessages = searchTextOf(session.map((m, i) => [i, m]));

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
      />,
    );

    expect(await findByText("message 999")).toBeInTheDocument();
    document.dispatchEvent(new CustomEvent(SESSION_COMMAND_EVENTS.sessionSearch));
    const input = await waitFor(() => {
      const el = document.querySelector<HTMLInputElement>(".session-search-input");
      expect(el).not.toBeNull();
      return el!;
    });

    fireEvent.input(input, { target: { value: "最早" } });

    // The match itself is centered, not just its row's top.
    await waitFor(() => expect(matchScrolls).toBe(2), { timeout: 5000 });
    expect(scrollIntoView).toHaveBeenCalledWith({ block: "center" });
    // Once on screen the reveal stops re-aligning.
    await new Promise((resolve) => setTimeout(resolve, 300));
    expect(matchScrolls).toBe(2);
    matchGeometry.mockRestore();
    viewportGeometry.mockRestore();
    scrollIntoView.mockRestore();
  }, 10_000);

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
    openWindowMessages = manyMessages;
    totalMessages = manyMessages.length;
    searchTextMessages = searchTextOf(manyMessages.map((m, i) => [i, m]));

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
      />,
    );

    // Opening lands at the newest messages. With content-visibility rendering
    // every LOADED row is real DOM (the browser only skips off-screen paint),
    // so older loaded rows are present in the tree even before scrolling to them
    // — which is what makes native find-in-page and text selection work.
    expect(await findByText("message 119")).toBeInTheDocument();
    expect(queryByText("target after search")).toBeInTheDocument();
    expect(queryByText("oldest still above")).toBeInTheDocument();

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
    // container to the very top keeps the oldest loaded row present and reachable.
    const messagesEl =
      document.querySelector<HTMLDivElement>(".session-messages");
    expect(messagesEl).not.toBeNull();
    messagesEl!.scrollTop = 0;
    fireEvent.scroll(messagesEl!);

    await waitFor(() =>
      expect(queryByText("oldest still above")).toBeInTheDocument(),
    );
  });

  it("keeps the view still when rows below the viewport change height", async () => {
    const observers: Array<{ callback: ResizeObserverCallback; targets: Set<Element> }> = [];
    const silent = globalThis.ResizeObserver;
    (globalThis as { ResizeObserver: unknown }).ResizeObserver = class {
      private readonly record: { callback: ResizeObserverCallback; targets: Set<Element> };
      constructor(callback: ResizeObserverCallback) {
        this.record = { callback, targets: new Set() };
        observers.push(this.record);
      }
      observe(target: Element) {
        this.record.targets.add(target);
      }
      unobserve(target: Element) {
        this.record.targets.delete(target);
      }
      disconnect() {
        this.record.targets.clear();
      }
    };
    try {
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
        />,
      );
      expect(await findByText("General Kenobi reply")).toBeInTheDocument();
      const scroller = document.querySelector<HTMLDivElement>(".session-messages")!;
      const [newest, older] = [...scroller.querySelectorAll<HTMLElement>(".session-entry")];
      const rowObserver = await waitFor(() => {
        const found = observers.find((observer) => observer.targets.has(newest));
        expect(found).toBeDefined();
        return found!;
      });

      // An 800px viewport over 5000px of content, 1000px up from the newest
      // end. offsetTop counts from the viewport top at scrollTop 0, so the
      // newest row sits below the view and the older row inside it.
      let scrollTop = -1000;
      Object.defineProperty(scroller, "scrollTop", {
        configurable: true,
        get: () => scrollTop,
        set: (value: number) => {
          scrollTop = value;
        },
      });
      Object.defineProperty(scroller, "scrollHeight", { configurable: true, get: () => 5000 });
      Object.defineProperty(newest, "offsetTop", { configurable: true, get: () => 300 });
      Object.defineProperty(older, "offsetTop", { configurable: true, get: () => -900 });
      const resize = (...rows: Array<[Element, number]>) =>
        rowObserver.callback(
          rows.map(([target, blockSize]) => ({ target, borderBoxSize: [{ blockSize }] })) as unknown as ResizeObserverEntry[],
          {} as ResizeObserver,
        );

      resize([newest, 100], [older, 100]);
      resize([newest, 160]);
      expect(scrollTop).toBe(-1060);
      resize([older, 180]);
      expect(scrollTop).toBe(-1060);
      // Mid-bounce past the oldest edge a write would lose to the animation.
      scrollTop = -4300;
      resize([newest, 200]);
      expect(scrollTop).toBe(-4300);
    } finally {
      (globalThis as { ResizeObserver: unknown }).ResizeObserver = silent;
    }
  });

  it("re-centers and re-aligns a far minimap jump after row layout settles", async () => {
    let targetScrolls = 0;
    const scrollIntoView = vi.spyOn(Element.prototype, "scrollIntoView").mockImplementation(function (this: Element) {
      if (this.getAttribute("data-entry-key")?.startsWith("msg-450-")) targetScrolls += 1;
    });
    const nativeRect = Element.prototype.getBoundingClientRect;
    const targetGeometry = vi
      .spyOn(Element.prototype, "getBoundingClientRect")
      .mockImplementation(function (this: Element) {
        if (!this.getAttribute("data-entry-key")?.startsWith("msg-450-")) return nativeRect.call(this);
        const top = targetScrolls < 2 ? 100 : 0;
        return { x: 0, y: top, width: 0, height: 0, top, right: 0, bottom: top, left: 0, toJSON: () => ({}) };
      });
    // Open a 900-message session at its newest tail, then jump to a turn in
    // the middle. The jump must fetch a small window around the target — NOT
    // the hundreds of messages in between — and align again after the newly
    // revealed content-visibility rows replace their estimated heights.
    const all = Array.from({ length: 900 }, (_, index) => messageAt(index));
    openWindowMessages = all.slice(600);
    openWindowStart = 600;
    totalMessages = 900;
    messagesWindowMessages = all.slice(300, 600);
    outlineEntries = [
      { ordinal: 0, message_index: 450, user_text: "middle turn", reply_text: "" },
      {
        ordinal: 1,
        message_index: 700,
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
      />,
    );

    await waitFor(() => expect(document.body.textContent).toContain("message 899"), { timeout: 5000 });
    const middleTick = await waitFor(() => getByLabelText("middle turn"), { timeout: 5000 });

    fireEvent.click(middleTick);

    await waitFor(() =>
      expect(messagesWindowCalls).toContainEqual(
        expect.objectContaining({ offset: 300, limit: 300 }),
      ),
    );
    // No bulk fetch of the gap between the tail and the target.
    for (const call of messagesWindowCalls) {
      expect(call?.limit).toBeLessThanOrEqual(600);
    }
    const targetMessage = await findByText("message 450");
    const targetRow = targetMessage.closest(".session-entry");
    expect(targetRow).not.toBeNull();
    await waitFor(() => {
      expect(scrollIntoView.mock.instances.filter((instance) => instance === targetRow)).toHaveLength(2);
    });
    targetGeometry.mockRestore();
    scrollIntoView.mockRestore();
  }, 10_000);

  it("keeps the latest minimap click when an earlier re-center returns later", async () => {
    const all = Array.from({ length: 900 }, (_, index) => messageAt(index));
    openWindowMessages = all.slice(600);
    openWindowStart = 600;
    totalMessages = 900;
    messagesWindowMessages = all.slice(300, 600);
    let releaseMessagesWindow = () => {};
    messagesWindowGate = new Promise((resolve) => {
      releaseMessagesWindow = () => resolve();
    });
    outlineEntries = [
      { ordinal: 0, message_index: 450, user_text: "middle turn", reply_text: "" },
      { ordinal: 1, message_index: 700, user_text: "latest turn", reply_text: "" },
    ];

    const { findByText, getByLabelText, queryByText } = render(
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
      />,
    );

    await findByText("message 899");
    fireEvent.click(getByLabelText("middle turn"));
    await waitFor(() =>
      expect(messagesWindowCalls).toContainEqual(expect.objectContaining({ offset: 300, limit: 300 })),
    );
    fireEvent.click(getByLabelText("latest turn"));
    messagesWindowGate = null;
    releaseMessagesWindow();

    await waitFor(() => {
      expect(queryByText("message 700")).toBeInTheDocument();
      expect(queryByText("message 450")).not.toBeInTheDocument();
    });
  }, 10_000);
});
