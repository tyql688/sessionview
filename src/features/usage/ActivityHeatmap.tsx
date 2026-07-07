import { type CSSProperties, useState } from "react";
import { CircleDollarSign, Hash } from "lucide-react";
import { Button } from "@/components/ui/button";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { useI18n } from "@/i18n/index";
import type {
  HeatmapCell,
  HeatmapGrid,
  HeatmapMetric,
} from "@/features/usage/heatmap";
import { fmtCost, fmtTokens } from "@/features/usage/formatters";
import { cn } from "@/lib/utils";

const METRICS: HeatmapMetric[] = ["tokens", "cost"];
const WEEKDAY_ROWS = [0, 1, 2, 3, 4, 5, 6];
const LEGEND_LEVELS = [0, 1, 2, 3, 4] as const;

export interface ActivityHeatmapProps {
  grid: HeatmapGrid;
  metric: HeatmapMetric;
  setMetric: (metric: HeatmapMetric) => void;
  year: number | null;
  setYear: (year: number | null) => void;
  availableYears: number[];
  loading: boolean;
}

export function ActivityHeatmap(props: ActivityHeatmapProps) {
  const { t, locale } = useI18n();
  const [hovered, setHovered] = useState<HeatmapCell | null>(null);

  const localeTag = locale === "zh" ? "zh-CN" : "en-US";

  const monthLabel = (month: number): string =>
    new Intl.DateTimeFormat(localeTag, { month: "short" }).format(
      new Date(2020, month - 1, 1),
    );

  // GitHub labels only Mon / Wed / Fri (rows 1, 3, 5). 2023-01-01 was a Sunday.
  const weekdayLabel = (row: number): string =>
    row % 2 === 1
      ? new Intl.DateTimeFormat(localeTag, { weekday: "short" }).format(
          new Date(2023, 0, 1 + row),
        )
      : "";

  const formatDate = (iso: string): string => {
    const [y, m, d] = iso.split("-").map(Number);
    return new Intl.DateTimeFormat(localeTag, {
      year: "numeric",
      month: "short",
      day: "numeric",
    }).format(new Date(y, m - 1, d));
  };

  const metricNoun = (metric: HeatmapMetric): string => {
    switch (metric) {
      case "sessions":
        return t("usage.sessions");
      case "turns":
        return t("usage.turns");
      case "tokens":
        return t("usage.tokens");
      case "cost":
        return t("usage.cost").toLowerCase();
    }
  };

  /** A value with its unit, e.g. "28 sessions", "12.3M tokens", or "$3.40". */
  const valueWithNoun = (metric: HeatmapMetric, value: number): string => {
    if (metric === "cost") return fmtCost(value);
    const text =
      metric === "tokens" ? fmtTokens(value) : value.toLocaleString();
    return `${text} ${metricNoun(metric)}`;
  };

  const timeframe =
    props.year === null
      ? t("usage.activityTrailing")
      : t("usage.activityInYear").replace("{year}", String(props.year));

  const totalLabel = valueWithNoun(props.metric, props.grid.total);
  const headline = `${totalLabel} ${timeframe}`;

  /** "N tokens on Apr 9, 2026" — shared by the inspector line and cell titles. */
  const cellTooltip = (cell: HeatmapCell): string =>
    t("usage.activityDayTooltip")
      .replace("{value}", valueWithNoun(props.metric, cell.value))
      .replace("{date}", formatDate(cell.date));

  const inspectorText = hovered
    ? cellTooltip(hovered)
    : t("usage.activityHint");

  const flatCells = props.grid.weeks.flatMap((week) => week.cells);
  const weekCount = props.grid.weeks.length;

  return (
    <section className="usage-card usage-heatmap-card">
      <div className="usage-heatmap-topbar">
        <div className="usage-heatmap-heading">
          <div className="usage-section-title">{t("usage.activityTitle")}</div>
          <div className="usage-section-subtitle">{timeframe}</div>
        </div>
        <div className="usage-heatmap-controls">
          <ToggleGroup
            className="usage-metric-toggle"
            size="sm"
            spacing={0}
            value={[props.metric]}
            onValueChange={(next) => {
              const value = next[0];
              if (value === "tokens" || value === "cost") {
                props.setMetric(value);
              }
            }}
          >
            {METRICS.map((metric) => (
              <ToggleGroupItem
                key={metric}
                value={metric}
                className={cn(
                  "usage-metric-btn h-auto min-w-0",
                  props.metric === metric && "active",
                )}
              >
                {metric === "tokens" ? (
                  <Hash aria-hidden="true" data-icon="inline-start" />
                ) : (
                  <CircleDollarSign
                    aria-hidden="true"
                    data-icon="inline-start"
                  />
                )}
                {t(`usage.${metric}`)}
              </ToggleGroupItem>
            ))}
          </ToggleGroup>

          <ToggleGroup
            className="usage-heatmap-years"
            size="sm"
            spacing={1}
            value={[props.year === null ? "trailing" : String(props.year)]}
            onValueChange={(next) => {
              const value = next[0];
              if (!value) return;
              props.setYear(value === "trailing" ? null : Number(value));
            }}
          >
            <ToggleGroupItem
              value="trailing"
              className={cn(
                "usage-year-btn h-auto min-w-0",
                props.year === null && "active",
              )}
            >
              {t("usage.activityYearTrailing")}
            </ToggleGroupItem>
            {props.availableYears.map((year) => (
              <ToggleGroupItem
                key={year}
                value={String(year)}
                className={cn(
                  "usage-year-btn h-auto min-w-0",
                  props.year === year && "active",
                )}
              >
                {year}
              </ToggleGroupItem>
            ))}
          </ToggleGroup>
        </div>
      </div>

      <div className="usage-heatmap-summary" aria-live="polite">
        <div className="usage-heatmap-total">{totalLabel}</div>
        <div className="usage-heatmap-inspector">{inspectorText}</div>
      </div>

      {weekCount > 0 && (
        <div className="usage-heatmap-scroll">
          <fieldset
            className={`usage-heatmap-graph${props.loading ? " is-loading" : ""}`}
            aria-label={headline}
            style={{ "--weeks": String(weekCount) } as CSSProperties}
          >
            <div className="usage-heatmap-corner" aria-hidden="true" />

            <div className="usage-heatmap-months">
              {props.grid.monthLabels.map((label) => (
                <span
                  key={`${label.month}-${label.weekIndex}`}
                  className="usage-heatmap-month"
                  style={{ gridColumnStart: String(label.weekIndex + 1) }}
                >
                  {monthLabel(label.month)}
                </span>
              ))}
            </div>

            <div className="usage-heatmap-weekdays">
              {WEEKDAY_ROWS.map((row) => (
                <span key={row} className="usage-heatmap-weekday">
                  {weekdayLabel(row)}
                </span>
              ))}
            </div>

            <div className="usage-heatmap-cells">
              {flatCells.map((cell) => {
                if (!cell.inRange) {
                  return (
                    <span
                      key={cell.date}
                      className="usage-heatmap-cell is-empty"
                      data-level={cell.level}
                      aria-hidden="true"
                    />
                  );
                }

                const label = cellTooltip(cell);
                return (
                  <Button
                    key={cell.date}
                    variant="ghost"
                    type="button"
                    className="usage-heatmap-cell h-auto min-h-0 min-w-0 p-0 active:translate-y-0"
                    data-level={cell.level}
                    title={label}
                    aria-label={label}
                    onBlur={() => setHovered(null)}
                    onFocus={() => setHovered(cell)}
                    onMouseEnter={() => setHovered(cell)}
                    onMouseLeave={() => setHovered(null)}
                  />
                );
              })}
            </div>
          </fieldset>
        </div>
      )}

      <div className="usage-heatmap-footer">
        {props.grid.activeDays === 0 && !props.loading && (
          <span className="usage-heatmap-empty">
            {t("usage.activityNoData")}
          </span>
        )}
        <div className="usage-heatmap-legend">
          <span>{t("usage.activityLess")}</span>
          {LEGEND_LEVELS.map((level) => (
            <span
              key={level}
              className="usage-heatmap-cell is-legend"
              data-level={level}
            />
          ))}
          <span>{t("usage.activityMore")}</span>
        </div>
      </div>
    </section>
  );
}
