import { For, Show, createSignal } from "solid-js";
import type { Accessor } from "solid-js";
import { useI18n } from "../../i18n/index";
import type {
  HeatmapCell,
  HeatmapGrid,
  HeatmapMetric,
} from "../../lib/heatmap";
import { fmtCost, fmtTokens } from "./formatters";

const METRICS: HeatmapMetric[] = ["tokens", "cost"];
const WEEKDAY_ROWS = [0, 1, 2, 3, 4, 5, 6];
const LEGEND_LEVELS = [0, 1, 2, 3, 4] as const;

export interface ActivityHeatmapProps {
  grid: Accessor<HeatmapGrid>;
  metric: Accessor<HeatmapMetric>;
  setMetric: (metric: HeatmapMetric) => void;
  year: Accessor<number | null>;
  setYear: (year: number | null) => void;
  availableYears: Accessor<number[]>;
  loading: Accessor<boolean>;
}

export function ActivityHeatmap(props: ActivityHeatmapProps) {
  const { t, locale } = useI18n();
  const [hovered, setHovered] = createSignal<HeatmapCell | null>(null);

  const localeTag = () => (locale() === "zh" ? "zh-CN" : "en-US");

  const monthLabel = (month: number): string =>
    new Intl.DateTimeFormat(localeTag(), { month: "short" }).format(
      new Date(2020, month - 1, 1),
    );

  // GitHub labels only Mon / Wed / Fri (rows 1, 3, 5). 2023-01-01 was a Sunday.
  const weekdayLabel = (row: number): string =>
    row % 2 === 1
      ? new Intl.DateTimeFormat(localeTag(), { weekday: "short" }).format(
          new Date(2023, 0, 1 + row),
        )
      : "";

  const formatDate = (iso: string): string => {
    const [y, m, d] = iso.split("-").map(Number);
    return new Intl.DateTimeFormat(localeTag(), {
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

  const timeframe = (): string =>
    props.year() === null
      ? t("usage.activityTrailing")
      : t("usage.activityInYear").replace("{year}", String(props.year()));

  const headline = (): string =>
    `${valueWithNoun(props.metric(), props.grid().total)} ${timeframe()}`;

  /** "N tokens on Apr 9, 2026" — shared by the inspector line and cell titles. */
  const cellTooltip = (cell: HeatmapCell): string =>
    t("usage.activityDayTooltip")
      .replace("{value}", valueWithNoun(props.metric(), cell.value))
      .replace("{date}", formatDate(cell.date));

  const inspectorText = (): string => {
    const cell = hovered();
    return cell ? cellTooltip(cell) : t("usage.activityHint");
  };

  const flatCells = () => props.grid().weeks.flatMap((week) => week.cells);
  const weekCount = () => props.grid().weeks.length;

  return (
    <section class="usage-card usage-heatmap-card">
      <div class="usage-section-header">
        <div class="usage-heatmap-heading">
          <div class="usage-section-title">{headline()}</div>
          <div class="usage-metric-toggle">
            <For each={METRICS}>
              {(metric) => (
                <button
                  class={`usage-metric-btn${props.metric() === metric ? " active" : ""}`}
                  aria-pressed={props.metric() === metric}
                  onClick={() => props.setMetric(metric)}
                  type="button"
                >
                  {t(`usage.${metric}`)}
                </button>
              )}
            </For>
          </div>
        </div>
        <div class="usage-heatmap-years">
          <button
            class={`usage-year-btn${props.year() === null ? " active" : ""}`}
            aria-pressed={props.year() === null}
            onClick={() => props.setYear(null)}
            type="button"
          >
            {t("usage.activityYearTrailing")}
          </button>
          <For each={props.availableYears()}>
            {(year) => (
              <button
                class={`usage-year-btn${props.year() === year ? " active" : ""}`}
                aria-pressed={props.year() === year}
                onClick={() => props.setYear(year)}
                type="button"
              >
                {year}
              </button>
            )}
          </For>
        </div>
      </div>

      <div class="usage-heatmap-inspector">{inspectorText()}</div>

      <Show when={weekCount() > 0}>
        <div class="usage-heatmap-scroll">
          <div
            class="usage-heatmap-graph"
            classList={{ "is-loading": props.loading() }}
            role="img"
            aria-label={headline()}
            style={{ "--weeks": String(weekCount()) }}
          >
            <div class="usage-heatmap-corner" aria-hidden="true" />

            <div class="usage-heatmap-months">
              <For each={props.grid().monthLabels}>
                {(label) => (
                  <span
                    class="usage-heatmap-month"
                    style={{ "grid-column-start": String(label.weekIndex + 1) }}
                  >
                    {monthLabel(label.month)}
                  </span>
                )}
              </For>
            </div>

            <div class="usage-heatmap-weekdays">
              <For each={WEEKDAY_ROWS}>
                {(row) => (
                  <span class="usage-heatmap-weekday">{weekdayLabel(row)}</span>
                )}
              </For>
            </div>

            <div class="usage-heatmap-cells">
              <For each={flatCells()}>
                {(cell) => (
                  <div
                    class="usage-heatmap-cell"
                    classList={{ "is-empty": !cell.inRange }}
                    data-level={cell.level}
                    title={cell.inRange ? cellTooltip(cell) : undefined}
                    onMouseEnter={() => {
                      if (cell.inRange) setHovered(cell);
                    }}
                    onMouseLeave={() => setHovered(null)}
                  />
                )}
              </For>
            </div>
          </div>
        </div>
      </Show>

      <div class="usage-heatmap-footer">
        <Show when={props.grid().activeDays === 0 && !props.loading()}>
          <span class="usage-heatmap-empty">{t("usage.activityNoData")}</span>
        </Show>
        <div class="usage-heatmap-legend">
          <span>{t("usage.activityLess")}</span>
          <For each={LEGEND_LEVELS}>
            {(level) => (
              <span class="usage-heatmap-cell is-legend" data-level={level} />
            )}
          </For>
          <span>{t("usage.activityMore")}</span>
        </div>
      </div>
    </section>
  );
}
