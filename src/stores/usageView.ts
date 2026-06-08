import { createSignal } from "solid-js";
import type { ChartMetric, UsageSortState } from "../lib/usage";
import type { HeatmapMetric } from "../lib/heatmap";
import type { LimitOption } from "../components/UsagePanel/ProjectTable";

// Persistent Usage-panel UI state. UsagePanel is mounted under a `<Show>` in
// App, so it is destroyed and recreated on every view switch. Component-local
// signals would reset to defaults each time the user navigates away and back.
// Hoisting the persistent input signals to module scope keeps the user's
// range, provider selection, chart metric, table sorts, and row limits intact
// across remounts. Ephemeral per-visit state (hover, dialogs, in-flight flags)
// stays local in the component.

export interface CustomDateRange {
  start: string; // YYYY-MM-DD, inclusive
  end: string; // YYYY-MM-DD, inclusive
}

const [rangeDays, setRangeDays] = createSignal<number | null>(7);
// Non-null overrides rangeDays with an explicit [start, end] date window.
const [customRange, setCustomRange] = createSignal<CustomDateRange | null>(
  null,
);
const [selectedProviders, setSelectedProviders] = createSignal<Set<string>>(
  new Set(),
);
const [didInitProviders, setDidInitProviders] = createSignal(false);
const [providerSelectionTouched, setProviderSelectionTouched] =
  createSignal(false);
const [projectLimit, setProjectLimit] = createSignal<LimitOption>(10);
const [sessionLimit, setSessionLimit] = createSignal<LimitOption>(10);
const [chartMetric, setChartMetric] = createSignal<ChartMetric>("tokens");
// Activity-calendar coloring metric and selected year (null = trailing 52 weeks).
const [calendarMetric, setCalendarMetric] =
  createSignal<HeatmapMetric>("tokens");
const [calendarYear, setCalendarYear] = createSignal<number | null>(null);
const [modelSort, setModelSort] = createSignal<UsageSortState>({
  col: "cost",
  asc: false,
});
const [projectSort, setProjectSort] = createSignal<UsageSortState>({
  col: "cost",
  asc: false,
});
const [sessionSort, setSessionSort] = createSignal<UsageSortState>({
  col: "updated_at",
  asc: false,
});

export type { ChartMetric, UsageSortState } from "../lib/usage";
export type { HeatmapMetric } from "../lib/heatmap";
export type { LimitOption } from "../components/UsagePanel/ProjectTable";

export {
  rangeDays,
  setRangeDays,
  customRange,
  setCustomRange,
  selectedProviders,
  setSelectedProviders,
  didInitProviders,
  setDidInitProviders,
  providerSelectionTouched,
  setProviderSelectionTouched,
  projectLimit,
  setProjectLimit,
  sessionLimit,
  setSessionLimit,
  chartMetric,
  setChartMetric,
  calendarMetric,
  setCalendarMetric,
  calendarYear,
  setCalendarYear,
  modelSort,
  setModelSort,
  projectSort,
  setProjectSort,
  sessionSort,
  setSessionSort,
};
