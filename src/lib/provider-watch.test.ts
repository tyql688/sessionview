import { describe, expect, it } from "vitest";
import {
  getPollWatchProviders,
  getProviderWatchConfig,
} from "@/lib/provider-watch";
import type { Provider } from "@/lib/types";

const ALL_PROVIDERS: Provider[] = [
  "claude",
  "codex",
  "antigravity",
  "opencode",
  "kimi",
  "cursor",
  "cc-mirror",
];

describe("provider-watch", () => {
  it("getProviderWatchConfig returns config for all providers", () => {
    for (const key of ALL_PROVIDERS) {
      const config = getProviderWatchConfig(key);
      expect(config).toBeDefined();
      expect(config.debounceMs).toBeGreaterThan(0);
    }
  });

  it("combines provider strategy with frontend watch behavior", () => {
    const config = getProviderWatchConfig("antigravity");
    expect(config.strategy).toBe("fs");
    expect(config.debounceMs).toBeGreaterThan(0);
  });

  it("returns poll providers from the active catalog", () => {
    expect(getPollWatchProviders()).toEqual(["opencode"]);
  });
});
