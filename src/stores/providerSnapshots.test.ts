import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ProviderSnapshot } from "../lib/types";

const getProviderSnapshots = vi.fn<() => Promise<ProviderSnapshot[]>>();

vi.mock("../lib/tauri", () => ({
  getProviderSnapshots,
}));

async function loadStore() {
  return import("./providerSnapshots");
}

describe("providerSnapshots store", () => {
  beforeEach(() => {
    vi.resetModules();
    getProviderSnapshots.mockReset();
  });

  it("uses fallback values before snapshots load", async () => {
    const {
      getProviderLabel,
      getProvidersForWatchStrategy,
      getProviderSortOrder,
      getProviderWatchStrategy,
    } = await loadStore();

    expect(getProviderLabel("claude")).toBe("Claude Code");
    expect(getProviderLabel("cc-mirror", "cczai")).toBe("cczai");
    expect(getProviderLabel("cc-mirror")).toBe("CC-Mirror");
    expect(getProviderWatchStrategy("antigravity")).toBe("fs");
    expect(getProvidersForWatchStrategy("poll")).toEqual(["opencode"]);
    expect(getProviderSortOrder("claude")).toBeLessThan(
      getProviderSortOrder("codex"),
    );
  });

  it("switches watch providers to the loaded snapshots", async () => {
    getProviderSnapshots.mockResolvedValue([
      {
        key: "claude",
        label: "Claude Code",
        color: "var(--claude)",
        sort_order: 0,
        watch_strategy: "fs",
        path: "/claude",
        exists: true,
        session_count: 2,
      },
      {
        key: "codex",
        label: "Codex",
        color: "var(--codex)",
        sort_order: 1,
        watch_strategy: "poll",
        path: "/codex",
        exists: true,
        session_count: 3,
      },
    ]);

    const {
      getProvidersForWatchStrategy,
      getProviderSnapshotVersion,
      listProviderSnapshots,
      loadProviderSnapshots,
    } = await loadStore();

    expect(getProvidersForWatchStrategy("poll")).toEqual(["opencode"]);

    await loadProviderSnapshots();

    expect(getProviderSnapshotVersion()).toBe(1);
    expect(getProvidersForWatchStrategy("poll")).toEqual(["codex", "opencode"]);
    expect(listProviderSnapshots().map((snapshot) => snapshot.key)).toEqual([
      "claude",
      "cc-mirror",
      "codex",
      "antigravity",
      "opencode",
      "kimi",
      "cursor",
      "pi",
    ]);
  });

  it("keeps fallback values and warns when snapshot load fails", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    getProviderSnapshots.mockRejectedValue(new Error("boom"));

    const {
      getProvidersForWatchStrategy,
      getProviderSnapshotVersion,
      loadProviderSnapshots,
    } = await loadStore();

    await loadProviderSnapshots();

    expect(getProviderSnapshotVersion()).toBe(0);
    expect(getProvidersForWatchStrategy("poll")).toEqual(["opencode"]);
    expect(warn).toHaveBeenCalledWith(
      "failed to load provider snapshots:",
      expect.any(Error),
    );

    warn.mockRestore();
  });
});
