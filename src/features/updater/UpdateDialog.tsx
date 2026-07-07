import { useEffect, useState } from "react";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { useI18n } from "@/i18n/index";
import { downloadAndInstall, useAvailableVersion, useUpdaterError, useUpdaterPhase } from "@/features/updater/updater";

export function UpdateDialog() {
  const { t } = useI18n();
  const phase = useUpdaterPhase();
  const availableVersion = useAvailableVersion();
  const errorDetail = useUpdaterError();
  const [dismissedVersion, setDismissedVersion] = useState<string | null>(null);

  useEffect(() => {
    if (!availableVersion) setDismissedVersion(null);
  }, [availableVersion]);

  const busy = phase === "downloading" || phase === "installing";
  const canDownload = availableVersion !== null && (phase === "available" || phase === "error");
  const open =
    availableVersion !== null &&
    (busy || phase === "error" || (phase === "available" && dismissedVersion !== availableVersion));

  function dismiss() {
    if (busy || availableVersion === null) return;
    setDismissedVersion(availableVersion);
  }

  const title =
    phase === "error"
      ? t("updater.updateFailedTitle")
      : t("updater.updateAvailableTitle").replace("{version}", availableVersion ?? "");
  const description =
    phase === "error"
      ? t("updater.updateFailedDescription")
      : t("updater.updateAvailableDescription").replace("{version}", availableVersion ?? "");
  const primaryLabel =
    phase === "installing"
      ? t("updater.installing")
      : phase === "downloading"
        ? t("updater.downloading")
        : t("updater.downloadUpdate");

  return (
    <Dialog
      open={open}
      onOpenChange={(nextOpen) => {
        if (!nextOpen) dismiss();
      }}
    >
      <DialogContent showCloseButton={!busy}>
        <DialogHeader>
          <DialogTitle>{title}</DialogTitle>
          <DialogDescription>{description}</DialogDescription>
        </DialogHeader>

        {phase === "error" && errorDetail && <div className="session-error">{errorDetail}</div>}

        <DialogFooter>
          <Button variant="outline" onClick={dismiss} disabled={busy}>
            {t("updater.later")}
          </Button>
          <Button disabled={!canDownload || busy} onClick={() => void downloadAndInstall()}>
            {primaryLabel}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
