import type { Update } from "@tauri-apps/plugin-updater";
import { create } from "zustand";

export type UpdatePhase =
  | "idle"
  | "checking"
  | "upToDate"
  | "available"
  | "downloading"
  | "installing"
  | "error";

interface UpdaterState {
  phase: UpdatePhase;
  availableVersion: string | null;
  errorDetail: string | null;
}

export const useUpdaterStore = create<UpdaterState>(() => ({
  phase: "idle",
  availableVersion: null,
  errorDetail: null,
}));

let pendingUpdate: Update | null = null;
let isChecking = false;
let resetTimer: ReturnType<typeof setTimeout> | null = null;

function setPhase(phase: UpdatePhase) {
  useUpdaterStore.setState({ phase });
}

// Imperative getters (also used by the phase-machine guards below).
export const getUpdaterPhase = (): UpdatePhase =>
  useUpdaterStore.getState().phase;
export const getAvailableVersion = (): string | null =>
  useUpdaterStore.getState().availableVersion;
export const getUpdaterError = (): string | null =>
  useUpdaterStore.getState().errorDetail;

/** Clear any pending phase-reset timer. */
function clearResetTimer() {
  if (resetTimer !== null) {
    clearTimeout(resetTimer);
    resetTimer = null;
  }
}

/** Set phase after a delay, cancelling any previous delayed reset. */
function scheduleReset(target: UpdatePhase, ms: number) {
  clearResetTimer();
  resetTimer = setTimeout(() => {
    resetTimer = null;
    setPhase(target);
  }, ms);
}

export async function checkForUpdate(): Promise<void> {
  if (
    isChecking ||
    getUpdaterPhase() === "downloading" ||
    getUpdaterPhase() === "installing"
  )
    return;
  isChecking = true;
  clearResetTimer();
  setPhase("checking");

  try {
    const { check } = await import("@tauri-apps/plugin-updater");
    const update = await check();
    if (update) {
      pendingUpdate = update;
      useUpdaterStore.setState({
        availableVersion: update.version,
        phase: "available",
      });
    } else {
      pendingUpdate = null;
      useUpdaterStore.setState({ availableVersion: null, phase: "upToDate" });
      scheduleReset("idle", 3000);
    }
  } catch (e) {
    const msg = e instanceof Error ? e.message : String(e);
    console.error("[updater] check failed:", msg);
    useUpdaterStore.setState({ errorDetail: msg, phase: "error" });
    scheduleReset("idle", 3000);
  } finally {
    isChecking = false;
  }
}

export async function downloadAndInstall(): Promise<void> {
  if (!pendingUpdate || getUpdaterPhase() !== "available") return;
  const update = pendingUpdate;

  clearResetTimer();
  setPhase("downloading");
  try {
    await update.downloadAndInstall();
    setPhase("installing");
    const { relaunch } = await import("@tauri-apps/plugin-process");
    await relaunch();
  } catch (e) {
    const msg = e instanceof Error ? e.message : String(e);
    console.error("[updater] install failed:", msg);
    useUpdaterStore.setState({ errorDetail: msg, phase: "error" });
    scheduleReset("available", 3000);
  }
}

// Reactive hooks for components.
export function useUpdaterPhase(): UpdatePhase {
  return useUpdaterStore((state) => state.phase);
}

export function useAvailableVersion(): string | null {
  return useUpdaterStore((state) => state.availableVersion);
}

export function useUpdaterError(): string | null {
  return useUpdaterStore((state) => state.errorDetail);
}
