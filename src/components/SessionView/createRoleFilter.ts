import { createMemo, createSignal } from "solid-js";
import type { Accessor } from "solid-js";
import type { MessageRole } from "../../lib/types";
import type { ProcessedEntry } from "./hooks";

export interface CreateRoleFilterResult {
  /** Currently hidden roles. */
  hiddenRoles: Accessor<Set<MessageRole>>;
  /** Per-role message counts for the filter toolbar. */
  roleCounts: Accessor<Record<string, number>>;
  /** Processed entries with hidden roles removed. */
  filteredEntries: Accessor<ProcessedEntry[]>;
  /** Toggle a role's visibility. */
  toggleRole: (role: MessageRole) => void;
}

/**
 * Owns the role-filter slice of SessionView: the `hiddenRoles` set plus the
 * derived `filteredEntries` and `roleCounts` memos. Bodies are moved verbatim
 * from the inline component so dependency tracking is unchanged.
 */
export function createRoleFilter(
  processedEntries: Accessor<ProcessedEntry[]>,
): CreateRoleFilterResult {
  const [hiddenRoles, setHiddenRoles] = createSignal<Set<MessageRole>>(
    new Set(),
  );

  // Apply role filtering
  const filteredEntries = createMemo(() => {
    const hidden = hiddenRoles();
    if (hidden.size === 0) return processedEntries();
    return processedEntries().filter((e) => {
      if (e.type === "time-sep") return true;
      if (e.type === "merged-tools") return !hidden.has("tool");
      return !hidden.has(e.msg.role);
    });
  });

  // Role counts for filter toolbar
  const roleCounts = createMemo(() => {
    const counts: Record<string, number> = {
      user: 0,
      assistant: 0,
      tool: 0,
      system: 0,
    };
    for (const e of processedEntries()) {
      if (e.type === "message")
        counts[e.msg.role] = (counts[e.msg.role] || 0) + 1;
      else if (e.type === "merged-tools") counts.tool += e.messages.length;
    }
    return counts;
  });

  function toggleRole(role: MessageRole) {
    setHiddenRoles((prev) => {
      const next = new Set(prev);
      if (next.has(role)) next.delete(role);
      else next.add(role);
      return next;
    });
  }

  return { hiddenRoles, roleCounts, filteredEntries, toggleRole };
}
