import { describe, it, expect } from "vitest";
import { render, fireEvent } from "@testing-library/react";
import { useState } from "react";
import { ActivityHeatmap } from "@/features/usage/ActivityHeatmap";
import { buildHeatmapGrid, type HeatmapMetric } from "@/features/usage/heatmap";
import type { ActivityDay } from "@/lib/types";

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
  // Metric/year are owned by the harness (controlled component); mirror the
  // latest committed values into closure vars so tests can read them.
  let latestMetric: HeatmapMetric = opts?.metric ?? "sessions";
  let latestYear: number | null = opts?.year ?? null;

  function Harness() {
    const [metric, setMetric] = useState<HeatmapMetric>(
      opts?.metric ?? "sessions",
    );
    const [year, setYear] = useState<number | null>(opts?.year ?? null);
    latestMetric = metric;
    latestYear = year;
    const grid = buildHeatmapGrid(days, metric, start, end);

    return (
      <ActivityHeatmap
        grid={grid}
        metric={metric}
        setMetric={setMetric}
        year={year}
        setYear={setYear}
        availableYears={opts?.years ?? [2026, 2025]}
        loading={false}
      />
    );
  }

  const result = render(<Harness />);
  return { ...result, metric: () => latestMetric, year: () => latestYear };
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
