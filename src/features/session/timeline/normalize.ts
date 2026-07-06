import { parseTimestamp } from "../../../lib/formatters";
import type { Message } from "../../../lib/types";
import type {
  CommandKind,
  ImageRef,
  NormalizeResult,
  NormalizeSkips,
  TimelineItem,
} from "./types";

const THINKING_PREFIX = "[thinking]\n";
const LEGACY_LOCAL_COMMAND_PREFIX = "[local_command]";

/** Known `[tag] detail` system subtypes emitted by the backend. Anything
 * tagged but not listed here still renders — as a "plain" marker carrying the
 * full content, never dropped. */
const SYSTEM_SUBTYPES = new Set([
  "turn_duration",
  "compact_boundary",
  "microcompact_boundary",
  "stop_hook_summary",
  "api_error",
  "away_summary",
  "scheduled_task_fire",
  "pr_link",
  "error",
  "turn_aborted",
  "context_compacted",
]);

/** Injected prompt-template noise: rendered nowhere today, counted in
 * `skipped.template` so the suppression stays observable. */
const TEMPLATE_MARKERS = [
  "</observation>",
  "</command-message>",
  "<INSTRUCTIONS>",
  "<environment_context>",
  "<permissions instructions>",
  "</facts>",
  "</narrative>",
  "</concepts>",
  "<system-reminder>",
];

const IMAGE_PLACEHOLDER_RE =
  /\[Image(?:\s*#\d+)?(?::\s*source:\s*([^\]]+))?\]/g;

/** Split content into markdown text and the image refs embedded in it. */
export function extractImages(content: string): {
  markdown: string;
  images: ImageRef[];
} {
  if (!content.includes("[Image")) {
    return { markdown: content, images: [] };
  }
  const images: ImageRef[] = [];
  const markdown = content
    .replace(IMAGE_PLACEHOLDER_RE, (_match, source: string | undefined) => {
      images.push({ source: source?.trim() ?? null });
      return "";
    })
    .trim();
  return { markdown, images };
}

function commandKind(msg: Message): CommandKind | null {
  if (msg.message_kind === "command_input") return "input";
  if (msg.message_kind === "command_output") return "output";
  if (msg.content.trimStart().startsWith(LEGACY_LOCAL_COMMAND_PREFIX)) {
    return msg.role === "user" ? "input" : "output";
  }
  return null;
}

function stripLegacyCommandPrefix(content: string): string {
  const trimmed = content.trimStart();
  if (!trimmed.startsWith(LEGACY_LOCAL_COMMAND_PREFIX)) return content;
  return trimmed.slice(LEGACY_LOCAL_COMMAND_PREFIX.length).trimStart();
}

function isTemplateNoise(content: string): boolean {
  return TEMPLATE_MARKERS.some((marker) => content.includes(marker));
}

/** Orphaned Anthropic tool results: a generated `toulu_…` id where no
 * metadata could recover a usable display name. */
function isOrphanTool(msg: Message): boolean {
  return !!msg.tool_name?.startsWith("toulu_") && !msg.tool_metadata;
}

function isEmptyTool(msg: Message): boolean {
  return !msg.content && !msg.tool_input && !msg.tool_name;
}

export interface NormalizeOptions {
  /** Absolute session index of messages[0]. */
  windowStart: number;
}

/**
 * Message[] → TimelineItem[]. The single place that understands the backend's
 * string conventions; per-provider quirks belong here too when they become
 * unavoidable. Unrecognized shapes map to `kind: "unknown"` (rendered as a
 * visible raw block) — never silently dropped. The only messages not emitted
 * are three deliberate, counted suppressions (see NormalizeSkips).
 */
export function normalizeMessages(
  messages: Message[],
  opts: NormalizeOptions,
): NormalizeResult {
  const items: TimelineItem[] = [];
  const skipped: NormalizeSkips = { empty: 0, template: 0, orphanTool: 0 };

  messages.forEach((msg, i) => {
    const index = opts.windowStart + i;
    const ts = parseTimestamp(msg.timestamp);

    switch (msg.role) {
      case "user":
      case "assistant": {
        if (msg.content.trim().length === 0) {
          skipped.empty += 1;
          return;
        }
        if (isTemplateNoise(msg.content)) {
          skipped.template += 1;
          return;
        }
        const command = commandKind(msg);
        const { markdown, images } = extractImages(
          stripLegacyCommandPrefix(msg.content),
        );
        if (msg.role === "user") {
          items.push({ kind: "user", index, markdown, images, ts, command });
        } else {
          items.push({
            kind: "assistantText",
            index,
            markdown,
            images,
            ts,
            usage: msg.token_usage,
            model: msg.model ?? null,
            command,
          });
        }
        return;
      }
      case "system": {
        if (msg.content.startsWith(THINKING_PREFIX)) {
          items.push({
            kind: "thinking",
            index,
            text: msg.content.slice(THINKING_PREFIX.length),
            ts,
          });
          return;
        }
        if (msg.content.trim().length === 0) {
          skipped.empty += 1;
          return;
        }
        const tagged = msg.content.match(/^\[(\w+)\]\s*(.*)/s);
        if (tagged && SYSTEM_SUBTYPES.has(tagged[1])) {
          items.push({
            kind: "systemMarker",
            index,
            subtype: tagged[1],
            detail: tagged[2],
            ts,
          });
          return;
        }
        items.push({
          kind: "systemMarker",
          index,
          subtype: "plain",
          detail: msg.content,
          ts,
        });
        return;
      }
      case "tool": {
        if (isOrphanTool(msg)) {
          skipped.orphanTool += 1;
          return;
        }
        if (isEmptyTool(msg)) {
          skipped.empty += 1;
          return;
        }
        items.push({ kind: "toolStep", index, message: msg, ts });
        return;
      }
      default: {
        items.push({ kind: "unknown", index, raw: msg });
      }
    }
  });

  return { items, skipped };
}
