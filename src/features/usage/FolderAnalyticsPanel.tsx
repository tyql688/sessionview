import { type CSSProperties, useEffect, useMemo, useState } from "react";
import {
  Activity,
  ArrowLeft,
  CircleDollarSign,
  Folder,
  Hash,
  type LucideIcon,
  MessageSquare,
  Wrench,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { useI18n } from "@/i18n/index";
import { getProjectDailyUsage, getProjectToolUsage, refreshPricingCatalog, startRefreshUsage } from "@/lib/tauri";
import type { ProjectCost, ProjectDailyUsage, ProjectToolUsageStats } from "@/lib/types";
import {
  setCustomRange,
  setRangeDays,
  type CustomDateRange,
  useCustomRange,
  useRangeDays,
  useSelectedProviders,
} from "@/features/usage/usageView";
import { Toolbar, type ProviderChipInfo } from "@/features/usage/Toolbar";
import { useProviderSelection, useUsageDerived, useUsageResources } from "@/features/usage/hooks";
import { fmtCost, fmtPct, fmtTokens } from "@/features/usage/formatters";
import { ProjectTrendPanel } from "@/features/usage/ProjectTrendPanel";
import { errorMessage } from "@/lib/errors";
import { toast, toastError, toastInfo } from "@/stores/toast";
import { cn } from "@/lib/utils";

interface FolderOverviewProps {
  projects: ProjectCost[];
  totalTokens: number;
  formatProjectName: (project: string, projectPath: string) => string;
  formatProjectPath: (projectPath: string) => string;
  onSelectProject: (projectPath: string) => void;
}

interface FolderDetailProps {
  project: ProjectCost;
  selectedProviderKeys: string[];
  rangeDays: number | null;
  customRange: CustomDateRange | null;
  activeRangeLabel: string;
  providerInfo: (key: string) => ProviderChipInfo;
  formatProjectName: (project: string, projectPath: string) => string;
  formatProjectPath: (projectPath: string) => string;
  formatModelName: (model: string) => string;
  onBack: () => void;
}

function percent(value: number): string {
  if (value <= 0) return "0%";
  if (value < 0.01) return "<1%";
  return `${Math.round(value * 100)}%`;
}

function barWidth(value: number, max: number): string {
  if (value <= 0 || max <= 0) return "0%";
  return `${Math.max(3, (value / max) * 100)}%`;
}

interface ToolUsagePanelProps {
  stats: ProjectToolUsageStats | null;
  loading: boolean;
  error: string | null;
}

interface FolderSummaryTileProps {
  icon: LucideIcon;
  label: string;
  value: string;
  detail: string;
  tone: "blue" | "green" | "amber" | "pink";
}

function FolderSummaryTile(props: FolderSummaryTileProps) {
  const Icon = props.icon;
  return (
    <div className={cn("usage-summary-stat", `usage-summary-stat-${props.tone}`)}>
      <span className="usage-summary-stat-icon">
        <Icon className="size-4" aria-hidden="true" />
      </span>
      <span className="usage-summary-stat-label">{props.label}</span>
      <strong className="usage-summary-stat-value">{props.value}</strong>
      <span className="usage-summary-stat-detail">{props.detail}</span>
    </div>
  );
}

function FolderDetailSummary(props: { project: ProjectCost }) {
  const { t } = useI18n();
  const tokenBreakdown = [
    {
      label: t("usage.input"),
      value: props.project.input_tokens,
    },
    {
      label: t("usage.output"),
      value: props.project.output_tokens,
    },
    {
      label: t("usage.cacheRead"),
      value: props.project.cache_read_tokens,
    },
    {
      label: t("usage.cacheWrite"),
      value: props.project.cache_write_tokens,
    },
  ].map((item) => {
    const shareValue = props.project.tokens > 0 ? item.value / props.project.tokens : 0;
    return {
      ...item,
      formattedValue: fmtTokens(item.value),
      share: fmtPct(shareValue),
      shareValue,
    };
  });
  const items: FolderSummaryTileProps[] = [
    {
      icon: Hash,
      label: t("usage.tokens"),
      value: fmtTokens(props.project.tokens),
      detail: t("usage.currentRange"),
      tone: "pink",
    },
    {
      icon: CircleDollarSign,
      label: t("usage.cost"),
      value: fmtCost(props.project.cost),
      detail: t("usage.currentRange"),
      tone: "blue",
    },
    {
      icon: MessageSquare,
      label: t("usage.sessions"),
      value: props.project.sessions.toLocaleString(),
      detail: t("usage.currentRange"),
      tone: "green",
    },
    {
      icon: Activity,
      label: t("usage.turns"),
      value: props.project.turns.toLocaleString(),
      detail: t("usage.currentRange"),
      tone: "amber",
    },
  ];

  return (
    <section className="usage-card usage-summary-card folder-detail-summary-card">
      <div className="usage-summary-stat-grid">
        {items.map((item) => (
          <FolderSummaryTile
            key={item.label}
            icon={item.icon}
            label={item.label}
            value={item.value}
            detail={item.detail}
            tone={item.tone}
          />
        ))}
      </div>

      <div className="usage-token-mix">
        <div className="usage-token-mix-header">
          <span>{t("usage.tokenMix")}</span>
          <small>{t("usage.currentRange")}</small>
        </div>
        <div className="usage-breakdown-grid">
          {tokenBreakdown.map((item) => (
            <div key={item.label} className="usage-breakdown-item">
              <span className="usage-breakdown-label">{item.label}</span>
              <strong className="usage-breakdown-value">{item.formattedValue}</strong>
              <span className="usage-breakdown-pct">{item.share}</span>
              <span className="usage-breakdown-bar" aria-hidden="true">
                <span style={{ width: `${Math.max(3, item.shareValue * 100)}%` }} />
              </span>
            </div>
          ))}
        </div>
      </div>
    </section>
  );
}

function ToolUsagePanel(props: ToolUsagePanelProps) {
  const { t } = useI18n();
  const maxCount = Math.max(...(props.stats?.tools ?? []).map((tool) => tool.count), 1);

  return (
    <section className="usage-card folder-detail-panel folder-tool-panel">
      <div className="folder-panel-heading">
        <span>
          <Wrench className="size-3.5" aria-hidden="true" />
          {t("usage.toolUsage")}
        </span>
        {props.stats && (
          <small>{t("usage.toolUsageMeta").replace("{count}", String(props.stats.sessions_scanned))}</small>
        )}
      </div>

      {props.loading ? (
        <div className="folder-detail-muted">{t("usage.loadingTools")}</div>
      ) : props.error ? (
        <div className="folder-detail-muted">{props.error}</div>
      ) : props.stats && props.stats.tools.length > 0 ? (
        <div className="folder-tool-list">
          {props.stats.tools.slice(0, 9).map((tool) => (
            <div key={tool.key} className="folder-tool-row">
              <span className="folder-tool-name">{tool.label}</span>
              <span className="folder-tool-track" aria-hidden="true">
                <span style={{ width: barWidth(tool.count, maxCount) }} />
              </span>
              <strong>{tool.count.toLocaleString()}</strong>
              <small>{tool.sessions.toLocaleString()}</small>
            </div>
          ))}
        </div>
      ) : (
        <div className="folder-detail-muted">{t("usage.noToolUsage")}</div>
      )}
    </section>
  );
}

function FolderOverview(props: FolderOverviewProps) {
  const { t } = useI18n();
  const maxTokens = Math.max(...props.projects.map((project) => project.tokens), 1);
  const totalSessions = props.projects.reduce((sum, project) => sum + project.sessions, 0);
  const totalCost = props.projects.reduce((sum, project) => sum + project.cost, 0);
  const topProjects = props.projects.slice(0, 5);

  return (
    <section className="usage-card folder-analytics-overview-card">
      <div className="usage-section-header">
        <div>
          <div className="usage-section-title">{t("usage.folderOverview")}</div>
          <div className="usage-section-subtitle">
            {t("usage.folderOverviewSubtitle")
              .replace("{count}", props.projects.length.toLocaleString())
              .replace("{tokens}", fmtTokens(props.totalTokens))}
          </div>
        </div>
      </div>

      <div className="folder-analytics-board">
        <div className="folder-analytics-card-section">
          <div className="folder-analytics-list-heading">
            <span>{t("usage.folderAllFolders")}</span>
            <small>{t("usage.folderListHint")}</small>
          </div>
          <div className="folder-analytics-card-grid">
            {props.projects.map((project, index) => {
              const title = props.formatProjectName(project.project, project.project_path);
              const path = props.formatProjectPath(project.project_path);
              const share = props.totalTokens > 0 ? project.tokens / props.totalTokens : 0;
              return (
                <Button
                  key={project.project_path}
                  variant="ghost"
                  className={cn(
                    "folder-analytics-card h-auto min-w-0 active:translate-y-0",
                    index < 3 && "is-featured",
                  )}
                  aria-label={t("usage.openFolderAnalysis").replace("{folder}", title)}
                  title={path}
                  type="button"
                  onClick={() => props.onSelectProject(project.project_path)}
                >
                  <span className="folder-analytics-card-head">
                    <span className="folder-analytics-card-icon">
                      <Folder className="size-4" aria-hidden="true" />
                    </span>
                    <span className="folder-analytics-card-copy">
                      <strong>{title}</strong>
                      <small>{path}</small>
                    </span>
                    {index < 3 && <span className="folder-analytics-rank">{index + 1}</span>}
                  </span>

                  <span className="folder-analytics-card-main">
                    <strong>
                      <Hash className="size-3.5" aria-hidden="true" />
                      {fmtTokens(project.tokens)}
                    </strong>
                    <small>{percent(share)}</small>
                  </span>

                  <span className="folder-analytics-card-track" aria-hidden="true">
                    <span style={{ width: barWidth(project.tokens, maxTokens) }} />
                  </span>

                  <span className="folder-analytics-card-foot">
                    <span>
                      <CircleDollarSign className="size-3.5" aria-hidden="true" />
                      {fmtCost(project.cost)}
                    </span>
                    <span>{t("usage.folderSessionCount").replace("{count}", project.sessions.toLocaleString())}</span>
                  </span>
                </Button>
              );
            })}
          </div>
        </div>

        <aside className="folder-analytics-sidebar">
          <div className="folder-analytics-summary-grid">
            <div className="folder-analytics-summary-stat is-primary">
              <span className="folder-analytics-summary-icon">
                <Hash className="size-4" aria-hidden="true" />
              </span>
              <span>
                <small>{t("usage.tokens")}</small>
                <strong>{fmtTokens(props.totalTokens)}</strong>
              </span>
            </div>
            <div className="folder-analytics-summary-stat">
              <span className="folder-analytics-summary-icon">
                <CircleDollarSign className="size-4" aria-hidden="true" />
              </span>
              <span>
                <small>{t("usage.cost")}</small>
                <strong>{fmtCost(totalCost)}</strong>
              </span>
            </div>
            <div className="folder-analytics-summary-stat">
              <span className="folder-analytics-summary-icon">
                <MessageSquare className="size-4" aria-hidden="true" />
              </span>
              <span>
                <small>{t("usage.sessions")}</small>
                <strong>{totalSessions.toLocaleString()}</strong>
              </span>
            </div>
            <div className="folder-analytics-summary-stat">
              <span className="folder-analytics-summary-icon">
                <Folder className="size-4" aria-hidden="true" />
              </span>
              <span>
                <small>{t("usage.folderAllFolders")}</small>
                <strong>{props.projects.length.toLocaleString()}</strong>
              </span>
            </div>
          </div>

          <div className="folder-analytics-top-panel">
            <div className="folder-panel-heading">
              <span>{t("usage.folderTopFolders")}</span>
              <small>{t("usage.folderListHint")}</small>
            </div>
            <div className="folder-analytics-top-list">
              {topProjects.map((project, index) => {
                const title = props.formatProjectName(project.project, project.project_path);
                const path = props.formatProjectPath(project.project_path);
                const share = props.totalTokens > 0 ? project.tokens / props.totalTokens : 0;
                return (
                  <Button
                    key={project.project_path}
                    variant="ghost"
                    className="folder-analytics-top-row h-auto min-w-0 active:translate-y-0"
                    aria-label={t("usage.openFolderAnalysis").replace("{folder}", title)}
                    title={path}
                    type="button"
                    onClick={() => props.onSelectProject(project.project_path)}
                  >
                    <span className="folder-analytics-top-rank">{index + 1}</span>
                    <span className="folder-analytics-top-copy">
                      <strong>{title}</strong>
                      <small>{fmtTokens(project.tokens)}</small>
                    </span>
                    <span className="folder-analytics-top-share">{percent(share)}</span>
                    <span className="folder-analytics-card-track" aria-hidden="true">
                      <span style={{ width: barWidth(project.tokens, maxTokens) }} />
                    </span>
                  </Button>
                );
              })}
            </div>
          </div>
        </aside>
      </div>
    </section>
  );
}

function FolderDetail(props: FolderDetailProps) {
  const { t } = useI18n();
  const [toolStats, setToolStats] = useState<ProjectToolUsageStats | null>(null);
  const [toolLoading, setToolLoading] = useState(true);
  const [toolError, setToolError] = useState<string | null>(null);
  const [trendDays, setTrendDays] = useState<ProjectDailyUsage[] | null>(null);
  const [trendLoading, setTrendLoading] = useState(true);
  const [trendError, setTrendError] = useState<string | null>(null);
  const title = props.formatProjectName(props.project.project, props.project.project_path);
  const path = props.formatProjectPath(props.project.project_path);
  const maxProviderTokens = Math.max(...props.project.by_provider.map((entry) => entry.tokens), 1);
  const maxModelTokens = Math.max(...props.project.by_model.map((entry) => entry.tokens), 1);

  useEffect(() => {
    let disposed = false;
    setToolLoading(true);
    setToolError(null);
    setToolStats(null);
    void getProjectToolUsage(
      props.project.project_path,
      props.selectedProviderKeys,
      props.rangeDays,
      props.customRange?.start ?? null,
      props.customRange?.end ?? null,
    ).then(
      (stats) => {
        if (!disposed) {
          setToolStats(stats);
          setToolLoading(false);
        }
      },
      (error: unknown) => {
        if (!disposed) {
          console.error("failed to load project tool usage", error);
          setToolError(errorMessage(error));
          setToolLoading(false);
        }
      },
    );
    return () => {
      disposed = true;
    };
  }, [props.project.project_path, props.selectedProviderKeys, props.rangeDays, props.customRange]);

  useEffect(() => {
    let disposed = false;
    setTrendLoading(true);
    setTrendError(null);
    setTrendDays(null);
    void getProjectDailyUsage(
      props.project.project_path,
      props.selectedProviderKeys,
      props.rangeDays,
      props.customRange?.start ?? null,
      props.customRange?.end ?? null,
    ).then(
      (days) => {
        if (!disposed) {
          setTrendDays(days);
          setTrendLoading(false);
        }
      },
      (error: unknown) => {
        if (!disposed) {
          console.error("failed to load project daily usage", error);
          setTrendError(errorMessage(error));
          setTrendLoading(false);
        }
      },
    );
    return () => {
      disposed = true;
    };
  }, [props.project.project_path, props.selectedProviderKeys, props.rangeDays, props.customRange]);

  return (
    <div className="folder-detail-stack">
      <section className="usage-card folder-detail-hero">
        <Button variant="ghost" size="sm" className="folder-detail-back active:translate-y-0" onClick={props.onBack}>
          <ArrowLeft className="size-4" aria-hidden="true" />
          {t("usage.backToFolders")}
        </Button>
        <div className="folder-detail-title-row">
          <span className="folder-detail-icon">
            <Folder className="size-5" aria-hidden="true" />
          </span>
          <div className="folder-detail-title-copy">
            <span className="usage-overline">{t("usage.folderDetail")}</span>
            <h2>{title}</h2>
            <p title={path}>{path}</p>
          </div>
        </div>
      </section>

      <FolderDetailSummary project={props.project} />

      <ProjectTrendPanel
        days={trendDays}
        loading={trendLoading}
        error={trendError}
        rangeDays={props.rangeDays}
        customRange={props.customRange}
        activeRangeLabel={props.activeRangeLabel}
        providerInfo={props.providerInfo}
        formatModelName={props.formatModelName}
      />

      <div className="folder-detail-grid">
        <section className="usage-card folder-detail-panel">
          <div className="folder-panel-heading">
            <span>{t("usage.sourceMix")}</span>
          </div>
          <div className="folder-detail-list">
            {props.project.by_provider.map((entry) => {
              const info = props.providerInfo(entry.provider);
              const share = props.project.tokens > 0 ? entry.tokens / props.project.tokens : 0;
              const style = {
                "--usage-project-color": info.color,
                "--usage-project-share": barWidth(entry.tokens, maxProviderTokens),
              } as CSSProperties;
              return (
                <div key={entry.provider} className="usage-project-source-row" style={style}>
                  <span className="usage-provider-dot" style={{ background: info.color }} />
                  <span className="usage-project-source-name">{info.label}</span>
                  <span className="usage-project-source-track" aria-hidden="true">
                    <span />
                  </span>
                  <strong>{fmtTokens(entry.tokens)}</strong>
                  <small>{percent(share)}</small>
                </div>
              );
            })}
          </div>
        </section>

        <section className="usage-card folder-detail-panel">
          <div className="folder-panel-heading">
            <span>{t("usage.folderModelMix")}</span>
          </div>
          <div className="folder-detail-list">
            {props.project.by_model.map((entry) => {
              const share = props.project.tokens > 0 ? entry.tokens / props.project.tokens : 0;
              return (
                <div key={entry.model} className="folder-detail-model-row">
                  <span className="usage-model-tag">{props.formatModelName(entry.model)}</span>
                  <span className="folder-detail-model-track" aria-hidden="true">
                    <span style={{ width: barWidth(entry.tokens, maxModelTokens) }} />
                  </span>
                  <strong>{fmtTokens(entry.tokens)}</strong>
                  <small>{percent(share)}</small>
                </div>
              );
            })}
          </div>
        </section>

        <ToolUsagePanel stats={toolStats} loading={toolLoading} error={toolError} />
      </div>
    </div>
  );
}

export function FolderAnalyticsPanel() {
  const { t } = useI18n();
  const rangeDays = useRangeDays();
  const customRange = useCustomRange();
  const selectedProviders = useSelectedProviders();
  const [selectedProjectPath, setSelectedProjectPath] = useState<string | null>(null);
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

  const { stats, sessionCount, indexStats, pricingStatus, refetchPricingStatus, activeMaintenanceJob } =
    useUsageResources(selectedProviderKeys, { includeCalendar: false });

  const {
    formatModelName,
    formatProjectName,
    formatProjectPath,
    activeRangeLabel,
    emptyMessage,
    formattedPricingUpdatedAt,
    formattedUsageUpdatedAt,
    pricingStatusError,
    indexStatsError,
    pricingModelCountLabel,
    maintenanceStatusText,
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

  const data = stats.data;
  const projects = useMemo(
    () => [...(data?.project_costs ?? [])].sort((left, right) => right.tokens - left.tokens || right.cost - left.cost),
    [data?.project_costs],
  );
  const selectedProject = selectedProjectPath
    ? (projects.find((project) => project.project_path === selectedProjectPath) ?? null)
    : null;
  const totalTokens = useMemo(() => projects.reduce((sum, project) => sum + project.tokens, 0), [projects]);

  async function handleRefreshUsage() {
    try {
      const started = await startRefreshUsage();
      if (!started) {
        toastInfo(t("toast.maintenanceBusy"));
      }
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
    <div className="usage-panel folder-analytics-panel">
      <Toolbar
        title={t("usage.folderAnalyticsTitle")}
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
        onCustomRangeChange={(range) => {
          setCustomRange(range);
        }}
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
        onToggleProvider={(key) => {
          toggleProvider(key);
        }}
        onToggleAllProviders={() => {
          selectAllProviders();
        }}
        providerInfo={providerInfo}
        providerSessionCount={(key) => {
          const counts = stats.data?.provider_session_counts;
          return counts?.find((c) => c.provider === key)?.count ?? 0;
        }}
      />

      <div className="usage-content-stack">
        {!data ? (
          <div className="usage-loading">{t("common.loading")}</div>
        ) : data.total_turns > 0 && projects.length > 0 ? (
          selectedProject ? (
            <FolderDetail
              project={selectedProject}
              selectedProviderKeys={selectedProviderKeys}
              rangeDays={rangeDays}
              customRange={customRange}
              activeRangeLabel={activeRangeLabel}
              providerInfo={providerInfo}
              formatProjectName={formatProjectName}
              formatProjectPath={formatProjectPath}
              formatModelName={formatModelName}
              onBack={() => setSelectedProjectPath(null)}
            />
          ) : (
            <FolderOverview
              projects={projects}
              totalTokens={totalTokens}
              formatProjectName={formatProjectName}
              formatProjectPath={formatProjectPath}
              onSelectProject={setSelectedProjectPath}
            />
          )
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
      />
    </div>
  );
}
