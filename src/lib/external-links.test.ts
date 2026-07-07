import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();

vi.mock("@/lib/tauri", () => ({
  openInFolder: vi.fn(),
}));

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

  it("reveals absolute paths in the file manager instead of the opener", async () => {
    const { openExternalUrl } = await import("@/lib/external-links");
    const { openInFolder } = await import("@/lib/tauri");

    await openExternalUrl("/local/path");
    expect(openInFolder).toHaveBeenCalledWith("/local/path");
    expect(invoke).not.toHaveBeenCalled();
  });

  it("rejects relative urls before invoking opener", async () => {
    const { openExternalUrl } = await import("@/lib/external-links");

    await expect(openExternalUrl("docs/readme.md")).rejects.toThrow(
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

describe("localPathFrom", () => {
  async function subject() {
    const { localPathFrom } = await import("@/lib/external-links");
    return localPathFrom;
  }

  it("extracts posix paths from file URLs", async () => {
    expect((await subject())("file:///Users/dev/notes.md")).toBe(
      "/Users/dev/notes.md",
    );
  });

  it("decodes percent-encoded characters", async () => {
    expect((await subject())("file:///Users/dev/my%20file.md")).toBe(
      "/Users/dev/my file.md",
    );
  });

  it("strips the pre-drive slash on windows file URLs", async () => {
    expect((await subject())("file:///C:/Users/dev/x.md")).toBe(
      "C:/Users/dev/x.md",
    );
  });

  it("passes through absolute and home-relative paths", async () => {
    const localPathFrom = await subject();
    expect(localPathFrom("/tmp/a.txt")).toBe("/tmp/a.txt");
    expect(localPathFrom("~/notes/a.md")).toBe("~/notes/a.md");
  });

  it("returns null for web URLs", async () => {
    const localPathFrom = await subject();
    expect(localPathFrom("https://example.com/")).toBeNull();
    expect(localPathFrom("mailto:a@b.c")).toBeNull();
  });
});
