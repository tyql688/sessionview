import { create } from "zustand";

/// Compact layout kicks in at phone-ish widths; the shell swaps the desktop
/// IDE topology (activity bar + sidebar + editor row) for a single-pane stack
/// with bottom navigation. Width-driven so it is debuggable in a narrow
/// desktop window, not device-sniffed.
const COMPACT_QUERY = "(max-width: 768px)";
/// Coarse pointer gates the touch-interaction variants (long-press menus,
/// always-visible close buttons, no HTML5 drag-and-drop).
const COARSE_QUERY = "(pointer: coarse)";

interface ViewportState {
  isCompact: boolean;
  isCoarse: boolean;
}

function queryMatches(query: string): boolean {
  if (typeof window === "undefined" || typeof window.matchMedia !== "function") {
    return false;
  }
  return window.matchMedia(query).matches;
}

const useViewportStore = create<ViewportState>(() => ({
  isCompact: queryMatches(COMPACT_QUERY),
  isCoarse: queryMatches(COARSE_QUERY),
}));

function applyCompactAttribute(isCompact: boolean) {
  if (typeof document === "undefined") return;
  if (isCompact) {
    document.documentElement.setAttribute("data-compact", "");
  } else {
    document.documentElement.removeAttribute("data-compact");
  }
}

// Subscribe once at module load — the shell reads the store reactively and
// the stylesheet keys off the root attribute, so both stay in sync with the
// media queries for the app's whole lifetime.
if (typeof window !== "undefined" && typeof window.matchMedia === "function") {
  applyCompactAttribute(useViewportStore.getState().isCompact);
  const compactList = window.matchMedia(COMPACT_QUERY);
  const coarseList = window.matchMedia(COARSE_QUERY);
  compactList.addEventListener("change", (event) => {
    useViewportStore.setState({ isCompact: event.matches });
    applyCompactAttribute(event.matches);
  });
  coarseList.addEventListener("change", (event) => {
    useViewportStore.setState({ isCoarse: event.matches });
  });
}

export const useIsCompact = () => useViewportStore((s) => s.isCompact);
export const useIsCoarse = () => useViewportStore((s) => s.isCoarse);

export function isCompactViewport(): boolean {
  return useViewportStore.getState().isCompact;
}

export function isCoarsePointer(): boolean {
  return useViewportStore.getState().isCoarse;
}

/** Test-only: force viewport flags (jsdom has no real matchMedia). */
export function _setViewportForTest(state: Partial<ViewportState>) {
  useViewportStore.setState(state);
  if (state.isCompact !== undefined) applyCompactAttribute(state.isCompact);
}
