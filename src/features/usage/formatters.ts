import { fmtTokens as fmtTokensShared, shortenHomePath } from "@/lib/formatters";
import type { ChartMetric, UsageSortState } from "@/lib/usage";

/** Chip-width provider label: the first word of the snapshot label
 * ("Claude Code" → "Claude", "Cursor CLI" → "Cursor"). Derived instead of
 * hand-listed, so new providers are covered automatically — the previous
 * lookup table had already drifted (no "pi" entry). */
export function shortProviderLabel(fullLabel: string): string {
  return fullLabel.split(" ")[0];
}

/** Compact token formatter, shared with the rest of the app. */
export const fmtTokens = fmtTokensShared;

export function fmtCost(n: number): string {
  return `$${n.toFixed(2)}`;
}

export function fmtPct(n: number): string {
  return `${(n * 100).toFixed(0)}%`;
}

export function makeFmtChartValue(metric: () => ChartMetric): (n: number) => string {
  return (n) => (metric() === "cost" ? fmtCost(n) : fmtTokens(n));
}

export function fmtTrend(pct: number | null): string {
  if (pct === null) return "";
  const abs = Math.abs(pct * 100);
  const arrow = pct > 0 ? "\u2191" : pct < 0 ? "\u2193" : "";
  return `${arrow} ${abs.toFixed(0)}%`;
}

export function trendClass(pct: number | null, invertColor: boolean = false): string {
  if (pct === null) return "";
  if (pct > 0) return invertColor ? "usage-trend-down" : "usage-trend-up";
  if (pct < 0) return invertColor ? "usage-trend-up" : "usage-trend-down";
  return "";
}

export function fmtActive(ts: number): string {
  const now = Date.now() / 1000;
  const diff = now - ts;
  if (diff < 60) return "<1m";
  if (diff < 3600) return `${Math.floor(diff / 60)}m`;
  if (diff < 86400) return `${Math.floor(diff / 3600)}h`;
  return `${Math.floor(diff / 86400)}d`;
}

export function formatProjectPath(projectPath: string, fallback: string): string {
  return shortenHomePath(projectPath || fallback);
}

export function sortIcon(currentSort: UsageSortState, col: string): "↕" | "↑" | "↓" {
  if (currentSort.col !== col) return "↕";
  return currentSort.asc ? "↑" : "↓";
}
