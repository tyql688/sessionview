import { useI18n } from "@/i18n/index";
import type {
  ChartMetric,
  HoveredDaySummary,
  UsageDailyChartData,
} from "@/lib/usage";
import type { ProviderChipInfo } from "@/features/usage/Toolbar";

export interface ChartProps {
  dailyChartData: UsageDailyChartData;
  hoveredDate: string | null;
  setHoveredDate: (date: string | null) => void;
  hoveredDaySummary: HoveredDaySummary | null;
  chartMetric: ChartMetric;
  setChartMetric: (metric: ChartMetric) => void;
  activeRangeLabel: string;
  fmtChartValue: (n: number) => string;
  providerInfo: (key: string) => ProviderChipInfo;
}

export function Chart(props: ChartProps) {
  const { t } = useI18n();
  const summary = props.hoveredDaySummary;

  return (
    <section className="usage-card usage-chart-card">
      <div className="usage-section-header">
        <div className="usage-section-title-row">
          <div className="usage-chart-heading">
            <div className="usage-section-title">{t("usage.dailyUsage")}</div>
            <div className="usage-section-subtitle">
              {props.activeRangeLabel}
            </div>
            <div className="usage-metric-toggle">
              <button
                className={`usage-metric-btn${props.chartMetric === "tokens" ? " active" : ""}`}
                aria-pressed={props.chartMetric === "tokens"}
                onClick={() => props.setChartMetric("tokens")}
                type="button"
              >
                {t("usage.tokens")}
              </button>
              <button
                className={`usage-metric-btn${props.chartMetric === "cost" ? " active" : ""}`}
                aria-pressed={props.chartMetric === "cost"}
                onClick={() => props.setChartMetric("cost")}
                type="button"
              >
                {t("usage.cost")}
              </button>
            </div>
          </div>
        </div>
        <div className="usage-chart-inspector">
          {summary ? (
            <>
              <div className="usage-chart-inspector-date">{summary.date}</div>
              <div className="usage-chart-inspector-total">
                {props.fmtChartValue(summary.total)}
              </div>
              <div className="usage-chart-inspector-breakdown">
                {summary.breakdown.map((entry, i) => (
                  <span key={i} className="usage-chart-inspector-item">
                    <span
                      className="usage-provider-dot"
                      style={{ background: entry.color }}
                    />
                    {entry.label}
                    <strong>{props.fmtChartValue(entry.value)}</strong>
                  </span>
                ))}
              </div>
            </>
          ) : (
            <div className="usage-chart-hint">{t("usage.hoverHint")}</div>
          )}
        </div>
      </div>

      {props.dailyChartData.dates.length > 0 && (
        <>
          <div className="usage-chart-wrap">
            <div className="usage-daily-bars">
              {props.dailyChartData.dates.map((date) => {
                const providers = props.dailyChartData.byDate.get(date)!;
                const max = props.dailyChartData.maxValue;
                const active = props.hoveredDate === date;
                return (
                  <button
                    key={date}
                    className={`usage-bar-col${active ? " active" : ""}`}
                    onBlur={() => props.setHoveredDate(null)}
                    onFocus={() => props.setHoveredDate(date)}
                    onMouseEnter={() => props.setHoveredDate(date)}
                    onMouseLeave={() => props.setHoveredDate(null)}
                    title={`${date} · ${props.fmtChartValue(
                      [...providers.values()].reduce(
                        (sum, value) => sum + value,
                        0,
                      ),
                    )}`}
                    type="button"
                  >
                    {props.dailyChartData.providers
                      .slice()
                      .reverse()
                      .map((provider) => {
                        const val = providers.get(provider) ?? 0;
                        const color = props.providerInfo(provider).color;
                        return val > 0 ? (
                          <span
                            key={provider}
                            className={`usage-bar-seg${
                              props.hoveredDate && !active
                                ? " usage-bar-seg-muted"
                                : ""
                            }`}
                            style={{
                              height: `${Math.max(4, (val / max) * 100)}%`,
                              background: color,
                            }}
                          />
                        ) : null;
                      })}
                  </button>
                );
              })}
            </div>
            <div className="usage-bar-labels">
              {props.dailyChartData.dates.map((date) => (
                <span
                  key={date}
                  className={props.hoveredDate === date ? "active" : undefined}
                >
                  {date.slice(5)}
                </span>
              ))}
            </div>
          </div>

          <div className="usage-legend">
            {props.dailyChartData.providers.map((provider) => (
              <span key={provider} className="usage-legend-item">
                <span
                  className="usage-provider-dot"
                  style={{
                    background: props.providerInfo(provider).color,
                  }}
                />
                {props.providerInfo(provider).label}
              </span>
            ))}
          </div>
        </>
      )}
    </section>
  );
}
