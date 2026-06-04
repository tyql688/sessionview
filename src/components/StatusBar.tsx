import { createMemo, onMount, Show } from "solid-js";
import { useI18n } from "../i18n/index";
import type { Locale } from "../i18n/index";
import { theme, setTheme, applyTheme } from "../stores/theme";
import type { Theme } from "../stores/theme";
import { phase, availableVersion, downloadAndInstall } from "../stores/updater";
import { fmtTokens } from "../lib/formatters";
import type { TodayTokens } from "../lib/tauri";

export function StatusBar(props: {
  sessionCount: number;
  providerCount: number;
  isIndexing?: boolean;
  lastScanTime?: number;
  todayCost?: number;
  todayTokens?: TodayTokens;
}) {
  const { t, locale, setLocale } = useI18n();

  onMount(() => {
    applyTheme(theme());
  });

  const lastScanLabel = createMemo(() => {
    const ts = props.lastScanTime;
    if (!ts) return null;
    const diff = Math.floor((Date.now() - ts) / 1000);
    if (diff < 60) return t("status.justNow");
    if (diff < 3600) return `${Math.floor(diff / 60)}m`;
    if (diff < 86400) return `${Math.floor(diff / 3600)}h`;
    return `${Math.floor(diff / 86400)}d`;
  });

  const todayCostLabel = createMemo(() => {
    const cost = props.todayCost;
    if (cost === undefined || cost === 0) return null;
    return cost < 0.01 ? "<$0.01" : `$${cost.toFixed(2)}`;
  });

  const hasTokens = createMemo(() => {
    const t = props.todayTokens;
    return t && (t.input > 0 || t.output > 0);
  });

  function cycleTheme() {
    const order: Theme[] = ["light", "dark", "system"];
    const idx = order.indexOf(theme());
    const next = order[(idx + 1) % order.length];
    setTheme(next);
  }

  const themeIcon = () => {
    switch (theme()) {
      case "light":
        return "☀️";
      case "dark":
        return "🌙";
      case "system":
        return "💻";
    }
  };

  const themeLabel = () => {
    switch (theme()) {
      case "light":
        return t("status.themeLight");
      case "dark":
        return t("status.themeDark");
      case "system":
        return t("status.themeSystem");
    }
  };

  const updateLabel = () => {
    switch (phase()) {
      case "available":
        return `↑ v${availableVersion()}`;
      case "downloading":
      case "installing":
        return t("settings.updating");
      case "error":
        return t("settings.updateFailed");
      default:
        return null;
    }
  };

  const isBusy = () =>
    phase() === "downloading" ||
    phase() === "installing" ||
    phase() === "error";

  return (
    <div class="statusbar">
      <div class="statusbar-left">
        <span class={props.isIndexing ? "status-dot-indexing" : "status-dot"} />
        <span>
          {props.isIndexing ? (
            t("status.indexing")
          ) : (
            <>
              {t("status.indexed")} — {props.sessionCount.toLocaleString()}{" "}
              {t("status.sessions")}
            </>
          )}
        </span>
        <span class="status-separator">·</span>
        <span>
          {props.providerCount} {t("status.providers")}
        </span>
        <Show when={lastScanLabel()}>
          <span class="status-separator">·</span>
          <span
            title={
              props.lastScanTime
                ? new Date(props.lastScanTime).toLocaleString()
                : ""
            }
          >
            {t("status.lastScan")} {lastScanLabel()}
          </span>
        </Show>
        <Show when={hasTokens() || todayCostLabel()}>
          <span class="status-separator">·</span>
          <span class="status-today">
            <Show when={hasTokens()}>
              <span
                class="status-badge status-badge-tokens"
                title={(() => {
                  const tk = props.todayTokens!;
                  return `${t("common.inputTokens")}: ${tk.input.toLocaleString()}, ${t("common.outputTokens")}: ${tk.output.toLocaleString()}${tk.cache_read > 0 ? `, ${t("common.cacheReadTokens")}: ${tk.cache_read.toLocaleString()}` : ""}${tk.cache_write > 0 ? `, ${t("common.cacheWriteTokens")}: ${tk.cache_write.toLocaleString()}` : ""}`;
                })()}
              >
                {"\u2191"}
                {fmtTokens(props.todayTokens!.input)}
                {" \u2193"}
                {fmtTokens(props.todayTokens!.output)} {t("common.tokens")}
                <Show
                  when={
                    props.todayTokens!.cache_read +
                      props.todayTokens!.cache_write >
                    0
                  }
                >
                  {" · "}
                  <span class="cache-read-label">
                    {t("common.cacheRead")}{" "}
                    {fmtTokens(props.todayTokens!.cache_read)}
                  </span>
                  {" · "}
                  {t("common.cacheWrite")}{" "}
                  {fmtTokens(props.todayTokens!.cache_write)}
                </Show>
              </span>
            </Show>
            <Show when={todayCostLabel()}>
              <span class="status-badge status-badge-cost">
                {todayCostLabel()}
              </span>
            </Show>
          </span>
        </Show>
      </div>
      <div class="statusbar-right">
        <Show when={updateLabel() !== null}>
          <button
            class={`update-badge${isBusy() ? " busy" : ""}`}
            disabled={isBusy()}
            onClick={() => {
              if (phase() === "available") void downloadAndInstall();
            }}
            title={updateLabel() ?? ""}
          >
            {updateLabel()}
          </button>
        </Show>
        <button class="theme-toggle" onClick={cycleTheme} title={themeLabel()}>
          {themeIcon()}
        </button>
        <span class="locale-toggle">
          <button
            class={`locale-btn${locale() === "en" ? " active" : ""}`}
            onClick={() => setLocale("en" as Locale)}
          >
            EN
          </button>
          <span class="locale-divider">|</span>
          <button
            class={`locale-btn${locale() === "zh" ? " active" : ""}`}
            onClick={() => setLocale("zh" as Locale)}
          >
            中
          </button>
        </span>
      </div>
    </div>
  );
}
