import type { CSSProperties } from "react";
import { useI18n } from "@/i18n/index";
import type { MaintenanceJob, ProviderSnapshot } from "@/lib/types";
import type { CustomDateRange } from "@/stores/usageView";
import { toLocalISODate } from "@/lib/formatters";

export interface ProviderChipInfo {
  color: string;
  label: string;
  fullLabel: string;
}

export interface ToolbarProps {
  activeRangeLabel: string;
  selectedProviderCount: number;
  activeMaintenanceJob: MaintenanceJob | null;
  maintenanceStatusText: string;

  rangeDays: number | null;
  onRangeChange: (days: number | null) => void;
  customRange: CustomDateRange | null;
  onCustomRangeChange: (range: CustomDateRange) => void;

  isRefreshingPricing: boolean;
  onRefreshPricing: () => void;
  onRequestRefreshUsage: () => void;

  formattedPricingUpdatedAt: string;
  formattedUsageUpdatedAt: string;
  pricingModelCountLabel: string;
  pricingStatusError: string | null;
  indexStatsError: string | null;

  scannedProviderSnapshots: ProviderSnapshot[];
  scannedProviderKeysCount: number;
  allProvidersSelected: boolean;
  isProviderSelected: (key: string) => boolean;
  onToggleProvider: (key: string) => void;
  onToggleAllProviders: () => void;
  providerInfo: (key: string) => ProviderChipInfo;
  providerSessionCount: (key: string) => number;
}

export function Toolbar(props: ToolbarProps) {
  const { t } = useI18n();
  const customRange = props.customRange;

  const ranges: { days: number | null; label: () => string }[] = [
    { days: 1, label: () => t("usage.rangeToday") },
    { days: 7, label: () => t("usage.range7d") },
    { days: 30, label: () => t("usage.range30d") },
    { days: 90, label: () => t("usage.range90d") },
    { days: null, label: () => t("usage.rangeAll") },
  ];

  // Native date inputs commit half-typed years like 0001-05-31; anything
  // before this floor is treated as a typo and the field is restored.
  const MIN_USAGE_DATE = "2000-01-01";

  const enterCustomRange = () => {
    if (customRange) return;
    // Seed with the last 7 days so the panel shows data immediately.
    const end = new Date();
    const start = new Date(end);
    start.setDate(start.getDate() - 6);
    props.onCustomRangeChange({
      start: toLocalISODate(start),
      end: toLocalISODate(end),
    });
  };

  const updateCustomRange = (
    field: keyof CustomDateRange,
    input: HTMLInputElement,
  ) => {
    const current = customRange;
    if (!current) return;
    const value = input.value;
    if (!value || value < MIN_USAGE_DATE) {
      // Cleared or absurd value: restore the field instead of querying with it.
      input.value = current[field];
      return;
    }
    const next = { ...current, [field]: value };
    // Keep the window ordered no matter which side the user moved.
    if (next.start > next.end) {
      props.onCustomRangeChange(
        field === "start"
          ? { start: next.start, end: next.start }
          : { start: next.end, end: next.end },
      );
      return;
    }
    props.onCustomRangeChange(next);
  };

  return (
    <section className="usage-card usage-toolbar-card">
      <div className="usage-toolbar-main">
        <div className="usage-toolbar-copy">
          <div className="usage-title-row">
            <h1 className="usage-title">{t("usage.title")}</h1>
            <span className="usage-subtitle-pill">
              {props.activeRangeLabel}
            </span>
          </div>
          <div className="usage-toolbar-subline">
            <span className="usage-subtitle">
              {props.selectedProviderCount} {t("usage.providers")}
            </span>
            <span
              className={`usage-status-pill${props.activeMaintenanceJob ? " is-active" : ""}`}
            >
              <span className="usage-status-dot" />
              <span>{props.maintenanceStatusText}</span>
            </span>
          </div>
        </div>
        <div className="usage-toolbar-actions">
          <div className="usage-range-group">
            {ranges.map((range) => {
              const active =
                customRange === null && props.rangeDays === range.days;
              return (
                <button
                  key={String(range.days)}
                  className={`usage-range-btn${active ? " active" : ""}`}
                  aria-pressed={active}
                  onClick={() => props.onRangeChange(range.days)}
                  type="button"
                >
                  {range.label()}
                </button>
              );
            })}
            <button
              className={`usage-range-btn${customRange ? " active" : ""}`}
              aria-pressed={customRange !== null}
              onClick={enterCustomRange}
              type="button"
            >
              {t("usage.rangeCustom")}
            </button>
          </div>
          {customRange && (
            <div className="usage-custom-range">
              <input
                type="date"
                className="usage-date-input"
                aria-label={t("usage.customRangeStart")}
                value={customRange.start}
                min={MIN_USAGE_DATE}
                max={customRange.end}
                onChange={(e) => updateCustomRange("start", e.currentTarget)}
              />
              <span className="usage-custom-range-sep">~</span>
              <input
                type="date"
                className="usage-date-input"
                aria-label={t("usage.customRangeEnd")}
                value={customRange.end}
                min={customRange.start}
                onChange={(e) => updateCustomRange("end", e.currentTarget)}
              />
            </div>
          )}
          <button
            className="usage-action-btn"
            onClick={props.onRefreshPricing}
            disabled={
              props.isRefreshingPricing || props.activeMaintenanceJob !== null
            }
            type="button"
          >
            {props.isRefreshingPricing
              ? "..."
              : t("settings.refreshPricingCatalog")}
          </button>
          <button
            className="usage-action-btn usage-action-btn-primary"
            onClick={props.onRequestRefreshUsage}
            disabled={props.activeMaintenanceJob !== null}
            type="button"
          >
            {props.activeMaintenanceJob === "refresh_usage"
              ? "..."
              : t("usage.refreshUsage")}
          </button>
        </div>
      </div>

      <div className="usage-toolbar-meta">
        <span
          className="usage-meta-pill"
          title={props.pricingStatusError ?? undefined}
        >
          {t("usage.pricingUpdatedShort").replace(
            "{count}",
            props.pricingModelCountLabel,
          )}
        </span>
        <span
          className="usage-meta-pill"
          title={props.pricingStatusError ?? undefined}
        >
          {t("usage.pricingUpdatedAtShort").replace(
            "{updatedAt}",
            props.formattedPricingUpdatedAt,
          )}
        </span>
        <span
          className="usage-meta-pill"
          title={props.indexStatsError ?? undefined}
        >
          {t("usage.usageUpdatedShort").replace(
            "{updatedAt}",
            props.formattedUsageUpdatedAt,
          )}
        </span>
      </div>

      <div className="usage-chips">
        <button
          className={`usage-chip usage-chip-all${props.allProvidersSelected ? " active" : " inactive"}`}
          aria-pressed={props.allProvidersSelected}
          onClick={props.onToggleAllProviders}
          type="button"
        >
          <span className="usage-chip-label">{t("usage.allProviders")}</span>
          <span className="usage-chip-count">
            {props.scannedProviderKeysCount}
          </span>
        </button>
        {props.scannedProviderSnapshots.map((snapshot) => {
          const info = props.providerInfo(snapshot.key);
          const active = props.isProviderSelected(snapshot.key);
          const filteredCount = props.providerSessionCount(snapshot.key);
          return (
            <button
              key={snapshot.key}
              className={`usage-chip${active ? " active" : " inactive"}`}
              aria-pressed={active}
              onClick={() => props.onToggleProvider(snapshot.key)}
              style={{ "--provider-accent": info.color } as CSSProperties}
              title={info.fullLabel}
              type="button"
            >
              <span
                className="usage-chip-dot"
                style={{ background: info.color }}
              />
              <span className="usage-chip-label">{info.label}</span>
              {filteredCount > 0 && (
                <span className="usage-chip-count">{filteredCount}</span>
              )}
            </button>
          );
        })}
      </div>
    </section>
  );
}
