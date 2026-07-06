import { useState } from "react";
import { useI18n } from "@/i18n/index";
import { startRefreshUsage, refreshPricingCatalog } from "@/lib/tauri";
import {
  useRangeDays,
  setRangeDays,
  useCustomRange,
  setCustomRange,
  useSelectedProviders,
  useProjectLimit,
  setProjectLimit,
  useSessionLimit,
  setSessionLimit,
  useChartMetric,
  setChartMetric,
  useCalendarMetric,
  setCalendarMetric,
  useCalendarYear,
  setCalendarYear,
  useModelSort,
  setModelSort,
  useProjectSort,
  setProjectSort,
  useSessionSort,
  setSessionSort,
} from "@/features/usage/usageView";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { toast, toastError, toastInfo } from "@/stores/toast";
import { buildHoveredDaySummary } from "@/lib/usage";
import { makeFmtChartValue } from "@/features/usage/formatters";
import { Toolbar } from "@/features/usage/Toolbar";
import { SummaryCards } from "@/features/usage/SummaryCards";
import { ActivityHeatmap } from "@/features/usage/ActivityHeatmap";
import { Chart } from "@/features/usage/Chart";
import { TopModels } from "@/features/usage/TopModels";
import { ModelTable } from "@/features/usage/ModelTable";
import { ProjectTable } from "@/features/usage/ProjectTable";
import { SessionTable } from "@/features/usage/SessionTable";
import {
  useProviderSelection,
  useUsageResources,
  useUsageDerived,
} from "@/features/usage/hooks";

export function UsagePanel() {
  const { t } = useI18n();

  const rangeDays = useRangeDays();
  const customRange = useCustomRange();
  const selectedProviders = useSelectedProviders();
  const projectLimit = useProjectLimit();
  const sessionLimit = useSessionLimit();
  const chartMetric = useChartMetric();
  const calendarMetric = useCalendarMetric();
  const calendarYear = useCalendarYear();
  const modelSort = useModelSort();
  const projectSort = useProjectSort();
  const sessionSort = useSessionSort();

  // Ephemeral per-visit state — intentionally resets each time the panel
  // remounts. Persistent UI state lives in the `usageView` store so it survives
  // the remount when switching views.
  const [hoveredDate, setHoveredDate] = useState<string | null>(null);
  const [showClearUsageConfirm, setShowClearUsageConfirm] = useState(false);
  const [isRefreshingPricing, setIsRefreshingPricing] = useState(false);

  const {
    scannedProviderSnapshots,
    scannedProviderKeys,
    selectedProviderKeys,
    allProvidersSelected,
    toggleProvider,
    selectAllProviders,
    providerInfo,
  } = useProviderSelection();

  const {
    stats,
    calendar,
    heatmapGrid,
    availableYears,
    sessionCount,
    indexStats,
    pricingStatus,
    refetchPricingStatus,
    activeMaintenanceJob,
  } = useUsageResources(selectedProviderKeys);

  const {
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
  } = useUsageDerived({
    stats,
    sessionCount,
    indexStats,
    pricingStatus,
    activeMaintenanceJob,
    selectedProviderKeys,
    scannedProviderKeys,
    allProvidersSelected,
  });

  const fmtChartValue = makeFmtChartValue(() => chartMetric);

  const hoveredDaySummary = buildHoveredDaySummary(
    hoveredDate,
    dailyChartData,
    providerInfo,
  );

  async function handleRefreshUsage() {
    try {
      const started = await startRefreshUsage();
      if (!started) {
        toastInfo(t("toast.maintenanceBusy"));
        return;
      }
      setHoveredDate(null);
    } catch (error) {
      toastError(String(error));
    }
  }

  async function handleRefreshPricing() {
    setIsRefreshingPricing(true);
    try {
      await refreshPricingCatalog();
      await refetchPricingStatus();
      toast(t("toast.pricingRefreshOk"));
    } catch (error) {
      toastError(String(error));
    } finally {
      setIsRefreshingPricing(false);
    }
  }

  const data = stats.data;

  return (
    <div className="usage-panel">
      <Toolbar
        activeRangeLabel={activeRangeLabel}
        selectedProviderCount={selectedProviderKeys.length}
        activeMaintenanceJob={activeMaintenanceJob}
        maintenanceStatusText={maintenanceStatusText}
        rangeDays={rangeDays}
        onRangeChange={(days) => {
          setCustomRange(null);
          setRangeDays(days);
        }}
        customRange={customRange}
        onCustomRangeChange={setCustomRange}
        isRefreshingPricing={isRefreshingPricing}
        onRefreshPricing={() => void handleRefreshPricing()}
        onRequestRefreshUsage={() => setShowClearUsageConfirm(true)}
        formattedPricingUpdatedAt={formattedPricingUpdatedAt}
        formattedUsageUpdatedAt={formattedUsageUpdatedAt}
        pricingModelCountLabel={pricingModelCountLabel}
        pricingStatusError={pricingStatusError}
        indexStatsError={indexStatsError}
        scannedProviderSnapshots={scannedProviderSnapshots}
        scannedProviderKeysCount={scannedProviderKeys.length}
        allProvidersSelected={allProvidersSelected}
        isProviderSelected={(key) => selectedProviders.has(key)}
        onToggleProvider={toggleProvider}
        onToggleAllProviders={selectAllProviders}
        providerInfo={providerInfo}
        providerSessionCount={(key) => {
          const counts = stats.data?.provider_session_counts;
          return counts?.find((c) => c.provider === key)?.count ?? 0;
        }}
      />

      <div className="usage-content-stack">
        {!data ? (
          <div className="usage-loading">{t("common.loading")}</div>
        ) : data.total_turns > 0 ? (
          <>
            <div className="usage-summary-row">
              <SummaryCards
                totalCost={data.total_cost}
                totalCostTrend={totalCostTrend}
                summaryStats={summaryStats}
                tokenBreakdown={tokenBreakdown}
              />
              <ActivityHeatmap
                grid={heatmapGrid}
                metric={calendarMetric}
                setMetric={setCalendarMetric}
                year={calendarYear}
                setYear={setCalendarYear}
                availableYears={availableYears}
                loading={calendar.loading}
              />
            </div>

            <div className="usage-overview-grid">
              <Chart
                dailyChartData={dailyChartData}
                hoveredDate={hoveredDate}
                setHoveredDate={setHoveredDate}
                hoveredDaySummary={hoveredDaySummary}
                chartMetric={chartMetric}
                setChartMetric={setChartMetric}
                activeRangeLabel={activeRangeLabel}
                fmtChartValue={fmtChartValue}
                providerInfo={providerInfo}
              />
              <TopModels
                topModels={topModels}
                maxTopModelCost={maxTopModelCost}
                formatModelName={formatModelName}
              />
            </div>

            <ModelTable
              sortedModels={sortedModels}
              modelSort={modelSort}
              onSort={makeSortHandler(modelSort, setModelSort)}
              formatModelName={formatModelName}
            />

            <ProjectTable
              visibleProjects={visibleProjects}
              totalProjectCount={sortedProjects.length}
              projectLimit={projectLimit}
              onLimitChange={setProjectLimit}
              projectSort={projectSort}
              onSort={makeSortHandler(projectSort, setProjectSort)}
              providerInfo={providerInfo}
              formatProjectName={formatProjectName}
              formatProjectPath={formatProjectPath}
            />

            <SessionTable
              visibleSessions={visibleSessions}
              totalSessionCount={sortedSessions.length}
              sessionLimit={sessionLimit}
              onLimitChange={setSessionLimit}
              sessionSort={sessionSort}
              onSort={makeSortHandler(sessionSort, setSessionSort)}
              providerInfo={providerInfo}
              formatProjectName={formatProjectName}
              formatProjectPath={formatProjectPath}
              formatModelName={formatModelName}
            />
          </>
        ) : (
          <section className="usage-card usage-empty">
            <p className="usage-empty-text">{emptyMessage}</p>
          </section>
        )}
      </div>

      <ConfirmDialog
        open={showClearUsageConfirm}
        title={t("usage.refreshUsage")}
        message={t("usage.refreshUsageConfirm")}
        confirmLabel={t("usage.refreshUsage")}
        onConfirm={() => {
          setShowClearUsageConfirm(false);
          void handleRefreshUsage();
        }}
        onCancel={() => setShowClearUsageConfirm(false)}
        danger={true}
      />
    </div>
  );
}
