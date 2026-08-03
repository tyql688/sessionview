import { Folder, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { useI18n } from "@/i18n/index";
import { removeBlockedFolder, useBlockedFolders, useBlockedFoldersError } from "@/stores/settings";

interface BlockedViewProps {
  onRefreshTree?: () => void;
  onOpenExplorer: () => void;
}

export function BlockedView(props: BlockedViewProps) {
  const { t } = useI18n();
  const blockedFolders = useBlockedFolders();
  const blockedFoldersError = useBlockedFoldersError();

  return (
    <div className="blocked-view">
      <div className="explorer-header">{t("settings.blockedFolders")}</div>
      {blockedFoldersError && <div className="session-error">{blockedFoldersError}</div>}
      {!blockedFoldersError && blockedFolders.length > 0 ? (
        <div className="blocked-list">
          {blockedFolders.map((folder) => {
            const short = folder.split("/").slice(-2).join("/");
            return (
              <div className="blocked-item" title={folder} key={folder}>
                <Folder className="blocked-item-icon" size={14} strokeWidth={1.5} aria-hidden="true" />
                <span className="blocked-item-label">{short}</span>
                <Button
                  variant="ghost"
                  size="icon-xs"
                  title={t("settings.unblock")}
                  aria-label={`${t("settings.unblock")}: ${short}`}
                  onClick={() => {
                    removeBlockedFolder(folder);
                    props.onRefreshTree?.();
                  }}
                >
                  <X className="size-3" aria-hidden="true" />
                </Button>
              </div>
            );
          })}
        </div>
      ) : (
        !blockedFoldersError && (
          <div className="empty-state">
            <p className="empty-state-text">{t("settings.noBlockedFolders")}</p>
            <p className="empty-state-hint">{t("blocked.hint")}</p>
            <Button variant="outline" size="sm" onClick={props.onOpenExplorer}>
              {t("blocked.browseProjects")}
            </Button>
          </div>
        )
      )}
    </div>
  );
}
