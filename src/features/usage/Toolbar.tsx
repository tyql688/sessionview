import { Button } from "@/components/ui/button";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import type { CSSProperties } from "react";
import { useI18n } from "@/i18n/index";
import type { MaintenanceJob, ProviderSnapshot } from "@/lib/types";
import type { CustomDateRange } from "@/features/usage/usageView";
import { toLocalISODate } from "@/lib/formatters";
import { cn } from "@/lib/utils";
import { DatePicker } from "@/features/usage/DatePicker";

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

  const updateCustomRange = (field: keyof CustomDateRange, value: string) => {
    const current = customRange;
    if (!current) return;
    if (!value || value < MIN_USAGE_DATE) return;
    const next = { ...current, [field]: value };
    // Keep the window ordered no matter which side the user moved.
    if (next.start > next.end) {
      props.onCustomRangeChange(
        field === "start" ? { start: next.start, end: next.start } : { start: next.end, end: next.end },
      );
      return;
    }
    props.onCustomRangeChange(next);
  };

  const activeRangeValue = customRange !== null ? "custom" : props.rangeDays === null ? "all" : String(props.rangeDays);

  const handleRangeValueChange = (next: string[]) => {
    const value = next[0];
    if (!value) return;
    if (value === "custom") {
      enterCustomRange();
      return;
    }
    props.onRangeChange(value === "all" ? null : Number(value));
  };

  return (
    <section className="usage-card usage-toolbar-card">
      <div className="usage-toolbar-main">
        <div className="usage-toolbar-copy">
          <div className="usage-title-row">
            <h1 className="usage-title">{t("usage.title")}</h1>
            <span className="usage-subtitle-pill">{props.activeRangeLabel}</span>
          </div>
          <div className="usage-toolbar-subline">
            <span className="usage-subtitle">
              {props.selectedProviderCount} {t("usage.providers")}
            </span>
            <span className={`usage-status-pill${props.activeMaintenanceJob ? " is-active" : ""}`}>
              <span className="usage-status-dot" />
              <span>{props.maintenanceStatusText}</span>
            </span>
          </div>
        </div>
        <div className="usage-toolbar-actions">
          <ToggleGroup
            className="usage-range-group"
            size="sm"
            spacing={0}
            value={[activeRangeValue]}
            onValueChange={handleRangeValueChange}
          >
            {ranges.map((range) => {
              const active = customRange === null && props.rangeDays === range.days;
              const value = range.days === null ? "all" : String(range.days);
              return (
                <ToggleGroupItem
                  key={value}
                  value={value}
                  className={cn("usage-range-btn h-auto min-w-0", active && "active")}
                >
                  {range.label()}
                </ToggleGroupItem>
              );
            })}
            <ToggleGroupItem value="custom" className={cn("usage-range-btn h-auto min-w-0", customRange && "active")}>
              {t("usage.rangeCustom")}
            </ToggleGroupItem>
          </ToggleGroup>
          {customRange && (
            <div className="usage-custom-range">
              <DatePicker
                label={t("usage.customRangeStart")}
                value={customRange.start}
                min={MIN_USAGE_DATE}
                max={customRange.end}
                onChange={(value) => updateCustomRange("start", value)}
              />
              <span className="usage-custom-range-sep">~</span>
              <DatePicker
                label={t("usage.customRangeEnd")}
                value={customRange.end}
                min={customRange.start}
                onChange={(value) => updateCustomRange("end", value)}
              />
            </div>
          )}
          <Button
            variant="outline"
            size="sm"
            onClick={props.onRefreshPricing}
            disabled={props.isRefreshingPricing || props.activeMaintenanceJob !== null}
            type="button"
          >
            {props.isRefreshingPricing ? "..." : t("settings.refreshPricingCatalog")}
          </Button>
          <Button
            size="sm"
            onClick={props.onRequestRefreshUsage}
            disabled={props.activeMaintenanceJob !== null}
            type="button"
          >
            {props.activeMaintenanceJob === "refresh_usage" ? "..." : t("usage.refreshUsage")}
          </Button>
        </div>
      </div>

      <div className="usage-toolbar-meta">
        <span className="usage-meta-pill" title={props.pricingStatusError ?? undefined}>
          {t("usage.pricingUpdatedShort").replace("{count}", props.pricingModelCountLabel)}
        </span>
        <span className="usage-meta-pill" title={props.pricingStatusError ?? undefined}>
          {t("usage.pricingUpdatedAtShort").replace("{updatedAt}", props.formattedPricingUpdatedAt)}
        </span>
        <span className="usage-meta-pill" title={props.indexStatsError ?? undefined}>
          {t("usage.usageUpdatedShort").replace("{updatedAt}", props.formattedUsageUpdatedAt)}
        </span>
      </div>

      <div className="usage-chips">
        <Button
          variant="ghost"
          size="sm"
          className={cn(
            "usage-chip usage-chip-all active:translate-y-0",
            props.allProvidersSelected ? "active" : "inactive",
          )}
          aria-pressed={props.allProvidersSelected}
          onClick={props.onToggleAllProviders}
          type="button"
        >
          <span className="usage-chip-label">{t("usage.allProviders")}</span>
          <span className="usage-chip-count">{props.scannedProviderKeysCount}</span>
        </Button>
        {props.scannedProviderSnapshots.map((snapshot) => {
          const info = props.providerInfo(snapshot.key);
          const active = props.isProviderSelected(snapshot.key);
          const filteredCount = props.providerSessionCount(snapshot.key);
          return (
            <Button
              key={snapshot.key}
              variant="ghost"
              size="sm"
              className={cn("usage-chip active:translate-y-0", active ? "active" : "inactive")}
              aria-pressed={active}
              onClick={() => props.onToggleProvider(snapshot.key)}
              style={{ "--provider-accent": info.color } as CSSProperties}
              title={info.fullLabel}
              type="button"
            >
              <span className="usage-chip-dot" style={{ background: info.color }} />
              <span className="usage-chip-label">{info.label}</span>
              {filteredCount > 0 && <span className="usage-chip-count">{filteredCount}</span>}
            </Button>
          );
        })}
      </div>
    </section>
  );
}
