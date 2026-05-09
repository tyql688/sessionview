import { createSignal } from "solid-js";
import type { Provider } from "../lib/types";
import { errorMessage } from "../lib/errors";
import { detectTerminal } from "../lib/tauri";

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

const VALID_PROVIDERS: Provider[] = [
  "claude",
  "codex",
  "gemini",
  "opencode",
  "kimi",
  "cc-mirror",
  "qwen",
];

function parseStoredStringArray<T extends string>(
  storageKey: string,
  label: string,
  isValid: (value: string) => value is T,
): { value: T[]; error: string | null } {
  const raw = localStorage.getItem(storageKey);
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
      throw new Error(
        `invalid ${label} entry: ${JSON.stringify(invalidValue)}`,
      );
    }

    const value = parsed.filter(isValid) as T[];
    if (value.length !== parsed.length) {
      console.warn(`Removed unsupported ${label} entries from localStorage`);
      localStorage.setItem(storageKey, JSON.stringify(value));
    }

    return { value, error: null };
  } catch (error) {
    const message = `Failed to parse ${label}: ${errorMessage(error)}`;
    console.error(message, error);
    return { value: [], error: message };
  }
}

const storedTerminal = localStorage.getItem(
  "cc-session-terminal",
) as TerminalApp | null;

const [terminalApp, setTerminalAppSignal] = createSignal<TerminalApp>(
  storedTerminal || "terminal",
);

// Auto-detect terminal on first launch
if (!storedTerminal) {
  detectTerminal()
    .then((detected) => {
      const valid: TerminalApp[] = [
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
      if (valid.includes(detected as TerminalApp)) {
        setTerminalAppSignal(detected as TerminalApp);
        localStorage.setItem("cc-session-terminal", detected);
      }
    })
    .catch((error) => {
      console.error("Failed to detect terminal app:", error);
    });
}

export function setTerminalApp(t: TerminalApp) {
  setTerminalAppSignal(t);
  localStorage.setItem("cc-session-terminal", t);
}

export { terminalApp };

// Provider toggle: store disabled providers in localStorage
const initialDisabledProviders = parseStoredStringArray<Provider>(
  "cc-session-disabled-providers",
  "disabled providers setting",
  (value): value is Provider => VALID_PROVIDERS.includes(value as Provider),
);

const [disabledProviders, setDisabledProvidersSignal] = createSignal<
  Provider[]
>(initialDisabledProviders.value);
const [disabledProvidersError, setDisabledProvidersError] = createSignal<
  string | null
>(initialDisabledProviders.error);

export function toggleProvider(id: Provider) {
  setDisabledProvidersSignal((prev) => {
    const next = prev.includes(id)
      ? prev.filter((p) => p !== id)
      : [...prev, id];
    setDisabledProvidersError(null);
    localStorage.setItem("cc-session-disabled-providers", JSON.stringify(next));
    return next;
  });
}

export { disabledProviders, disabledProvidersError };

// Time grouping toggle
const [timeGrouping, setTimeGroupingSignal] = createSignal<boolean>(
  localStorage.getItem("cc-session-time-grouping") !== "false",
);

export function setTimeGrouping(v: boolean) {
  setTimeGroupingSignal(v);
  localStorage.setItem("cc-session-time-grouping", String(v));
}

export { timeGrouping };

// Show subagents toggle (default on)
const [showOrphans, setShowOrphansSignal] = createSignal<boolean>(
  localStorage.getItem("cc-session-show-orphans") !== "false",
);

export function setShowOrphans(v: boolean) {
  setShowOrphansSignal(v);
  localStorage.setItem("cc-session-show-orphans", String(v));
}

export { showOrphans };

// Blocked folders: sessions from these project paths are hidden
const initialBlockedFolders = parseStoredStringArray<string>(
  "cc-session-blocked-folders",
  "blocked folders setting",
  (value): value is string => value.length > 0,
);

const [blockedFolders, setBlockedFoldersSignal] = createSignal<string[]>(
  initialBlockedFolders.value,
);
const [blockedFoldersError, setBlockedFoldersError] = createSignal<
  string | null
>(initialBlockedFolders.error);

export function addBlockedFolder(path: string) {
  setBlockedFoldersSignal((prev) => {
    if (prev.includes(path)) return prev;
    const next = [...prev, path];
    setBlockedFoldersError(null);
    localStorage.setItem("cc-session-blocked-folders", JSON.stringify(next));
    return next;
  });
}

export function removeBlockedFolder(path: string) {
  setBlockedFoldersSignal((prev) => {
    const next = prev.filter((p) => p !== path);
    setBlockedFoldersError(null);
    localStorage.setItem("cc-session-blocked-folders", JSON.stringify(next));
    return next;
  });
}

export function isPathBlocked(path: string): boolean {
  return blockedFolders().some(
    (blocked) => path === blocked || path.startsWith(blocked + "/"),
  );
}

export { blockedFolders, blockedFoldersError };
