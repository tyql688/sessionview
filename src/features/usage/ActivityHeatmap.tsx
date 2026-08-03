import { type CSSProperties, type KeyboardEvent, useEffect, useMemo, useRef, useState } from "react";
import { CircleDollarSign, Hash } from "lucide-react";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { useIsCompact } from "@/stores/viewport";
import { useI18n } from "@/i18n/index";
import type { HeatmapCell, HeatmapGrid, HeatmapMetric } from "@/features/usage/heatmap";
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
    new Intl.DateTimeFormat(localeTag, { month: "short" }).format(new Date(2020, month - 1, 1));

  // GitHub labels only Mon / Wed / Fri (rows 1, 3, 5). 2023-01-01 was a Sunday.
  const weekdayLabel = (row: number): string =>
    row % 2 === 1 ? new Intl.DateTimeFormat(localeTag, { weekday: "short" }).format(new Date(2023, 0, 1 + row)) : "";

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
    const text = metric === "tokens" ? fmtTokens(value) : value.toLocaleString();
    return `${text} ${metricNoun(metric)}`;
  };

  const timeframe =
    props.year === null ? t("usage.activityTrailing") : t("usage.activityInYear").replace("{year}", String(props.year));

  const totalLabel = valueWithNoun(props.metric, props.grid.total);
  const headline = `${totalLabel} ${timeframe}`;

  /** "N tokens on Apr 9, 2026" — shared by the inspector line and cell titles. */
  const cellTooltip = (cell: HeatmapCell): string =>
    t("usage.activityDayTooltip")
      .replace("{value}", valueWithNoun(props.metric, cell.value))
      .replace("{date}", formatDate(cell.date));

  const inspectorText = hovered ? cellTooltip(hovered) : t("usage.activityHint");

  const flatCells = useMemo(() => props.grid.weeks.flatMap((week) => week.cells), [props.grid.weeks]);
  const inRangeCells = useMemo(() => flatCells.filter((cell) => cell.inRange), [flatCells]);
  const weekCount = props.grid.weeks.length;
  const [focusedDate, setFocusedDate] = useState<string | null>(() => inRangeCells.at(-1)?.date ?? null);

  useEffect(() => {
    setFocusedDate((current) =>
      current && inRangeCells.some((cell) => cell.date === current) ? current : (inRangeCells.at(-1)?.date ?? null),
    );
  }, [inRangeCells]);

  // Compact layouts scroll the grid horizontally (see mobile.css); land on
  // the newest weeks, which are what the reader came for.
  const isCompact = useIsCompact();
  const scrollRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const el = scrollRef.current;
    if (isCompact && el) el.scrollLeft = el.scrollWidth;
  }, [isCompact, weekCount]);

  const handleCellKeyDown = (event: KeyboardEvent<HTMLTableCellElement>, date: string) => {
    const currentIndex = flatCells.findIndex((cell) => cell.date === date);
    const firstIndex = flatCells.findIndex((cell) => cell.inRange);
    const lastIndex = flatCells.findLastIndex((cell) => cell.inRange);
    if (currentIndex < 0 || firstIndex < 0 || lastIndex < 0) return;

    const weekday = currentIndex % 7;
    let nextIndex: number;
    switch (event.key) {
      case "ArrowLeft":
        nextIndex = currentIndex - 7;
        break;
      case "ArrowRight":
        nextIndex = currentIndex + 7;
        break;
      case "ArrowUp":
        nextIndex = weekday > 0 ? currentIndex - 1 : currentIndex;
        break;
      case "ArrowDown":
        nextIndex = weekday < 6 ? currentIndex + 1 : currentIndex;
        break;
      case "Home":
        nextIndex = flatCells.findIndex((cell, index) => index % 7 === weekday && cell.inRange);
        break;
      case "End":
        nextIndex = flatCells.findLastIndex((cell, index) => index % 7 === weekday && cell.inRange);
        break;
      default:
        return;
    }

    event.preventDefault();
    const nextCell = flatCells[Math.max(firstIndex, Math.min(lastIndex, nextIndex))];
    if (!nextCell?.inRange) return;
    setFocusedDate(nextCell.date);
    requestAnimationFrame(() => {
      scrollRef.current
        ?.querySelector<HTMLElement>(`[data-heatmap-date="${nextCell.date}"]`)
        ?.focus({ preventScroll: true });
    });
  };

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
                className={cn("usage-metric-btn h-auto min-w-0", props.metric === metric && "active")}
              >
                {metric === "tokens" ? (
                  <Hash aria-hidden="true" data-icon="inline-start" />
                ) : (
                  <CircleDollarSign aria-hidden="true" data-icon="inline-start" />
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
              className={cn("usage-year-btn h-auto min-w-0", props.year === null && "active")}
            >
              {t("usage.activityYearTrailing")}
            </ToggleGroupItem>
            {props.availableYears.map((year) => (
              <ToggleGroupItem
                key={year}
                value={String(year)}
                className={cn("usage-year-btn h-auto min-w-0", props.year === year && "active")}
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
        <div className="usage-heatmap-scroll" ref={scrollRef}>
          <div
            className={`usage-heatmap-graph${props.loading ? " is-loading" : ""}`}
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

            <table className="usage-heatmap-cells" aria-busy={props.loading}>
              <caption className="sr-only">{headline}</caption>
              <tbody className="usage-heatmap-body">
                {WEEKDAY_ROWS.map((row) => (
                  <tr className="usage-heatmap-row" key={row}>
                    {props.grid.weeks.map((week) => {
                      const cell = week.cells[row];
                      if (!cell.inRange) {
                        return (
                          <td
                            key={cell.date}
                            className="usage-heatmap-cell is-empty"
                            data-level={cell.level}
                            aria-hidden="true"
                            tabIndex={-1}
                          />
                        );
                      }

                      const label = cellTooltip(cell);
                      return (
                        <td
                          key={cell.date}
                          className="usage-heatmap-cell"
                          data-heatmap-date={cell.date}
                          data-level={cell.level}
                          tabIndex={cell.date === focusedDate ? 0 : -1}
                          title={label}
                          aria-label={label}
                          onBlur={() => setHovered(null)}
                          onFocus={() => {
                            setFocusedDate(cell.date);
                            setHovered(cell);
                          }}
                          onKeyDown={(event) => handleCellKeyDown(event, cell.date)}
                          onMouseEnter={() => setHovered(cell)}
                          onMouseLeave={() => setHovered(null)}
                        />
                      );
                    })}
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>
      )}

      <div className="usage-heatmap-footer">
        {props.grid.activeDays === 0 && !props.loading && (
          <span className="usage-heatmap-empty">{t("usage.activityNoData")}</span>
        )}
        <div className="usage-heatmap-legend">
          <span>{t("usage.activityLess")}</span>
          {LEGEND_LEVELS.map((level) => (
            <span key={level} className="usage-heatmap-cell is-legend" data-level={level} />
          ))}
          <span>{t("usage.activityMore")}</span>
        </div>
      </div>
    </section>
  );
}
