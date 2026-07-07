export type AutoIndexInterval = "5m" | "10m" | "30m" | "startup";

export const DEFAULT_AUTO_INDEX_INTERVAL: AutoIndexInterval = "5m";
export const AUTO_INDEX_INTERVAL_OPTIONS: readonly AutoIndexInterval[] = ["5m", "10m", "30m", "startup"];

export function isAutoIndexInterval(value: string): value is AutoIndexInterval {
  return AUTO_INDEX_INTERVAL_OPTIONS.includes(value as AutoIndexInterval);
}

export function autoIndexIntervalMs(value: AutoIndexInterval): number | null {
  switch (value) {
    case "5m":
      return 5 * 60 * 1000;
    case "10m":
      return 10 * 60 * 1000;
    case "30m":
      return 30 * 60 * 1000;
    case "startup":
      return null;
  }
}
