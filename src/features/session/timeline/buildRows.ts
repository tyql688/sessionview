import type { ActivityItem, TimelineItem, TimelineRow } from "./types";

export interface BuildRowsOptions {
  /** True while live-watch is active — marks the trailing activity group as
   * running so it renders a live timer instead of a final duration. */
  live: boolean;
}

/**
 * TimelineItem[] → TimelineRow[]: thinking and tool steps between a user
 * message and the next assistant reply collapse into one activity group;
 * everything else renders as its own row. Group timestamps stay null when the
 * provider records none — the UI hides the duration instead of inventing one.
 */
export function buildRows(
  items: TimelineItem[],
  opts: BuildRowsOptions,
): TimelineRow[] {
  const rows: TimelineRow[] = [];
  let activity: Extract<TimelineRow, { kind: "activity" }> | null = null;
  let lastUserTs: number | null = null;

  const closeActivity = () => {
    activity = null;
  };
  const appendToActivity = (item: ActivityItem) => {
    if (!activity) {
      activity = {
        kind: "activity",
        firstIndex: item.index,
        items: [],
        startTs: lastUserTs,
        endTs: null,
        running: false,
      };
      rows.push(activity);
    }
    activity.items.push(item);
    if (item.ts !== null) activity.endTs = item.ts;
  };

  for (const item of items) {
    switch (item.kind) {
      case "user":
        closeActivity();
        lastUserTs = item.ts;
        rows.push({ kind: "user", item });
        break;
      case "thinking":
      case "toolStep":
        appendToActivity(item);
        break;
      case "assistantText":
        closeActivity();
        rows.push({ kind: "assistant", item });
        break;
      case "systemMarker":
        closeActivity();
        rows.push({ kind: "marker", item });
        break;
      case "unknown":
        closeActivity();
        rows.push({ kind: "unknown", item });
        break;
    }
  }

  const last = rows[rows.length - 1];
  if (opts.live && last?.kind === "activity") {
    last.running = true;
  }
  return rows;
}
