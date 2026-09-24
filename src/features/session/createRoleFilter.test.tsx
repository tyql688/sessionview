import { act, renderHook } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import type { Message } from "@/lib/types";
import type { ProcessedEntry } from "@/features/session/hooks";
import { useRoleFilter } from "@/features/session/createRoleFilter";

const baseMessage: Message = {
  role: "assistant",
  content: "",
  timestamp: null,
  tool_name: null,
  tool_input: null,
  token_usage: null,
};

function messageEntry(role: Message["role"], index: number): ProcessedEntry {
  const msg = { ...baseMessage, role, content: `${role} ${index}` };
  return {
    key: `msg-${index}-${role}`,
    type: "message",
    msg,
    messageIndex: index,
  };
}

describe("useRoleFilter", () => {
  it("keeps every role visible when focus mode is off", () => {
    const entries = [
      messageEntry("user", 0),
      messageEntry("assistant", 1),
      messageEntry("tool", 2),
      messageEntry("system", 3),
    ];

    const { result } = renderHook(() => useRoleFilter(entries, false));

    expect(result.current.filteredEntries).toEqual(entries);
  });

  it("shows only user and assistant messages in focus mode", () => {
    const entries = [
      messageEntry("user", 0),
      messageEntry("assistant", 1),
      messageEntry("tool", 2),
      messageEntry("system", 3),
    ];

    const { result } = renderHook(() => useRoleFilter(entries, true));

    expect(
      result.current.filteredEntries.map((entry) =>
        entry.type === "message" ? entry.msg.role : entry.type,
      ),
    ).toEqual(["user", "assistant"]);
    expect(result.current.hiddenRoles.has("tool")).toBe(true);
    expect(result.current.hiddenRoles.has("system")).toBe(true);
  });

  it("drops time separators entirely in focus mode", () => {
    const entries = [
      messageEntry("user", 0),
      { key: "sep-1", type: "time-sep" as const, time: "10:00" },
      messageEntry("tool", 1),
      { key: "sep-2", type: "time-sep" as const, time: "10:20" },
      messageEntry("assistant", 2),
    ];

    const { result } = renderHook(() => useRoleFilter(entries, true));

    expect(result.current.filteredEntries.every((entry) => entry.type !== "time-sep")).toBe(true);
  });

  it("collapses separator runs left behind by hidden roles", () => {
    const entries = [
      messageEntry("user", 0),
      { key: "sep-1", type: "time-sep" as const, time: "10:00" },
      messageEntry("tool", 1),
      { key: "sep-2", type: "time-sep" as const, time: "10:20" },
      messageEntry("assistant", 2),
      { key: "sep-3", type: "time-sep" as const, time: "10:40" },
      messageEntry("tool", 3),
    ];

    const { result } = renderHook(() => useRoleFilter(entries, false));
    act(() => result.current.toggleRole("tool"));

    const kinds = result.current.filteredEntries.map((entry) =>
      entry.type === "message" ? entry.msg.role : entry.type,
    );
    // One separator survives before the assistant; the trailing one (its
    // message was hidden) disappears instead of dangling at the end.
    expect(kinds).toEqual(["user", "time-sep", "assistant"]);
  });
});
