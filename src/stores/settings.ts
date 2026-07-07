import { create } from "zustand";
import { errorMessage } from "@/lib/errors";
import { detectTerminal } from "@/lib/tauri";
import type { Provider } from "@/lib/types";
import { DEFAULT_AUTO_INDEX_INTERVAL, isAutoIndexInterval, type AutoIndexInterval } from "@/lib/auto-index";

export type TerminalApp =
  | "terminal"
  | "iterm2"
  | "ghostty"
  | "kitty"
  | "warp"
  | "wezterm"
  | "alacritty" // macOS
  | "windows-terminal"
  | "powershell"
  | "cmd" // Windows
  | "gnome-terminal"
  | "konsole"
  | "xterm"; // Linux

// Record<Provider, true> forces a compile error when a Provider variant is
// added without updating this map, so stored settings can never silently drop
// a valid provider on load.
const PROVIDER_FLAGS: Record<Provider, true> = {
  claude: true,
  codex: true,
  antigravity: true,
  opencode: true,
  kimi: true,
  cursor: true,
  "cc-mirror": true,
  pi: true,
};
const VALID_PROVIDERS = Object.keys(PROVIDER_FLAGS) as Provider[];

const VALID_TERMINALS: TerminalApp[] = [
  "terminal",
  "iterm2",
  "ghostty",
  "kitty",
  "warp",
  "wezterm",
  "alacritty",
  "windows-terminal",
  "powershell",
  "cmd",
  "gnome-terminal",
  "konsole",
  "xterm",
];

function readStorage(key: string): string | null {
  if (typeof localStorage === "undefined" || typeof localStorage.getItem !== "function") {
    return null;
  }
  try {
    return localStorage.getItem(key);
  } catch (error) {
    console.error(`Failed to read localStorage key ${key}:`, error);
    return null;
  }
}

function writeStorage(key: string, value: string): void {
  if (typeof localStorage === "undefined" || typeof localStorage.setItem !== "function") {
    return;
  }
  try {
    localStorage.setItem(key, value);
  } catch (error) {
    console.error(`Failed to write localStorage key ${key}:`, error);
  }
}

function parseStoredStringArray<T extends string>(
  storageKey: string,
  label: string,
  isValid: (value: string) => value is T,
): { value: T[]; error: string | null } {
  const raw = readStorage(storageKey);
  if (raw === null) {
    return { value: [], error: null };
  }

  try {
    const parsed = JSON.parse(raw);
    if (!Array.isArray(parsed)) {
      throw new Error(`${label} must be a JSON array`);
    }

    const invalidValue = parsed.find((value) => typeof value !== "string");
    if (invalidValue !== undefined) {
      throw new Error(`invalid ${label} entry: ${JSON.stringify(invalidValue)}`);
    }

    const value = parsed.filter(isValid) as T[];
    if (value.length !== parsed.length) {
      console.warn(`Removed unsupported ${label} entries from localStorage`);
      writeStorage(storageKey, JSON.stringify(value));
    }

    return { value, error: null };
  } catch (error) {
    const message = `Failed to parse ${label}: ${errorMessage(error)}`;
    console.error(message, error);
    return { value: [], error: message };
  }
}

export type ExplorerGrouping = "provider" | "directory";

function readStoredExplorerGrouping(): ExplorerGrouping {
  const raw = readStorage("sessionview-explorer-grouping");
  if (raw === null || raw === "provider") return "provider";
  if (raw === "directory") return "directory";
  console.warn(`Ignoring invalid explorer grouping setting: ${raw}`);
  return "provider";
}

function readStoredAutoIndexInterval(): AutoIndexInterval {
  const raw = readStorage("sessionview-auto-index-interval");
  if (raw === null) return DEFAULT_AUTO_INDEX_INTERVAL;
  if (isAutoIndexInterval(raw)) return raw;
  console.warn(`Ignoring invalid auto index interval setting: ${raw}`);
  return DEFAULT_AUTO_INDEX_INTERVAL;
}

const storedTerminal = readStorage("sessionview-terminal") as TerminalApp | null;
const initialDisabledProviders = parseStoredStringArray<Provider>(
  "sessionview-disabled-providers",
  "disabled providers setting",
  (value): value is Provider => VALID_PROVIDERS.includes(value as Provider),
);
const initialBlockedFolders = parseStoredStringArray<string>(
  "sessionview-blocked-folders",
  "blocked folders setting",
  (value): value is string => value.length > 0,
);

interface SettingsState {
  terminalApp: TerminalApp;
  disabledProviders: Provider[];
  disabledProvidersError: string | null;
  showOrphans: boolean;
  focusMode: boolean;
  explorerGrouping: ExplorerGrouping;
  autoIndexInterval: AutoIndexInterval;
  blockedFolders: string[];
  blockedFoldersError: string | null;
}

const useSettingsStore = create<SettingsState>(() => ({
  terminalApp: storedTerminal || "terminal",
  disabledProviders: initialDisabledProviders.value,
  disabledProvidersError: initialDisabledProviders.error,
  showOrphans: readStorage("sessionview-show-orphans") !== "false",
  focusMode: readStorage("sessionview-focus-mode") === "true",
  explorerGrouping: readStoredExplorerGrouping(),
  autoIndexInterval: readStoredAutoIndexInterval(),
  blockedFolders: initialBlockedFolders.value,
  blockedFoldersError: initialBlockedFolders.error,
}));

// Auto-detect terminal on first launch.
if (!storedTerminal) {
  detectTerminal()
    .then((detected) => {
      if (VALID_TERMINALS.includes(detected as TerminalApp)) {
        useSettingsStore.setState({ terminalApp: detected as TerminalApp });
        writeStorage("sessionview-terminal", detected);
      }
    })
    .catch((error) => {
      console.error("Failed to detect terminal app:", error);
    });
}

export function setTerminalApp(t: TerminalApp) {
  useSettingsStore.setState({ terminalApp: t });
  writeStorage("sessionview-terminal", t);
}

export function toggleProvider(id: Provider) {
  const prev = useSettingsStore.getState().disabledProviders;
  const next = prev.includes(id) ? prev.filter((p) => p !== id) : [...prev, id];
  useSettingsStore.setState({
    disabledProviders: next,
    disabledProvidersError: null,
  });
  writeStorage("sessionview-disabled-providers", JSON.stringify(next));
}

export function setShowOrphans(v: boolean) {
  useSettingsStore.setState({ showOrphans: v });
  writeStorage("sessionview-show-orphans", String(v));
}

export function setFocusMode(v: boolean) {
  useSettingsStore.setState({ focusMode: v });
  writeStorage("sessionview-focus-mode", String(v));
}

export function setExplorerGrouping(mode: ExplorerGrouping) {
  useSettingsStore.setState({ explorerGrouping: mode });
  writeStorage("sessionview-explorer-grouping", mode);
}

export function setAutoIndexInterval(interval: AutoIndexInterval) {
  useSettingsStore.setState({ autoIndexInterval: interval });
  writeStorage("sessionview-auto-index-interval", interval);
}

export function addBlockedFolder(path: string) {
  const prev = useSettingsStore.getState().blockedFolders;
  if (prev.includes(path)) return;
  const next = [...prev, path];
  useSettingsStore.setState({
    blockedFolders: next,
    blockedFoldersError: null,
  });
  writeStorage("sessionview-blocked-folders", JSON.stringify(next));
}

export function removeBlockedFolder(path: string) {
  const prev = useSettingsStore.getState().blockedFolders;
  const next = prev.filter((p) => p !== path);
  useSettingsStore.setState({
    blockedFolders: next,
    blockedFoldersError: null,
  });
  writeStorage("sessionview-blocked-folders", JSON.stringify(next));
}

export function isPathBlocked(path: string): boolean {
  return useSettingsStore
    .getState()
    .blockedFolders.some((blocked) => path === blocked || path.startsWith(`${blocked}/`));
}

// Imperative getters for non-reactive logic (filtering, tree building).
export const getBlockedFolders = () => useSettingsStore.getState().blockedFolders;

// Reactive hooks for components.
export const useTerminalApp = () => useSettingsStore((s) => s.terminalApp);
export const useDisabledProviders = () => useSettingsStore((s) => s.disabledProviders);
export const useDisabledProvidersError = () => useSettingsStore((s) => s.disabledProvidersError);
export const useShowOrphans = () => useSettingsStore((s) => s.showOrphans);
export const useFocusMode = () => useSettingsStore((s) => s.focusMode);
export const useExplorerGrouping = () => useSettingsStore((s) => s.explorerGrouping);
export const useAutoIndexInterval = () => useSettingsStore((s) => s.autoIndexInterval);
export const useBlockedFolders = () => useSettingsStore((s) => s.blockedFolders);
export const useBlockedFoldersError = () => useSettingsStore((s) => s.blockedFoldersError);
