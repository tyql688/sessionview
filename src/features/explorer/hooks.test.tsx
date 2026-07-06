import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { TreeNode } from "@/lib/types";

vi.mock("@/lib/tauri", () => ({
  detectTerminal: vi.fn().mockResolvedValue("terminal"),
}));

import {
  addBlockedFolder,
  getBlockedFolders,
  removeBlockedFolder,
} from "@/stores/settings";
import { filterBlockedFolders, groupTreeByDirectory } from "@/features/explorer/hooks";

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

describe("groupTreeByDirectory", () => {
  function session(
    id: string,
    provider: TreeNode["provider"],
    updatedAt: number,
  ): TreeNode {
    return {
      id,
      label: id,
      node_type: "session",
      children: [],
      count: 0,
      provider,
      updated_at: updatedAt,
    };
  }

  function providerTree(): TreeNode[] {
    return [
      {
        id: "claude",
        label: "Claude",
        node_type: "provider",
        children: [
          {
            id: "claude:/repo/app",
            label: "app",
            node_type: "project",
            project_path: "/repo/app",
            children: [session("c1", "claude", 100), session("c2", "claude", 300)],
            count: 2,
            provider: "claude",
          },
        ],
        count: 2,
        provider: "claude",
      },
      {
        id: "codex",
        label: "Codex",
        node_type: "provider",
        children: [
          {
            id: "codex:/repo/app",
            label: "app",
            node_type: "project",
            project_path: "/repo/app",
            children: [session("x1", "codex", 200)],
            count: 1,
            provider: "codex",
          },
          {
            id: "codex:none",
            label: "(No Project)",
            node_type: "project",
            children: [session("x2", "codex", 999)],
            count: 1,
            provider: "codex",
          },
        ],
        count: 2,
        provider: "codex",
      },
    ];
  }

  it("merges the same directory across providers, newest session first", () => {
    const groups = groupTreeByDirectory(providerTree(), "No Project");
    const app = groups.find((g) => g.project_path === "/repo/app");
    expect(app).toBeDefined();
    expect(app?.count).toBe(3);
    expect(app?.children.map((c) => c.id)).toEqual(["c2", "x1", "c1"]);
    expect(app?.id).toBe("dir:/repo/app");
  });

  it("sinks pathless sessions into a trailing labeled bucket", () => {
    const groups = groupTreeByDirectory(providerTree(), "No Project");
    const last = groups[groups.length - 1];
    expect(last?.label).toBe("No Project");
    expect(last?.project_path).toBeUndefined();
    expect(last?.children.map((c) => c.id)).toEqual(["x2"]);
  });

  it("orders directories by most recent activity", () => {
    const tree = providerTree();
    tree.push({
      id: "pi",
      label: "Pi",
      node_type: "provider",
      children: [
        {
          id: "pi:/repo/hot",
          label: "hot",
          node_type: "project",
          project_path: "/repo/hot",
          children: [session("p1", "pi", 5000)],
          count: 1,
          provider: "pi",
        },
      ],
      count: 1,
      provider: "pi",
    });
    const groups = groupTreeByDirectory(tree, "No Project");
    expect(groups[0]?.project_path).toBe("/repo/hot");
  });
});
