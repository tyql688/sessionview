import { type CSSProperties, useState } from "react";
import { useI18n } from "@/i18n/index";
import type { HeatmapCell, HeatmapGrid, HeatmapMetric } from "@/lib/heatmap";
import { fmtCost, fmtTokens } from "@/components/UsagePanel/formatters";

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

  const headline = `${valueWithNoun(props.metric, props.grid.total)} ${timeframe}`;

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
      <div className="usage-section-header">
        <div className="usage-heatmap-heading">
          <div className="usage-section-title">{headline}</div>
          <div className="usage-metric-toggle">
            {METRICS.map((metric) => (
              <button
                key={metric}
                className={`usage-metric-btn${props.metric === metric ? " active" : ""}`}
                aria-pressed={props.metric === metric}
                onClick={() => props.setMetric(metric)}
                type="button"
              >
                {t(`usage.${metric}`)}
              </button>
            ))}
          </div>
        </div>
        <div className="usage-heatmap-years">
          <button
            className={`usage-year-btn${props.year === null ? " active" : ""}`}
            aria-pressed={props.year === null}
            onClick={() => props.setYear(null)}
            type="button"
          >
            {t("usage.activityYearTrailing")}
          </button>
          {props.availableYears.map((year) => (
            <button
              key={year}
              className={`usage-year-btn${props.year === year ? " active" : ""}`}
              aria-pressed={props.year === year}
              onClick={() => props.setYear(year)}
              type="button"
            >
              {year}
            </button>
          ))}
        </div>
      </div>

      <div className="usage-heatmap-inspector">{inspectorText}</div>

      {weekCount > 0 && (
        <div className="usage-heatmap-scroll">
          <div
            className={`usage-heatmap-graph${props.loading ? " is-loading" : ""}`}
            role="img"
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
              {flatCells.map((cell) => (
                <div
                  key={cell.date}
                  className={`usage-heatmap-cell${!cell.inRange ? " is-empty" : ""}`}
                  data-level={cell.level}
                  title={cell.inRange ? cellTooltip(cell) : undefined}
                  onMouseEnter={() => {
                    if (cell.inRange) setHovered(cell);
                  }}
                  onMouseLeave={() => setHovered(null)}
                />
              ))}
            </div>
          </div>
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
