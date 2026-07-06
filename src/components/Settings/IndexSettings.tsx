import { useCallback, useEffect, useState } from "react";
import { useI18n } from "@/i18n/index";
import type { IndexStats } from "@/lib/types";
import { getIndexStats, startRebuildIndex, clearIndex } from "@/lib/tauri";
import { toast, toastError, toastInfo } from "@/stores/toast";
import { errorMessage } from "@/lib/errors";
import { formatFileSize } from "@/lib/formatters";
import { ConfirmDialog } from "@/components/ConfirmDialog";

export function IndexSettings(props: { onIndexChanged: () => void }) {
  const { t } = useI18n();
  const [showClearIndexConfirm, setShowClearIndexConfirm] = useState(false);

  const [indexStats, setIndexStats] = useState<IndexStats | undefined>(
    undefined,
  );
  const [statsError, setStatsError] = useState<unknown>(null);

  const refetchStats = useCallback(async () => {
    try {
      const stats = await getIndexStats();
      setIndexStats(stats);
      setStatsError(null);
    } catch (e) {
      setStatsError(e);
    }
  }, []);

  useEffect(() => {
    void refetchStats();
  }, [refetchStats]);

  const indexStatsError = statsError ? errorMessage(statsError) : null;

  async function handleRebuildIndex() {
    try {
      const started = await startRebuildIndex();
      if (!started) {
        toastInfo(t("toast.maintenanceBusy"));
        return;
      }
    } catch (_e) {
      toastError(t("toast.rebuildFailed"));
    }
  }

  return (
    <div className="settings-section">
      <div className="settings-section-title">{t("settings.index")}</div>

      {indexStatsError && (
        <div className="session-error">{indexStatsError}</div>
      )}

      <div className="settings-row">
        <div className="settings-label">{t("settings.totalSessions")}</div>
        <span className="settings-stat">
          {indexStats ? indexStats.session_count.toLocaleString() : "—"}
        </span>
      </div>

      <div className="settings-row">
        <div className="settings-label">{t("settings.dbSize")}</div>
        <span className="settings-stat">
          {indexStats ? formatFileSize(indexStats.db_size_bytes) : "—"}
        </span>
      </div>

      <div className="settings-row settings-row-spaced">
        <button className="settings-btn" onClick={handleRebuildIndex}>
          {t("settings.rebuildIndex")}
        </button>
        <button
          className="settings-btn settings-btn-danger"
          onClick={() => setShowClearIndexConfirm(true)}
        >
          {t("settings.clearIndex")}
        </button>
      </div>

      <div className="settings-help-text">{t("settings.rebuildIndexNote")}</div>
      <div className="settings-help-text">
        {t("settings.rebuildShortcutNote")}
      </div>

      <ConfirmDialog
        open={showClearIndexConfirm}
        title={t("settings.clearIndex")}
        message={t("settings.clearIndexConfirm")}
        confirmLabel={t("settings.clearIndex")}
        onConfirm={async () => {
          setShowClearIndexConfirm(false);
          try {
            await clearIndex();
            toast(t("toast.clearIndexOk"));
            void refetchStats();
            props.onIndexChanged();
          } catch (e) {
            toastError(errorMessage(e));
          }
        }}
        onCancel={() => setShowClearIndexConfirm(false)}
        danger={true}
      />
    </div>
  );
}
