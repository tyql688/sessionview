import type { ActivityDay } from "./types";

/** Which per-day metric drives the calendar's count + cell intensity. */
export type HeatmapMetric = "sessions" | "turns" | "tokens" | "cost";

/** Intensity bucket: 0 = no activity, 1 (lightest) … 4 (darkest). */
export type HeatmapLevel = 0 | 1 | 2 | 3 | 4;

export interface HeatmapCell {
  /** ISO `YYYY-MM-DD`. Present even for padding cells (kept for keying). */
  date: string;
  /** Whether the date falls inside the requested [start, end] window. */
  inRange: boolean;
  /** The day's metric value (0 when no data or out of range). */
  value: number;
  /** Full day record when there is activity, else null. */
  day: ActivityDay | null;
  level: HeatmapLevel;
}

export interface HeatmapWeek {
  /** Exactly 7 cells, index 0 = Sunday … 6 = Saturday. */
  cells: HeatmapCell[];
}

export interface HeatmapMonthLabel {
  /** Index into `weeks` of the column where this month first appears. */
  weekIndex: number;
  /** Month number 1–12 (the caller localizes the label text). */
  month: number;
}

export interface HeatmapGrid {
  weeks: HeatmapWeek[];
  monthLabels: HeatmapMonthLabel[];
  /** Sum of the metric across every in-range day. */
  total: number;
  /** Count of in-range days with non-zero activity. */
  activeDays: number;
  maxValue: number;
}

// --- ISO date helpers (UTC-based to avoid DST / timezone drift) -------------

function isoToUTC(iso: string): Date {
  const [y, m, d] = iso.split("-").map(Number);
  return new Date(Date.UTC(y, m - 1, d));
}

function utcToISO(date: Date): string {
  const y = date.getUTCFullYear();
  const m = String(date.getUTCMonth() + 1).padStart(2, "0");
  const d = String(date.getUTCDate()).padStart(2, "0");
  return `${y}-${m}-${d}`;
}

/** Add (or subtract) whole days to an ISO date. */
export function addDays(iso: string, days: number): string {
  const date = isoToUTC(iso);
  date.setUTCDate(date.getUTCDate() + days);
  return utcToISO(date);
}

/** Day of week for an ISO date: 0 = Sunday … 6 = Saturday. */
export function weekday(iso: string): number {
  return isoToUTC(iso).getUTCDay();
}

/**
 * Inclusive [start, end] window for the calendar.
 *  - `year === null` → trailing ~52 weeks of data ending today (GitHub's
 *    default view). After the grid snaps to whole Sun→Sat weeks this renders
 *    as 52 or 53 columns depending on today's weekday.
 *  - `year` given   → that whole calendar year, clamped to today for the
 *    current year so we never render empty future cells. A future year would
 *    yield start > end (empty grid); callers feed only years that have data.
 */
export function dateRangeForYear(
  year: number | null,
  today: string,
): { start: string; end: string } {
  if (year === null) {
    return { start: addDays(today, -363), end: today };
  }
  const currentYear = Number(today.slice(0, 4));
  const start = `${year}-01-01`;
  const end = year >= currentYear ? today : `${year}-12-31`;
  return { start, end };
}

function metricValue(day: ActivityDay, metric: HeatmapMetric): number {
  switch (metric) {
    case "sessions":
      return day.sessions;
    case "turns":
      return day.turns;
    case "tokens":
      return day.tokens;
    case "cost":
      return day.cost;
  }
}

/** Quartile thresholds [q1, q2, q3] over the non-zero values. */
function quartiles(values: number[]): [number, number, number] {
  if (values.length === 0) return [0, 0, 0];
  const sorted = [...values].sort((a, b) => a - b);
  const at = (p: number) =>
    sorted[Math.min(sorted.length - 1, Math.floor(p * sorted.length))];
  return [at(0.25), at(0.5), at(0.75)];
}

function levelForValue(
  value: number,
  [q1, q2, q3]: [number, number, number],
): HeatmapLevel {
  if (value <= 0) return 0;
  if (value <= q1) return 1;
  if (value <= q2) return 2;
  if (value <= q3) return 3;
  return 4;
}

/**
 * Lay out per-day activity as GitHub-style week columns. Weeks start on Sunday;
 * the grid pads to whole weeks (leading/trailing cells outside [start, end] are
 * marked `inRange: false`). Intensity is bucketed by quartiles of the non-zero
 * in-range values so any metric scale (sessions … tokens) reads sensibly.
 */
export function buildHeatmapGrid(
  days: ActivityDay[],
  metric: HeatmapMetric,
  start: string,
  end: string,
): HeatmapGrid {
  const byDate = new Map(days.map((day) => [day.date, day]));

  // Snap the grid to whole weeks: back to the Sunday on/before start, forward
  // to the Saturday on/after end.
  const gridStart = addDays(start, -weekday(start));
  const gridEnd = addDays(end, 6 - weekday(end));

  // Pass 1: materialize cells and collect non-zero in-range values for bucketing.
  interface RawCell {
    date: string;
    inRange: boolean;
    value: number;
    day: ActivityDay | null;
  }
  const raw: RawCell[] = [];
  const nonZero: number[] = [];
  let total = 0;
  let activeDays = 0;
  let maxValue = 0;

  for (let cursor = gridStart; cursor <= gridEnd; cursor = addDays(cursor, 1)) {
    const inRange = cursor >= start && cursor <= end;
    const day = byDate.get(cursor) ?? null;
    const value = inRange && day ? metricValue(day, metric) : 0;
    if (inRange && value > 0) {
      nonZero.push(value);
      total += value;
      activeDays += 1;
      if (value > maxValue) maxValue = value;
    }
    raw.push({ date: cursor, inRange, value, day });
  }

  const thresholds = quartiles(nonZero);

  // Pass 2: assign levels and chunk into weeks of 7 (Sunday-first).
  const weeks: HeatmapWeek[] = [];
  for (let i = 0; i < raw.length; i += 7) {
    const cells: HeatmapCell[] = raw.slice(i, i + 7).map((cell) => ({
      date: cell.date,
      inRange: cell.inRange,
      value: cell.value,
      day: cell.day,
      level: cell.inRange ? levelForValue(cell.value, thresholds) : 0,
    }));
    weeks.push({ cells });
  }

  // One label per month, at the column where that month's first in-range day sits.
  const monthLabels: HeatmapMonthLabel[] = [];
  let lastMonth = -1;
  weeks.forEach((week, weekIndex) => {
    const firstInRange = week.cells.find((cell) => cell.inRange);
    if (!firstInRange) return;
    const month = Number(firstInRange.date.slice(5, 7));
    if (month !== lastMonth) {
      monthLabels.push({ weekIndex, month });
      lastMonth = month;
    }
  });

  return { weeks, monthLabels, total, activeDays, maxValue };
}
