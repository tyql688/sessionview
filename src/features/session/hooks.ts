import type { Message, MessageRole } from "@/lib/types";
import { parseTimestamp, formatTimeOnly } from "@/lib/formatters";
import { isAgentToolMessage } from "@/lib/subagent";

/// A renderable timeline row: a message, a merged run of tool calls, or a
/// time separator.
export type ProcessedEntry =
  | {
      key: string;
      type: "message";
      msg: Message;
      messageIndex: number;
    }
  | { key: string; type: "time-sep"; time: string }
  | {
      key: string;
      type: "merged-tools";
      tools: string[];
      messages: Message[];
      messageIndices: number[];
    };

/**
 * Rough per-row height (px) used for `contain-intrinsic-size`, so off-screen
 * content-visibility rows reserve close to their real height and revealing them
 * on scroll doesn't shift the read position. Only the FIRST paint uses this
 * value; `contain-intrinsic-size: auto` then remembers each row's measured size,
 * so re-scrolls are exact. Accuracy only needs to be in the right ballpark — a
 * flat guess makes short rows jump ~200px, an in-range guess ~50px.
 */
export function estimateEntryHeight(entry: ProcessedEntry): number {
  if (entry.type === "time-sep") return 32;
  // Tool rows render collapsed by default (one summary line); their payload
  // size only affects the expanded body. A content-based estimate reserves
  // hundreds of blank pixels per row (measured est 242px vs real 38px), which
  // the first paint then snaps back, jumping the timeline during scroll.
  if (entry.type === "merged-tools" || entry.msg.role === "tool") return 44;
  const content = entry.msg.content;
  let height = 44; // role header + vertical padding
  // Alternating prose / fenced-code segments split on ``` markers.
  const segments = content.split("```");
  for (let i = 0; i < segments.length; i += 1) {
    const segment = segments[i];
    if (i % 2 === 1) {
      const lang = segment.split("\n", 1)[0]?.trim().toLowerCase();
      if (lang === "mermaid") {
        height += 372; // fixed-height mermaid canvas + toolbar (see markdown.css)
        continue;
      }
      const lines = segment.split("\n").length;
      height += lines * 21 + 44; // code line height + block chrome
    } else {
      const lines = segment.split("\n").reduce((sum, line) => sum + Math.max(1, Math.ceil(line.length / 90)), 0);
      height += lines * 22; // wrapped prose line height
    }
  }
  const imageCount = (content.match(/\[Image:/g) ?? []).length;
  height += imageCount * 260; // inline images reserve a preview-sized block
  return Math.max(48, Math.round(height));
}

/**
 * Search covers user + assistant dialogue only, in-session and global alike:
 * tool/thinking blocks render collapsed, so hits inside them would be matches
 * the highlight pass cannot show or scroll to. The backend's
 * `build_session_search_text` (in-session) and `indexable_content_text`
 * (global index) select the searchable text by the same rule.
 */
export function isSearchableRole(role: MessageRole): boolean {
  return role === "user" || role === "assistant";
}

/** Harness-injected template content (Codex context and instructions,
 * slash-command wrappers, claude-mem observations): present anywhere, each
 * marks the whole message as injected. */
const SYSTEM_CONTENT_MARKERS = [
  "</observation>",
  "</command-message>",
  "<INSTRUCTIONS>",
  "<environment_context>",
  "<permissions instructions>",
  "</facts>",
  "</narrative>",
  "</concepts>",
];

/**
 * Whether a user/assistant message is injected system content, which the
 * timeline hides and in-session search therefore skips. A system reminder
 * marks the message only when it opens it: replies quote the tag in prose and
 * code, and a quote must not hide the reply.
 */
export function isSystemContent(message: Pick<Message, "role" | "content">): boolean {
  if (message.role === "tool" || message.role === "system") return false;
  const content = message.content.trimStart();
  return content.startsWith("<system-reminder>") || SYSTEM_CONTENT_MARKERS.some((marker) => content.includes(marker));
}

/** First absolute message index an entry anchors to (null for separators). */
export function entryFirstMessageIndex(entry: ProcessedEntry): number | null {
  if (entry.type === "message") return entry.messageIndex;
  if (entry.type === "merged-tools") return entry.messageIndices[0];
  return null;
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
export function processMessages(msgs: Message[], windowStart: number): ProcessedEntry[] {
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
      while (j < renderableMsgs.length && isMergeableToolMessage(renderableMsgs[j].msg)) {
        toolGroup.push(renderableMsgs[j].msg);
        toolIndices.push(renderableMsgs[j].messageIndex);
        j++;
      }
      if (toolGroup.length > 1) {
        const toolNames = toolGroup.map((m) => m.tool_name).filter((n): n is string => !!n && n.trim().length > 0);
        entries.push({
          // Keys are built on absolute indices so prepending an older page
          // never re-keys (and remounts) the already-rendered rows.
          key: `tools-${toolIndices[0]}-${toolGroup[0].timestamp ?? "none"}`,
          type: "merged-tools",
          tools: toolNames,
          messages: toolGroup,
          messageIndices: toolIndices,
        });
      } else {
        entries.push({
          key: `msg-${messageIndex}-${msg.role}-${msg.timestamp ?? "none"}`,
          type: "message",
          msg,
          messageIndex,
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
        });
      }
    }

    entries.push({
      key: `msg-${messageIndex}-${msg.role}-${msg.timestamp ?? "none"}`,
      type: "message",
      msg,
      messageIndex,
    });
    i++;
  }

  return entries;
}
