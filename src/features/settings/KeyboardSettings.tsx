import { useI18n } from "@/i18n/index";
import { isMac } from "@/lib/platform";

export function KeyboardSettings() {
  const { t } = useI18n();

  return (
    <div className="settings-section">
      <div className="settings-section-title">{t("keyboard.title")}</div>

      <div className="settings-shortcuts-group">
        <div className="settings-shortcuts-label">
          {t("keyboard.navigation")}
        </div>
        <div className="settings-shortcut-row">
          <span>{t("keyboard.search")}</span>
          <kbd>{isMac ? "\u2318" : "Ctrl+"}K</kbd>
        </div>
        <div className="settings-shortcut-row">
          <span>{t("keyboard.switchTab")}</span>
          <kbd>{isMac ? "\u2318" : "Ctrl+"}1-9</kbd>
        </div>
        <div className="settings-shortcut-row">
          <span>{t("keyboard.nextTab")}</span>
          <kbd>{isMac ? "\u2318]" : "Ctrl+Tab"}</kbd>
        </div>
        <div className="settings-shortcut-row">
          <span>{t("keyboard.prevTab")}</span>
          <kbd>{isMac ? "\u2318[" : "Shift+Ctrl+Tab"}</kbd>
        </div>
      </div>

      <div className="settings-shortcuts-group">
        <div className="settings-shortcuts-label">{t("keyboard.tabs")}</div>
        <div className="settings-shortcut-row">
          <span>{t("keyboard.closeTab")}</span>
          <kbd>{isMac ? "\u2318" : "Ctrl+"}W</kbd>
        </div>
        <div className="settings-shortcut-row">
          <span>{t("keyboard.closeAllTabs")}</span>
          <kbd>{isMac ? "\u21E7\u2318" : "Shift+Ctrl+"}W</kbd>
        </div>
      </div>

      <div className="settings-shortcuts-group">
        <div className="settings-shortcuts-label">{t("keyboard.session")}</div>
        <div className="settings-shortcut-row">
          <span>{t("keyboard.resumeSession")}</span>
          <kbd>{isMac ? "\u21E7\u2318" : "Shift+Ctrl+"}R</kbd>
        </div>
        <div className="settings-shortcut-row">
          <span>{t("keyboard.exportSession")}</span>
          <kbd>{isMac ? "\u21E7\u2318" : "Shift+Ctrl+"}E</kbd>
        </div>
        <div className="settings-shortcut-row">
          <span>{t("keyboard.toggleFavorite")}</span>
          <kbd>{isMac ? "\u2318" : "Ctrl+"}B</kbd>
        </div>
        <div className="settings-shortcut-row">
          <span>{t("keyboard.toggleWatch")}</span>
          <kbd>{isMac ? "\u2318" : "Ctrl+"}L</kbd>
        </div>
        <div className="settings-shortcut-row">
          <span>{t("keyboard.deleteSession")}</span>
          <kbd>
            {isMac ? "\u2318" : "Ctrl+"}
            {"\u232B"}
          </kbd>
        </div>
      </div>

      <div className="settings-shortcuts-group">
        <div className="settings-shortcuts-label">{t("keyboard.general")}</div>
        <div className="settings-shortcut-row">
          <span>{t("keyboard.showShortcuts")}</span>
          <kbd>{isMac ? "\u2318" : "Ctrl+"}/</kbd>
        </div>
        <div className="settings-shortcut-row">
          <span>{t("keyboard.showShortcuts")}</span>
          <kbd>?</kbd>
        </div>
        <div className="settings-shortcut-row">
          <span>{t("keyboard.escape")}</span>
          <kbd>Esc</kbd>
        </div>
      </div>
    </div>
  );
}
