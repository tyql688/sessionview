import { describe, expect, it } from "vitest";
import en from "@/i18n/en.json";
import enSource from "@/i18n/en.json?raw";
import zh from "@/i18n/zh.json";
import zhSource from "@/i18n/zh.json?raw";

function parseString(source: string, index: number): [string, number] {
  let value = "";
  let i = index + 1;
  while (i < source.length) {
    const char = source[i];
    if (char === '"') return [value, i + 1];
    if (char === "\\") {
      const next = source[i + 1];
      if (next === undefined) return [value, i + 1];
      value += next;
      i += 2;
      continue;
    }
    value += char;
    i += 1;
  }
  return [value, i];
}

function nextNonWhitespace(source: string, index: number): string | undefined {
  for (let i = index; i < source.length; i += 1) {
    if (!/\s/.test(source[i])) return source[i];
  }
  return undefined;
}

function duplicateObjectKeys(source: string): string[] {
  const duplicates: string[] = [];
  const stack: Array<{
    type: "object" | "array";
    keys: Set<string>;
    expectingKey: boolean;
    path: string[];
    pendingKey?: string;
  }> = [];

  for (let i = 0; i < source.length; i += 1) {
    const char = source[i];
    const parent = stack[stack.length - 1];

    if (char === '"') {
      const [value, end] = parseString(source, i);
      const next = nextNonWhitespace(source, end);
      const ctx = stack[stack.length - 1];
      if (ctx?.type === "object" && ctx.expectingKey && next === ":") {
        const path = [...ctx.path, value].join(".");
        if (ctx.keys.has(value)) duplicates.push(path);
        ctx.keys.add(value);
        ctx.pendingKey = value;
        ctx.expectingKey = false;
      }
      i = end - 1;
      continue;
    }

    if (char === "{") {
      const path =
        parent?.type === "object" && parent.pendingKey
          ? [...parent.path, parent.pendingKey]
          : (parent?.path ?? []);
      stack.push({
        type: "object",
        keys: new Set(),
        expectingKey: true,
        path,
      });
      continue;
    }

    if (char === "[") {
      const path =
        parent?.type === "object" && parent.pendingKey
          ? [...parent.path, parent.pendingKey]
          : (parent?.path ?? []);
      stack.push({ type: "array", keys: new Set(), expectingKey: false, path });
      continue;
    }

    if (char === "}") {
      stack.pop();
      continue;
    }

    if (char === "]") {
      stack.pop();
      continue;
    }

    if (char === "," && parent?.type === "object") {
      parent.expectingKey = true;
      parent.pendingKey = undefined;
    }
  }

  return duplicates;
}

describe("i18n dictionaries", () => {
  it("do not contain duplicate object keys", () => {
    for (const [file, source] of [
      ["en.json", enSource],
      ["zh.json", zhSource],
    ] as const) {
      expect(duplicateObjectKeys(source), file).toEqual([]);
    }
  });

  it("contains labels for recent system events", () => {
    for (const dict of [en, zh]) {
      expect(dict.system.awaySummary).toBeTruthy();
      expect(dict.system.scheduledTask).toBeTruthy();
      expect(dict.system.taskStatus).toBeTruthy();
      expect(dict.system.subagentTask).toBeTruthy();
      expect(dict.system.skillActivation).toBeTruthy();
      expect(dict.system.kimiContext).toBeTruthy();
      expect(dict.system.prLink).toBeTruthy();
      expect(dict.system.error).toBeTruthy();
      expect(dict.system.turnAborted).toBeTruthy();
      expect(dict.system.contextCompacted).toBeTruthy();
    }
  });
});
