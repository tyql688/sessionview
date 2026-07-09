import { Activity, ChartColumn, CircleDollarSign, Hash, PieChart, type LucideIcon } from "lucide-react";
import { useI18n } from "@/i18n/index";
import { fmtCost, fmtTrend, trendClass } from "@/features/usage/formatters";
import { cn } from "@/lib/utils";

interface SummaryStatItem {
  label: string;
  value: string;
  trend: number | null;
}

interface TokenBreakdownItem {
  label: string;
  value: string;
  share: string;
  shareValue: number;
}

export interface SummaryCardsProps {
  totalCost: number;
  totalCostTrend: number | null;
  summaryStats: SummaryStatItem[];
  tokenBreakdown: TokenBreakdownItem[];
}

interface StatTileProps {
  icon: LucideIcon;
  label: string;
  value: string;
  trend: number | null;
  detail: string;
  tone: "blue" | "green" | "amber" | "pink";
  invertTrend?: boolean;
}

const SUMMARY_STAT_META: Array<{
  icon: LucideIcon;
  tone: StatTileProps["tone"];
}> = [
  { icon: PieChart, tone: "green" },
  { icon: Activity, tone: "amber" },
  { icon: Hash, tone: "pink" },
];

function StatTile(props: StatTileProps) {
  const Icon = props.icon;
  return (
    <div className={cn("usage-summary-stat", `usage-summary-stat-${props.tone}`)}>
      <span className="usage-summary-stat-icon">
        <Icon className="size-4" aria-hidden="true" />
      </span>
      <span className="usage-summary-stat-label">{props.label}</span>
      <strong className="usage-summary-stat-value">{props.value}</strong>
      <span className="usage-summary-stat-detail">
        {props.trend !== null ? (
          <span className={`usage-trend ${trendClass(props.trend, props.invertTrend)}`}>{fmtTrend(props.trend)}</span>
        ) : (
          props.detail
        )}
      </span>
    </div>
  );
}

export function SummaryCards(props: SummaryCardsProps) {
  const { t } = useI18n();
  const statTiles: StatTileProps[] = [
    {
      icon: CircleDollarSign,
      label: t("usage.estCost"),
      value: fmtCost(props.totalCost),
      trend: props.totalCostTrend,
      detail: t("usage.pricingNote"),
      tone: "blue",
      invertTrend: true,
    },
    ...props.summaryStats.map((item, index) => {
      const meta = SUMMARY_STAT_META[index] ?? { icon: ChartColumn, tone: "blue" as const };
      return {
        icon: meta.icon,
        label: item.label,
        value: item.value,
        trend: item.trend,
        detail: t("usage.currentRange"),
        tone: meta.tone,
      };
    }),
  ];

  return (
    <section className="usage-card usage-summary-card">
      <div className="usage-summary-stat-grid">
        {statTiles.map((item) => (
          <StatTile
            key={item.label}
            icon={item.icon}
            label={item.label}
            value={item.value}
            trend={item.trend}
            detail={item.detail}
            tone={item.tone}
            invertTrend={item.invertTrend}
          />
        ))}
      </div>

      <div className="usage-token-mix">
        <div className="usage-token-mix-header">
          <span>{t("usage.tokenMix")}</span>
          <small>{t("usage.currentRange")}</small>
        </div>
        <div className="usage-breakdown-grid">
          {props.tokenBreakdown.map((item) => (
            <div key={item.label} className="usage-breakdown-item">
              <span className="usage-breakdown-label">{item.label}</span>
              <strong className="usage-breakdown-value">{item.value}</strong>
              <span className="usage-breakdown-pct">{item.share}</span>
              <span className="usage-breakdown-bar" aria-hidden="true">
                <span style={{ width: `${Math.max(3, item.shareValue * 100)}%` }} />
              </span>
            </div>
          ))}
        </div>
      </div>

      <div className="usage-summary-notes">
        <span className="usage-note-pill">{t("usage.rebuildKeepsSessions")}</span>
        <span className="usage-note-pill">{t("usage.pricingSourceNote")}</span>
      </div>
    </section>
  );
}
