import { createSignal, createMemo, Show } from "solid-js";
import { useI18n } from "../../i18n/index";
import { startRefreshUsage, refreshPricingCatalog } from "../../lib/tauri";
import {
  rangeDays,
  setRangeDays,
  customRange,
  setCustomRange,
  selectedProviders,
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
} from "../../stores/usageView";
import { ConfirmDialog } from "../ConfirmDialog";
import { toast, toastError, toastInfo } from "../../stores/toast";
import { buildHoveredDaySummary } from "../../lib/usage";
import { makeFmtChartValue } from "./formatters";
import { Toolbar } from "./Toolbar";
import { SummaryCards } from "./SummaryCards";
import { ActivityHeatmap } from "./ActivityHeatmap";
import { Chart } from "./Chart";
import { TopModels } from "./TopModels";
import { ModelTable } from "./ModelTable";
import { ProjectTable } from "./ProjectTable";
import { SessionTable } from "./SessionTable";
import {
  createProviderSelection,
  createUsageResources,
  createUsageDerived,
} from "./hooks";

export function UsagePanel() {
  const { t } = useI18n();

  // Ephemeral per-visit state — intentionally resets each time the panel
  // remounts. Persistent UI state lives in the `usageView` store so it survives
  // the `<Show>`-driven remount when switching views.
  const [hoveredDate, setHoveredDate] = createSignal<string | null>(null);
  const [showClearUsageConfirm, setShowClearUsageConfirm] = createSignal(false);
  const [isRefreshingPricing, setIsRefreshingPricing] = createSignal(false);

  const {
    scannedProviderSnapshots,
    scannedProviderKeys,
    selectedProviderKeys,
    allProvidersSelected,
    toggleProvider,
    selectAllProviders,
    providerInfo,
  } = createProviderSelection();

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
  } = createUsageResources(selectedProviderKeys);

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
  } = createUsageDerived({
    stats,
    sessionCount,
    indexStats,
    pricingStatus,
    activeMaintenanceJob,
    selectedProviderKeys,
    scannedProviderKeys,
    allProvidersSelected,
  });

  const fmtChartValue = makeFmtChartValue(chartMetric);

  const hoveredDaySummary = createMemo(() =>
    buildHoveredDaySummary(hoveredDate(), dailyChartData(), providerInfo),
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

  return (
    <div class="usage-panel">
      <Toolbar
        activeRangeLabel={activeRangeLabel}
        selectedProviderCount={() => selectedProviderKeys().length}
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
        scannedProviderKeysCount={() => scannedProviderKeys().length}
        allProvidersSelected={allProvidersSelected}
        isProviderSelected={(key) => selectedProviders().has(key)}
        onToggleProvider={toggleProvider}
        onToggleAllProviders={selectAllProviders}
        providerInfo={providerInfo}
        providerSessionCount={(key) => {
          const counts = stats()?.provider_session_counts;
          return counts?.find((c) => c.provider === key)?.count ?? 0;
        }}
      />

      <div class="usage-content-stack">
        <Show
          when={stats()}
          fallback={<div class="usage-loading">{t("common.loading")}</div>}
        >
          {(data) => (
            <Show
              when={data().total_turns > 0}
              fallback={
                <section class="usage-card usage-empty">
                  <p class="usage-empty-text">{emptyMessage()}</p>
                </section>
              }
            >
              <div class="usage-summary-row">
                <SummaryCards
                  totalCost={() => data().total_cost}
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
                  loading={() => calendar.loading}
                />
              </div>

              <div class="usage-overview-grid">
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
                onSort={makeSortHandler(setModelSort)}
                formatModelName={formatModelName}
              />

              <ProjectTable
                visibleProjects={visibleProjects}
                totalProjectCount={() => sortedProjects().length}
                projectLimit={projectLimit}
                onLimitChange={setProjectLimit}
                projectSort={projectSort}
                onSort={makeSortHandler(setProjectSort)}
                providerInfo={providerInfo}
                formatProjectName={formatProjectName}
                formatProjectPath={formatProjectPath}
              />

              <SessionTable
                visibleSessions={visibleSessions}
                totalSessionCount={() => sortedSessions().length}
                sessionLimit={sessionLimit}
                onLimitChange={setSessionLimit}
                sessionSort={sessionSort}
                onSort={makeSortHandler(setSessionSort)}
                providerInfo={providerInfo}
                formatProjectName={formatProjectName}
                formatProjectPath={formatProjectPath}
                formatModelName={formatModelName}
              />
            </Show>
          )}
        </Show>
      </div>

      <ConfirmDialog
        open={showClearUsageConfirm()}
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
