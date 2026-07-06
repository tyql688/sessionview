import { useMemo, useState } from "react";
import type { MessageRole } from "@/lib/types";
import type { ProcessedEntry } from "@/components/SessionView/hooks";

export interface CreateRoleFilterResult {
  /** Currently hidden roles. */
  hiddenRoles: Set<MessageRole>;
  /** Per-role message counts for the filter toolbar. */
  roleCounts: Record<string, number>;
  /** Processed entries with hidden roles removed. */
  filteredEntries: ProcessedEntry[];
  /** Toggle a role's visibility. */
  toggleRole: (role: MessageRole) => void;
}

/**
 * Owns the role-filter slice of SessionView: the `hiddenRoles` set plus the
 * derived `filteredEntries` and `roleCounts` memos. Bodies are moved verbatim
 * from the inline component so dependency tracking is unchanged.
 *
 * Now a React hook: call it at the top level of a component.
 */
export function useRoleFilter(
  processedEntries: ProcessedEntry[],
): CreateRoleFilterResult {
  const [hiddenRoles, setHiddenRoles] = useState<Set<MessageRole>>(new Set());

  // Apply role filtering
  const filteredEntries = useMemo(() => {
    const hidden = hiddenRoles;
    if (hidden.size === 0) return processedEntries;
    return processedEntries.filter((e) => {
      if (e.type === "time-sep") return true;
      if (e.type === "merged-tools") return !hidden.has("tool");
      return !hidden.has(e.msg.role);
    });
  }, [processedEntries, hiddenRoles]);

  // Role counts for filter toolbar
  const roleCounts = useMemo(() => {
    const counts: Record<string, number> = {
      user: 0,
      assistant: 0,
      tool: 0,
      system: 0,
    };
    for (const e of processedEntries) {
      if (e.type === "message")
        counts[e.msg.role] = (counts[e.msg.role] || 0) + 1;
      else if (e.type === "merged-tools") counts.tool += e.messages.length;
    }
    return counts;
  }, [processedEntries]);

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
