import { describe, it, expect } from "vitest";
import { buildFavoritesTree, buildTrashTree } from "@/lib/tree-builders";
import type { SessionMeta, TrashMeta } from "@/lib/types";

function makeSession(overrides: Partial<SessionMeta> = {}): SessionMeta {
  return {
    id: "sess-1",
    provider: "claude",
    title: "Test Session",
    project_path: "/home/user/project",
    project_name: "project",
    created_at: 1711800000,
    updated_at: 1711800000,
    message_count: 5,
    file_size_bytes: 1024,
    source_path: "/home/user/.claude/projects/project/session.jsonl",
    is_sidechain: false,
    input_tokens: 0,
    output_tokens: 0,
    cache_read_tokens: 0,
    cache_write_tokens: 0,
    ...overrides,
  };
}

function makeTrashItem(overrides: Partial<TrashMeta> = {}): TrashMeta {
  return {
    id: "trash-1",
    provider: "claude",
    title: "Trashed Session",
    original_path: "/home/user/.claude/projects/myproject/session.jsonl",
    trashed_at: 1711800000,
    trash_file: "/trash/trash-1.jsonl",
    project_name: "myproject",
    variant_name: undefined,
    ...overrides,
  };
}

describe("buildFavoritesTree", () => {
  it("returns [] for empty input", () => {
    expect(buildFavoritesTree([], "No Project")).toEqual([]);
  });

  it("groups by provider then project", () => {
    const sessions = [
      makeSession({
        id: "s1",
        provider: "claude",
        project_name: "proj-a",
        project_path: "/a",
      }),
      makeSession({
        id: "s2",
        provider: "claude",
        project_name: "proj-a",
        project_path: "/a",
      }),
      makeSession({
        id: "s3",
        provider: "codex",
        project_name: "proj-b",
        project_path: "/b",
      }),
    ];
    const tree = buildFavoritesTree(sessions, "No Project");

    expect(tree).toHaveLength(2);

    const claudeNode = tree.find((n) => n.provider === "claude");
    expect(claudeNode).toBeDefined();
    expect(claudeNode!.node_type).toBe("provider");
    expect(claudeNode!.count).toBe(2);
    expect(claudeNode!.children).toHaveLength(1);
    expect(claudeNode!.children[0].node_type).toBe("project");
    expect(claudeNode!.children[0].children).toHaveLength(2);

    const codexNode = tree.find((n) => n.provider === "codex");
    expect(codexNode).toBeDefined();
    expect(codexNode!.count).toBe(1);
  });

  it("orders provider groups by provider sort order", () => {
    const sessions = [
      makeSession({
        id: "s1",
        provider: "kimi",
        project_name: "proj-k",
        project_path: "/k",
      }),
      makeSession({
        id: "s2",
        provider: "claude",
        project_name: "proj-c",
        project_path: "/c",
      }),
      makeSession({
        id: "s3",
        provider: "antigravity",
        project_name: "proj-g",
        project_path: "/g",
      }),
    ];

    const tree = buildFavoritesTree(sessions, "No Project");
    expect(tree.map((node) => node.provider)).toEqual([
      "claude",
      "antigravity",
      "kimi",
    ]);
  });

  it("groups cc-mirror favorites as top-level variant groups", () => {
    const sessions = [
      makeSession({
        id: "m1",
        provider: "cc-mirror",
        variant_name: "cczai",
        project_name: "proj-a",
        project_path: "/a",
      }),
      makeSession({
        id: "m2",
        provider: "cc-mirror",
        variant_name: "cczai",
        project_name: "proj-b",
        project_path: "/b",
      }),
    ];

    const tree = buildFavoritesTree(sessions, "No Project");
    expect(tree).toHaveLength(1);
    expect(tree[0].label).toBe("cczai");
    expect(tree[0].node_type).toBe("provider");
    expect(tree[0].children).toHaveLength(2);
    expect(tree[0].children[0].node_type).toBe("project");
  });
});

describe("buildTrashTree", () => {
  const labels = { unknown: "Unknown", untitled: "Untitled" };

  it("returns [] for empty input", () => {
    expect(buildTrashTree([], labels)).toEqual([]);
  });

  it("derives project from provider-aware original_path fallback", () => {
    const items = [
      makeTrashItem({
        id: "t1",
        project_name: "",
        original_path: "/home/user/.claude/projects/myproject/session.jsonl",
      }),
    ];
    const tree = buildTrashTree(items, labels);

    expect(tree).toHaveLength(1);
    expect(tree[0].node_type).toBe("provider");
    expect(tree[0].children).toHaveLength(1);
    expect(tree[0].children[0].label).toBe("myproject");
    expect(tree[0].children[0].children).toHaveLength(1);
    expect(tree[0].children[0].children[0].id).toBe("t1");
  });

  it("does not treat codex session ids as project names", () => {
    const items = [
      makeTrashItem({
        id: "t1",
        provider: "codex",
        project_name: "",
        original_path:
          "/Users/test/.codex/sessions/2026/05/09/rollout-2026-05-09T12-00-00-abc123.jsonl",
      }),
    ];

    const tree = buildTrashTree(items, labels);
    expect(tree[0].children[0].label).toBe("Unknown");
  });

  it("falls back to unknown for kimi legacy entries", () => {
    const items = [
      makeTrashItem({
        id: "t1",
        provider: "kimi",
        project_name: "",
        original_path:
          "/Users/test/.kimi/sessions/d43b8ea075dfbc269128c50a437f3627/de8cd3a2-30c1-40bf-ad19-f43acc708caa/wire.jsonl",
      }),
    ];

    const tree = buildTrashTree(items, labels);
    expect(tree[0].children[0].label).toBe("Unknown");
  });

  it("falls back to unknown for codex legacy entries", () => {
    const items = [
      makeTrashItem({
        id: "t1",
        provider: "codex",
        project_name: "",
        original_path:
          "/Users/test/.codex/sessions/2026/04/08/rollout-2026-04-08T13-13-10-019d6b82-3dd6-7981-a67b-6b13b9166661.jsonl",
      }),
    ];

    const tree = buildTrashTree(items, labels);
    expect(tree[0].children[0].label).toBe("Unknown");
  });

  it("groups cc-mirror trash entries as top-level variant groups", () => {
    const items = [
      makeTrashItem({
        id: "t1",
        provider: "cc-mirror",
        variant_name: "cczai",
        project_name: "proj-a",
      }),
    ];

    const tree = buildTrashTree(items, labels);
    expect(tree[0].label).toBe("cczai");
    expect(tree[0].node_type).toBe("provider");
    expect(tree[0].children[0].label).toBe("proj-a");
  });
});
