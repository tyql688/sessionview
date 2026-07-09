import type { CSSProperties } from "react";
import { Button } from "@/components/ui/button";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { useI18n } from "@/i18n/index";
import type { ChartMetric, HoveredDaySummary, UsageDailyChartData } from "@/lib/usage";
import type { ProviderChipInfo } from "@/features/usage/Toolbar";
import { cn } from "@/lib/utils";

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
            <div className="usage-section-subtitle">{props.activeRangeLabel}</div>
            <ToggleGroup
              className="usage-metric-toggle"
              size="sm"
              spacing={0}
              value={[props.chartMetric]}
              onValueChange={(next) => {
                const value = next[0];
                if (value === "tokens" || value === "cost") {
                  props.setChartMetric(value);
                }
              }}
            >
              <ToggleGroupItem
                value="tokens"
                className={cn("usage-metric-btn h-auto min-w-0", props.chartMetric === "tokens" && "active")}
              >
                {t("usage.tokens")}
              </ToggleGroupItem>
              <ToggleGroupItem
                value="cost"
                className={cn("usage-metric-btn h-auto min-w-0", props.chartMetric === "cost" && "active")}
              >
                {t("usage.cost")}
              </ToggleGroupItem>
            </ToggleGroup>
          </div>
        </div>
        <div className="usage-chart-inspector">
          {summary ? (
            <>
              <div className="usage-chart-inspector-date">{summary.date}</div>
              <div className="usage-chart-inspector-total">{props.fmtChartValue(summary.total)}</div>
              <div className="usage-chart-inspector-breakdown">
                {summary.breakdown.map((entry, i) => (
                  <span key={i} className="usage-chart-inspector-item">
                    <span className="usage-provider-dot" style={{ background: entry.color }} />
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
                  <Button
                    key={date}
                    variant="ghost"
                    className={cn(
                      "usage-bar-col h-full items-stretch justify-start rounded-none p-0 active:translate-y-0",
                      active && "active",
                    )}
                    onBlur={() => props.setHoveredDate(null)}
                    onFocus={() => props.setHoveredDate(date)}
                    onMouseEnter={() => props.setHoveredDate(date)}
                    onMouseLeave={() => props.setHoveredDate(null)}
                    title={`${date} · ${props.fmtChartValue(
                      [...providers.values()].reduce((sum, value) => sum + value, 0),
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
                            className={`usage-bar-seg${props.hoveredDate && !active ? " usage-bar-seg-muted" : ""}`}
                            style={
                              {
                                "--usage-bar-color": color,
                                height: `${Math.max(4, (val / max) * 100)}%`,
                              } as CSSProperties
                            }
                          />
                        ) : null;
                      })}
                  </Button>
                );
              })}
            </div>
            <div className="usage-bar-labels">
              {props.dailyChartData.dates.map((date) => (
                <span key={date} className={props.hoveredDate === date ? "active" : undefined}>
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
