import { useI18n } from "@/i18n/index";
import {
  removeBlockedFolder,
  useBlockedFolders,
  useBlockedFoldersError,
} from "@/stores/settings";

export function BlockedView(props: { onRefreshTree?: () => void }) {
  const { t } = useI18n();
  const blockedFolders = useBlockedFolders();
  const blockedFoldersError = useBlockedFoldersError();

  return (
    <div className="blocked-view">
      <div className="explorer-header">{t("settings.blockedFolders")}</div>
      {blockedFoldersError && (
        <div className="session-error">{blockedFoldersError}</div>
      )}
      {!blockedFoldersError && blockedFolders.length > 0 ? (
        <div className="blocked-list">
          {blockedFolders.map((folder) => {
            const short = folder.split("/").slice(-2).join("/");
            return (
              <div className="blocked-item" title={folder} key={folder}>
                <svg
                  width="14"
                  height="14"
                  fill="none"
                  stroke="currentColor"
                  strokeWidth="1.5"
                  viewBox="0 0 24 24"
                  className="blocked-item-icon"
                >
                  <path d="M22 19a2 2 0 01-2 2H4a2 2 0 01-2-2V5a2 2 0 012-2h5l2 3h9a2 2 0 012 2z" />
                </svg>
                <span className="blocked-item-label">{short}</span>
                <button
                  className="blocked-item-btn"
                  title={t("settings.unblock")}
                  onClick={() => {
                    removeBlockedFolder(folder);
                    props.onRefreshTree?.();
                  }}
                >
                  <svg
                    width="12"
                    height="12"
                    fill="none"
                    stroke="currentColor"
                    strokeWidth="1.5"
                    viewBox="0 0 24 24"
                  >
                    <line x1="18" y1="6" x2="6" y2="18" />
                    <line x1="6" y1="6" x2="18" y2="18" />
                  </svg>
                </button>
              </div>
            );
          })}
        </div>
      ) : (
        !blockedFoldersError && (
          <div className="empty-state">
            <p className="empty-state-text">{t("settings.noBlockedFolders")}</p>
            <p className="empty-state-hint">{t("blocked.hint")}</p>
          </div>
        )
      )}
    </div>
  );
}
