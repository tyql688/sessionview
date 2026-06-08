import { describe, it, expect } from "vitest";
import { render, fireEvent } from "@solidjs/testing-library";
import { createSignal } from "solid-js";
import { ActivityHeatmap } from "./ActivityHeatmap";
import { buildHeatmapGrid, type HeatmapMetric } from "../../lib/heatmap";
import type { ActivityDay } from "../../lib/types";

function day(date: string, sessions: number): ActivityDay {
  return {
    date,
    sessions,
    turns: sessions * 2,
    tokens: sessions * 100,
    cost: sessions,
  };
}

function setup(opts?: {
  days?: ActivityDay[];
  metric?: HeatmapMetric;
  year?: number | null;
  years?: number[];
}) {
  const days = opts?.days ?? [day("2026-01-01", 5), day("2026-01-03", 1)];
  const start = "2026-01-01";
  const end = "2026-01-07";
  const [metric, setMetric] = createSignal<HeatmapMetric>(
    opts?.metric ?? "sessions",
  );
  const [year, setYear] = createSignal<number | null>(opts?.year ?? null);
  const grid = () => buildHeatmapGrid(days, metric(), start, end);

  const result = render(() => (
    <ActivityHeatmap
      grid={grid}
      metric={metric}
      setMetric={setMetric}
      year={year}
      setYear={setYear}
      availableYears={() => opts?.years ?? [2026, 2025]}
      loading={() => false}
    />
  ));
  return { ...result, metric, year };
}

describe("ActivityHeatmap", () => {
  it("renders a headline with the metric total and timeframe", () => {
    const { getByText } = setup();
    // 5 + 1 = 6 sessions over the in-range window.
    expect(getByText("6 sessions in the last year")).toBeInTheDocument();
  });

  it("lays out one cell per day of every whole week", () => {
    const { container } = setup();
    const grid = buildHeatmapGrid(
      [day("2026-01-01", 5)],
      "sessions",
      "2026-01-01",
      "2026-01-07",
    );
    const expected = grid.weeks.length * 7;
    const cells = container.querySelectorAll(
      ".usage-heatmap-cells .usage-heatmap-cell",
    );
    expect(cells.length).toBe(expected);
  });

  it("switches the coloring metric when a toggle is clicked", () => {
    const { getByRole, metric } = setup();
    fireEvent.click(getByRole("button", { name: "tokens" }));
    expect(metric()).toBe("tokens");
  });

  it("selects a calendar year from the year picker", () => {
    const { getByRole, year } = setup();
    fireEvent.click(getByRole("button", { name: "2025" }));
    expect(year()).toBe(2025);
  });

  it("shows the day's activity in the inspector on hover", () => {
    const { container, getByText } = setup();
    // Hint shown before any hover.
    expect(getByText("Hover a day to see its activity")).toBeInTheDocument();

    const cell = container.querySelector<HTMLElement>('[title^="5 sessions"]');
    expect(cell).not.toBeNull();
    fireEvent.mouseEnter(cell!);

    const inspector = container.querySelector(".usage-heatmap-inspector");
    expect(inspector?.textContent).toContain("5 sessions");
  });
});
