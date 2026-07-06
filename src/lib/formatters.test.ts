import { describe, it, expect, afterEach } from "vitest";
import {
  parseTimestamp,
  fmtK,
  fmtWan,
  fmtTokens,
  formatFileSize,
  formatTimestamp,
  shortenHomePath,
  toLocalISODate,
  formatTreeTime,
} from "@/lib/formatters";
import { getLocale, i18next } from "@/i18n/index";

describe("parseTimestamp", () => {
  it("parses epoch seconds and converts to ms", () => {
    expect(parseTimestamp("1711800000")).toBe(1711800000000);
  });
  it("passes through epoch ms", () => {
    expect(parseTimestamp("1711800000000")).toBe(1711800000000);
  });
  it("parses ISO 8601 string", () => {
    const result = parseTimestamp("2026-03-30T12:00:00Z");
    expect(result).toBeGreaterThan(0);
  });
  it("returns null for null input", () => {
    expect(parseTimestamp(null)).toBeNull();
  });
  it("returns null for invalid string", () => {
    expect(parseTimestamp("not-a-date")).toBeNull();
  });
});

describe("toLocalISODate", () => {
  it("formats a local-calendar date as YYYY-MM-DD", () => {
    expect(toLocalISODate(new Date(2026, 3, 9))).toBe("2026-04-09");
  });
  it("zero-pads single-digit month and day", () => {
    expect(toLocalISODate(new Date(2026, 0, 1))).toBe("2026-01-01");
  });
});

describe("fmtK", () => {
  it("formats trillions", () => {
    expect(fmtK(1_230_000_000_000)).toBe("1.2T");
  });
  it("formats billions", () => {
    expect(fmtK(3_400_000_000)).toBe("3.4B");
  });
  it("formats millions", () => {
    expect(fmtK(1_500_000)).toBe("1.5M");
  });
  it("formats thousands", () => {
    expect(fmtK(2_500)).toBe("2.5K");
  });
  it("returns raw number for small values", () => {
    expect(fmtK(42)).toBe("42");
  });
});

describe("fmtWan", () => {
  it("formats 万亿 (1e12)", () => {
    expect(fmtWan(1_230_000_000_000)).toBe("1.2万亿");
  });
  it("formats 亿 (1e8)", () => {
    expect(fmtWan(340_000_000)).toBe("3.4亿");
  });
  it("formats 万 (1e4)", () => {
    expect(fmtWan(15_000)).toBe("1.5万");
  });
  it("returns raw number below 1万", () => {
    expect(fmtWan(9_999)).toBe("9999");
  });
});

describe("fmtTokens", () => {
  const initialLocale = getLocale();
  afterEach(async () => {
    await i18next.changeLanguage(initialLocale);
  });

  it("uses 万/亿 scale for zh locale", async () => {
    await i18next.changeLanguage("zh");
    expect(fmtTokens(340_000_000)).toBe("3.4亿");
  });
  it("uses K/M/B scale for en locale", async () => {
    await i18next.changeLanguage("en");
    expect(fmtTokens(340_000_000)).toBe("340.0M");
  });
});

describe("formatFileSize", () => {
  it("formats bytes", () => {
    expect(formatFileSize(500)).toBe("500 B");
  });
  it("formats kilobytes", () => {
    expect(formatFileSize(2048)).toBe("2.0 KB");
  });
  it("formats megabytes", () => {
    expect(formatFileSize(1_500_000)).toBe("1.4 MB");
  });
  it("returns dash for zero", () => {
    expect(formatFileSize(0)).toBe("\u2014");
  });
});

describe("formatTimestamp", () => {
  it("returns dash for zero epoch", () => {
    expect(formatTimestamp(0)).toBe("\u2014");
  });
  it("returns 'just now' for recent epoch", () => {
    const nowEpoch = Math.floor(Date.now() / 1000);
    expect(formatTimestamp(nowEpoch)).toBe("just now");
  });
  it("returns Chinese for zh locale", () => {
    const nowEpoch = Math.floor(Date.now() / 1000);
    expect(formatTimestamp(nowEpoch, "zh")).toBe("\u521a\u521a");
  });
});

describe("shortenHomePath", () => {
  it("replaces unix and windows user homes", () => {
    expect(shortenHomePath("/Users/alice/project/src/main.ts")).toBe(
      "~/project/src/main.ts",
    );
    expect(shortenHomePath("/home/bob/project/src/main.ts")).toBe(
      "~/project/src/main.ts",
    );
    expect(shortenHomePath("C:\\Users\\Alice\\project\\src\\main.ts")).toBe(
      "~/project/src/main.ts",
    );
    expect(
      shortenHomePath("*** Update File: /Users/alice/project/src/main.ts"),
    ).toBe("*** Update File: ~/project/src/main.ts");
  });
});

describe("formatTreeTime", () => {
  const now = new Date("2026-07-06T20:00:00");

  it("shows clock time for today", () => {
    const epoch = new Date("2026-07-06T09:05:00").getTime() / 1000;
    expect(formatTreeTime(epoch, now)).toBe("09:05");
  });

  it("shows month/day within the current year", () => {
    const epoch = new Date("2026-03-15T09:05:00").getTime() / 1000;
    expect(formatTreeTime(epoch, now)).toBe("3/15");
  });

  it("shows a short year for older sessions", () => {
    const epoch = new Date("2025-12-31T09:05:00").getTime() / 1000;
    expect(formatTreeTime(epoch, now)).toBe("25/12/31");
  });
});
