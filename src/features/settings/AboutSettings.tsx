import { Button } from "@/components/ui/button";
import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useI18n } from "@/i18n/index";
import { errorMessage } from "@/lib/errors";
import { invokeWithToast } from "@/lib/tauri";
import {
  useUpdaterPhase,
  useAvailableVersion,
  useUpdaterError,
  checkForUpdate,
  downloadAndInstall,
} from "@/features/updater/updater";

export function AboutSettings() {
  const { t } = useI18n();
  const [version, setVersion] = useState<string | null>(null);
  const [versionError, setVersionError] = useState<string | null>(null);
  const phase = useUpdaterPhase();
  const availableVersion = useAvailableVersion();
  const errorDetail = useUpdaterError();

  useEffect(() => {
    void (async () => {
      try {
        const { getVersion } = await import("@tauri-apps/api/app");
        setVersion(await getVersion());
        setVersionError(null);
      } catch (error) {
        console.error("Failed to load app version:", error);
        setVersion(null);
        setVersionError(errorMessage(error));
      }
    })();
  }, []);

  const buttonLabel = () => {
    switch (phase) {
      case "checking":
        return "...";
      case "upToDate":
        return t("settings.upToDate");
      case "available":
        return `↑ v${availableVersion}`;
      case "downloading":
      case "installing":
        return t("settings.updating");
      case "error":
        return t("settings.updateFailed");
      default:
        return t("settings.checkUpdate");
    }
  };

  const isDisabled = () =>
    phase === "checking" || phase === "upToDate" || phase === "downloading" || phase === "installing";

  function handleClick() {
    if (phase === "available") {
      void downloadAndInstall();
    } else if (phase === "idle" || phase === "error") {
      void checkForUpdate();
    }
  }

  return (
    <div className="settings-section">
      <div className="settings-section-title">{t("settings.about")}</div>

      <div className="settings-row">
        <div className="settings-label">{t("settings.version")}</div>
        <div style={{ display: "flex", alignItems: "center", gap: "8px" }}>
          <span className="settings-stat" title={versionError ?? undefined}>
            {version ?? "—"}
          </span>
          <Button
            variant="outline"
            size="sm"
            disabled={isDisabled()}
            onClick={handleClick}
            title={phase === "error" ? (errorDetail ?? "") : ""}
          >
            {buttonLabel()}
          </Button>
        </div>
      </div>

      <div className="settings-row">
        <div className="settings-label">{t("settings.github")}</div>
        <a
          className="settings-stat link-accent"
          href="https://github.com/tyql688/cc-session"
          onClick={(e) => {
            e.preventDefault();
            void invokeWithToast(
              invoke<void>("open_external", {
                url: "https://github.com/tyql688/cc-session",
              }),
              "open GitHub link",
            );
          }}
        >
          cc-session
        </a>
      </div>
    </div>
  );
}
