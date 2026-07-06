import { useI18n } from "@/i18n/index";
import {
  fmtCost,
  fmtTrend,
  trendClass,
} from "@/components/UsagePanel/formatters";

export interface SummaryStatItem {
  label: string;
  value: string;
  trend: number | null;
}

export interface TokenBreakdownItem {
  label: string;
  value: string;
  share: string;
}

export interface SummaryCardsProps {
  totalCost: number;
  totalCostTrend: number | null;
  summaryStats: SummaryStatItem[];
  tokenBreakdown: TokenBreakdownItem[];
}

export function SummaryCards(props: SummaryCardsProps) {
  const { t } = useI18n();

  return (
    <section className="usage-card usage-summary-card">
      <div className="usage-summary-main">
        <div className="usage-summary-hero">
          <span className="usage-overline">{t("usage.estCost")}</span>
          <div className="usage-cost-row">
            <div className="usage-cost-hero">{fmtCost(props.totalCost)}</div>
            {props.totalCostTrend !== null && (
              <span
                className={`usage-trend ${trendClass(props.totalCostTrend, true)}`}
              >
                {fmtTrend(props.totalCostTrend)}
              </span>
            )}
          </div>
          <div className="usage-cost-detail">{t("usage.pricingNote")}</div>
        </div>

        <div className="usage-summary-kpis">
          {props.summaryStats.map((item) => (
            <div key={item.label} className="usage-summary-stat">
              <span className="usage-kpi-label">{item.label}</span>
              <strong className="usage-kpi-value">{item.value}</strong>
              <span className="usage-kpi-sub">
                {item.trend !== null ? (
                  <span className={`usage-trend ${trendClass(item.trend)}`}>
                    {fmtTrend(item.trend)}
                  </span>
                ) : (
                  " "
                )}
              </span>
            </div>
          ))}
        </div>
      </div>

      <div className="usage-breakdown-grid">
        {props.tokenBreakdown.map((item) => (
          <div key={item.label} className="usage-breakdown-item">
            <span className="usage-breakdown-label">{item.label}</span>
            <strong className="usage-breakdown-value">{item.value}</strong>
            <span className="usage-breakdown-pct">{item.share}</span>
          </div>
        ))}
      </div>

      <div className="usage-summary-notes">
        <span className="usage-note-pill">
          {t("usage.rebuildKeepsSessions")}
        </span>
        <span className="usage-note-pill">{t("usage.pricingSourceNote")}</span>
      </div>
    </section>
  );
}
