import { describe, it, expect, beforeEach, afterAll } from "vitest";
import {
  fmtTokens,
  fmtCost,
  fmtPct,
  fmtTrend,
  trendClass,
  fmtActive,
  sortIcon,
  makeFmtChartValue,
} from "@/features/usage/formatters";
import { getLocale, i18next } from "@/i18n/index";

// fmtTokens follows the UI language; pin it so assertions don't depend on the
// machine's navigator.language.
const initialLocale = getLocale();
beforeEach(async () => {
  await i18next.changeLanguage("en");
});
afterAll(async () => {
  await i18next.changeLanguage(initialLocale);
});

describe("fmtTokens", () => {
  it("uses 万/亿 scale when the UI language is Chinese", async () => {
    await i18next.changeLanguage("zh");
    expect(fmtTokens(15_000)).toBe("1.5万");
    expect(fmtTokens(340_000_000)).toBe("3.4亿");
  });
  it("returns localized integer under 1K", () => {
    expect(fmtTokens(0)).toBe("0");
    expect(fmtTokens(999)).toBe("999");
  });
  it("uses K suffix under 1M", () => {
    expect(fmtTokens(1_000)).toBe("1.0K");
    expect(fmtTokens(12_345)).toBe("12.3K");
    expect(fmtTokens(999_999)).toBe("1000.0K");
  });
  it("uses M suffix for 1M and above", () => {
    expect(fmtTokens(1_000_000)).toBe("1.0M");
    expect(fmtTokens(12_500_000)).toBe("12.5M");
  });
});

describe("fmtCost", () => {
  it("formats as dollars with 2 decimals", () => {
    expect(fmtCost(0)).toBe("$0.00");
    expect(fmtCost(12.345)).toBe("$12.35");
    expect(fmtCost(1000)).toBe("$1000.00");
  });
});

describe("fmtPct", () => {
  it("rounds to integer percent", () => {
    expect(fmtPct(0)).toBe("0%");
    expect(fmtPct(0.1234)).toBe("12%");
    expect(fmtPct(1)).toBe("100%");
  });
});

describe("fmtTrend", () => {
  it("returns empty string for null", () => {
    expect(fmtTrend(null)).toBe("");
  });
  it("includes up arrow for positive", () => {
    expect(fmtTrend(0.25)).toBe("\u2191 25%");
  });
  it("includes down arrow for negative", () => {
    expect(fmtTrend(-0.5)).toBe("\u2193 50%");
  });
  it("omits arrow for zero", () => {
    expect(fmtTrend(0)).toBe(" 0%");
  });
});

describe("trendClass", () => {
  it("returns empty for null", () => {
    expect(trendClass(null)).toBe("");
    expect(trendClass(null, true)).toBe("");
  });
  it("defaults: up=up, down=down", () => {
    expect(trendClass(0.1)).toBe("usage-trend-up");
    expect(trendClass(-0.1)).toBe("usage-trend-down");
    expect(trendClass(0)).toBe("");
  });
  it("inverted: up=down, down=up (for cost where up is bad)", () => {
    expect(trendClass(0.1, true)).toBe("usage-trend-down");
    expect(trendClass(-0.1, true)).toBe("usage-trend-up");
  });
});

describe("fmtActive", () => {
  const now = Math.floor(Date.now() / 1000);
  it("returns <1m for very recent", () => {
    expect(fmtActive(now)).toBe("<1m");
    expect(fmtActive(now - 30)).toBe("<1m");
  });
  it("returns minutes under an hour", () => {
    expect(fmtActive(now - 120)).toBe("2m");
  });
  it("returns hours under a day", () => {
    expect(fmtActive(now - 2 * 3600)).toBe("2h");
  });
  it("returns days beyond", () => {
    expect(fmtActive(now - 3 * 86400)).toBe("3d");
  });
});

describe("sortIcon", () => {
  it("returns neutral arrow for other column", () => {
    expect(sortIcon({ col: "cost", asc: false }, "turns")).toBe("↕");
  });
  it("returns direction arrow for active column", () => {
    expect(sortIcon({ col: "cost", asc: true }, "cost")).toBe("↑");
    expect(sortIcon({ col: "cost", asc: false }, "cost")).toBe("↓");
  });
});

describe("makeFmtChartValue", () => {
  it("uses fmtCost for cost metric", () => {
    const fmt = makeFmtChartValue(() => "cost");
    expect(fmt(123.45)).toBe("$123.45");
  });
  it("uses fmtTokens for tokens metric", () => {
    const fmt = makeFmtChartValue(() => "tokens");
    expect(fmt(12_345)).toBe("12.3K");
  });
});
