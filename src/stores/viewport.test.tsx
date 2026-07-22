import { describe, it, expect, afterEach } from "vitest";
import { renderHook, act } from "@testing-library/react";
import { useIsCompact, useIsCoarse, isCompactViewport, isCoarsePointer, _setViewportForTest } from "@/stores/viewport";

describe("viewport store", () => {
  afterEach(() => {
    act(() => _setViewportForTest({ isCompact: false, isCoarse: false }));
  });

  it("defaults to desktop flags in the test environment", () => {
    expect(isCompactViewport()).toBe(false);
    expect(isCoarsePointer()).toBe(false);
  });

  it("exposes reactive hooks that follow the store", () => {
    const compact = renderHook(() => useIsCompact());
    const coarse = renderHook(() => useIsCoarse());
    expect(compact.result.current).toBe(false);
    expect(coarse.result.current).toBe(false);

    act(() => _setViewportForTest({ isCompact: true, isCoarse: true }));

    expect(compact.result.current).toBe(true);
    expect(coarse.result.current).toBe(true);
  });

  it("mirrors the compact flag onto the root data attribute", () => {
    act(() => _setViewportForTest({ isCompact: true }));
    expect(document.documentElement.hasAttribute("data-compact")).toBe(true);

    act(() => _setViewportForTest({ isCompact: false }));
    expect(document.documentElement.hasAttribute("data-compact")).toBe(false);
  });
});
