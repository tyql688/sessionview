import { useEffect } from "react";
import { Button } from "@/components/ui/button";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { useI18n } from "@/i18n/index";
import type { Locale } from "@/i18n/index";
import { useTheme, setTheme, applyTheme, getTheme } from "@/stores/theme";
import type { Theme } from "@/stores/theme";
import { useUpdaterPhase, useAvailableVersion, downloadAndInstall } from "@/features/updater/updater";
import { fmtTokens } from "@/lib/formatters";
import type { TodayTokens } from "@/lib/tauri";
import { cn } from "@/lib/utils";

export function StatusBar(props: {
  sessionCount: number;
  providerCount: number;
  isIndexing?: boolean;
  lastScanTime?: number;
  nextAutoIndexTime?: number;
  todayCost?: number;
  todayTokens?: TodayTokens;
}) {
  const { t, locale, setLocale } = useI18n();
  const theme = useTheme();
  const phase = useUpdaterPhase();
  const availableVersion = useAvailableVersion();

  useEffect(() => {
    applyTheme(getTheme());
  }, []);

  const lastScanLabel = () => {
    const ts = props.lastScanTime;
    if (!ts) return null;
    const diff = Math.floor((Date.now() - ts) / 1000);
    if (diff < 60) return t("status.justNow");
    if (diff < 3600) return `${Math.floor(diff / 60)}m`;
    if (diff < 86400) return `${Math.floor(diff / 3600)}h`;
    return `${Math.floor(diff / 86400)}d`;
  };

  const todayCostLabel = () => {
    const cost = props.todayCost;
    if (cost === undefined || cost === 0) return null;
    return cost < 0.01 ? "<$0.01" : `$${cost.toFixed(2)}`;
  };

  const nextAutoIndexLabel = () => {
    const ts = props.nextAutoIndexTime;
    if (!ts) return null;
    return new Date(ts).toLocaleTimeString(locale, {
      hour: "2-digit",
      minute: "2-digit",
    });
  };

  const hasTokens = () => {
    const t = props.todayTokens;
    return t && (t.input > 0 || t.output > 0);
  };

  function cycleTheme() {
    const order: Theme[] = ["light", "dark", "system"];
    const idx = order.indexOf(getTheme());
    const next = order[(idx + 1) % order.length];
    setTheme(next);
  }

  const themeIcon = () => {
    switch (theme) {
      case "light":
        return "☀️";
      case "dark":
        return "🌙";
      case "system":
        return "💻";
    }
  };

  const themeLabel = () => {
    switch (theme) {
      case "light":
        return t("status.themeLight");
      case "dark":
        return t("status.themeDark");
      case "system":
        return t("status.themeSystem");
    }
  };

  const updateLabel = () => {
    switch (phase) {
      case "available":
        return `↑ v${availableVersion}`;
      case "downloading":
      case "installing":
        return t("settings.updating");
      case "error":
        return t("settings.updateFailed");
      default:
        return null;
    }
  };

  const isBusy = () => phase === "downloading" || phase === "installing" || phase === "error";

  return (
    <div className="statusbar">
      <div className="statusbar-left">
        <span className={props.isIndexing ? "status-dot-indexing" : "status-dot"} />
        <span>
          {props.isIndexing ? (
            t("status.indexing")
          ) : (
            <>
              {t("status.indexed")} — {props.sessionCount.toLocaleString()} {t("status.sessions")}
            </>
          )}
        </span>
        <span className="status-separator">·</span>
        <span>
          {props.providerCount} {t("status.providers")}
        </span>
        {lastScanLabel() && (
          <>
            <span className="status-separator">·</span>
            <span title={props.lastScanTime ? new Date(props.lastScanTime).toLocaleString() : ""}>
              {t("status.lastUpdate")} {lastScanLabel()}
            </span>
          </>
        )}
        {nextAutoIndexLabel() && (
          <>
            <span className="status-separator">·</span>
            <span title={props.nextAutoIndexTime ? new Date(props.nextAutoIndexTime).toLocaleString() : ""}>
              {t("status.nextUpdate")} {nextAutoIndexLabel()}
            </span>
          </>
        )}
        {(hasTokens() || todayCostLabel()) && (
          <>
            <span className="status-separator">·</span>
            <span className="status-today">
              {hasTokens() && (
                <span
                  className="status-badge status-badge-tokens"
                  title={(() => {
                    const tk = props.todayTokens!;
                    return `${t("common.inputTokens")}: ${tk.input.toLocaleString()}, ${t("common.outputTokens")}: ${tk.output.toLocaleString()}${tk.cache_read > 0 ? `, ${t("common.cacheReadTokens")}: ${tk.cache_read.toLocaleString()}` : ""}${tk.cache_write > 0 ? `, ${t("common.cacheWriteTokens")}: ${tk.cache_write.toLocaleString()}` : ""}`;
                  })()}
                >
                  {"\u2191"}
                  {fmtTokens(props.todayTokens!.input)}
                  {" \u2193"}
                  {fmtTokens(props.todayTokens!.output)} {t("common.tokens")}
                  {props.todayTokens!.cache_read + props.todayTokens!.cache_write > 0 && (
                    <>
                      {" · "}
                      <span className="cache-read-label">
                        {t("common.cacheRead")} {fmtTokens(props.todayTokens!.cache_read)}
                      </span>
                      {" · "}
                      {t("common.cacheWrite")} {fmtTokens(props.todayTokens!.cache_write)}
                    </>
                  )}
                </span>
              )}
              {todayCostLabel() && <span className="status-badge status-badge-cost">{todayCostLabel()}</span>}
            </span>
          </>
        )}
      </div>
      <div className="statusbar-right">
        {updateLabel() !== null && (
          <Button
            variant="ghost"
            size="xs"
            className={cn("update-badge active:translate-y-0", isBusy() && "busy")}
            disabled={isBusy()}
            onClick={() => {
              if (phase === "available") void downloadAndInstall();
            }}
            title={updateLabel() ?? ""}
          >
            {updateLabel()}
          </Button>
        )}
        <Button
          variant="ghost"
          size="icon-xs"
          className="theme-toggle h-[18px] min-w-0 px-1 active:translate-y-0"
          onClick={cycleTheme}
          title={themeLabel()}
        >
          {themeIcon()}
        </Button>
        <ToggleGroup
          className="locale-toggle"
          size="sm"
          spacing={0}
          value={[locale]}
          onValueChange={(next) => {
            const value = next[0];
            if (value === "en" || value === "zh") {
              setLocale(value as Locale);
            }
          }}
        >
          <ToggleGroupItem
            value="en"
            className={cn("locale-btn h-[18px] min-w-0 px-1 text-[11px]", locale === "en" && "active")}
          >
            EN
          </ToggleGroupItem>
          <span className="locale-divider">|</span>
          <ToggleGroupItem
            value="zh"
            className={cn("locale-btn h-[18px] min-w-0 px-1 text-[11px]", locale === "zh" && "active")}
          >
            中
          </ToggleGroupItem>
        </ToggleGroup>
      </div>
    </div>
  );
}
