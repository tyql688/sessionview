import { useEffect, useMemo, useState, type CSSProperties } from "react";
import {
  Activity,
  ChartColumn,
  CircleAlert,
  Clock3,
  Gauge,
  Loader2,
  PieChart,
  TrendingUp,
  Wrench,
  type LucideIcon,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { ToolKindGlyph } from "@/features/session/ToolGlyph";
import {
  buildSessionAnalytics,
  type SessionAnalytics,
  type TokenTimelineBucket,
} from "@/features/session/sessionAnalytics";
import { useI18n } from "@/i18n/index";
import { errorMessage } from "@/lib/errors";
import { formatDuration, formatTimeOnly } from "@/lib/formatters";
import { getSessionDetail, isLoadCanceledError } from "@/lib/tauri";
import type { Message, SessionMeta } from "@/lib/types";
import { cn } from "@/lib/utils";
import { toastError } from "@/stores/toast";

interface SessionAnalyticsDialogProps {
  open: boolean;
  sessionId: string;
  meta: SessionMeta;
  onOpenChange: (open: boolean) => void;
}

interface StatTileProps {
  icon: LucideIcon;
  label: string;
  value: string;
  detail: string;
  tone: "blue" | "green" | "amber" | "pink";
}

interface InsightItemProps {
  icon: LucideIcon;
  label: string;
  value: string;
  detail: string;
}

interface ToolListProps {
  analytics: SessionAnalytics;
}

interface TokenTimelineProps {
  buckets: TokenTimelineBucket[];
}

interface AnalyticsContentProps {
  analytics: SessionAnalytics;
}

const TOOL_COLORS = [
  "var(--accent)",
  "var(--success)",
  "var(--warning-amber)",
  "var(--accent-secondary)",
  "var(--danger-strong)",
  "var(--codex)",
  "var(--opencode)",
  "var(--cc-mirror)",
];

function percent(value: number): string {
  return `${Math.round(value * 100)}%`;
}

function formatRatio(value: number | null): string {
  if (value === null) return "—";
  return value >= 10 ? value.toFixed(0) : value.toFixed(1);
}

function trimFixed(value: string): string {
  return value.replace(/\.0$/, "");
}

function formatUnit(value: number, divisor: number, suffix: string, decimals: number): string {
  return `${trimFixed((value / divisor).toFixed(decimals))}${suffix}`;
}

function formatAnalyticsTokens(value: number, locale: string): string {
  const rounded = Math.max(0, Math.round(value));
  if (locale.startsWith("zh")) {
    if (rounded >= 1_0000_0000_0000) return formatUnit(rounded, 1_0000_0000_0000, "万亿", 1);
    if (rounded >= 1_0000_0000) return formatUnit(rounded, 1_0000_0000, "亿", 1);
    if (rounded >= 1_0000) return formatUnit(rounded, 1_0000, "万", rounded >= 100_0000 ? 0 : 1);
    return rounded.toLocaleString("zh-CN");
  }

  if (rounded >= 1_000_000_000_000) return formatUnit(rounded, 1_000_000_000_000, "T", 1);
  if (rounded >= 1_000_000_000) return formatUnit(rounded, 1_000_000_000, "B", 1);
  if (rounded >= 1_000_000) return formatUnit(rounded, 1_000_000, "M", 1);
  if (rounded >= 1_000) return formatUnit(rounded, 1_000, "K", 1);
  return rounded.toLocaleString("en-US");
}

function formatMaybeTokens(value: number | null, locale: string): string {
  return value === null ? "—" : formatAnalyticsTokens(value, locale);
}

function timeRangeLabel(start: number | null, end: number | null): string {
  if (start === null && end === null) return "—";
  if (start !== null && end !== null && start !== end) {
    return `${formatTimeOnly(start)} – ${formatTimeOnly(end)}`;
  }
  const only = start ?? end;
  return only === null ? "—" : formatTimeOnly(only);
}

function durationLabel(analytics: SessionAnalytics): string {
  if (analytics.firstTimestamp === null || analytics.lastTimestamp === null) return "—";
  const duration = analytics.lastTimestamp - analytics.firstTimestamp;
  return duration > 0 ? formatDuration(duration) : "< 1 min";
}

function StatTile(props: StatTileProps) {
  const Icon = props.icon;
  return (
    <div className={cn("session-analytics-stat", `session-analytics-stat-${props.tone}`)}>
      <span className="session-analytics-stat-icon">
        <Icon className="size-4" aria-hidden="true" />
      </span>
      <span className="session-analytics-stat-label">{props.label}</span>
      <strong className="session-analytics-stat-value">{props.value}</strong>
      <span className="session-analytics-stat-detail">{props.detail}</span>
    </div>
  );
}

function InsightItem(props: InsightItemProps) {
  const Icon = props.icon;
  return (
    <div className="session-analytics-insight">
      <span className="session-analytics-insight-icon">
        <Icon className="size-3.5" aria-hidden="true" />
      </span>
      <div className="session-analytics-insight-copy">
        <span>{props.label}</span>
        <strong>{props.value}</strong>
        <small>{props.detail}</small>
      </div>
    </div>
  );
}

function ToolList(props: ToolListProps) {
  const { t } = useI18n();
  const visible = props.analytics.toolDistribution.slice(0, 7);
  const hidden = props.analytics.toolDistribution.slice(7);
  const hiddenCount = hidden.reduce((sum, item) => sum + item.count, 0);
  const items =
    hiddenCount > 0
      ? [
          ...visible,
          {
            key: "other-tools",
            label: t("analytics.otherTools"),
            count: hiddenCount,
            share: hiddenCount / props.analytics.toolCalls,
            category: "tool",
            canonicalName: "Tool",
          },
        ]
      : visible;

  if (items.length === 0) {
    return <div className="session-analytics-empty">{t("analytics.noTools")}</div>;
  }

  return (
    <div className="session-analytics-tool-list">
      {items.map((item, index) => {
        const color = TOOL_COLORS[index % TOOL_COLORS.length];
        const style = {
          "--tool-color": color,
          "--tool-share": `${Math.max(3, item.share * 100)}%`,
        } as CSSProperties;
        return (
          <div className="session-analytics-tool-row" key={item.key} style={style}>
            <span className="session-analytics-tool-icon">
              <ToolKindGlyph category={item.category} canonicalName={item.canonicalName} className="size-3.5" />
            </span>
            <span className="session-analytics-tool-name">{item.label}</span>
            <span className="session-analytics-tool-bar" aria-hidden="true">
              <span />
            </span>
            <strong>{item.count.toLocaleString()}</strong>
            <small>{percent(item.share)}</small>
          </div>
        );
      })}
    </div>
  );
}

function TokenTimeline(props: TokenTimelineProps) {
  const { t, locale } = useI18n();
  const [activeBucketIndex, setActiveBucketIndex] = useState<number | null>(null);
  const [hoverBucketIndex, setHoverBucketIndex] = useState<number | null>(null);
  const width = 640;
  const height = 226;
  const chartTop = 18;
  const chartBottom = 170;
  const chartLeft = 48;
  const chartRight = 622;
  const chartHeight = chartBottom - chartTop;
  const chartWidth = chartRight - chartLeft;
  const maxValue = Math.max(...props.buckets.map((bucket) => bucket.total), 1);
  const maxCumulative = Math.max(...props.buckets.map((bucket) => bucket.cumulative), 1);
  const step = chartWidth / Math.max(props.buckets.length, 1);
  const barWidth = Math.max(3, Math.min(13, step * 0.56));
  const activeIndex =
    props.buckets.length === 0 ? -1 : Math.min(activeBucketIndex ?? props.buckets.length - 1, props.buckets.length - 1);
  const activeBucket = activeIndex >= 0 ? (props.buckets[activeIndex] ?? null) : null;
  const activeX = activeBucket ? chartLeft + step * activeIndex + step / 2 : null;
  const activeLineY = activeBucket ? chartBottom - (activeBucket.cumulative / maxCumulative) * chartHeight : null;
  const yTicks = [1, 0.5, 0].map((ratio) => ({
    key: ratio,
    label: ratio === 0 ? "0" : formatAnalyticsTokens(maxValue * ratio, locale),
    y: chartBottom - ratio * chartHeight,
  }));
  const xTickIndexes = [...new Set([0, Math.floor((props.buckets.length - 1) / 2), props.buckets.length - 1])].filter(
    (index) => index >= 0,
  );
  const linePoints = props.buckets
    .map((bucket, index) => {
      const x = chartLeft + step * index + step / 2;
      const y = chartBottom - (bucket.cumulative / maxCumulative) * chartHeight;
      return `${x.toFixed(2)},${y.toFixed(2)}`;
    })
    .join(" ");

  if (props.buckets.length === 0) {
    return <div className="session-analytics-empty">{t("analytics.noTokens")}</div>;
  }

  return (
    <div className="session-analytics-token-chart">
      <div className="session-analytics-token-stage">
        <svg viewBox={`0 0 ${width} ${height}`} role="img" aria-label={t("analytics.tokenTimeline")}>
          <defs>
            <linearGradient id="session-token-fill" x1="0" x2="0" y1="0" y2="1">
              <stop offset="0%" stopColor="var(--accent)" stopOpacity="0.22" />
              <stop offset="100%" stopColor="var(--accent)" stopOpacity="0.02" />
            </linearGradient>
          </defs>
          <line className="session-analytics-axis" x1={chartLeft} x2={chartRight} y1={chartBottom} y2={chartBottom} />
          {yTicks.map((tick) => (
            <g key={tick.key}>
              <line className="session-analytics-grid" x1={chartLeft} x2={chartRight} y1={tick.y} y2={tick.y} />
              <text className="session-analytics-tick-label" textAnchor="end" x={chartLeft - 8} y={tick.y + 3}>
                {tick.label}
              </text>
            </g>
          ))}
          {xTickIndexes.map((index) => {
            const bucket = props.buckets[index];
            if (!bucket) return null;
            const x = chartLeft + step * index + step / 2;
            return (
              <g key={bucket.key}>
                <line className="session-analytics-x-tick" x1={x} x2={x} y1={chartBottom} y2={chartBottom + 5} />
                <text className="session-analytics-x-label" textAnchor="middle" x={x} y={chartBottom + 20}>
                  {timeRangeLabel(bucket.startTime, bucket.endTime)}
                </text>
              </g>
            );
          })}
          <polyline
            className="session-analytics-token-area"
            points={`${chartLeft},${chartBottom} ${linePoints} ${chartRight},${chartBottom}`}
          />
          {props.buckets.map((bucket, index) => {
            const x = chartLeft + step * index + (step - barWidth) / 2;
            let y = chartBottom;
            const scale = chartHeight / maxValue;
            const inputHeight = bucket.input > 0 ? Math.max(1, bucket.input * scale) : 0;
            const outputHeight = bucket.output > 0 ? Math.max(1, bucket.output * scale) : 0;
            const cacheHeight =
              bucket.cacheRead + bucket.cacheWrite > 0
                ? Math.max(1, (bucket.cacheRead + bucket.cacheWrite) * scale)
                : 0;
            y -= cacheHeight;
            const cacheY = y;
            y -= outputHeight;
            const outputY = y;
            y -= inputHeight;
            const inputY = y;
            const title = `${timeRangeLabel(bucket.startTime, bucket.endTime)} · ${formatAnalyticsTokens(bucket.total, locale)}`;
            const bucketStyle = {
              "--bucket-delay": `${Math.min(index * 14, 360)}ms`,
            } as CSSProperties;
            return (
              <g
                className={cn(
                  "session-analytics-token-bucket",
                  index === activeIndex && "is-active",
                  index === hoverBucketIndex && "is-hovered",
                )}
                key={bucket.key}
                style={bucketStyle}
              >
                <title>{title}</title>
                {inputHeight > 0 && (
                  <rect
                    className="session-analytics-token-input"
                    height={inputHeight}
                    rx="2"
                    width={barWidth}
                    x={x}
                    y={inputY}
                  />
                )}
                {outputHeight > 0 && (
                  <rect
                    className="session-analytics-token-output"
                    height={outputHeight}
                    rx="2"
                    width={barWidth}
                    x={x}
                    y={outputY}
                  />
                )}
                {cacheHeight > 0 && (
                  <rect
                    className="session-analytics-token-cache"
                    height={cacheHeight}
                    rx="2"
                    width={barWidth}
                    x={x}
                    y={cacheY}
                  />
                )}
              </g>
            );
          })}
          <polyline className="session-analytics-token-line" pathLength={1} points={linePoints} />
          {activeX !== null && activeLineY !== null && (
            <g className="session-analytics-token-marker">
              <line x1={activeX} x2={activeX} y1={chartTop} y2={chartBottom} />
              <circle cx={activeX} cy={activeLineY} r="4" />
            </g>
          )}
        </svg>
        <div className="session-analytics-token-hotspots" aria-hidden="false">
          {props.buckets.map((bucket, index) => {
            const title = `${timeRangeLabel(bucket.startTime, bucket.endTime)} · ${formatAnalyticsTokens(bucket.total, locale)}`;
            const hotspotStyle = {
              "--bucket-left": `${((chartLeft + step * index) / width) * 100}%`,
              "--bucket-width": `${(step / width) * 100}%`,
              "--bucket-top": `${(chartTop / height) * 100}%`,
              "--bucket-height": `${(chartHeight / height) * 100}%`,
            } as CSSProperties;
            return (
              <button
                aria-label={title}
                aria-pressed={index === activeIndex}
                className="session-analytics-token-hotspot"
                key={bucket.key}
                onBlur={() => setHoverBucketIndex(null)}
                onClick={() => setActiveBucketIndex(index)}
                onFocus={() => setHoverBucketIndex(index)}
                onMouseEnter={() => setHoverBucketIndex(index)}
                onMouseLeave={() => setHoverBucketIndex(null)}
                style={hotspotStyle}
                type="button"
              />
            );
          })}
        </div>
      </div>
      {activeBucket && (
        <div className="session-analytics-token-detail">
          <span>
            <small>{t("analytics.selectedWindow")}</small>
            <strong>{timeRangeLabel(activeBucket.startTime, activeBucket.endTime)}</strong>
          </span>
          <span>
            <small>{t("analytics.totalInWindow")}</small>
            <strong>{formatAnalyticsTokens(activeBucket.total, locale)}</strong>
          </span>
          <span>
            <small>{t("common.inputTokens")}</small>
            <strong>{formatAnalyticsTokens(activeBucket.input, locale)}</strong>
          </span>
          <span>
            <small>{t("common.outputTokens")}</small>
            <strong>{formatAnalyticsTokens(activeBucket.output, locale)}</strong>
          </span>
          <span>
            <small>{t("analytics.cacheTokens")}</small>
            <strong>{formatAnalyticsTokens(activeBucket.cacheRead + activeBucket.cacheWrite, locale)}</strong>
          </span>
          <span>
            <small>{t("analytics.cumulativeAfter")}</small>
            <strong>{formatAnalyticsTokens(activeBucket.cumulative, locale)}</strong>
          </span>
        </div>
      )}
    </div>
  );
}

function AnalyticsContent(props: AnalyticsContentProps) {
  const { t, locale } = useI18n();
  const analytics = props.analytics;
  const topTool = analytics.toolDistribution[0] ?? null;
  const peakPoint = analytics.peakTokenPoint;
  const messageTotal =
    analytics.roleCounts.user +
    analytics.roleCounts.assistant +
    analytics.roleCounts.tool +
    analytics.roleCounts.system;
  const cacheShare = analytics.cacheShare;

  return (
    <>
      <div className="session-analytics-stat-grid">
        <StatTile
          detail={t("analytics.messagesAnalyzed", { count: messageTotal })}
          icon={Activity}
          label={t("analytics.totalTokens")}
          tone="blue"
          value={formatAnalyticsTokens(analytics.tokenTotals.total, locale)}
        />
        <StatTile
          detail={t("analytics.toolTypesDetail", { count: analytics.toolTypes })}
          icon={Wrench}
          label={t("analytics.toolCalls")}
          tone="green"
          value={analytics.toolCalls.toLocaleString()}
        />
        <StatTile
          detail={t("analytics.sessionSpanDetail")}
          icon={Clock3}
          label={t("analytics.sessionSpan")}
          tone="amber"
          value={durationLabel(analytics)}
        />
        <StatTile
          detail={t("analytics.perAssistantTurn")}
          icon={Gauge}
          label={t("analytics.avgTokens")}
          tone="pink"
          value={formatMaybeTokens(analytics.averageTokensPerAssistantTurn, locale)}
        />
      </div>

      <div className="session-analytics-main-grid">
        <section className="session-analytics-panel">
          <div className="session-analytics-panel-header">
            <div>
              <h3>{t("analytics.toolDistribution")}</h3>
              <p>{t("analytics.toolDistributionDesc")}</p>
            </div>
            <PieChart className="size-4" aria-hidden="true" />
          </div>
          <ToolList analytics={analytics} />
        </section>

        <section className="session-analytics-panel session-analytics-panel-wide">
          <div className="session-analytics-panel-header">
            <div>
              <h3>{t("analytics.tokenTimeline")}</h3>
              <p>{t("analytics.tokenTimelineDesc")}</p>
            </div>
            <TrendingUp className="size-4" aria-hidden="true" />
          </div>
          <TokenTimeline buckets={analytics.tokenBuckets} />
          <div className="session-analytics-token-legend">
            <span className="input">{t("common.inputTokens")}</span>
            <span className="output">{t("common.outputTokens")}</span>
            <span className="cache">{t("analytics.cacheTokens")}</span>
            <span className="line">{t("analytics.cumulative")}</span>
          </div>
        </section>
      </div>

      <section className="session-analytics-insights">
        <div className="session-analytics-panel-header">
          <div>
            <h3>{t("analytics.insights")}</h3>
            <p>{t("analytics.insightsDesc")}</p>
          </div>
          <ChartColumn className="size-4" aria-hidden="true" />
        </div>
        <div className="session-analytics-insight-grid">
          <InsightItem
            detail={
              topTool ? t("analytics.shareOfToolCalls", { share: percent(topTool.share) }) : t("analytics.noTools")
            }
            icon={Wrench}
            label={t("analytics.mostUsedTool")}
            value={topTool?.label ?? "—"}
          />
          <InsightItem
            detail={peakPoint ? timeRangeLabel(peakPoint.timestamp, peakPoint.timestamp) : t("analytics.noTokens")}
            icon={TrendingUp}
            label={t("analytics.peakTurn")}
            value={peakPoint ? formatAnalyticsTokens(peakPoint.total, locale) : "—"}
          />
          <InsightItem
            detail={t("analytics.cacheShareDetail")}
            icon={Gauge}
            label={t("analytics.cacheShare")}
            value={cacheShare === null ? "—" : percent(cacheShare)}
          />
          <InsightItem
            detail={t("analytics.toolsPerUserTurn")}
            icon={Activity}
            label={t("analytics.toolDensity")}
            value={formatRatio(analytics.toolsPerUserTurn)}
          />
        </div>
      </section>
    </>
  );
}

export function SessionAnalyticsDialog(props: SessionAnalyticsDialogProps) {
  const { t } = useI18n();
  const [messages, setMessages] = useState<Message[]>([]);
  const [meta, setMeta] = useState<SessionMeta>(props.meta);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!props.open) return;
    let disposed = false;
    setLoading(true);
    setError(null);
    setMessages([]);
    setMeta(props.meta);

    void getSessionDetail(props.sessionId)
      .then((detail) => {
        if (disposed) return;
        setMessages(detail.messages);
        setMeta(detail.meta);
      })
      .catch((err: unknown) => {
        if (disposed) return;
        const message = isLoadCanceledError(err) ? t("session.loadInterrupted") : errorMessage(err);
        console.error("load session analytics failed:", err);
        setError(message);
        toastError(t("analytics.loadFailed"));
      })
      .finally(() => {
        if (!disposed) setLoading(false);
      });

    return () => {
      disposed = true;
    };
  }, [props.open, props.sessionId, props.meta, t]);

  const analytics = useMemo(() => buildSessionAnalytics(messages, meta), [messages, meta]);

  return (
    <Dialog open={props.open} onOpenChange={props.onOpenChange}>
      <DialogContent
        className="session-analytics-dialog"
        overlayClassName="session-analytics-overlay"
        showCloseButton={true}
        unstyled={true}
      >
        <DialogHeader className="session-analytics-header">
          <div className="session-analytics-title-row">
            <div>
              <DialogTitle className="session-analytics-title">{t("analytics.title")}</DialogTitle>
              <DialogDescription className="session-analytics-subtitle">
                {t("analytics.subtitle", { title: meta.title })}
              </DialogDescription>
            </div>
          </div>
        </DialogHeader>

        <div className="session-analytics-body">
          {loading ? (
            <div className="session-analytics-loading">
              <Loader2 className="size-5 animate-spin" aria-hidden="true" />
              <span>{t("analytics.loading")}</span>
            </div>
          ) : error ? (
            <div className="session-analytics-error">
              <CircleAlert className="size-5" aria-hidden="true" />
              <span>{error}</span>
              <Button variant="outline" size="sm" onClick={() => props.onOpenChange(false)}>
                {t("common.close")}
              </Button>
            </div>
          ) : (
            <AnalyticsContent analytics={analytics} />
          )}
        </div>
      </DialogContent>
    </Dialog>
  );
}
