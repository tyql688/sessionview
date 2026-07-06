import { describe, it, expect } from "vitest";
import { errorMessage } from "@/lib/errors";

describe("errorMessage", () => {
  it("extracts message from Error", () => {
    expect(errorMessage(new Error("boom"))).toBe("boom");
  });
  it("returns string as-is", () => {
    expect(errorMessage("something failed")).toBe("something failed");
  });
  it("converts object to string", () => {
    expect(errorMessage({ code: 42 })).toBe("[object Object]");
  });
  it("converts null", () => {
    expect(errorMessage(null)).toBe("null");
  });
});
