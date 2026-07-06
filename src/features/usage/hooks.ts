import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { listenBackendEvent, type UnlistenFn } from "@/lib/backend-events";
import { useI18n } from "@/i18n/index";
import {
  getActivityCalendar,
  getIndexStats,
  getPricingCatalogStatus,
  getSessionCount,
  getUsageStats,
} from "@/lib/tauri";
import {
  getProviderSnapshotVersion,
  listProviderSnapshots,
  refreshProviderSnapshots,
  useProviderSnapshotVersion,
} from "@/stores/providerSnapshots";
import {
  useRangeDays,
  useCustomRange,
  useSelectedProviders,
  setSelectedProviders,
  useDidInitProviders,
  setDidInitProviders,
  useProviderSelectionTouched,
  setProviderSelectionTouched,
  useProjectLimit,
  useSessionLimit,
  useChartMetric,
  useCalendarMetric,
  useCalendarYear,
  useModelSort,
  useProjectSort,
  useSessionSort,
  type CustomDateRange,
} from "@/features/usage/usageView";
import {
  buildDailyChartData,
  compareUsageValues,
  filterScannedProviderSnapshots,
  makeEmptyUsageStats,
  totalUsageTokens,
  trendPercent,
  type UsageSortState,
} from "@/lib/usage";
import {
  buildHeatmapGrid,
  dateRangeForYear,
  type HeatmapGrid,
} from "@/features/usage/heatmap";
import { formatLocalDateTime, toLocalISODate } from "@/lib/formatters";
import { errorMessage } from "@/lib/errors";
import type {
  ActivityCalendar,
  IndexStats,
  MaintenanceJob,
  ModelCost,
  PricingCatalogStatus,
  ProjectCost,
  SessionCostRow,
  UsageStats,
} from "@/lib/types";
import {
  SHORT_PROVIDER_LABELS,
  fmtTokens,
  fmtPct,
  formatProjectPath as formatProjectPathRaw,
} from "@/features/usage/formatters";
import type { ProviderChipInfo } from "@/features/usage/Toolbar";

// --- Async resource helper (mirrors Solid's createResource) -------------------

interface ResourceState<T> {
  data: T | undefined;
  loading: boolean;
  error: unknown;
}

interface ResourceActions {
  refetch: () => Promise<void>;
}

/**
 * Fetches `fetcher(source)` whenever `source` changes to a non-null value,
 * mirroring Solid's `createResource(source, fetcher)`: a null source skips the
 * fetch and retains the last value. `refetch()` re-runs against the current
 * source and resolves when the fetch settles. Callers memoize `source` so its
 * identity is the refetch trigger, exactly like the tracked Solid signal.
 */
function useResource<S, T>(
  source: S | null,
  fetcher: (source: S) => Promise<T>,
): [ResourceState<T>, ResourceActions] {
  const [state, setState] = useState<ResourceState<T>>(() => ({
    data: undefined,
    loading: source !== null,
    error: undefined,
  }));

  const fetcherRef = useRef(fetcher);
  fetcherRef.current = fetcher;
  const sourceRef = useRef(source);
  sourceRef.current = source;

  useEffect(() => {
    if (source === null) return;
    let cancelled = false;
    setState((prev) => ({ ...prev, loading: true, error: undefined }));
    void fetcherRef.current(source).then(
      (data) => {
        if (!cancelled) setState({ data, loading: false, error: undefined });
      },
      (error: unknown) => {
        if (!cancelled) {
          setState((prev) => ({ ...prev, loading: false, error }));
        }
      },
    );
    return () => {
      cancelled = true;
    };
  }, [source]);

  const refetch = useCallback(async () => {
    const currentSource = sourceRef.current;
    if (currentSource === null) return;
    setState((prev) => ({ ...prev, loading: true, error: undefined }));
    try {
      const data = await fetcherRef.current(currentSource);
      setState({ data, loading: false, error: undefined });
    } catch (error) {
      setState((prev) => ({ ...prev, loading: false, error }));
    }
  }, []);

  return [state, { refetch }];
}

// --- Provider selection ------------------------------------------------------

export function useProviderSelection() {
  // Re-render (and re-read the imperative getters below, which read a zustand
  // store) whenever provider snapshots finish loading.
  const snapshotVersion = useProviderSnapshotVersion();
  const selectedProviders = useSelectedProviders();
  const providerSelectionTouched = useProviderSelectionTouched();

  const providerSnapshots = useMemo(
    () => listProviderSnapshots(),
    // Re-read whenever the snapshot version changes.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [snapshotVersion],
  );
  const scannedProviderSnapshots = useMemo(
    () => filterScannedProviderSnapshots(providerSnapshots),
    [providerSnapshots],
  );
  const scannedProviderKeys = useMemo(
    () => scannedProviderSnapshots.map((snapshot) => snapshot.key),
    [scannedProviderSnapshots],
  );
  const providerSnapshotMap = useMemo(
    () =>
      new Map(providerSnapshots.map((snapshot) => [snapshot.key, snapshot])),
    [providerSnapshots],
  );

  useEffect(() => {
    const keys = scannedProviderKeys;
    const snapshotsLoaded = getProviderSnapshotVersion() > 0;
    if (!snapshotsLoaded && keys.length === 0) return;
    if (!providerSelectionTouched) {
      setSelectedProviders(new Set(keys));
    }
    setDidInitProviders(true);
  }, [scannedProviderKeys, providerSelectionTouched]);

  const selectedProviderKeys = useMemo(
    () => scannedProviderKeys.filter((key) => selectedProviders.has(key)),
    [scannedProviderKeys, selectedProviders],
  );
  const allProvidersSelected = useMemo(
    () =>
      scannedProviderKeys.length > 0 &&
      selectedProviderKeys.length === scannedProviderKeys.length,
    [scannedProviderKeys, selectedProviderKeys],
  );

  const toggleProvider = (key: string) => {
    setProviderSelectionTouched(true);
    const next = new Set(selectedProviders);
    if (next.has(key)) next.delete(key);
    else next.add(key);
    setSelectedProviders(next);
  };

  const selectAllProviders = () => {
    setProviderSelectionTouched(true);
    if (allProvidersSelected) {
      setSelectedProviders(new Set<string>());
      return;
    }
    setSelectedProviders(new Set<string>(scannedProviderKeys));
  };

  const providerInfo = (key: string): ProviderChipInfo => {
    const snapshot = providerSnapshotMap.get(key as never);
    return {
      color: snapshot?.color ?? `var(--${key})`,
      label: SHORT_PROVIDER_LABELS[key] ?? snapshot?.label ?? key,
      fullLabel: snapshot?.label ?? key,
    };
  };

  return {
    scannedProviderSnapshots,
    scannedProviderKeys,
    selectedProviderKeys,
    allProvidersSelected,
    toggleProvider,
    selectAllProviders,
    providerInfo,
  };
}

// --- Backend resources & refresh wiring --------------------------------------

interface StatsQuery {
  providers: string[];
  range: number | null;
  custom: CustomDateRange | null;
}

interface CalendarQuery {
  providers: string[];
  window: { start: string; end: string };
}

export function useUsageResources(selectedProviderKeys: string[]) {
  const didInitProviders = useDidInitProviders();
  const rangeDays = useRangeDays();
  const customRange = useCustomRange();
  const calendarMetric = useCalendarMetric();
  const calendarYear = useCalendarYear();

  const [activeMaintenanceJob, setActiveMaintenanceJob] =
    useState<MaintenanceJob | null>(null);

  const statsSource = useMemo<StatsQuery | null>(
    () =>
      didInitProviders
        ? {
            providers: selectedProviderKeys,
            range: rangeDays,
            custom: customRange,
          }
        : null,
    [didInitProviders, selectedProviderKeys, rangeDays, customRange],
  );
  const [stats, { refetch: refetchStats }] = useResource(
    statsSource,
    async (params: StatsQuery): Promise<UsageStats> => {
      if (params.providers.length === 0) {
        return makeEmptyUsageStats();
      }
      // A custom date window overrides the preset day count.
      if (params.custom) {
        return getUsageStats(
          params.providers,
          null,
          params.custom.start,
          params.custom.end,
        );
      }
      return getUsageStats(params.providers, params.range);
    },
  );

  // Activity calendar: its own data window (a year), independent of the range
  // filter that drives the stats above. The window is derived once here so the
  // fetched range and the laid-out grid can never disagree (e.g. across a
  // midnight tick if today were read twice). Refetches on provider/window change.
  const calendarWindow = useMemo(
    () => dateRangeForYear(calendarYear, toLocalISODate()),
    [calendarYear],
  );
  const calendarSource = useMemo<CalendarQuery | null>(
    () =>
      didInitProviders
        ? { providers: selectedProviderKeys, window: calendarWindow }
        : null,
    [didInitProviders, selectedProviderKeys, calendarWindow],
  );
  const [calendar, { refetch: refetchCalendar }] = useResource(
    calendarSource,
    async (params: CalendarQuery): Promise<ActivityCalendar> => {
      if (params.providers.length === 0) {
        return { days: [], available_years: [] };
      }
      return getActivityCalendar(
        params.providers,
        params.window.start,
        params.window.end,
      );
    },
  );

  useEffect(() => {
    if (calendar.error) {
      console.error("failed to load activity calendar", calendar.error);
    }
  }, [calendar.error]);

  const availableYears = useMemo(
    () => calendar.data?.available_years ?? [],
    [calendar.data],
  );
  const heatmapGrid = useMemo<HeatmapGrid>(() => {
    const { start, end } = calendarWindow;
    return buildHeatmapGrid(
      calendar.data?.days ?? [],
      calendarMetric,
      start,
      end,
    );
  }, [calendarWindow, calendar.data, calendarMetric]);

  const [sessionCount, { refetch: refetchSessionCount }] = useResource(
    true,
    () => getSessionCount(),
  );
  const [indexStats, { refetch: refetchIndexStats }] = useResource(true, () =>
    getIndexStats(),
  );
  const [pricingStatus, { refetch: refetchPricingStatus }] = useResource(
    true,
    (): Promise<PricingCatalogStatus> => getPricingCatalogStatus(),
  );

  // Latest-value refs so the mount-time listeners below read current state
  // instead of the values captured when they were registered.
  const indexStatsRef = useRef(indexStats);
  indexStatsRef.current = indexStats;
  const activeMaintenanceJobRef = useRef(activeMaintenanceJob);
  activeMaintenanceJobRef.current = activeMaintenanceJob;

  useEffect(() => {
    const handleUsageDataChanged = () => {
      void refreshProviderSnapshots();
      void refetchStats();
      void refetchCalendar();
      void refetchSessionCount();
      void refetchPricingStatus();
      void refetchIndexStats();
    };

    const handleUsageDataChangedIfStale = () => {
      if (document.visibilityState === "hidden") return;
      const usageRefreshedAt =
        indexStatsRef.current.data?.usage_last_refreshed_at;
      if (!usageRefreshedAt) return;
      const parsed = Date.parse(usageRefreshedAt);
      if (!Number.isNaN(parsed) && Date.now() - parsed < 5 * 60 * 1000) return;
      handleUsageDataChanged();
    };

    window.addEventListener("usage-data-changed", handleUsageDataChanged);
    window.addEventListener("focus", handleUsageDataChangedIfStale);
    document.addEventListener(
      "visibilitychange",
      handleUsageDataChangedIfStale,
    );

    let disposed = false;
    let unlistenMaintenance: UnlistenFn | undefined;
    void listenBackendEvent("maintenance-status", (payload) => {
      if (payload.phase === "started") {
        setActiveMaintenanceJob(payload.job);
        return;
      }
      if (
        activeMaintenanceJobRef.current === payload.job &&
        (payload.phase === "finished" || payload.phase === "failed")
      ) {
        setActiveMaintenanceJob(null);
      }
    }).then((unlisten) => {
      // The component may unmount before listen() resolves; drop the stale
      // subscription immediately instead of leaking it.
      if (disposed) unlisten();
      else unlistenMaintenance = unlisten;
    });

    return () => {
      disposed = true;
      window.removeEventListener("usage-data-changed", handleUsageDataChanged);
      window.removeEventListener("focus", handleUsageDataChangedIfStale);
      document.removeEventListener(
        "visibilitychange",
        handleUsageDataChangedIfStale,
      );
      unlistenMaintenance?.();
    };
  }, [
    refetchStats,
    refetchCalendar,
    refetchSessionCount,
    refetchPricingStatus,
    refetchIndexStats,
  ]);

  return {
    stats,
    calendar,
    heatmapGrid,
    availableYears,
    sessionCount,
    indexStats,
    pricingStatus,
    refetchPricingStatus,
    activeMaintenanceJob,
  };
}

// --- Derived display data -----------------------------------------------------

export interface UsageDerivedDeps {
  stats: ResourceState<UsageStats>;
  sessionCount: ResourceState<number>;
  indexStats: ResourceState<IndexStats>;
  pricingStatus: ResourceState<PricingCatalogStatus>;
  activeMaintenanceJob: MaintenanceJob | null;
  selectedProviderKeys: string[];
  scannedProviderKeys: string[];
  allProvidersSelected: boolean;
}

export function useUsageDerived(deps: UsageDerivedDeps) {
  const { t } = useI18n();
  const {
    stats,
    sessionCount,
    indexStats,
    pricingStatus,
    activeMaintenanceJob,
    selectedProviderKeys,
    scannedProviderKeys,
    allProvidersSelected,
  } = deps;

  const modelSort = useModelSort();
  const projectSort = useProjectSort();
  const sessionSort = useSessionSort();
  const projectLimit = useProjectLimit();
  const sessionLimit = useSessionLimit();
  const chartMetric = useChartMetric();
  const customRange = useCustomRange();
  const rangeDays = useRangeDays();

  // The zustand sort setters take a plain value (not the Solid signal's
  // functional-update form), so the current sort is threaded in to compute the
  // toggle. Behavior is unchanged.
  const makeSortHandler =
    (current: UsageSortState, setter: (next: UsageSortState) => void) =>
    (col: string) => {
      setter({ col, asc: current.col === col ? !current.asc : false });
    };

  const sortedModels = useMemo(() => {
    const data = stats.data?.model_costs ?? [];
    const { col, asc } = modelSort;
    return [...data].sort((a, b) =>
      compareUsageValues(
        a[col as keyof ModelCost],
        b[col as keyof ModelCost],
        asc,
      ),
    );
  }, [stats.data, modelSort]);

  const sortedProjects = useMemo(() => {
    const data = stats.data?.project_costs ?? [];
    const { col, asc } = projectSort;
    return [...data].sort((a, b) =>
      compareUsageValues(
        a[col as keyof ProjectCost],
        b[col as keyof ProjectCost],
        asc,
      ),
    );
  }, [stats.data, projectSort]);

  const sortedSessions = useMemo(() => {
    const data = stats.data?.recent_sessions ?? [];
    const { col, asc } = sessionSort;
    return [...data].sort((a, b) =>
      compareUsageValues(
        a[col as keyof SessionCostRow],
        b[col as keyof SessionCostRow],
        asc,
      ),
    );
  }, [stats.data, sessionSort]);

  const visibleProjects = useMemo(
    () => sortedProjects.slice(0, projectLimit),
    [sortedProjects, projectLimit],
  );
  const visibleSessions = useMemo(
    () => sortedSessions.slice(0, sessionLimit),
    [sortedSessions, sessionLimit],
  );

  const formatModelName = (model: string): string =>
    model.trim().length > 0 ? model : t("common.unknown");
  const formatProjectName = (project: string, projectPath: string): string => {
    if (project.trim().length > 0) return project;
    const name = projectPath.split(/[\\/]/).filter(Boolean).at(-1);
    return name || t("common.unknown");
  };
  const formatProjectPath = (projectPath: string): string =>
    formatProjectPathRaw(projectPath, t("common.unknown"));

  const totalTokens = useMemo(() => totalUsageTokens(stats.data), [stats.data]);

  const dailyChartData = useMemo(
    () =>
      buildDailyChartData(
        stats.data?.daily_usage ?? [],
        selectedProviderKeys,
        chartMetric,
      ),
    [stats.data, selectedProviderKeys, chartMetric],
  );

  const topModels = useMemo(() => sortedModels.slice(0, 4), [sortedModels]);
  const maxTopModelCost = useMemo(() => topModels[0]?.cost ?? 0, [topModels]);

  const activeRangeLabel = ((): string => {
    if (customRange) return `${customRange.start} ~ ${customRange.end}`;
    switch (rangeDays) {
      case 1:
        return t("usage.rangeToday");
      case 7:
        return t("usage.range7d");
      case 30:
        return t("usage.range30d");
      case 90:
        return t("usage.range90d");
      default:
        return t("usage.rangeAll");
    }
  })();

  const showRebuildHint = ((): boolean => {
    const data = stats.data;
    if (!data || data.total_turns > 0) return false;
    if (selectedProviderKeys.length === 0) return false;
    return (
      rangeDays === null &&
      customRange === null &&
      allProvidersSelected &&
      (sessionCount.data ?? 0) > 0
    );
  })();

  const emptyMessage = ((): string => {
    if (scannedProviderKeys.length === 0) return t("usage.noData");
    if (selectedProviderKeys.length === 0) return t("usage.selectProvider");
    if (showRebuildHint) return t("usage.rebuildHint");
    if ((sessionCount.data ?? 0) === 0) return t("usage.noData");
    return t("usage.noResults");
  })();

  const formattedPricingUpdatedAt = ((): string => {
    if (pricingStatus.error) return t("error.message");
    const updatedAt = pricingStatus.data?.updated_at;
    return updatedAt
      ? formatLocalDateTime(updatedAt)
      : t("settings.pricingNotFetched");
  })();

  const formattedUsageUpdatedAt = ((): string => {
    if (indexStats.error) return t("error.message");
    const updatedAt = indexStats.data?.usage_last_refreshed_at;
    return updatedAt ? formatLocalDateTime(updatedAt) : t("usage.notRefreshed");
  })();

  const pricingStatusError = pricingStatus.error
    ? errorMessage(pricingStatus.error)
    : null;

  const indexStatsError = indexStats.error
    ? errorMessage(indexStats.error)
    : null;

  const pricingModelCountLabel = ((): string => {
    if (pricingStatus.error) return t("error.message");
    if (pricingStatus.loading && !pricingStatus.data)
      return t("common.loading");
    return String(pricingStatus.data?.model_count ?? 0);
  })();

  const maintenanceStatusText = ((): string => {
    if (activeMaintenanceJob === "refresh_usage")
      return t("usage.refreshUsageRunning");
    if (activeMaintenanceJob === "rebuild_index")
      return t("usage.rebuildRunning");
    return t("usage.usageReady");
  })();

  const totalCostTrend = trendPercent(
    stats.data?.total_cost ?? 0,
    stats.data?.prev_period,
    "total_cost",
  );

  const summaryStats = [
    {
      label: t("usage.sessions"),
      value: (stats.data?.total_sessions ?? 0).toLocaleString(),
      trend: trendPercent(
        stats.data?.total_sessions ?? 0,
        stats.data?.prev_period,
        "total_sessions",
      ),
    },
    {
      label: t("usage.turns"),
      value: (stats.data?.total_turns ?? 0).toLocaleString(),
      trend: trendPercent(
        stats.data?.total_turns ?? 0,
        stats.data?.prev_period,
        "total_turns",
      ),
    },
    {
      label: t("usage.tokens"),
      value: fmtTokens(totalTokens),
      trend: trendPercent(totalTokens, stats.data?.prev_period, "total_tokens"),
    },
  ];

  const tokenBreakdown = [
    {
      label: t("usage.input"),
      value: fmtTokens(stats.data?.total_input_tokens ?? 0),
      share:
        totalTokens > 0
          ? fmtPct((stats.data?.total_input_tokens ?? 0) / totalTokens)
          : "0%",
    },
    {
      label: t("usage.output"),
      value: fmtTokens(stats.data?.total_output_tokens ?? 0),
      share:
        totalTokens > 0
          ? fmtPct((stats.data?.total_output_tokens ?? 0) / totalTokens)
          : "0%",
    },
    {
      label: t("usage.cacheRead"),
      value: fmtTokens(stats.data?.total_cache_read_tokens ?? 0),
      share:
        totalTokens > 0
          ? fmtPct((stats.data?.total_cache_read_tokens ?? 0) / totalTokens)
          : "0%",
    },
    {
      label: t("usage.cacheWrite"),
      value: fmtTokens(stats.data?.total_cache_write_tokens ?? 0),
      share:
        totalTokens > 0
          ? fmtPct((stats.data?.total_cache_write_tokens ?? 0) / totalTokens)
          : "0%",
    },
  ];

  return {
    makeSortHandler,
    sortedModels,
    sortedProjects,
    sortedSessions,
    visibleProjects,
    visibleSessions,
    formatModelName,
    formatProjectName,
    formatProjectPath,
    dailyChartData,
    topModels,
    maxTopModelCost,
    activeRangeLabel,
    emptyMessage,
    formattedPricingUpdatedAt,
    formattedUsageUpdatedAt,
    pricingStatusError,
    indexStatsError,
    pricingModelCountLabel,
    maintenanceStatusText,
    totalCostTrend,
    summaryStats,
    tokenBreakdown,
  };
}
