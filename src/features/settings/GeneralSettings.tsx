import { useI18n } from "@/i18n/index";
import type { Locale } from "@/i18n/index";
import { useTheme, setTheme } from "@/stores/theme";
import type { Theme } from "@/stores/theme";
import {
  useTerminalApp,
  setTerminalApp,
  useShowOrphans,
  setShowOrphans,
} from "@/stores/settings";
import type { TerminalApp } from "@/stores/settings";
import { isMac, isWindows } from "@/lib/platform";

const validThemes: Theme[] = ["light", "dark", "system"];
const validTerminals: TerminalApp[] = isMac
  ? ["terminal", "iterm2", "ghostty", "kitty", "warp", "wezterm", "alacritty"]
  : isWindows
    ? ["windows-terminal", "powershell", "cmd"]
    : ["alacritty", "kitty", "wezterm", "gnome-terminal", "konsole", "xterm"];

function handleThemeChange(value: string) {
  if (validThemes.includes(value as Theme)) setTheme(value as Theme);
}

function handleTerminalChange(value: string) {
  if (validTerminals.includes(value as TerminalApp))
    setTerminalApp(value as TerminalApp);
}

export function GeneralSettings() {
  const { t, locale, setLocale } = useI18n();
  const theme = useTheme();
  const terminalApp = useTerminalApp();
  const showOrphans = useShowOrphans();

  function handleLanguageChange(value: string) {
    setLocale(value as Locale);
  }

  return (
    <div className="settings-section">
      <div className="settings-section-title">{t("settings.general")}</div>

      <div className="settings-row">
        <div>
          <div className="settings-label">{t("settings.theme")}</div>
        </div>
        <select
          className="settings-select"
          value={theme}
          onChange={(e) => handleThemeChange(e.currentTarget.value)}
        >
          <option value="light">{t("settings.themeLight")}</option>
          <option value="dark">{t("settings.themeDark")}</option>
          <option value="system">{t("settings.themeSystem")}</option>
        </select>
      </div>

      <div className="settings-row">
        <div>
          <div className="settings-label">{t("settings.language")}</div>
        </div>
        <select
          className="settings-select"
          value={locale}
          onChange={(e) => handleLanguageChange(e.currentTarget.value)}
        >
          <option value="en">{t("settings.languageEnglish")}</option>
          <option value="zh">{t("settings.languageChinese")}</option>
        </select>
      </div>

      <div className="settings-row">
        <div>
          <div className="settings-label">{t("settings.terminal")}</div>
          <div className="settings-desc">{t("settings.terminalDesc")}</div>
        </div>
        <select
          className="settings-select"
          value={terminalApp}
          onChange={(e) => handleTerminalChange(e.currentTarget.value)}
        >
          {isMac ? (
            <>
              <option value="terminal">Terminal.app</option>
              <option value="iterm2">iTerm2</option>
              <option value="ghostty">Ghostty</option>
              <option value="kitty">Kitty</option>
              <option value="warp">Warp</option>
              <option value="wezterm">WezTerm</option>
              <option value="alacritty">Alacritty</option>
            </>
          ) : isWindows ? (
            <>
              <option value="windows-terminal">Windows Terminal</option>
              <option value="powershell">PowerShell</option>
              <option value="cmd">Command Prompt</option>
            </>
          ) : (
            <>
              <option value="alacritty">Alacritty</option>
              <option value="kitty">Kitty</option>
              <option value="wezterm">WezTerm</option>
              <option value="gnome-terminal">GNOME Terminal</option>
              <option value="konsole">Konsole</option>
              <option value="xterm">xterm</option>
            </>
          )}
        </select>
      </div>

      <div className="settings-row">
        <div>
          <div className="settings-label">{t("settings.showOrphans")}</div>
          <div className="settings-desc">{t("settings.showOrphansDesc")}</div>
        </div>
        <input
          type="checkbox"
          className="settings-checkbox"
          checked={showOrphans}
          onChange={(e) => setShowOrphans(e.currentTarget.checked)}
        />
      </div>
    </div>
  );
}
