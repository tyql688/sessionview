import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { useI18n } from "../i18n/index";
import { isMac } from "../lib/platform";

const mod = isMac ? "⌘" : "Ctrl+";
const shift = isMac ? "⇧" : "Shift+";
const opt = isMac ? "⌥" : "Alt+";

interface ShortcutItem {
  keys: string;
  descKey: string;
}

interface ShortcutCategory {
  categoryKey: string;
  items: ShortcutItem[];
}

const shortcuts: ShortcutCategory[] = [
  {
    categoryKey: "keyboard.navigation",
    items: [
      { keys: `${mod}K`, descKey: "keyboard.globalSearch" },
      { keys: `${mod}1-9`, descKey: "keyboard.switchTab" },
      { keys: isMac ? `${mod}]` : "Ctrl+Tab", descKey: "keyboard.nextTab" },
      {
        keys: isMac ? `${mod}[` : `${shift}Ctrl+Tab`,
        descKey: "keyboard.prevTab",
      },
    ],
  },
  {
    categoryKey: "keyboard.tabs",
    items: [
      { keys: `${mod}W`, descKey: "keyboard.closeTab" },
      { keys: `${shift}${mod}W`, descKey: "keyboard.closeAllTabs" },
    ],
  },
  {
    categoryKey: "keyboard.session",
    items: [
      { keys: `${mod}F`, descKey: "keyboard.findInSession" },
      { keys: `${shift}${mod}R`, descKey: "keyboard.resumeSession" },
      { keys: `${shift}${mod}E`, descKey: "keyboard.exportSession" },
      { keys: `${mod}B`, descKey: "keyboard.toggleFavorite" },
      { keys: `${mod}L`, descKey: "keyboard.toggleWatch" },
      { keys: `${mod}⌫`, descKey: "keyboard.deleteSession" },
    ],
  },
  {
    categoryKey: "keyboard.splitView",
    items: [
      { keys: `${mod}\\`, descKey: "keyboard.splitEditor" },
      {
        keys: isMac ? `${mod}${opt}←` : `Ctrl+${opt}←`,
        descKey: "keyboard.focusGroupLeft",
      },
      {
        keys: isMac ? `${mod}${opt}→` : `Ctrl+${opt}→`,
        descKey: "keyboard.focusGroupRight",
      },
    ],
  },
  {
    categoryKey: "keyboard.general",
    items: [
      { keys: `${mod},`, descKey: "keyboard.openSettings" },
      { keys: `${mod}R`, descKey: "keyboard.refresh" },
      { keys: `${mod}/`, descKey: "keyboard.showShortcuts" },
      { keys: "?", descKey: "keyboard.showShortcuts" },
      { keys: "Esc", descKey: "keyboard.escape" },
    ],
  },
];

export function KeyboardOverlay(props: { show: boolean; onClose: () => void }) {
  const { t } = useI18n();

  return (
    <Dialog
      open={props.show}
      onOpenChange={(open) => {
        if (!open) props.onClose();
      }}
    >
      <DialogContent className="sm:max-w-2xl">
        <DialogHeader>
          <DialogTitle>{t("keyboard.title")}</DialogTitle>
        </DialogHeader>
        <div className="grid grid-cols-1 gap-x-8 gap-y-4 sm:grid-cols-2">
          {shortcuts.map((cat) => (
            <div key={cat.categoryKey}>
              <div className="mb-1.5 text-xs font-semibold tracking-wide text-muted-foreground uppercase">
                {t(cat.categoryKey)}
              </div>
              {cat.items.map((item, i) => (
                <div
                  className="flex items-center justify-between gap-4 py-1"
                  key={i}
                >
                  <span className="text-sm">
                    {t(item.descKey) || item.descKey}
                  </span>
                  <kbd className="rounded-md border bg-muted px-1.5 py-0.5 font-mono text-xs text-muted-foreground">
                    {item.keys}
                  </kbd>
                </div>
              ))}
            </div>
          ))}
        </div>
      </DialogContent>
    </Dialog>
  );
}
