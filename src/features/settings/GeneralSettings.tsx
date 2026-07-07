import { SelectField } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { useI18n } from "@/i18n/index";
import type { Locale } from "@/i18n/index";
import { useTheme, setTheme } from "@/stores/theme";
import type { Theme } from "@/stores/theme";
import {
  useTerminalApp,
  setTerminalApp,
  useShowOrphans,
  setShowOrphans,
  useFocusMode,
  setFocusMode,
} from "@/stores/settings";
import type { TerminalApp } from "@/stores/settings";
import { isMac, isWindows } from "@/lib/platform";

const TERMINAL_OPTIONS: { value: TerminalApp; label: string }[] = isMac
  ? [
      { value: "terminal", label: "Terminal.app" },
      { value: "iterm2", label: "iTerm2" },
      { value: "ghostty", label: "Ghostty" },
      { value: "kitty", label: "Kitty" },
      { value: "warp", label: "Warp" },
      { value: "wezterm", label: "WezTerm" },
      { value: "alacritty", label: "Alacritty" },
    ]
  : isWindows
    ? [
        { value: "windows-terminal", label: "Windows Terminal" },
        { value: "powershell", label: "PowerShell" },
        { value: "cmd", label: "Command Prompt" },
      ]
    : [
        { value: "alacritty", label: "Alacritty" },
        { value: "kitty", label: "Kitty" },
        { value: "wezterm", label: "WezTerm" },
        { value: "gnome-terminal", label: "GNOME Terminal" },
        { value: "konsole", label: "Konsole" },
        { value: "xterm", label: "xterm" },
      ];

const THEME_VALUES: Theme[] = ["light", "dark", "system"];
const LOCALE_VALUES: Locale[] = ["en", "zh"];

export function GeneralSettings() {
  const { t, locale, setLocale } = useI18n();
  const theme = useTheme();
  const terminalApp = useTerminalApp();
  const showOrphans = useShowOrphans();
  const focusMode = useFocusMode();

  const themeOptions = THEME_VALUES.map((value) => ({
    value,
    label:
      value === "light"
        ? t("settings.themeLight")
        : value === "dark"
          ? t("settings.themeDark")
          : t("settings.themeSystem"),
  }));
  const localeOptions = LOCALE_VALUES.map((value) => ({
    value,
    label: value === "en" ? t("settings.languageEnglish") : t("settings.languageChinese"),
  }));

  return (
    <div className="settings-section">
      <div className="settings-section-title">{t("settings.general")}</div>

      <div className="settings-row">
        <div>
          <div className="settings-label">{t("settings.theme")}</div>
        </div>
        <SelectField
          value={theme}
          options={themeOptions}
          onValueChange={setTheme}
          triggerClassName="w-44"
          aria-label={t("settings.theme")}
        />
      </div>

      <div className="settings-row">
        <div>
          <div className="settings-label">{t("settings.language")}</div>
        </div>
        <SelectField
          value={locale}
          options={localeOptions}
          onValueChange={(value) => void setLocale(value)}
          triggerClassName="w-44"
          aria-label={t("settings.language")}
        />
      </div>

      <div className="settings-row">
        <div>
          <div className="settings-label">{t("settings.terminal")}</div>
          <div className="settings-desc">{t("settings.terminalDesc")}</div>
        </div>
        <SelectField
          value={terminalApp}
          options={TERMINAL_OPTIONS}
          onValueChange={setTerminalApp}
          triggerClassName="w-44"
          aria-label={t("settings.terminal")}
        />
      </div>

      <div className="settings-row">
        <div>
          <div className="settings-label">{t("settings.showOrphans")}</div>
          <div className="settings-desc">{t("settings.showOrphansDesc")}</div>
        </div>
        <Switch checked={showOrphans} onCheckedChange={(checked) => setShowOrphans(checked)} />
      </div>

      <div className="settings-row">
        <div>
          <div className="settings-label">{t("settings.focusMode")}</div>
          <div className="settings-desc">{t("settings.focusModeDesc")}</div>
        </div>
        <Switch checked={focusMode} onCheckedChange={(checked) => setFocusMode(checked)} />
      </div>
    </div>
  );
}
