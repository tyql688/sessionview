import { render, waitFor } from "@solidjs/testing-library";
import { beforeAll, describe, expect, it, vi } from "vitest";

import type { Message, SessionMeta } from "../../lib/types";

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

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async (command: string) => {
    switch (command) {
      case "get_session_open_window":
        return {
          meta: META,
          window: {
            total: MESSAGES.length,
            start: 0,
            messages: MESSAGES,
            parse_warning_count: 0,
            token_totals: {
              input_tokens: 0,
              output_tokens: 0,
              cache_read_tokens: 0,
              cache_write_tokens: 0,
            },
          },
        };
      case "get_session_meta":
        return META;
      case "get_session_messages_window":
        return {
          total: MESSAGES.length,
          start: 0,
          messages: MESSAGES,
          parse_warning_count: 0,
          token_totals: {
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
          },
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
});
