import { describe, it, expect, vi, beforeEach } from "vitest";

// Mock tauri plugins before importing the store
vi.mock("@tauri-apps/plugin-updater", () => ({
  check: vi.fn(),
}));
vi.mock("@tauri-apps/plugin-process", () => ({
  relaunch: vi.fn(),
}));

// Reset module between tests so signals start fresh
describe("updater store", () => {
  beforeEach(() => {
    vi.resetModules();
  });

  it("starts in idle phase", async () => {
    const { getUpdaterPhase: phase } = await import("./updater");
    expect(phase()).toBe("idle");
  });

  it("sets phase to available when update found", async () => {
    const { check } = await import("@tauri-apps/plugin-updater");
    vi.mocked(check).mockResolvedValue({
      version: "1.0.0",
      downloadAndInstall: vi.fn(),
    } as unknown as Awaited<ReturnType<typeof check>>);

    const {
      checkForUpdate,
      getUpdaterPhase: phase,
      getAvailableVersion: availableVersion,
    } = await import("./updater");
    await checkForUpdate();

    expect(phase()).toBe("available");
    expect(availableVersion()).toBe("1.0.0");
  });

  it("shows upToDate then resets to idle when already up to date", async () => {
    vi.useFakeTimers();
    const { check } = await import("@tauri-apps/plugin-updater");
    vi.mocked(check).mockResolvedValue(null);

    const { checkForUpdate, getUpdaterPhase: phase } =
      await import("./updater");
    await checkForUpdate();

    expect(phase()).toBe("upToDate");
    vi.advanceTimersByTime(3000);
    expect(phase()).toBe("idle");
    vi.useRealTimers();
  });

  it("sets error phase on check failure, then resets to idle", async () => {
    vi.useFakeTimers();
    const { check } = await import("@tauri-apps/plugin-updater");
    vi.mocked(check).mockRejectedValue(new Error("network error"));

    const {
      checkForUpdate,
      getUpdaterPhase: phase,
      getUpdaterError: errorDetail,
    } = await import("./updater");
    await checkForUpdate();

    expect(phase()).toBe("error");
    expect(errorDetail()).toBe("network error");
    vi.advanceTimersByTime(3000);
    expect(phase()).toBe("idle");
    vi.useRealTimers();
  });

  it("stale timer does not clobber available state", async () => {
    vi.useFakeTimers();
    const { check } = await import("@tauri-apps/plugin-updater");

    // First call: error → schedules reset to idle in 3s
    vi.mocked(check).mockRejectedValueOnce(new Error("timeout"));
    const { checkForUpdate, getUpdaterPhase: phase } =
      await import("./updater");
    await checkForUpdate();
    expect(phase()).toBe("error");

    // Second call before timer fires: succeeds with update
    vi.mocked(check).mockResolvedValueOnce({
      version: "2.0.0",
      downloadAndInstall: vi.fn(),
    } as unknown as Awaited<ReturnType<typeof check>>);
    await checkForUpdate();
    expect(phase()).toBe("available");

    // Old timer fires — should NOT clobber available
    vi.advanceTimersByTime(3000);
    expect(phase()).toBe("available");
    vi.useRealTimers();
  });

  it("sets errorDetail on download failure", async () => {
    vi.useFakeTimers();
    const { check } = await import("@tauri-apps/plugin-updater");
    const mockUpdate = {
      version: "2.0.0",
      downloadAndInstall: vi
        .fn()
        .mockRejectedValue(new Error("signature mismatch")),
    };
    vi.mocked(check).mockResolvedValue(
      mockUpdate as unknown as Awaited<ReturnType<typeof check>>,
    );

    const {
      checkForUpdate,
      downloadAndInstall,
      getUpdaterPhase: phase,
      getUpdaterError: errorDetail,
    } = await import("./updater");
    await checkForUpdate();
    expect(phase()).toBe("available");

    await downloadAndInstall();
    expect(phase()).toBe("error");
    expect(errorDetail()).toBe("signature mismatch");

    vi.advanceTimersByTime(3000);
    expect(phase()).toBe("available");
    vi.useRealTimers();
  });
});
