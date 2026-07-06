import type { SessionRef } from "../lib/types";
import {
  dispatchSessionCommand,
  SESSION_COMMAND_EVENTS,
} from "../lib/session-command-events";

export interface KeyboardDeps {
  activeTabId: () => string | null;
  openTabs: () => SessionRef[];
  showKeyboardOverlay: () => boolean;
  setActiveTabId: (id: string | null) => void;
  setShowKeyboardOverlay: (v: boolean | ((prev: boolean) => boolean)) => void;
  setShowSearchOverlay: (v: boolean | ((prev: boolean) => boolean)) => void;
  setActiveView: (view: string) => void;
  closeTab: (id: string) => void;
  closeAllTabs: () => void;
  splitToRight: (sessionId: string) => void;
  focusAdjacentGroup: (direction: "left" | "right") => void;
  startRebuildIndex: () => void;
  syncFromDisk: (opts?: {
    showSpinner?: boolean;
    changedPaths?: string[];
  }) => void;
}

export function createKeyboardHandler(
  deps: KeyboardDeps,
): (e: KeyboardEvent) => void {
  return (e: KeyboardEvent) => {
    const mod = e.metaKey || e.ctrlKey;

    // Cmd+/ : Toggle keyboard shortcuts overlay
    if (mod && e.key === "/") {
      e.preventDefault();
      deps.setShowKeyboardOverlay((prev) => !prev);
      return;
    }

    // Unmodified ? when not in an input: show keyboard shortcuts
    if (
      e.key === "?" &&
      !mod &&
      !e.altKey &&
      !(document.activeElement instanceof HTMLInputElement) &&
      !(document.activeElement instanceof HTMLTextAreaElement) &&
      !document.activeElement?.hasAttribute("contenteditable")
    ) {
      e.preventDefault();
      deps.setShowKeyboardOverlay(true);
      return;
    }

    // Cmd+Shift+W / Ctrl+Shift+W: Close all tabs
    if (mod && e.shiftKey && (e.key === "w" || e.key === "W")) {
      e.preventDefault();
      deps.closeAllTabs();
      return;
    }

    // Cmd+W / Ctrl+W: Close active tab
    if (mod && e.key === "w") {
      e.preventDefault();
      const id = deps.activeTabId();
      if (id) deps.closeTab(id);
      return;
    }

    // Cmd+1-9: Switch to tab by index
    if (mod && e.key >= "1" && e.key <= "9") {
      e.preventDefault();
      const idx = parseInt(e.key, 10) - 1;
      const tabs = deps.openTabs();
      if (idx < tabs.length) {
        deps.setActiveTabId(tabs[idx].id);
      }
      return;
    }

    // Escape: Close keyboard overlay
    if (e.key === "Escape") {
      if (deps.showKeyboardOverlay()) {
        deps.setShowKeyboardOverlay(false);
        return;
      }
      return;
    }

    // Cmd+] or Ctrl+Tab: Next tab
    if (
      (e.metaKey && e.key === "]") ||
      (e.ctrlKey && e.key === "Tab" && !e.shiftKey)
    ) {
      e.preventDefault();
      const tabs = deps.openTabs();
      const currentId = deps.activeTabId();
      if (tabs.length > 1 && currentId) {
        const idx = tabs.findIndex((t) => t.id === currentId);
        const nextIdx = (idx + 1) % tabs.length;
        deps.setActiveTabId(tabs[nextIdx].id);
      }
      return;
    }

    // Cmd+[ or Ctrl+Shift+Tab: Previous tab
    if (
      (e.metaKey && e.key === "[") ||
      (e.ctrlKey && e.key === "Tab" && e.shiftKey)
    ) {
      e.preventDefault();
      const tabs = deps.openTabs();
      const currentId = deps.activeTabId();
      if (tabs.length > 1 && currentId) {
        const idx = tabs.findIndex((t) => t.id === currentId);
        const prevIdx = (idx - 1 + tabs.length) % tabs.length;
        deps.setActiveTabId(tabs[prevIdx].id);
      }
      return;
    }

    // Cmd+, : Open settings
    if (mod && e.key === ",") {
      e.preventDefault();
      deps.setActiveView("settings");
      return;
    }

    // Cmd+\ : Split editor — move active tab to right group
    if (mod && e.key === "\\") {
      e.preventDefault();
      const id = deps.activeTabId();
      if (id) deps.splitToRight(id);
      return;
    }

    // Cmd+Option+→ : Focus right group
    if (mod && e.altKey && e.key === "ArrowRight") {
      e.preventDefault();
      deps.focusAdjacentGroup("right");
      return;
    }

    // Cmd+Option+← : Focus left group
    if (mod && e.altKey && e.key === "ArrowLeft") {
      e.preventDefault();
      deps.focusAdjacentGroup("left");
      return;
    }

    // Cmd+K or Cmd+Shift+F: Open global search overlay
    if (
      (mod && !e.shiftKey && (e.key === "k" || e.key === "K")) ||
      (mod && e.shiftKey && (e.key === "f" || e.key === "F"))
    ) {
      e.preventDefault();
      deps.setShowSearchOverlay(true);
      return;
    }

    // Cmd+R: Refresh index
    if (mod && !e.shiftKey && e.key === "r") {
      e.preventDefault();
      deps.startRebuildIndex();
      return;
    }

    // Session-scoped shortcuts (only when a tab is active)
    if (!deps.activeTabId()) return;

    // Cmd+F: Find in session
    if (mod && e.key === "f") {
      e.preventDefault();
      dispatchSessionCommand(SESSION_COMMAND_EVENTS.sessionSearch);
      return;
    }

    // Cmd+Shift+R: Resume session
    if (mod && e.shiftKey && (e.key === "r" || e.key === "R")) {
      e.preventDefault();
      dispatchSessionCommand(SESSION_COMMAND_EVENTS.resume);
      return;
    }

    // Cmd+Shift+E: Export session
    if (mod && e.shiftKey && (e.key === "e" || e.key === "E")) {
      e.preventDefault();
      dispatchSessionCommand(SESSION_COMMAND_EVENTS.exportSession);
      return;
    }

    // Cmd+B: Toggle favorite
    if (mod && e.key === "b") {
      e.preventDefault();
      dispatchSessionCommand(SESSION_COMMAND_EVENTS.favorite);
      return;
    }

    // Cmd+L: Toggle live watch
    if (mod && e.key === "l") {
      e.preventDefault();
      dispatchSessionCommand(SESSION_COMMAND_EVENTS.watch);
      return;
    }

    // Cmd+Backspace: Delete session
    if (mod && e.key === "Backspace") {
      e.preventDefault();
      dispatchSessionCommand(SESSION_COMMAND_EVENTS.delete);
      return;
    }
  };
}
