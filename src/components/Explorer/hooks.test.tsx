import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { TreeNode } from "../../lib/types";

vi.mock("../../lib/tauri", () => ({
  detectTerminal: vi.fn().mockResolvedValue("terminal"),
}));

import {
  addBlockedFolder,
  getBlockedFolders,
  removeBlockedFolder,
} from "../../stores/settings";
import { filterBlockedFolders } from "./hooks";

function clearBlockedFolders() {
  for (const folder of [...getBlockedFolders()]) {
    removeBlockedFolder(folder);
  }
}

function makeTree(): TreeNode[] {
  return [
    {
      id: "claude",
      label: "Claude",
      node_type: "provider",
      children: [
        {
          id: "claude:/repo/visible",
          label: "visible",
          node_type: "project",
          children: [
            {
              id: "visible-session",
              label: "Visible session",
              node_type: "session",
              children: [],
              count: 0,
              provider: "claude",
            },
          ],
          count: 1,
          provider: "claude",
          project_path: "/repo/visible",
        },
        {
          id: "claude:/repo/blocked",
          label: "blocked",
          node_type: "project",
          children: [
            {
              id: "blocked-session",
              label: "Blocked session",
              node_type: "session",
              children: [],
              count: 0,
              provider: "claude",
            },
          ],
          count: 1,
          provider: "claude",
          project_path: "/repo/blocked",
        },
      ],
      count: 2,
      provider: "claude",
    },
  ];
}

describe("filterBlockedFolders", () => {
  beforeEach(() => {
    clearBlockedFolders();
  });
  afterEach(() => {
    clearBlockedFolders();
  });

  it("returns the original tree when no blocked folders are configured", () => {
    const tree = makeTree();

    expect(filterBlockedFolders(tree)).toBe(tree);
  });

  it("filters blocked project paths when configured", () => {
    addBlockedFolder("/repo/blocked");
    const filtered = filterBlockedFolders(makeTree());

    expect(filtered).toHaveLength(1);
    expect(filtered[0].children.map((node) => node.id)).toEqual([
      "claude:/repo/visible",
    ]);
    expect(filtered[0].count).toBe(2);
  });
});
