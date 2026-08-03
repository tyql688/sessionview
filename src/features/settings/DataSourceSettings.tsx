import { ExternalLink } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import { useI18n } from "@/i18n/index";
import { shortenHomePath } from "@/lib/formatters";
import { openInFolder } from "@/lib/tauri";
import type { ProviderSnapshot } from "@/lib/types";
import { useDisabledProviders, useDisabledProvidersError, toggleProvider } from "@/stores/settings";
import { useProviderSnapshotVersion } from "@/stores/providerSnapshots";
import { toastError } from "@/stores/toast";

export function DataSourceSettings(props: { providerSnapshots: () => ProviderSnapshot[] }) {
  const { t } = useI18n();
  useProviderSnapshotVersion();
  const disabledProviders = useDisabledProviders();
  const disabledProvidersError = useDisabledProvidersError();

  return (
    <div className="settings-section">
      <div className="settings-section-title">{t("settings.dataSources")}</div>
      {disabledProvidersError && <div className="session-error">{disabledProvidersError}</div>}
      {props.providerSnapshots().map((info) => {
        const enabled = !disabledProviders.includes(info.key);
        const pathLabel = shortenHomePath(info.path);
        return (
          <div className="settings-row" key={info.key}>
            <div>
              <div className="settings-label">{info.label}</div>
              <div className="settings-desc flex-center-gap-sm">
                <span title={pathLabel}>{pathLabel}</span>
                {info.exists && (
                  <Button
                    variant="ghost"
                    size="icon-xs"
                    className="settings-open-folder active:translate-y-0"
                    title={t("settings.openInFinder")}
                    aria-label={t("settings.openInFinder")}
                    onClick={async () => {
                      try {
                        await openInFolder(info.path);
                      } catch (e) {
                        toastError(String(e));
                      }
                    }}
                  >
                    <ExternalLink size={12} aria-hidden="true" />
                  </Button>
                )}
              </div>
            </div>
            <div className="flex-center-gap-md">
              <span className="settings-stat">
                {info.session_count} {t("status.sessions")}
              </span>
              {info.exists && (
                <>
                  <span className="settings-stat">{enabled ? t("settings.enabled") : t("settings.disabled")}</span>
                  <Switch
                    checked={enabled}
                    aria-label={t("settings.providerEnabled").replace("{provider}", info.label)}
                    onCheckedChange={() => toggleProvider(info.key)}
                  />
                </>
              )}
              {!info.exists && <span className="settings-stat text-danger">{t("settings.disabled")}</span>}
            </div>
          </div>
        );
      })}
    </div>
  );
}
