import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
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
const TERMINAL_VALUES = TERMINAL_OPTIONS.map((option) => option.value);

export function GeneralSettings() {
  const { t, locale, setLocale } = useI18n();
  const theme = useTheme();
  const terminalApp = useTerminalApp();
  const showOrphans = useShowOrphans();

  return (
    <div className="settings-section">
      <div className="settings-section-title">{t("settings.general")}</div>

      <div className="settings-row">
        <div>
          <div className="settings-label">{t("settings.theme")}</div>
        </div>
        <Select
          value={theme}
          onValueChange={(value) => {
            if (THEME_VALUES.includes(value as Theme)) setTheme(value as Theme);
          }}
        >
          <SelectTrigger size="sm" className="w-40">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="light">{t("settings.themeLight")}</SelectItem>
            <SelectItem value="dark">{t("settings.themeDark")}</SelectItem>
            <SelectItem value="system">{t("settings.themeSystem")}</SelectItem>
          </SelectContent>
        </Select>
      </div>

      <div className="settings-row">
        <div>
          <div className="settings-label">{t("settings.language")}</div>
        </div>
        <Select
          value={locale}
          onValueChange={(value) => {
            if (LOCALE_VALUES.includes(value as Locale)) {
              void setLocale(value as Locale);
            }
          }}
        >
          <SelectTrigger size="sm" className="w-40">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="en">{t("settings.languageEnglish")}</SelectItem>
            <SelectItem value="zh">{t("settings.languageChinese")}</SelectItem>
          </SelectContent>
        </Select>
      </div>

      <div className="settings-row">
        <div>
          <div className="settings-label">{t("settings.terminal")}</div>
          <div className="settings-desc">{t("settings.terminalDesc")}</div>
        </div>
        <Select
          value={terminalApp}
          onValueChange={(value) => {
            if (TERMINAL_VALUES.includes(value as TerminalApp)) {
              setTerminalApp(value as TerminalApp);
            }
          }}
        >
          <SelectTrigger size="sm" className="w-40">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {TERMINAL_OPTIONS.map((option) => (
              <SelectItem key={option.value} value={option.value}>
                {option.label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>

      <div className="settings-row">
        <div>
          <div className="settings-label">{t("settings.showOrphans")}</div>
          <div className="settings-desc">{t("settings.showOrphansDesc")}</div>
        </div>
        <Switch
          checked={showOrphans}
          onCheckedChange={(checked) => setShowOrphans(checked)}
        />
      </div>
    </div>
  );
}
