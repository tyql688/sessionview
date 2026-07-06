import { create } from "zustand";

export type Theme = "light" | "dark" | "system";

function readStoredTheme(): Theme {
  if (
    typeof localStorage === "undefined" ||
    typeof localStorage.getItem !== "function"
  ) {
    return "system";
  }
  try {
    const stored = localStorage.getItem("cc-session-theme");
    return stored === "light" || stored === "dark" ? stored : "system";
  } catch (error) {
    console.error("Failed to read theme from localStorage:", error);
    return "system";
  }
}

function writeStoredTheme(theme: Theme): void {
  if (
    typeof localStorage === "undefined" ||
    typeof localStorage.setItem !== "function"
  ) {
    return;
  }
  try {
    localStorage.setItem("cc-session-theme", theme);
  } catch (error) {
    console.error("Failed to write theme to localStorage:", error);
  }
}

/** Resolve the OS color scheme; defaults to light when unavailable (tests/SSR). */
function resolveSystemTheme(): "light" | "dark" {
  if (
    typeof window === "undefined" ||
    typeof window.matchMedia !== "function"
  ) {
    return "light";
  }
  return window.matchMedia("(prefers-color-scheme: dark)").matches
    ? "dark"
    : "light";
}

export function applyTheme(theme: Theme) {
  if (typeof document === "undefined") {
    return;
  }
  // Always set an explicit light/dark attribute, resolving "system" via the OS
  // so the app shell follows OS dark mode.
  const resolved = theme === "system" ? resolveSystemTheme() : theme;
  document.documentElement.setAttribute("data-theme", resolved);
  writeStoredTheme(theme);
}

interface ThemeState {
  theme: Theme;
}

const useThemeStore = create<ThemeState>(() => ({
  theme: readStoredTheme(),
}));

export function setTheme(t: Theme) {
  useThemeStore.setState({ theme: t });
  applyTheme(t);
}

export function getTheme(): Theme {
  return useThemeStore.getState().theme;
}

export function useTheme(): Theme {
  return useThemeStore((state) => state.theme);
}

// Re-apply on OS theme change while tracking it ("system" mode), so a live
// light<->dark switch in the OS is reflected without a restart.
if (typeof window !== "undefined" && typeof window.matchMedia === "function") {
  window
    .matchMedia("(prefers-color-scheme: dark)")
    .addEventListener("change", () => {
      if (useThemeStore.getState().theme === "system") {
        applyTheme("system");
      }
    });
}
