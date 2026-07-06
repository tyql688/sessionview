import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();

vi.mock("@tauri-apps/api/core", () => ({
  invoke,
}));

describe("external links", () => {
  beforeEach(() => {
    invoke.mockReset();
    invoke.mockResolvedValue(undefined);
  });

  it("opens http urls through the Tauri opener plugin", async () => {
    const { openExternalUrl } = await import("@/lib/external-links");

    await openExternalUrl("https://example.com/docs?q=1#top");

    expect(invoke).toHaveBeenCalledWith("plugin:opener|open_url", {
      url: "https://example.com/docs?q=1#top",
    });
  });

  it("rejects relative urls before invoking opener", async () => {
    const { openExternalUrl } = await import("@/lib/external-links");

    await expect(openExternalUrl("/local/path")).rejects.toThrow(
      "Invalid external URL",
    );
    expect(invoke).not.toHaveBeenCalled();
  });

  it("rejects unsupported protocols", async () => {
    const { openExternalUrl } = await import("@/lib/external-links");

    await expect(openExternalUrl("javascript:alert(1)")).rejects.toThrow(
      "Unsupported external URL protocol",
    );
    expect(invoke).not.toHaveBeenCalled();
  });
});
