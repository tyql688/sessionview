import { type CSSProperties, type PointerEvent, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import {
  Activity,
  ArrowLeft,
  CircleDollarSign,
  Folder,
  Hash,
  type LucideIcon,
  MessageSquare,
  TrendingUp,
  Wrench,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
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
import { addDays } from "@/features/usage/heatmap";
import { toLocalISODate } from "@/lib/formatters";
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

type TrendMetric = "tokens" | "cost";
type TrendDimension = "total" | "provider" | "model";

interface TrendDayTotals {
  date: string;
  turns: number;
  inputTokens: number;
  outputTokens: number;
  cacheReadTokens: number;
  cacheWriteTokens: number;
  tokens: number;
  cost: number;
}

interface TrendSeries {
  id: string;
  label: string;
  color: string;
  values: number[];
  total: number;
}

interface TrendData {
  dates: string[];
  totals: TrendDayTotals[];
  series: TrendSeries[];
  maxValue: number;
  total: number;
}

interface TrendPoint {
  x: number;
  y: number;
  value: number;
}

interface ProjectTrendPanelProps {
  days: ProjectDailyUsage[] | null;
  loading: boolean;
  error: string | null;
  rangeDays: number | null;
  customRange: CustomDateRange | null;
  activeRangeLabel: string;
  providerInfo: (key: string) => ProviderChipInfo;
  formatModelName: (model: string) => string;
}

const TREND_MIN_WIDTH = 420;
const TREND_HEIGHT = 258;
const TREND_LEFT = 58;
const TREND_RIGHT_GUTTER = 26;
const TREND_TOP = 24;
const TREND_BOTTOM = 190;
const TREND_MODEL_COLORS = ["#0a84ff", "#14b8a6", "#8b5cf6", "#f59e0b", "#ec4899", "#64748b"];
const MAX_PROVIDER_SERIES = 8;
const MAX_MODEL_SERIES = 6;

function fmtCompactCost(value: number): string {
  if (value >= 1000) return `$${fmtTokens(value)}`;
  return fmtCost(value);
}

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

function makeTrendDates(
  days: ProjectDailyUsage[],
  rangeDays: number | null,
  customRange: CustomDateRange | null,
): string[] {
  if (rangeDays === null && customRange === null) {
    return [...new Set(days.map((day) => day.date))].sort();
  }

  const start = customRange?.start ?? addDays(toLocalISODate(), -Math.max(0, (rangeDays ?? 1) - 1));
  const end = customRange?.end ?? toLocalISODate();
  const result: string[] = [];
  for (let date = start; date <= end; date = addDays(date, 1)) {
    result.push(date);
  }
  return result;
}

function metricValue(row: ProjectDailyUsage, metric: TrendMetric): number {
  switch (metric) {
    case "tokens":
      return row.tokens;
    case "cost":
      return row.cost;
  }
}

function totalsMetricValue(totals: TrendDayTotals, metric: TrendMetric): number {
  switch (metric) {
    case "tokens":
      return totals.tokens;
    case "cost":
      return totals.cost;
  }
}

function formatTrendValue(metric: TrendMetric, value: number): string {
  return metric === "cost" ? fmtCost(value) : fmtTokens(value);
}

function formatTrendAxisValue(metric: TrendMetric, value: number): string {
  return metric === "cost" ? fmtCompactCost(value) : fmtTokens(value);
}

function isTrendMetric(value: string | undefined): value is TrendMetric {
  return value === "tokens" || value === "cost";
}

function isTrendDimension(value: string | undefined): value is TrendDimension {
  return value === "total" || value === "provider" || value === "model";
}

function addToSeries(bucket: Map<string, number[]>, id: string, index: number, value: number, length: number) {
  const values = bucket.get(id) ?? Array.from({ length }, () => 0);
  values[index] = (values[index] ?? 0) + value;
  bucket.set(id, values);
}

function seriesFromBucket(
  bucket: Map<string, number[]>,
  labelForId: (id: string) => string,
  colorForId: (id: string, index: number) => string,
  limit: number,
): TrendSeries[] {
  return [...bucket.entries()]
    .map(([id, values]) => ({
      id,
      label: labelForId(id),
      color: colorForId(id, 0),
      values,
      total: values.reduce((sum, value) => sum + value, 0),
    }))
    .filter((series) => series.total > 0)
    .sort((left, right) => right.total - left.total || left.label.localeCompare(right.label))
    .slice(0, limit)
    .map((series, index) => ({ ...series, color: colorForId(series.id, index) }));
}

function buildTrendData(
  rows: ProjectDailyUsage[],
  rangeDays: number | null,
  customRange: CustomDateRange | null,
  metric: TrendMetric,
  dimension: TrendDimension,
  providerInfo: (key: string) => ProviderChipInfo,
  formatModelName: (model: string) => string,
): TrendData {
  const dates = makeTrendDates(rows, rangeDays, customRange);
  const dateIndex = new Map(dates.map((date, index) => [date, index]));
  const totals = dates.map<TrendDayTotals>((date) => ({
    date,
    turns: 0,
    inputTokens: 0,
    outputTokens: 0,
    cacheReadTokens: 0,
    cacheWriteTokens: 0,
    tokens: 0,
    cost: 0,
  }));
  const providerBucket = new Map<string, number[]>();
  const modelBucket = new Map<string, number[]>();

  for (const row of rows) {
    const index = dateIndex.get(row.date);
    if (index === undefined) continue;
    const day = totals[index];
    if (!day) continue;
    day.turns += row.turns;
    day.inputTokens += row.input_tokens;
    day.outputTokens += row.output_tokens;
    day.cacheReadTokens += row.cache_read_tokens;
    day.cacheWriteTokens += row.cache_write_tokens;
    day.tokens += row.tokens;
    day.cost += row.cost;

    const value = metricValue(row, metric);
    addToSeries(providerBucket, row.provider, index, value, dates.length);
    addToSeries(modelBucket, row.model.trim(), index, value, dates.length);
  }

  const totalValues = totals.map((day) => totalsMetricValue(day, metric));
  const series =
    dimension === "total"
      ? [
          {
            id: "total",
            label: "",
            color: "var(--accent)",
            values: totalValues,
            total: totalValues.reduce((sum, value) => sum + value, 0),
          },
        ].filter((seriesItem) => seriesItem.total > 0)
      : dimension === "provider"
        ? seriesFromBucket(
            providerBucket,
            (id) => providerInfo(id).label,
            (id) => providerInfo(id).color,
            MAX_PROVIDER_SERIES,
          )
        : seriesFromBucket(
            modelBucket,
            (id) => formatModelName(id),
            (_id, index) => TREND_MODEL_COLORS[index % TREND_MODEL_COLORS.length]!,
            MAX_MODEL_SERIES,
          );
  const maxValue = Math.max(...series.flatMap((seriesItem) => seriesItem.values), 1);
  const total = totalValues.reduce((sum, value) => sum + value, 0);

  return { dates, totals, series, maxValue, total };
}

function makeTickIndexes(count: number): number[] {
  if (count <= 0) return [];
  if (count <= 7) return Array.from({ length: count }, (_value, index) => index);
  return [
    ...new Set([
      0,
      Math.round((count - 1) * 0.25),
      Math.round((count - 1) * 0.5),
      Math.round((count - 1) * 0.75),
      count - 1,
    ]),
  ];
}

function pointsForSeries(values: number[], width: number, maxValue: number): TrendPoint[] {
  const right = Math.max(TREND_LEFT + 1, width - TREND_RIGHT_GUTTER);
  const chartWidth = right - TREND_LEFT;
  const chartHeight = TREND_BOTTOM - TREND_TOP;
  return values.map((value, index) => {
    const x =
      values.length <= 1 ? TREND_LEFT + chartWidth / 2 : TREND_LEFT + (index / (values.length - 1)) * chartWidth;
    const y = TREND_BOTTOM - (value / maxValue) * chartHeight;
    return { x, y, value };
  });
}

function pathFromPoints(points: TrendPoint[]): string {
  return points.map((point, index) => `${index === 0 ? "M" : "L"} ${point.x} ${point.y}`).join(" ");
}

function areaPathFromPoints(points: TrendPoint[]): string {
  if (points.length === 0) return "";
  return `${pathFromPoints(points)} L ${points[points.length - 1]!.x} ${TREND_BOTTOM} L ${points[0]!.x} ${TREND_BOTTOM} Z`;
}

function ProjectTrendPanel(props: ProjectTrendPanelProps) {
  const { t } = useI18n();
  const [metric, setMetric] = useState<TrendMetric>("tokens");
  const [dimension, setDimension] = useState<TrendDimension>("total");
  const [activeIndex, setActiveIndex] = useState<number | null>(null);
  const plotRef = useRef<HTMLButtonElement | null>(null);
  const svgRef = useRef<SVGSVGElement | null>(null);
  const [chartWidth, setChartWidth] = useState(960);
  const trendData = useMemo(
    () =>
      buildTrendData(
        props.days ?? [],
        props.rangeDays,
        props.customRange,
        metric,
        dimension,
        props.providerInfo,
        props.formatModelName,
      ),
    [props.days, props.rangeDays, props.customRange, metric, dimension, props.providerInfo, props.formatModelName],
  );

  useLayoutEffect(() => {
    const node = plotRef.current;
    if (!node) return;
    const resize = () => {
      const next = Math.max(TREND_MIN_WIDTH, Math.round(node.getBoundingClientRect().width));
      setChartWidth((current) => (current === next ? current : next));
    };
    resize();
    const frame = requestAnimationFrame(resize);
    const observer = new ResizeObserver(resize);
    observer.observe(node);
    window.addEventListener("resize", resize);
    return () => {
      cancelAnimationFrame(frame);
      observer.disconnect();
      window.removeEventListener("resize", resize);
    };
  }, []);

  const activeIndexSafe =
    activeIndex !== null && activeIndex >= 0 && activeIndex < trendData.dates.length ? activeIndex : null;
  const activeTotals = activeIndexSafe !== null ? (trendData.totals[activeIndexSafe] ?? null) : null;
  const activeValue = activeTotals ? totalsMetricValue(activeTotals, metric) : trendData.total;
  const seriesPoints = trendData.series.map((seriesItem) => ({
    series: seriesItem,
    points: pointsForSeries(seriesItem.values, chartWidth, trendData.maxValue),
  }));
  const activeX =
    activeIndexSafe !== null && trendData.dates.length > 0
      ? trendData.dates.length <= 1
        ? TREND_LEFT + (Math.max(TREND_LEFT + 1, chartWidth - TREND_RIGHT_GUTTER) - TREND_LEFT) / 2
        : TREND_LEFT +
          (activeIndexSafe / (trendData.dates.length - 1)) *
            (Math.max(TREND_LEFT + 1, chartWidth - TREND_RIGHT_GUTTER) - TREND_LEFT)
      : null;
  const xTickIndexes = makeTickIndexes(trendData.dates.length);
  const chartRight = Math.max(TREND_LEFT + 1, chartWidth - TREND_RIGHT_GUTTER);
  const chartHeight = TREND_BOTTOM - TREND_TOP;
  const totalAreaPath = dimension === "total" && seriesPoints[0] ? areaPathFromPoints(seriesPoints[0].points) : "";
  const metricOptions: { value: TrendMetric; label: string }[] = [
    { value: "tokens", label: t("usage.tokens") },
    { value: "cost", label: t("usage.cost") },
  ];
  const dimensionOptions: { value: TrendDimension; label: string }[] = [
    { value: "total", label: t("usage.folderTrendTotal") },
    { value: "provider", label: t("usage.folderTrendProvider") },
    { value: "model", label: t("usage.folderTrendModel") },
  ];

  function updateActiveIndexFromPointer(event: PointerEvent<HTMLButtonElement>) {
    if (trendData.dates.length === 0) return;
    const measuredWidth = Math.round(event.currentTarget.getBoundingClientRect().width);
    if (measuredWidth > 0 && measuredWidth !== chartWidth) {
      setChartWidth(Math.max(TREND_MIN_WIDTH, measuredWidth));
    }
    const svg = svgRef.current;
    let svgX: number | null = null;
    const matrix = svg?.getScreenCTM();
    if (svg && matrix) {
      const point = svg.createSVGPoint();
      point.x = event.clientX;
      point.y = event.clientY;
      svgX = point.matrixTransform(matrix.inverse()).x;
    }
    if (svgX === null) {
      const rect = event.currentTarget.getBoundingClientRect();
      if (rect.width <= 0) return;
      svgX = ((event.clientX - rect.left) / rect.width) * chartWidth;
    }
    const ratio = clamp((svgX - TREND_LEFT) / (chartRight - TREND_LEFT), 0, 1);
    const index = trendData.dates.length <= 1 ? 0 : Math.round(ratio * (trendData.dates.length - 1));
    setActiveIndex(index);
  }
  const activeBreakdown =
    activeIndexSafe === null
      ? []
      : trendData.series
          .map((series) => ({
            id: series.id,
            label: dimension === "total" ? t("usage.folderTrendTotal") : series.label,
            color: series.color,
            value: series.values[activeIndexSafe] ?? 0,
          }))
          .filter((entry) => entry.value > 0)
          .slice(0, 4);
  const dimensionLabel =
    dimensionOptions.find((option) => option.value === dimension)?.label ?? t("usage.folderTrendTotal");

  return (
    <section className="usage-card usage-chart-card folder-detail-panel folder-trend-panel">
      <div className="usage-section-header">
        <div className="usage-section-title-row">
          <div className="usage-chart-heading">
            <div className="usage-section-title">
              <TrendingUp className="size-3.5" aria-hidden="true" />
              {t("usage.folderTrend")}
            </div>
            <div className="usage-section-subtitle">{props.activeRangeLabel}</div>
            <div className="folder-trend-controls">
              <ToggleGroup
                className="usage-metric-toggle folder-trend-toggle"
                size="sm"
                spacing={0}
                value={[metric]}
                onValueChange={(next) => {
                  const value = next[0];
                  if (isTrendMetric(value)) {
                    setMetric(value);
                    setActiveIndex(null);
                  }
                }}
              >
                {metricOptions.map((option) => (
                  <ToggleGroupItem
                    key={option.value}
                    value={option.value}
                    className={cn("usage-metric-btn h-auto min-w-0", metric === option.value && "active")}
                  >
                    {option.label}
                  </ToggleGroupItem>
                ))}
              </ToggleGroup>
              <ToggleGroup
                className="usage-metric-toggle folder-trend-toggle"
                size="sm"
                spacing={0}
                value={[dimension]}
                onValueChange={(next) => {
                  const value = next[0];
                  if (isTrendDimension(value)) {
                    setDimension(value);
                    setActiveIndex(null);
                  }
                }}
              >
                {dimensionOptions.map((option) => (
                  <ToggleGroupItem
                    key={option.value}
                    value={option.value}
                    className={cn("usage-metric-btn h-auto min-w-0", dimension === option.value && "active")}
                  >
                    {option.label}
                  </ToggleGroupItem>
                ))}
              </ToggleGroup>
            </div>
          </div>
        </div>
        <div className="usage-chart-inspector folder-trend-summary">
          {activeTotals ? (
            <>
              <div className="usage-chart-inspector-date">{activeTotals.date}</div>
              <div className="usage-chart-inspector-total">{formatTrendValue(metric, activeValue)}</div>
              <div className="usage-chart-inspector-breakdown">
                {activeBreakdown.map((entry) => (
                  <span key={entry.id} className="usage-chart-inspector-item">
                    <span className="usage-provider-dot" style={{ background: entry.color }} />
                    {entry.label}
                    <strong>{formatTrendValue(metric, entry.value)}</strong>
                  </span>
                ))}
              </div>
            </>
          ) : (
            <div className="usage-chart-hint">{t("usage.hoverHint")}</div>
          )}
        </div>
      </div>

      {props.loading ? (
        <div className="folder-detail-muted">{t("usage.loadingTrend")}</div>
      ) : props.error ? (
        <div className="folder-detail-muted">{props.error}</div>
      ) : trendData.dates.length === 0 || trendData.total === 0 || trendData.series.length === 0 ? (
        <div className="folder-detail-muted">{t("usage.noTrendData")}</div>
      ) : (
        <div className="folder-trend-chart">
          <div className="folder-trend-plot-card">
            <button
              ref={plotRef}
              type="button"
              className="folder-trend-plot"
              aria-label={t("usage.folderTrend")}
              onBlur={() => setActiveIndex(null)}
              onKeyDown={(event) => {
                if (trendData.dates.length === 0) return;
                if (event.key === "ArrowLeft") {
                  event.preventDefault();
                  setActiveIndex((current) => Math.max(0, (current ?? trendData.dates.length - 1) - 1));
                } else if (event.key === "ArrowRight") {
                  event.preventDefault();
                  setActiveIndex((current) =>
                    Math.min(trendData.dates.length - 1, (current ?? trendData.dates.length - 1) + 1),
                  );
                }
              }}
              onPointerDown={updateActiveIndexFromPointer}
              onPointerLeave={() => setActiveIndex(null)}
              onPointerMove={updateActiveIndexFromPointer}
            >
              <svg ref={svgRef} viewBox={`0 0 ${chartWidth} ${TREND_HEIGHT}`} aria-hidden="true">
                <defs>
                  <linearGradient id="folder-trend-fill" x1="0" x2="0" y1="0" y2="1">
                    <stop offset="0%" stopColor="var(--accent)" stopOpacity="0.18" />
                    <stop offset="100%" stopColor="var(--accent)" stopOpacity="0" />
                  </linearGradient>
                </defs>
                {[1, 0.75, 0.5, 0.25, 0].map((ratio) => {
                  const y = TREND_BOTTOM - ratio * chartHeight;
                  return (
                    <g key={ratio}>
                      <line className="folder-trend-grid" x1={TREND_LEFT} x2={chartRight} y1={y} y2={y} />
                      <text className="folder-trend-tick" x={TREND_LEFT - 10} y={y + 3} textAnchor="end">
                        {ratio === 0 ? "0" : formatTrendAxisValue(metric, trendData.maxValue * ratio)}
                      </text>
                    </g>
                  );
                })}
                {xTickIndexes.map((index) => {
                  const date = trendData.dates[index]!;
                  const x =
                    trendData.dates.length <= 1
                      ? TREND_LEFT + (chartRight - TREND_LEFT) / 2
                      : TREND_LEFT + (index / (trendData.dates.length - 1)) * (chartRight - TREND_LEFT);
                  return (
                    <text key={date} className="folder-trend-x-label" x={x} y={TREND_HEIGHT - 18} textAnchor="middle">
                      {date.slice(5)}
                    </text>
                  );
                })}
                {totalAreaPath && <path className="folder-trend-area" d={totalAreaPath} />}
                {seriesPoints.map(({ series, points }, index) => (
                  <path
                    key={series.id}
                    className={cn("folder-trend-line", index > 0 && "is-secondary")}
                    d={pathFromPoints(points)}
                    pathLength={1}
                    style={{ "--folder-trend-color": series.color } as CSSProperties}
                  />
                ))}
                {activeX !== null && (
                  <g className="folder-trend-marker">
                    <line x1={activeX} x2={activeX} y1={TREND_TOP} y2={TREND_BOTTOM} />
                    {seriesPoints.map(({ series, points }) => {
                      const point = activeIndexSafe !== null ? points[activeIndexSafe] : null;
                      if (!point || point.value <= 0) return null;
                      return (
                        <circle
                          key={series.id}
                          cx={point.x}
                          cy={point.y}
                          r="4.5"
                          style={{ "--folder-trend-color": series.color } as CSSProperties}
                        />
                      );
                    })}
                  </g>
                )}
              </svg>
            </button>
          </div>
          <aside className="folder-trend-side">
            <div className="folder-panel-heading">
              <span>{dimensionLabel}</span>
              <small>{formatTrendValue(metric, trendData.total)}</small>
            </div>
            <div className="folder-trend-legend">
              {trendData.series.map((series) => (
                <span key={series.id} className="folder-trend-legend-item">
                  <span className="folder-trend-legend-dot" style={{ background: series.color }} aria-hidden="true" />
                  <span className="folder-trend-legend-label">
                    {dimension === "total" ? t("usage.folderTrendTotal") : series.label}
                  </span>
                  <strong>{formatTrendValue(metric, series.total)}</strong>
                </span>
              ))}
            </div>
          </aside>
        </div>
      )}
    </section>
  );
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
        danger={true}
      />
    </div>
  );
}
