import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
const toastError = vi.fn();

vi.mock("@tauri-apps/api/core", () => ({
  invoke,
}));

vi.mock("@/stores/toast", () => ({
  toastError,
}));

describe("tauri api wrappers", () => {
  beforeEach(() => {
    invoke.mockReset();
    invoke.mockResolvedValue(undefined);
    toastError.mockReset();
  });

  it("getSessionDetail sends only sessionId", async () => {
    const { getSessionDetail } = await import("@/lib/tauri");

    await getSessionDetail("sess-1");

    expect(invoke).toHaveBeenCalledWith("get_session_detail", {
      sessionId: "sess-1",
    });
  });

  it("getSessionOpenWindow sends sessionId and window bounds", async () => {
    const { getSessionOpenWindow } = await import("@/lib/tauri");

    await getSessionOpenWindow("sess-1", -300, 300);

    expect(invoke).toHaveBeenCalledWith("get_session_open_window", {
      sessionId: "sess-1",
      offset: -300,
      limit: 300,
    });
  });

  it("getSessionOpenWindow sends request identity when provided", async () => {
    const { getSessionOpenWindow } = await import("@/lib/tauri");

    await getSessionOpenWindow("sess-1", -300, 300, "sess-1:open:1");

    expect(invoke).toHaveBeenCalledWith("get_session_open_window", {
      sessionId: "sess-1",
      offset: -300,
      limit: 300,
      requestId: "sess-1:open:1",
    });
  });

  it("cancelSessionLoad sends request identity when provided", async () => {
    const { cancelSessionLoad } = await import("@/lib/tauri");

    await cancelSessionLoad("sess-1", "sess-1:open:1");

    expect(invoke).toHaveBeenCalledWith("cancel_session_load", {
      sessionId: "sess-1",
      requestId: "sess-1:open:1",
    });
  });

  it("exportSession uses the simplified session-based payload", async () => {
    const { exportSession } = await import("@/lib/tauri");

    await exportSession("sess-1", "json", "/tmp/out.json");

    expect(invoke).toHaveBeenCalledWith("export_session", {
      sessionId: "sess-1",
      format: "json",
      outputPath: "/tmp/out.json",
    });
  });

  it("resumeSession sends sessionId plus terminal app", async () => {
    const { resumeSession } = await import("@/lib/tauri");

    await resumeSession("sess-1", "iTerm");

    expect(invoke).toHaveBeenCalledWith("resume_session", {
      sessionId: "sess-1",
      terminalApp: "iTerm",
    });
  });

  it("getResumeCommand sends only sessionId", async () => {
    const { getResumeCommand } = await import("@/lib/tauri");

    await getResumeCommand("sess-1");

    expect(invoke).toHaveBeenCalledWith("get_resume_command", {
      sessionId: "sess-1",
    });
  });

  it("trashSession sends only sessionId", async () => {
    const { trashSession } = await import("@/lib/tauri");

    await trashSession("sess-1");

    expect(invoke).toHaveBeenCalledWith("trash_session", {
      sessionId: "sess-1",
    });
  });

  it("getProviderSnapshots calls the snapshot endpoint", async () => {
    const { getProviderSnapshots } = await import("@/lib/tauri");

    await getProviderSnapshots();

    expect(invoke).toHaveBeenCalledWith("get_provider_snapshots");
  });

  it("readToolResultText sends path", async () => {
    const { readToolResultText } = await import("@/lib/tauri");

    await readToolResultText("/tmp/tool-results/out.txt");

    expect(invoke).toHaveBeenCalledWith("read_tool_result_text", {
      path: "/tmp/tool-results/out.txt",
    });
  });

  it("exportSessionsBatch sends string ids instead of tuple payloads", async () => {
    const { exportSessionsBatch } = await import("@/lib/tauri");

    await exportSessionsBatch(["s1", "s2"], "markdown", "/tmp/export.zip");

    expect(invoke).toHaveBeenCalledWith("export_sessions_batch", {
      items: ["s1", "s2"],
      format: "markdown",
      outputPath: "/tmp/export.zip",
    });
  });
});

describe("invokeWithToast", () => {
  let errSpy: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    toastError.mockReset();
    errSpy = vi.spyOn(console, "error").mockImplementation(() => {});
  });

  afterEach(() => {
    errSpy.mockRestore();
  });

  it("returns the resolved value on success and does not toast", async () => {
    const { invokeWithToast } = await import("@/lib/tauri");

    const result = await invokeWithToast(Promise.resolve(42), "compute answer");

    expect(result).toBe(42);
    expect(toastError).not.toHaveBeenCalled();
  });

  it("toasts with context + error message and rethrows on failure", async () => {
    const { invokeWithToast } = await import("@/lib/tauri");
    const err = new Error("boom");

    await expect(
      invokeWithToast(Promise.reject(err), "compute answer"),
    ).rejects.toBe(err);

    expect(toastError).toHaveBeenCalledWith("compute answer: boom");
    expect(errSpy).toHaveBeenCalledWith("compute answer: boom");
  });

  it("handles non-Error throwables by stringifying them", async () => {
    const { invokeWithToast } = await import("@/lib/tauri");

    await expect(
      invokeWithToast(Promise.reject("plain string"), "ctx"),
    ).rejects.toBe("plain string");

    expect(toastError).toHaveBeenCalledWith("ctx: plain string");
  });
});

describe("invokeWithFallback", () => {
  let errSpy: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    toastError.mockReset();
    errSpy = vi.spyOn(console, "error").mockImplementation(() => {});
  });

  afterEach(() => {
    errSpy.mockRestore();
  });

  it("returns the resolved value on success", async () => {
    const { invokeWithFallback } = await import("@/lib/tauri");

    const result = await invokeWithFallback(
      Promise.resolve(42),
      0,
      "refresh stats",
    );

    expect(result).toBe(42);
    expect(errSpy).not.toHaveBeenCalled();
  });

  it("returns the fallback and logs (but does NOT toast) on failure", async () => {
    const { invokeWithFallback } = await import("@/lib/tauri");

    const result = await invokeWithFallback(
      Promise.reject(new Error("network down")),
      99,
      "refresh stats",
    );

    expect(result).toBe(99);
    expect(errSpy).toHaveBeenCalledWith("refresh stats: network down");
    expect(toastError).not.toHaveBeenCalled();
  });

  it("accepts a widened fallback type (T | undefined)", async () => {
    const { invokeWithFallback } = await import("@/lib/tauri");

    const result = await invokeWithFallback<number, undefined>(
      Promise.reject(new Error("x")),
      undefined,
      "ctx",
    );

    expect(result).toBeUndefined();
  });
});
