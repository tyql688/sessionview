import { afterEach, describe, expect, it, vi } from "vitest";
import { preferredScrollBehavior } from "@/lib/motion";

afterEach(() => vi.unstubAllGlobals());

describe("preferredScrollBehavior", () => {
  it("disables smooth scrolling when reduced motion is requested", () => {
    vi.stubGlobal("window", { matchMedia: () => ({ matches: true }) });
    expect(preferredScrollBehavior()).toBe("auto");
  });

  it("uses smooth scrolling otherwise", () => {
    vi.stubGlobal("window", { matchMedia: () => ({ matches: false }) });
    expect(preferredScrollBehavior()).toBe("smooth");
  });
});
