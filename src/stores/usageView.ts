import { create } from "zustand";
import type { HeatmapMetric } from "../lib/heatmap";
import type { ChartMetric, UsageSortState } from "../lib/usage";

// Persistent Usage-panel UI state. UsagePanel is destroyed and recreated on
// every view switch, so hoisting the persistent inputs to a module-scope store
// keeps the user's range, provider selection, chart metric, table sorts, and
// row limits intact across remounts. Ephemeral per-visit state (hover, dialogs,
// in-flight flags) stays local in the component.

export interface CustomDateRange {
  start: string; // YYYY-MM-DD, inclusive
  end: string; // YYYY-MM-DD, inclusive
}

// Owned here (the store holds projectLimit/sessionLimit); the Usage tables
// import it from this store.
export type LimitOption = 10 | 25 | 50 | 100;

export type { ChartMetric, UsageSortState } from "../lib/usage";
export type { HeatmapMetric } from "../lib/heatmap";

interface UsageViewState {
  rangeDays: number | null;
  // Non-null overrides rangeDays with an explicit [start, end] date window.
  customRange: CustomDateRange | null;
  selectedProviders: Set<string>;
  didInitProviders: boolean;
  providerSelectionTouched: boolean;
  projectLimit: LimitOption;
  sessionLimit: LimitOption;
  chartMetric: ChartMetric;
  // Activity-calendar coloring metric and selected year (null = trailing 52wk).
  calendarMetric: HeatmapMetric;
  calendarYear: number | null;
  modelSort: UsageSortState;
  projectSort: UsageSortState;
  sessionSort: UsageSortState;
}

export const useUsageViewStore = create<UsageViewState>(() => ({
  rangeDays: 7,
  customRange: null,
  selectedProviders: new Set<string>(),
  didInitProviders: false,
  providerSelectionTouched: false,
  projectLimit: 10,
  sessionLimit: 10,
  chartMetric: "tokens",
  calendarMetric: "tokens",
  calendarYear: null,
  modelSort: { col: "cost", asc: false },
  projectSort: { col: "cost", asc: false },
  sessionSort: { col: "updated_at", asc: false },
}));

export const setRangeDays = (rangeDays: number | null) =>
  useUsageViewStore.setState({ rangeDays });
export const setCustomRange = (customRange: CustomDateRange | null) =>
  useUsageViewStore.setState({ customRange });
export const setSelectedProviders = (selectedProviders: Set<string>) =>
  useUsageViewStore.setState({ selectedProviders });
export const setDidInitProviders = (didInitProviders: boolean) =>
  useUsageViewStore.setState({ didInitProviders });
export const setProviderSelectionTouched = (
  providerSelectionTouched: boolean,
) => useUsageViewStore.setState({ providerSelectionTouched });
export const setProjectLimit = (projectLimit: LimitOption) =>
  useUsageViewStore.setState({ projectLimit });
export const setSessionLimit = (sessionLimit: LimitOption) =>
  useUsageViewStore.setState({ sessionLimit });
export const setChartMetric = (chartMetric: ChartMetric) =>
  useUsageViewStore.setState({ chartMetric });
export const setCalendarMetric = (calendarMetric: HeatmapMetric) =>
  useUsageViewStore.setState({ calendarMetric });
export const setCalendarYear = (calendarYear: number | null) =>
  useUsageViewStore.setState({ calendarYear });
export const setModelSort = (modelSort: UsageSortState) =>
  useUsageViewStore.setState({ modelSort });
export const setProjectSort = (projectSort: UsageSortState) =>
  useUsageViewStore.setState({ projectSort });
export const setSessionSort = (sessionSort: UsageSortState) =>
  useUsageViewStore.setState({ sessionSort });

export const useRangeDays = () => useUsageViewStore((s) => s.rangeDays);
export const useCustomRange = () => useUsageViewStore((s) => s.customRange);
export const useSelectedProviders = () =>
  useUsageViewStore((s) => s.selectedProviders);
export const useDidInitProviders = () =>
  useUsageViewStore((s) => s.didInitProviders);
export const useProviderSelectionTouched = () =>
  useUsageViewStore((s) => s.providerSelectionTouched);
export const useProjectLimit = () => useUsageViewStore((s) => s.projectLimit);
export const useSessionLimit = () => useUsageViewStore((s) => s.sessionLimit);
export const useChartMetric = () => useUsageViewStore((s) => s.chartMetric);
export const useCalendarMetric = () =>
  useUsageViewStore((s) => s.calendarMetric);
export const useCalendarYear = () => useUsageViewStore((s) => s.calendarYear);
export const useModelSort = () => useUsageViewStore((s) => s.modelSort);
export const useProjectSort = () => useUsageViewStore((s) => s.projectSort);
export const useSessionSort = () => useUsageViewStore((s) => s.sessionSort);
