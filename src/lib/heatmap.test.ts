import { describe, expect, it } from "vitest";

import {
  addDays,
  buildHeatmapGrid,
  dateRangeForYear,
  weekday,
  type HeatmapCell,
  type HeatmapGrid,
} from "./heatmap";
import type { ActivityDay } from "./types";

function day(date: string, sessions: number): ActivityDay {
  return {
    date,
    sessions,
    turns: sessions * 2,
    tokens: sessions * 100,
    cost: sessions,
  };
}

function cellsByDate(grid: HeatmapGrid): Map<string, HeatmapCell> {
  const map = new Map<string, HeatmapCell>();
  for (const week of grid.weeks) {
    for (const cell of week.cells) map.set(cell.date, cell);
  }
  return map;
}

describe("date helpers", () => {
  it("adds and subtracts days across month/year boundaries", () => {
    expect(addDays("2026-01-01", -1)).toBe("2025-12-31");
    expect(addDays("2026-12-31", 1)).toBe("2027-01-01");
    expect(addDays("2026-04-09", 7)).toBe("2026-04-16");
  });

  it("handles the leap day in 2024", () => {
    expect(addDays("2024-02-28", 1)).toBe("2024-02-29");
    expect(addDays("2024-02-29", 1)).toBe("2024-03-01");
    // 2026 is not a leap year.
    expect(addDays("2026-02-28", 1)).toBe("2026-03-01");
  });

  it("reports weekday with Sunday = 0", () => {
    expect(weekday("2026-01-01")).toBe(4); // Thursday
    expect(weekday("2025-12-28")).toBe(0); // Sunday
  });
});

describe("dateRangeForYear", () => {
  it("trailing window spans 52 whole weeks ending today", () => {
    const { start, end } = dateRangeForYear(null, "2026-06-08");
    expect(end).toBe("2026-06-08");
    expect(start).toBe("2025-06-10");
    // 364 days inclusive == 52 weeks.
    expect(weekday(start)).toBe(weekday(addDays(end, 1)));
  });

  it("a past year spans the full calendar year", () => {
    expect(dateRangeForYear(2025, "2026-06-08")).toEqual({
      start: "2025-01-01",
      end: "2025-12-31",
    });
  });

  it("the current year is clamped to today (no empty future cells)", () => {
    expect(dateRangeForYear(2026, "2026-06-08")).toEqual({
      start: "2026-01-01",
      end: "2026-06-08",
    });
  });
});

describe("buildHeatmapGrid", () => {
  it("snaps the grid to whole Sunday-first weeks and fills gaps", () => {
    const grid = buildHeatmapGrid(
      [day("2026-01-01", 5), day("2026-01-15", 1)],
      "sessions",
      "2026-01-01",
      "2026-01-31",
    );

    // Grid runs Sun 2025-12-28 .. Sat 2026-01-31 → 5 weeks of 7.
    expect(grid.weeks).toHaveLength(5);
    expect(grid.weeks[0].cells).toHaveLength(7);
    expect(grid.weeks[0].cells[0].date).toBe("2025-12-28");
    expect(weekday(grid.weeks[0].cells[0].date)).toBe(0);

    const cells = cellsByDate(grid);
    // Leading padding days (before Jan 1) are out of range.
    expect(cells.get("2025-12-28")?.inRange).toBe(false);
    expect(cells.get("2025-12-31")?.inRange).toBe(false);
    expect(cells.get("2026-01-01")?.inRange).toBe(true);

    // Gap day with no record is rendered empty (level 0), not missing.
    expect(cells.get("2026-01-08")?.value).toBe(0);
    expect(cells.get("2026-01-08")?.level).toBe(0);
    expect(cells.get("2026-01-08")?.day).toBeNull();

    // Days with data carry their record.
    expect(cells.get("2026-01-01")?.value).toBe(5);
    expect(cells.get("2026-01-01")?.day?.sessions).toBe(5);
  });

  it("totals and active-day count cover only in-range activity", () => {
    const grid = buildHeatmapGrid(
      [
        day("2025-12-31", 99), // out of range — must be ignored
        day("2026-01-01", 5),
        day("2026-01-15", 1),
      ],
      "sessions",
      "2026-01-01",
      "2026-01-31",
    );
    expect(grid.total).toBe(6);
    expect(grid.activeDays).toBe(2);
    expect(grid.maxValue).toBe(5);
  });

  it("buckets intensity into four quartile levels", () => {
    const values = [10, 20, 30, 40, 50, 60, 70, 80];
    const days = values.map((v, i) => day(addDays("2026-01-01", i), v));
    const grid = buildHeatmapGrid(days, "sessions", "2026-01-01", "2026-01-08");
    const cells = cellsByDate(grid);
    // thresholds [30, 50, 70] over the sorted values.
    expect(cells.get("2026-01-01")?.level).toBe(1); // 10
    expect(cells.get("2026-01-03")?.level).toBe(1); // 30
    expect(cells.get("2026-01-04")?.level).toBe(2); // 40
    expect(cells.get("2026-01-06")?.level).toBe(3); // 60
    expect(cells.get("2026-01-08")?.level).toBe(4); // 80
  });

  it("respects the selected metric", () => {
    const grid = buildHeatmapGrid(
      [day("2026-01-01", 5)],
      "tokens",
      "2026-01-01",
      "2026-01-07",
    );
    expect(cellsByDate(grid).get("2026-01-01")?.value).toBe(500);
    expect(grid.total).toBe(500);
  });

  it("emits one month label per month at its first column", () => {
    const grid = buildHeatmapGrid([], "sessions", "2026-01-01", "2026-02-28");
    const months = grid.monthLabels.map((label) => label.month);
    expect(months).toEqual([1, 2]);
    // January's label sits on the first week column.
    expect(grid.monthLabels[0].weekIndex).toBe(0);
    // February starts later; its column is strictly after January's.
    expect(grid.monthLabels[1].weekIndex).toBeGreaterThan(0);
  });

  it("lays out an empty calendar without any activity", () => {
    const grid = buildHeatmapGrid([], "sessions", "2026-01-01", "2026-01-31");
    expect(grid.total).toBe(0);
    expect(grid.activeDays).toBe(0);
    expect(grid.weeks.length).toBeGreaterThan(0);
    for (const week of grid.weeks) {
      for (const cell of week.cells) expect(cell.level).toBe(0);
    }
  });

  it("yields zero weeks for an inverted range (e.g. a future year)", () => {
    // dateRangeForYear(futureYear) can produce start > end; the grid must come
    // back empty so the component can guard against a degenerate 0-column grid.
    const grid = buildHeatmapGrid([], "tokens", "2027-01-01", "2026-06-08");
    expect(grid.weeks).toHaveLength(0);
    expect(grid.total).toBe(0);
    expect(grid.activeDays).toBe(0);
  });
});
