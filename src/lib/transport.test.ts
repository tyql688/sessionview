import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();

vi.mock("@tauri-apps/api/core", () => ({
  invoke,
}));

vi.mock("@/lib/runtime", () => ({
  isTauriRuntime: false,
  backendToken: vi.fn(() => "secret-token"),
  withBackendToken: (path: string) => path,
}));

describe("invokeBackend (headless transport)", () => {
  const fetchMock = vi.fn();

  beforeEach(() => {
    vi.stubGlobal("fetch", fetchMock);
    fetchMock.mockReset();
    invoke.mockReset();
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("POSTs camelCase args to /api/invoke and returns the JSON result", async () => {
    fetchMock.mockResolvedValue({
      ok: true,
      json: () => Promise.resolve(42),
    });
    const { invokeBackend } = await import("./transport");

    const result = await invokeBackend<number>("get_session_count", { sessionId: "s-1" });

    expect(result).toBe(42);
    expect(invoke).not.toHaveBeenCalled();
    expect(fetchMock).toHaveBeenCalledWith("/api/invoke/get_session_count", {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        "X-SessionView-Token": "secret-token",
      },
      body: JSON.stringify({ sessionId: "s-1" }),
    });
  });

  it("sends an empty object body for no-arg commands", async () => {
    fetchMock.mockResolvedValue({ ok: true, json: () => Promise.resolve(null) });
    const { invokeBackend } = await import("./transport");

    await invokeBackend("get_tree");

    const [, init] = fetchMock.mock.calls[0] as [string, RequestInit];
    expect(init.body).toBe("{}");
  });

  it("throws the server's error text verbatim so sentinel checks survive", async () => {
    fetchMock.mockResolvedValue({
      ok: false,
      text: () => Promise.resolve("load failed: __sessionview_load_canceled__"),
    });
    const { invokeBackend } = await import("./transport");
    const { isLoadCanceledError } = await import("./tauri");

    const error = await invokeBackend("get_session_detail", { sessionId: "s-1" }).catch((e: unknown) => e);

    expect(error).toBeInstanceOf(Error);
    expect(isLoadCanceledError(error)).toBe(true);
  });
});
