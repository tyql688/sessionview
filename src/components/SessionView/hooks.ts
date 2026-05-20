import type { Message } from "../../lib/types";
import { parseTimestamp, formatTimeOnly } from "../../lib/formatters";

/// Lowercased haystack used by in-session search. Computed once when the
/// entry is built so per-keystroke `findNewestMatchingEntryIndex` /
/// `countMatchingEntries` walks avoid re-lowercasing every entry.
export type ProcessedEntry =
  | { key: string; type: "message"; msg: Message; searchHaystack: string }
  | { key: string; type: "time-sep"; time: string; searchHaystack: string }
  | {
      key: string;
      type: "merged-tools";
      tools: string[];
      messages: Message[];
      searchHaystack: string;
    };

function messageHaystack(msg: Message): string {
  return (msg.content ?? "").toLocaleLowerCase();
}

function toolGroupHaystack(messages: Message[]): string {
  return messages
    .map((m) =>
      [m.tool_name, m.tool_input, m.content]
        .filter((value): value is string => !!value)
        .join("\n"),
    )
    .join("\n")
    .toLocaleLowerCase();
}

export function isRenderableMessage(msg: Message): boolean {
  if (msg.role === "tool") {
    // Hide orphaned Anthropic tool result ids when no metadata could recover
    // a useful display name.
    if (msg.tool_name?.startsWith("toolu_") && !msg.tool_metadata) {
      return false;
    }
    return !!msg.content || !!msg.tool_input || !!msg.tool_name;
  }

  return msg.content.trim().length > 0;
}

export function processMessages(msgs: Message[]): ProcessedEntry[] {
  const entries: ProcessedEntry[] = [];
  const renderableMsgs = msgs.filter(isRenderableMessage);
  let i = 0;

  while (i < renderableMsgs.length) {
    const msg = renderableMsgs[i];

    // Try to merge consecutive tool messages
    if (msg.role === "tool") {
      const toolGroup: Message[] = [msg];
      let j = i + 1;
      while (j < renderableMsgs.length && renderableMsgs[j].role === "tool") {
        toolGroup.push(renderableMsgs[j]);
        j++;
      }
      if (toolGroup.length > 1) {
        const toolNames = toolGroup
          .map((m) => m.tool_name)
          .filter((n): n is string => !!n && n.trim().length > 0);
        entries.push({
          key: `tools-${i}-${toolGroup[0].timestamp ?? "none"}`,
          type: "merged-tools",
          tools: toolNames,
          messages: toolGroup,
          searchHaystack: toolGroupHaystack(toolGroup),
        });
      } else {
        entries.push({
          key: `msg-${i}-${msg.role}-${msg.timestamp ?? "none"}`,
          type: "message",
          msg,
          searchHaystack: messageHaystack(msg),
        });
      }
      i = j;
      continue;
    }

    // Check time gap with previous message
    if (entries.length > 0) {
      const prevEntry = entries[entries.length - 1];
      let prevTs: number | null = null;
      if (prevEntry.type === "message") {
        prevTs = parseTimestamp(prevEntry.msg.timestamp);
      } else if (prevEntry.type === "merged-tools") {
        const lastTool = prevEntry.messages[prevEntry.messages.length - 1];
        prevTs = parseTimestamp(lastTool.timestamp);
      }
      const curTs = parseTimestamp(msg.timestamp);
      const TIME_GAP_THRESHOLD_MS = 5 * 60 * 1000; // 5 minutes
      if (prevTs && curTs && curTs - prevTs > TIME_GAP_THRESHOLD_MS) {
        entries.push({
          key: `sep-${i}-${curTs}`,
          type: "time-sep",
          time: formatTimeOnly(curTs),
          searchHaystack: "",
        });
      }
    }

    entries.push({
      key: `msg-${i}-${msg.role}-${msg.timestamp ?? "none"}`,
      type: "message",
      msg,
      searchHaystack: messageHaystack(msg),
    });
    i++;
  }

  return entries;
}
