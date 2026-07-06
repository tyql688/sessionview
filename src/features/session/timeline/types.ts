import type { Message, TokenUsage } from "../../../lib/types";

/** An inline image extracted from message content. `source` is the original
 * path/URL from the `[Image: source: …]` placeholder, or null for bare
 * `[Image]` markers whose source the provider did not record. */
export interface ImageRef {
  source: string | null;
}

export type CommandKind = "input" | "output";

/**
 * The timeline's own typed view of a message. All string-convention parsing
 * (`[thinking]\n` prefixes, `[subtype]` markers, image placeholders) happens
 * exactly once, in `normalizeMessages` — components never touch those
 * conventions. `index` is the ABSOLUTE session index of the source message.
 */
export type TimelineItem =
  | {
      kind: "user";
      index: number;
      markdown: string;
      images: ImageRef[];
      ts: number | null;
      command: CommandKind | null;
    }
  | {
      kind: "assistantText";
      index: number;
      markdown: string;
      images: ImageRef[];
      ts: number | null;
      usage: TokenUsage | null;
      model: string | null;
      command: CommandKind | null;
    }
  | { kind: "thinking"; index: number; text: string; ts: number | null }
  | {
      kind: "toolStep";
      index: number;
      /** Full source message — tool rendering needs name/input/metadata/content. */
      message: Message;
      ts: number | null;
    }
  | {
      kind: "systemMarker";
      index: number;
      /** Known `[tag]` subtype, or "plain" for untagged system text. */
      subtype: string;
      detail: string;
      ts: number | null;
    }
  | { kind: "unknown"; index: number; raw: Message };

/** Messages deliberately not rendered, counted so nothing vanishes silently. */
export interface NormalizeSkips {
  /** No renderable content (e.g. usage-only assistant placeholders). */
  empty: number;
  /** Injected prompt-template noise (system-reminder blocks etc.). */
  template: number;
  /** Orphaned tool results with an unusable generated name and no metadata. */
  orphanTool: number;
}

export interface NormalizeResult {
  items: TimelineItem[];
  skipped: NormalizeSkips;
}

export type ActivityItem = Extract<
  TimelineItem,
  { kind: "thinking" } | { kind: "toolStep" }
>;

export type TimelineRow =
  | { kind: "user"; item: Extract<TimelineItem, { kind: "user" }> }
  | {
      kind: "assistant";
      item: Extract<TimelineItem, { kind: "assistantText" }>;
    }
  | { kind: "marker"; item: Extract<TimelineItem, { kind: "systemMarker" }> }
  | { kind: "unknown"; item: Extract<TimelineItem, { kind: "unknown" }> }
  | {
      kind: "activity";
      /** Absolute index of the first item — stable row identity. */
      firstIndex: number;
      items: ActivityItem[];
      /** Timestamp of the user message that started the turn; null when the
       * provider has no per-message timestamps. */
      startTs: number | null;
      endTs: number | null;
      /** True only for the trailing group of a live-watched session. */
      running: boolean;
    };

export function rowKey(row: TimelineRow): string {
  return row.kind === "activity"
    ? `activity-${row.firstIndex}`
    : `${row.kind}-${row.item.index}`;
}
