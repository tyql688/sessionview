import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ProviderSnapshot } from "@/lib/types";

const getProviderSnapshots = vi.fn<() => Promise<ProviderSnapshot[]>>();

vi.mock("@/lib/tauri", () => ({
  getProviderSnapshots,
}));

async function loadStore() {
  return import("@/stores/providerSnapshots");
}

describe("providerSnapshots store", () => {
  beforeEach(() => {
    vi.resetModules();
    getProviderSnapshots.mockReset();
  });

  it("uses fallback values before snapshots load", async () => {
    const {
      getProviderLabel,
      getProviderSortOrder,
    } = await loadStore();

    expect(getProviderLabel("claude")).toBe("Claude Code");
    expect(getProviderLabel("cc-mirror", "cczai")).toBe("cczai");
    expect(getProviderLabel("cc-mirror")).toBe("CC-Mirror");
    expect(getProviderSortOrder("claude")).toBeLessThan(
      getProviderSortOrder("codex"),
    );
  });

  it("switches metadata to the loaded snapshots", async () => {
    getProviderSnapshots.mockResolvedValue([
      {
        key: "claude",
        label: "Claude Code",
        color: "var(--claude)",
        sort_order: 0,
        path: "/claude",
        exists: true,
        session_count: 2,
      },
      {
        key: "codex",
        label: "Codex",
        color: "var(--codex)",
        sort_order: 1,
        path: "/codex",
        exists: true,
        session_count: 3,
      },
    ]);

    const {
      getProviderSnapshotVersion,
      listProviderSnapshots,
      loadProviderSnapshots,
    } = await loadStore();

    await loadProviderSnapshots();

    expect(getProviderSnapshotVersion()).toBe(1);
    expect(listProviderSnapshots().map((snapshot) => snapshot.key)).toEqual([
      "claude",
      "cc-mirror",
      "codex",
      "antigravity",
      "opencode",
      "kimi",
      "cursor",
      "pi",
      "grok",
    ]);
  });

  it("keeps fallback values and warns when snapshot load fails", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    getProviderSnapshots.mockRejectedValue(new Error("boom"));

    const {
      getProviderSnapshotVersion,
      loadProviderSnapshots,
    } = await loadStore();

    await loadProviderSnapshots();

    expect(getProviderSnapshotVersion()).toBe(0);
    expect(warn).toHaveBeenCalledWith(
      "failed to load provider snapshots:",
      expect.any(Error),
    );

    warn.mockRestore();
  });
});
