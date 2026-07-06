import type { Message, MessageRole } from "../../lib/types";
import { parseTimestamp, formatTimeOnly } from "../../lib/formatters";
import { isAgentToolMessage } from "../../lib/subagent";

/// Lowercased haystack used by in-session search. Computed once when the
/// entry is built so per-keystroke search walks
/// avoid re-lowercasing every entry.
export type ProcessedEntry =
  | {
      key: string;
      type: "message";
      msg: Message;
      messageIndex: number;
      searchHaystack: string;
    }
  | { key: string; type: "time-sep"; time: string; searchHaystack: string }
  | {
      key: string;
      type: "merged-tools";
      tools: string[];
      messages: Message[];
      messageIndices: number[];
      searchHaystack: string;
    };

/**
 * Search covers user + assistant dialogue only, so in-session and global search
 * stay consistent. Tool calls/results, thinking, and system messages are
 * excluded from the haystack (here) and from the highlight pass (in index.tsx).
 */
export function isSearchableRole(role: MessageRole): boolean {
  return role === "user" || role === "assistant";
}

function messageHaystack(msg: Message): string {
  if (!isSearchableRole(msg.role)) return "";
  return (msg.content ?? "").toLocaleLowerCase();
}

function isMergeableToolMessage(msg: Message): boolean {
  return msg.role === "tool" && !isAgentToolMessage(msg);
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

/**
 * `windowStart` is the absolute session index of `msgs[0]` — messages arrive
 * as a window into the full session, but outline ordinals, `data-turn`
 * anchors, and `revealMessageIndex` all speak absolute indices. Emitting
 * window-relative indices here silently broke turn anchors and minimap jumps
 * for any session larger than the initial tail.
 */
export function processMessages(
  msgs: Message[],
  windowStart: number,
): ProcessedEntry[] {
  const entries: ProcessedEntry[] = [];
  const renderableMsgs = msgs
    .map((msg, i) => ({ msg, messageIndex: windowStart + i }))
    .filter(({ msg }) => isRenderableMessage(msg));
  let i = 0;

  while (i < renderableMsgs.length) {
    const { msg, messageIndex } = renderableMsgs[i];

    // Try to merge consecutive tool messages
    if (isMergeableToolMessage(msg)) {
      const toolGroup: Message[] = [msg];
      const toolIndices: number[] = [messageIndex];
      let j = i + 1;
      while (
        j < renderableMsgs.length &&
        isMergeableToolMessage(renderableMsgs[j].msg)
      ) {
        toolGroup.push(renderableMsgs[j].msg);
        toolIndices.push(renderableMsgs[j].messageIndex);
        j++;
      }
      if (toolGroup.length > 1) {
        const toolNames = toolGroup
          .map((m) => m.tool_name)
          .filter((n): n is string => !!n && n.trim().length > 0);
        entries.push({
          // Keys are built on absolute indices so prepending an older page
          // never re-keys (and remounts) the already-rendered rows.
          key: `tools-${toolIndices[0]}-${toolGroup[0].timestamp ?? "none"}`,
          type: "merged-tools",
          tools: toolNames,
          messages: toolGroup,
          messageIndices: toolIndices,
          // Tool groups are not searchable — search covers user + assistant only.
          searchHaystack: "",
        });
      } else {
        entries.push({
          key: `msg-${messageIndex}-${msg.role}-${msg.timestamp ?? "none"}`,
          type: "message",
          msg,
          messageIndex,
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
          key: `sep-${messageIndex}-${curTs}`,
          type: "time-sep",
          time: formatTimeOnly(curTs),
          searchHaystack: "",
        });
      }
    }

    entries.push({
      key: `msg-${messageIndex}-${msg.role}-${msg.timestamp ?? "none"}`,
      type: "message",
      msg,
      messageIndex,
      searchHaystack: messageHaystack(msg),
    });
    i++;
  }

  return entries;
}
