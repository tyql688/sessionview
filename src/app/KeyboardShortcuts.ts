import type { SessionRef } from "@/lib/types";
import { dispatchSessionCommand, SESSION_COMMAND_EVENTS } from "@/lib/session-command-events";
import { isCompactViewport } from "@/stores/viewport";

export interface KeyboardDeps {
  activeTabId: () => string | null;
  openTabs: () => SessionRef[];
  setActiveTabId: (id: string | null) => void;
  setShowKeyboardOverlay: (v: boolean | ((prev: boolean) => boolean)) => void;
  setShowSearchOverlay: (v: boolean | ((prev: boolean) => boolean)) => void;
  setActiveView: (view: string) => void;
  closeTab: (id: string) => void;
  closeAllTabs: () => void;
  reopenClosedTab: () => void;
  toggleSidebar: () => void;
  splitToRight: (sessionId: string) => void;
  focusAdjacentGroup: (direction: "left" | "right") => void;
  startRebuildIndex: () => void;
}

export function createKeyboardHandler(deps: KeyboardDeps): (e: KeyboardEvent) => void {
  return (e: KeyboardEvent) => {
    const mod = e.metaKey || e.ctrlKey;
    // Case-insensitive letter matching: CapsLock (and Shift combos) yield
    // uppercase e.key, which silently disabled the lowercase-only checks.
    const key = e.key.length === 1 ? e.key.toLowerCase() : e.key;
    const typing =
      document.activeElement instanceof HTMLInputElement ||
      document.activeElement instanceof HTMLTextAreaElement ||
      document.activeElement?.hasAttribute("contenteditable") === true;

    // Cmd+/ : Toggle keyboard shortcuts overlay
    if (mod && e.key === "/") {
      e.preventDefault();
      deps.setShowKeyboardOverlay((prev) => !prev);
      return;
    }

    // Unmodified ? when not in an input: show keyboard shortcuts
    if (e.key === "?" && !mod && !e.altKey && !typing) {
      e.preventDefault();
      deps.setShowKeyboardOverlay(true);
      return;
    }

    // Cmd+Shift+W / Ctrl+Shift+W: Close all tabs
    if (mod && e.shiftKey && key === "w") {
      e.preventDefault();
      deps.closeAllTabs();
      return;
    }

    // Cmd+W / Ctrl+W: Close active tab
    if (mod && key === "w") {
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

    // Cmd+] or Ctrl+Tab: Next tab
    if ((e.metaKey && e.key === "]") || (e.ctrlKey && e.key === "Tab" && !e.shiftKey)) {
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
    if ((e.metaKey && e.key === "[") || (e.ctrlKey && e.key === "Tab" && e.shiftKey)) {
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

    // Cmd+\ : Split editor — move active tab to right group.
    // Compact layouts force a single group, so splitting is a no-op there
    // (hardware keyboards exist on tablets and narrow desktop windows).
    if (mod && e.key === "\\") {
      e.preventDefault();
      if (isCompactViewport()) return;
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

    // Cmd+Shift+T: Reopen most recently closed tab
    if (mod && e.shiftKey && key === "t") {
      e.preventDefault();
      deps.reopenClosedTab();
      return;
    }

    // Cmd+B: Toggle the explorer sidebar (VS Code convention)
    if (mod && !e.shiftKey && key === "b") {
      e.preventDefault();
      deps.toggleSidebar();
      return;
    }

    // Cmd+K / Cmd+P / Cmd+Shift+F: Open global search overlay
    if ((mod && !e.shiftKey && (key === "k" || key === "p")) || (mod && e.shiftKey && key === "f")) {
      e.preventDefault();
      deps.setShowSearchOverlay(true);
      return;
    }

    // Cmd+R: Refresh index
    if (mod && !e.shiftKey && key === "r") {
      e.preventDefault();
      deps.startRebuildIndex();
      return;
    }

    // Session-scoped shortcuts (only when a tab is active)
    if (!deps.activeTabId()) return;

    // Cmd+F: Find in session
    if (mod && key === "f") {
      e.preventDefault();
      dispatchSessionCommand(SESSION_COMMAND_EVENTS.sessionSearch);
      return;
    }

    // Cmd+Shift+R: Resume session
    if (mod && e.shiftKey && key === "r") {
      e.preventDefault();
      dispatchSessionCommand(SESSION_COMMAND_EVENTS.resume);
      return;
    }

    // Cmd+Shift+E: Export session
    if (mod && e.shiftKey && key === "e") {
      e.preventDefault();
      dispatchSessionCommand(SESSION_COMMAND_EVENTS.exportSession);
      return;
    }

    // Cmd+D: Toggle favorite (browser bookmark convention). Skipped while
    // typing — hijacking a text field's combo to mutate the session is a
    // surprise side effect.
    if (mod && key === "d" && !typing) {
      e.preventDefault();
      dispatchSessionCommand(SESSION_COMMAND_EVENTS.favorite);
      return;
    }

    // Cmd+G / Cmd+Shift+G: next / previous in-session search match
    if (mod && key === "g") {
      e.preventDefault();
      dispatchSessionCommand(e.shiftKey ? SESSION_COMMAND_EVENTS.findPrev : SESSION_COMMAND_EVENTS.findNext);
      return;
    }
  };
}
