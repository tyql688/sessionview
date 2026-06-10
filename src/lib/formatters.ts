import { locale } from "../i18n/index";

export function parseTimestamp(ts: string | null): number | null {
  if (!ts) return null;
  const n = Number(ts);
  if (!Number.isNaN(n) && n > 0) {
    // If it looks like seconds (< 2e10), convert to ms
    return n < 2e10 ? n * 1000 : n;
  }
  const d = Date.parse(ts);
  return Number.isNaN(d) ? null : d;
}

export function formatTimeOnly(ms: number): string {
  const d = new Date(ms);
  return d.toLocaleTimeString(undefined, {
    hour: "numeric",
    minute: "2-digit",
  });
}

export function formatTimestamp(epoch: number, locale?: string): string {
  if (!epoch) return "\u2014";
  const now = Date.now();
  const ts = epoch * 1000;
  const diffMs = now - ts;
  const diffSec = Math.floor(diffMs / 1000);
  const isZh = locale === "zh";

  if (diffSec < 60) {
    return isZh ? "\u521a\u521a" : "just now";
  }
  const diffMin = Math.floor(diffSec / 60);
  if (diffMin < 60) {
    return isZh ? `${diffMin} \u5206\u949f\u524d` : `${diffMin} minutes ago`;
  }
  const diffHour = Math.floor(diffMin / 60);
  if (diffHour < 24) {
    return isZh ? `${diffHour} \u5c0f\u65f6\u524d` : `${diffHour} hours ago`;
  }
  const diffDay = Math.floor(diffHour / 24);
  if (diffDay < 7) {
    return isZh ? `${diffDay} \u5929\u524d` : `${diffDay} days ago`;
  }
  const d = new Date(ts);
  return d.toLocaleString();
}

export function formatAbsoluteTime(epoch: number): string {
  if (!epoch) return "\u2014";
  return new Date(epoch * 1000).toLocaleString();
}

/** Local-calendar date as `YYYY-MM-DD`. */
export function toLocalISODate(date: Date = new Date()): string {
  const yyyy = date.getFullYear();
  const mm = String(date.getMonth() + 1).padStart(2, "0");
  const dd = String(date.getDate()).padStart(2, "0");
  return `${yyyy}-${mm}-${dd}`;
}

export function formatLocalDateTime(value: string | null): string {
  if (!value) return "\u2014";
  const parsed = Date.parse(value);
  if (Number.isNaN(parsed)) return value;
  const date = new Date(parsed);
  const hh = String(date.getHours()).padStart(2, "0");
  const mi = String(date.getMinutes()).padStart(2, "0");
  const ss = String(date.getSeconds()).padStart(2, "0");
  return `${toLocalISODate(date)} ${hh}:${mi}:${ss}`;
}

/** Compact token/number formatter: `1.2T` / `3.4B` / `1.5M` / `2.5K` / `42`. */
export function fmtK(n: number): string {
  if (n >= 1_000_000_000_000) return `${(n / 1_000_000_000_000).toFixed(1)}T`;
  if (n >= 1_000_000_000) return `${(n / 1_000_000_000).toFixed(1)}B`;
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}K`;
  return String(n);
}

/** Chinese-scale token/number formatter: `1.2万亿` / `3.4亿` / `1.5万` / `42`. */
export function fmtWan(n: number): string {
  if (n >= 1_0000_0000_0000) return `${(n / 1_0000_0000_0000).toFixed(1)}万亿`;
  if (n >= 1_0000_0000) return `${(n / 1_0000_0000).toFixed(1)}亿`;
  if (n >= 1_0000) return `${(n / 1_0000).toFixed(1)}万`;
  return String(n);
}

/** Token formatter following the UI language: 万/亿 for zh, K/M/B/T otherwise. */
export function fmtTokens(n: number): string {
  return locale() === "zh" ? fmtWan(n) : fmtK(n);
}

export function formatFileSize(bytes: number): string {
  if (!bytes) return "—";
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

export function formatDuration(ms: number): string {
  if (ms < 60_000) return "< 1 min";
  const totalMin = Math.floor(ms / 60_000);
  if (totalMin < 60) return `${totalMin} min`;
  const hours = Math.floor(totalMin / 60);
  const mins = totalMin % 60;
  if (hours < 24) return mins > 0 ? `${hours}h ${mins}min` : `${hours}h`;
  const days = Math.floor(hours / 24);
  const remH = hours % 24;
  return remH > 0 ? `${days}d ${remH}h` : `${days}d`;
}

/** Replace home directory prefix with ~ for privacy. */
export function shortenHomePath(path: string): string {
  const normalized = path.replaceAll("\\", "/");
  const homePatterns = [/\/Users\/[^/\s]+/g, /\/home\/[^/\s]+/g];
  const windowsHomePattern = /[A-Z]:\/Users\/[^/\s]+/gi;
  let shortened = normalized.replace(windowsHomePattern, "~");
  for (const pat of homePatterns) {
    shortened = shortened.replace(pat, "~");
  }
  return shortened;
}
