import { For, Show } from "solid-js";
import type { Accessor } from "solid-js";
import { useI18n } from "../../i18n/index";
import type { MaintenanceJob, ProviderSnapshot } from "../../lib/types";
import type { CustomDateRange } from "../../stores/usageView";
import { toLocalISODate } from "../../lib/formatters";

export interface ProviderChipInfo {
  color: string;
  label: string;
  fullLabel: string;
}

export interface ToolbarProps {
  activeRangeLabel: Accessor<string>;
  selectedProviderCount: Accessor<number>;
  activeMaintenanceJob: Accessor<MaintenanceJob | null>;
  maintenanceStatusText: Accessor<string>;

  rangeDays: Accessor<number | null>;
  onRangeChange: (days: number | null) => void;
  customRange: Accessor<CustomDateRange | null>;
  onCustomRangeChange: (range: CustomDateRange) => void;

  isRefreshingPricing: Accessor<boolean>;
  onRefreshPricing: () => void;
  onRequestRefreshUsage: () => void;

  formattedPricingUpdatedAt: Accessor<string>;
  formattedUsageUpdatedAt: Accessor<string>;
  pricingModelCountLabel: Accessor<string>;
  pricingStatusError: Accessor<string | null>;
  indexStatsError: Accessor<string | null>;

  scannedProviderSnapshots: Accessor<ProviderSnapshot[]>;
  scannedProviderKeysCount: Accessor<number>;
  allProvidersSelected: Accessor<boolean>;
  isProviderSelected: (key: string) => boolean;
  onToggleProvider: (key: string) => void;
  onToggleAllProviders: () => void;
  providerInfo: (key: string) => ProviderChipInfo;
  providerSessionCount: (key: string) => number;
}

export function Toolbar(props: ToolbarProps) {
  const { t } = useI18n();

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
    if (props.customRange()) return;
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
    const current = props.customRange();
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
    <section class="usage-card usage-toolbar-card">
      <div class="usage-toolbar-main">
        <div class="usage-toolbar-copy">
          <div class="usage-title-row">
            <h1 class="usage-title">{t("usage.title")}</h1>
            <span class="usage-subtitle-pill">{props.activeRangeLabel()}</span>
          </div>
          <div class="usage-toolbar-subline">
            <span class="usage-subtitle">
              {props.selectedProviderCount()} {t("usage.providers")}
            </span>
            <span
              class={`usage-status-pill${props.activeMaintenanceJob() ? " is-active" : ""}`}
            >
              <span class="usage-status-dot" />
              <span>{props.maintenanceStatusText()}</span>
            </span>
          </div>
        </div>
        <div class="usage-toolbar-actions">
          <div class="usage-range-group">
            <For each={ranges}>
              {(range) => {
                const active = () =>
                  props.customRange() === null &&
                  props.rangeDays() === range.days;
                return (
                  <button
                    class={`usage-range-btn${active() ? " active" : ""}`}
                    aria-pressed={active()}
                    onClick={() => props.onRangeChange(range.days)}
                    type="button"
                  >
                    {range.label()}
                  </button>
                );
              }}
            </For>
            <button
              class={`usage-range-btn${props.customRange() ? " active" : ""}`}
              aria-pressed={props.customRange() !== null}
              onClick={enterCustomRange}
              type="button"
            >
              {t("usage.rangeCustom")}
            </button>
          </div>
          <Show when={props.customRange()}>
            {(range) => (
              <div class="usage-custom-range">
                <input
                  type="date"
                  class="usage-date-input"
                  aria-label={t("usage.customRangeStart")}
                  value={range().start}
                  min={MIN_USAGE_DATE}
                  max={range().end}
                  onChange={(e) => updateCustomRange("start", e.currentTarget)}
                />
                <span class="usage-custom-range-sep">~</span>
                <input
                  type="date"
                  class="usage-date-input"
                  aria-label={t("usage.customRangeEnd")}
                  value={range().end}
                  min={range().start}
                  onChange={(e) => updateCustomRange("end", e.currentTarget)}
                />
              </div>
            )}
          </Show>
          <button
            class="usage-action-btn"
            onClick={props.onRefreshPricing}
            disabled={
              props.isRefreshingPricing() ||
              props.activeMaintenanceJob() !== null
            }
            type="button"
          >
            {props.isRefreshingPricing()
              ? "..."
              : t("settings.refreshPricingCatalog")}
          </button>
          <button
            class="usage-action-btn usage-action-btn-primary"
            onClick={props.onRequestRefreshUsage}
            disabled={props.activeMaintenanceJob() !== null}
            type="button"
          >
            {props.activeMaintenanceJob() === "refresh_usage"
              ? "..."
              : t("usage.refreshUsage")}
          </button>
        </div>
      </div>

      <div class="usage-toolbar-meta">
        <span
          class="usage-meta-pill"
          title={props.pricingStatusError() ?? undefined}
        >
          {t("usage.pricingUpdatedShort").replace(
            "{count}",
            props.pricingModelCountLabel(),
          )}
        </span>
        <span
          class="usage-meta-pill"
          title={props.pricingStatusError() ?? undefined}
        >
          {t("usage.pricingUpdatedAtShort").replace(
            "{updatedAt}",
            props.formattedPricingUpdatedAt(),
          )}
        </span>
        <span
          class="usage-meta-pill"
          title={props.indexStatsError() ?? undefined}
        >
          {t("usage.usageUpdatedShort").replace(
            "{updatedAt}",
            props.formattedUsageUpdatedAt(),
          )}
        </span>
      </div>

      <div class="usage-chips">
        <button
          class={`usage-chip usage-chip-all${props.allProvidersSelected() ? " active" : " inactive"}`}
          aria-pressed={props.allProvidersSelected()}
          onClick={props.onToggleAllProviders}
          type="button"
        >
          <span class="usage-chip-label">{t("usage.allProviders")}</span>
          <span class="usage-chip-count">
            {props.scannedProviderKeysCount()}
          </span>
        </button>
        <For each={props.scannedProviderSnapshots()}>
          {(snapshot) => {
            const info = () => props.providerInfo(snapshot.key);
            const active = () => props.isProviderSelected(snapshot.key);
            const filteredCount = () =>
              props.providerSessionCount(snapshot.key);
            return (
              <button
                class={`usage-chip${active() ? " active" : " inactive"}`}
                aria-pressed={active()}
                onClick={() => props.onToggleProvider(snapshot.key)}
                style={{ "--provider-accent": info().color }}
                title={info().fullLabel}
                type="button"
              >
                <span
                  class="usage-chip-dot"
                  style={{ background: info().color }}
                />
                <span class="usage-chip-label">{info().label}</span>
                <Show when={filteredCount() > 0}>
                  <span class="usage-chip-count">{filteredCount()}</span>
                </Show>
              </button>
            );
          }}
        </For>
      </div>
    </section>
  );
}
