import { invoke } from "@tauri-apps/api/core";
import { useI18n } from "@/i18n/index";
import { shortenHomePath } from "@/lib/formatters";
import type { ProviderSnapshot } from "@/lib/types";
import {
  useDisabledProviders,
  useDisabledProvidersError,
  toggleProvider,
} from "@/stores/settings";
import { useProviderSnapshotVersion } from "@/stores/providerSnapshots";
import { toastError } from "@/stores/toast";

export function DataSourceSettings(props: {
  providerSnapshots: () => ProviderSnapshot[];
}) {
  const { t } = useI18n();
  useProviderSnapshotVersion();
  const disabledProviders = useDisabledProviders();
  const disabledProvidersError = useDisabledProvidersError();

  return (
    <div className="settings-section">
      <div className="settings-section-title">{t("settings.dataSources")}</div>
      {disabledProvidersError && (
        <div className="session-error">{disabledProvidersError}</div>
      )}
      {props.providerSnapshots().map((info) => (
        <div className="settings-row" key={info.key}>
          <div>
            <div className="settings-label">{info.label}</div>
            <div className="settings-desc flex-center-gap-sm">
              <span title={shortenHomePath(info.path)}>
                {shortenHomePath(info.path)}
              </span>
              {info.exists && (
                <button
                  className="settings-open-folder"
                  title={t("settings.openInFinder")}
                  onClick={async () => {
                    try {
                      await invoke("open_external", { url: info.path });
                    } catch (e) {
                      toastError(String(e));
                    }
                  }}
                >
                  <svg
                    width="12"
                    height="12"
                    viewBox="0 0 24 24"
                    fill="none"
                    stroke="currentColor"
                    strokeWidth="2"
                  >
                    <path d="M18 13v6a2 2 0 01-2 2H5a2 2 0 01-2-2V8a2 2 0 012-2h6" />
                    <polyline points="15 3 21 3 21 9" />
                    <line x1="10" y1="14" x2="21" y2="3" />
                  </svg>
                </button>
              )}
            </div>
          </div>
          <div className="flex-center-gap-md">
            <span className="settings-stat">
              {info.session_count} {t("status.sessions")}
            </span>
            {info.exists && (
              <button
                className={`settings-btn${disabledProviders.includes(info.key) ? " settings-btn-danger" : ""}`}
                onClick={() => toggleProvider(info.key)}
              >
                {disabledProviders.includes(info.key)
                  ? t("settings.disabled")
                  : t("settings.enabled")}
              </button>
            )}
            {!info.exists && (
              <span className="settings-stat text-danger">
                {t("settings.disabled")}
              </span>
            )}
          </div>
        </div>
      ))}
    </div>
  );
}
