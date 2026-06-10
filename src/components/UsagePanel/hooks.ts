import {
  createEffect,
  createMemo,
  createResource,
  createSignal,
  onCleanup,
  onMount,
  type Accessor,
  type Resource,
} from "solid-js";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { useI18n } from "../../i18n/index";
import {
  getActivityCalendar,
  getIndexStats,
  getPricingCatalogStatus,
  getSessionCount,
  getUsageStats,
} from "../../lib/tauri";
import {
  getProviderSnapshotVersion,
  listProviderSnapshots,
  refreshProviderSnapshots,
} from "../../stores/providerSnapshots";
import {
  rangeDays,
  customRange,
  selectedProviders,
  setSelectedProviders,
  didInitProviders,
  setDidInitProviders,
  providerSelectionTouched,
  setProviderSelectionTouched,
  projectLimit,
  sessionLimit,
  chartMetric,
  calendarMetric,
  calendarYear,
  modelSort,
  projectSort,
  sessionSort,
} from "../../stores/usageView";
import {
  buildDailyChartData,
  compareUsageValues,
  filterScannedProviderSnapshots,
  makeEmptyUsageStats,
  totalUsageTokens,
  trendPercent,
  type UsageSortState,
} from "../../lib/usage";
import {
  buildHeatmapGrid,
  dateRangeForYear,
  type HeatmapGrid,
} from "../../lib/heatmap";
import { formatLocalDateTime, toLocalISODate } from "../../lib/formatters";
import { errorMessage } from "../../lib/errors";
import type {
  ActivityCalendar,
  IndexStats,
  MaintenanceEvent,
  MaintenanceJob,
  ModelCost,
  PricingCatalogStatus,
  ProjectCost,
  ProviderSnapshot,
  SessionCostRow,
  UsageStats,
} from "../../lib/types";
import {
  SHORT_PROVIDER_LABELS,
  fmtTokens,
  fmtPct,
  formatProjectPath as formatProjectPathRaw,
} from "./formatters";
import type { ProviderChipInfo } from "./Toolbar";

// --- Provider selection ------------------------------------------------------

export function createProviderSelection() {
  const providerSnapshots = createMemo(() => listProviderSnapshots());
  const scannedProviderSnapshots = createMemo(() =>
    filterScannedProviderSnapshots(providerSnapshots()),
  );
  const scannedProviderKeys = createMemo(() =>
    scannedProviderSnapshots().map((snapshot) => snapshot.key),
  );
  const providerSnapshotMap = createMemo(
    () =>
      new Map(providerSnapshots().map((snapshot) => [snapshot.key, snapshot])),
  );

  createEffect(() => {
    const keys = scannedProviderKeys();
    const snapshotsLoaded = getProviderSnapshotVersion() > 0;
    if (!snapshotsLoaded && keys.length === 0) return;
    if (!providerSelectionTouched()) {
      setSelectedProviders(new Set(keys));
    }
    setDidInitProviders(true);
  });

  const selectedProviderKeys = createMemo(() => {
    const selected = selectedProviders();
    return scannedProviderKeys().filter((key) => selected.has(key));
  });
  const allProvidersSelected = createMemo(
    () =>
      scannedProviderKeys().length > 0 &&
      selectedProviderKeys().length === scannedProviderKeys().length,
  );

  const toggleProvider = (key: string) => {
    setProviderSelectionTouched(true);
    setSelectedProviders((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  };

  const selectAllProviders = () => {
    setProviderSelectionTouched(true);
    if (allProvidersSelected()) {
      setSelectedProviders(new Set<string>());
      return;
    }
    setSelectedProviders(new Set<string>(scannedProviderKeys()));
  };

  const providerInfo = (key: string): ProviderChipInfo => {
    const snapshot = providerSnapshotMap().get(key as never);
    return {
      color: snapshot?.color ?? `var(--${key})`,
      label: SHORT_PROVIDER_LABELS[key] ?? snapshot?.label ?? key,
      fullLabel: snapshot?.label ?? key,
    };
  };

  return {
    scannedProviderSnapshots: scannedProviderSnapshots as Accessor<
      ProviderSnapshot[]
    >,
    scannedProviderKeys,
    selectedProviderKeys,
    allProvidersSelected,
    toggleProvider,
    selectAllProviders,
    providerInfo,
  };
}

// --- Backend resources & refresh wiring --------------------------------------

export function createUsageResources(selectedProviderKeys: Accessor<string[]>) {
  const [activeMaintenanceJob, setActiveMaintenanceJob] =
    createSignal<MaintenanceJob | null>(null);

  const [stats, { refetch: refetchStats }] = createResource(
    () =>
      didInitProviders()
        ? {
            providers: selectedProviderKeys(),
            range: rangeDays(),
            custom: customRange(),
          }
        : null,
    async (params) => {
      if (!params || params.providers.length === 0) {
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
  const calendarWindow = createMemo(() =>
    dateRangeForYear(calendarYear(), toLocalISODate()),
  );
  const [calendar, { refetch: refetchCalendar }] = createResource(
    () =>
      didInitProviders()
        ? { providers: selectedProviderKeys(), window: calendarWindow() }
        : null,
    async (params): Promise<ActivityCalendar> => {
      if (!params || params.providers.length === 0) {
        return { days: [], available_years: [] };
      }
      return getActivityCalendar(
        params.providers,
        params.window.start,
        params.window.end,
      );
    },
  );

  createEffect(() => {
    if (calendar.error) {
      console.error("failed to load activity calendar", calendar.error);
    }
  });

  const availableYears = createMemo(() => calendar()?.available_years ?? []);
  const heatmapGrid = createMemo<HeatmapGrid>(() => {
    const { start, end } = calendarWindow();
    return buildHeatmapGrid(
      calendar()?.days ?? [],
      calendarMetric(),
      start,
      end,
    );
  });

  const [sessionCount, { refetch: refetchSessionCount }] = createResource(() =>
    getSessionCount(),
  );
  const [indexStats, { refetch: refetchIndexStats }] = createResource(() =>
    getIndexStats(),
  );
  const [pricingStatus, { refetch: refetchPricingStatus }] =
    createResource<PricingCatalogStatus>(() => getPricingCatalogStatus());

  let unlistenMaintenance: UnlistenFn | undefined;
  const handleUsageDataChanged = () => {
    void refreshProviderSnapshots();
    void refetchStats();
    void refetchCalendar();
    void refetchSessionCount();
    void refetchPricingStatus();
    void refetchIndexStats();
  };

  function handleUsageDataChangedIfStale() {
    if (document.visibilityState === "hidden") return;
    const usageRefreshedAt = indexStats()?.usage_last_refreshed_at;
    if (!usageRefreshedAt) return;
    const parsed = Date.parse(usageRefreshedAt);
    if (!Number.isNaN(parsed) && Date.now() - parsed < 5 * 60 * 1000) return;
    handleUsageDataChanged();
  }

  onMount(async () => {
    window.addEventListener("usage-data-changed", handleUsageDataChanged);
    window.addEventListener("focus", handleUsageDataChangedIfStale);
    document.addEventListener(
      "visibilitychange",
      handleUsageDataChangedIfStale,
    );
    unlistenMaintenance = await listen<MaintenanceEvent>(
      "maintenance-status",
      (event) => {
        const payload = event.payload;
        if (payload.phase === "started") {
          setActiveMaintenanceJob(payload.job);
          return;
        }
        if (
          activeMaintenanceJob() === payload.job &&
          (payload.phase === "finished" || payload.phase === "failed")
        ) {
          setActiveMaintenanceJob(null);
        }
      },
    );
  });

  onCleanup(() => {
    window.removeEventListener("usage-data-changed", handleUsageDataChanged);
    window.removeEventListener("focus", handleUsageDataChangedIfStale);
    document.removeEventListener(
      "visibilitychange",
      handleUsageDataChangedIfStale,
    );
    unlistenMaintenance?.();
  });

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
  stats: Resource<UsageStats>;
  sessionCount: Resource<number>;
  indexStats: Resource<IndexStats>;
  pricingStatus: Resource<PricingCatalogStatus>;
  activeMaintenanceJob: Accessor<MaintenanceJob | null>;
  selectedProviderKeys: Accessor<string[]>;
  scannedProviderKeys: Accessor<string[]>;
  allProvidersSelected: Accessor<boolean>;
}

export function createUsageDerived(deps: UsageDerivedDeps) {
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

  const makeSortHandler =
    (setter: (fn: (prev: UsageSortState) => UsageSortState) => void) =>
    (col: string) => {
      setter((prev) => ({ col, asc: prev.col === col ? !prev.asc : false }));
    };

  const sortedModels = createMemo(() => {
    const data = stats()?.model_costs ?? [];
    const { col, asc } = modelSort();
    return [...data].sort((a, b) =>
      compareUsageValues(
        a[col as keyof ModelCost],
        b[col as keyof ModelCost],
        asc,
      ),
    );
  });

  const sortedProjects = createMemo(() => {
    const data = stats()?.project_costs ?? [];
    const { col, asc } = projectSort();
    return [...data].sort((a, b) =>
      compareUsageValues(
        a[col as keyof ProjectCost],
        b[col as keyof ProjectCost],
        asc,
      ),
    );
  });

  const sortedSessions = createMemo(() => {
    const data = stats()?.recent_sessions ?? [];
    const { col, asc } = sessionSort();
    return [...data].sort((a, b) =>
      compareUsageValues(
        a[col as keyof SessionCostRow],
        b[col as keyof SessionCostRow],
        asc,
      ),
    );
  });

  const visibleProjects = createMemo(() =>
    sortedProjects().slice(0, projectLimit()),
  );
  const visibleSessions = createMemo(() =>
    sortedSessions().slice(0, sessionLimit()),
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

  const totalTokens = createMemo(() => totalUsageTokens(stats()));

  const dailyChartData = createMemo(() =>
    buildDailyChartData(
      stats()?.daily_usage ?? [],
      selectedProviderKeys(),
      chartMetric(),
    ),
  );

  const topModels = createMemo(() => sortedModels().slice(0, 4));
  const maxTopModelCost = createMemo(() => topModels()[0]?.cost ?? 0);

  const activeRangeLabel = createMemo(() => {
    const custom = customRange();
    if (custom) return `${custom.start} ~ ${custom.end}`;
    switch (rangeDays()) {
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
  });

  const showRebuildHint = createMemo(() => {
    const data = stats();
    if (!data || data.total_turns > 0) return false;
    if (selectedProviderKeys().length === 0) return false;
    return (
      rangeDays() === null &&
      customRange() === null &&
      allProvidersSelected() &&
      (sessionCount() ?? 0) > 0
    );
  });

  const emptyMessage = createMemo(() => {
    if (scannedProviderKeys().length === 0) return t("usage.noData");
    if (selectedProviderKeys().length === 0) return t("usage.selectProvider");
    if (showRebuildHint()) return t("usage.rebuildHint");
    if ((sessionCount() ?? 0) === 0) return t("usage.noData");
    return t("usage.noResults");
  });

  const formattedPricingUpdatedAt = createMemo(() => {
    if (pricingStatus.error) return t("error.message");
    const updatedAt = pricingStatus()?.updated_at;
    return updatedAt
      ? formatLocalDateTime(updatedAt)
      : t("settings.pricingNotFetched");
  });

  const formattedUsageUpdatedAt = createMemo(() => {
    if (indexStats.error) return t("error.message");
    const updatedAt = indexStats()?.usage_last_refreshed_at;
    return updatedAt ? formatLocalDateTime(updatedAt) : t("usage.notRefreshed");
  });

  const pricingStatusError = createMemo(() =>
    pricingStatus.error ? errorMessage(pricingStatus.error) : null,
  );

  const indexStatsError = createMemo(() =>
    indexStats.error ? errorMessage(indexStats.error) : null,
  );

  const pricingModelCountLabel = createMemo(() => {
    if (pricingStatus.error) return t("error.message");
    if (pricingStatus.loading && !pricingStatus()) return t("common.loading");
    return String(pricingStatus()?.model_count ?? 0);
  });

  const maintenanceStatusText = createMemo(() => {
    const job = activeMaintenanceJob();
    if (job === "refresh_usage") return t("usage.refreshUsageRunning");
    if (job === "rebuild_index") return t("usage.rebuildRunning");
    return t("usage.usageReady");
  });

  const totalCostTrend = createMemo(() =>
    trendPercent(stats()?.total_cost ?? 0, stats()?.prev_period, "total_cost"),
  );

  const summaryStats = createMemo(() => {
    const data = stats();
    return [
      {
        label: t("usage.sessions"),
        value: (data?.total_sessions ?? 0).toLocaleString(),
        trend: trendPercent(
          data?.total_sessions ?? 0,
          data?.prev_period,
          "total_sessions",
        ),
      },
      {
        label: t("usage.turns"),
        value: (data?.total_turns ?? 0).toLocaleString(),
        trend: trendPercent(
          data?.total_turns ?? 0,
          data?.prev_period,
          "total_turns",
        ),
      },
      {
        label: t("usage.tokens"),
        value: fmtTokens(totalTokens()),
        trend: trendPercent(totalTokens(), data?.prev_period, "total_tokens"),
      },
    ];
  });

  const tokenBreakdown = createMemo(() => {
    const data = stats();
    const tokenTotal = totalTokens();
    return [
      {
        label: t("usage.input"),
        value: fmtTokens(data?.total_input_tokens ?? 0),
        share:
          tokenTotal > 0
            ? fmtPct((data?.total_input_tokens ?? 0) / tokenTotal)
            : "0%",
      },
      {
        label: t("usage.output"),
        value: fmtTokens(data?.total_output_tokens ?? 0),
        share:
          tokenTotal > 0
            ? fmtPct((data?.total_output_tokens ?? 0) / tokenTotal)
            : "0%",
      },
      {
        label: t("usage.cacheRead"),
        value: fmtTokens(data?.total_cache_read_tokens ?? 0),
        share:
          tokenTotal > 0
            ? fmtPct((data?.total_cache_read_tokens ?? 0) / tokenTotal)
            : "0%",
      },
      {
        label: t("usage.cacheWrite"),
        value: fmtTokens(data?.total_cache_write_tokens ?? 0),
        share:
          tokenTotal > 0
            ? fmtPct((data?.total_cache_write_tokens ?? 0) / tokenTotal)
            : "0%",
      },
    ];
  });

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
