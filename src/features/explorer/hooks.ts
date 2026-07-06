import type { TreeNode, SessionRef, Provider } from "@/lib/types";
import { getBlockedFolders, isPathBlocked } from "@/stores/settings";

/** Filter out projects whose path matches a blocked folder. */
export function filterBlockedFolders(tree: TreeNode[]): TreeNode[] {
  if (getBlockedFolders().length === 0) {
    return tree;
  }

  function prune(nodes: TreeNode[]): TreeNode[] {
    return nodes.flatMap((node) => {
      const path = node.project_path ?? "";
      if (node.node_type === "project" && path && isPathBlocked(path)) {
        return [];
      }

      if (node.node_type === "session") {
        return [node];
      }

      const children = prune(node.children);
      if (children.length === 0) {
        return [];
      }

      return [{ ...node, children }];
    });
  }

  return prune(tree);
}

function countSessions(nodes: TreeNode[]): number {
  let n = 0;
  for (const node of nodes) {
    if (node.node_type === "session") n++;
    else n += countSessions(node.children);
  }
  return n;
}

/** Remove sidechain subagents and update counts. */
export function filterOrphanSubagents(tree: TreeNode[]): TreeNode[] {
  function prune(nodes: TreeNode[]): TreeNode[] {
    return nodes
      .map((node) => {
        const children = prune(node.children);
        // Strip sidechain children from session nodes
        const filtered =
          node.node_type === "session"
            ? children.filter((c) => !c.is_sidechain)
            : children;
        return {
          ...node,
          children: filtered,
          count: node.node_type !== "session" ? countSessions(filtered) : 0,
        };
      })
      .filter((node) => {
        // Remove sidechain sessions at project level
        if (node.node_type === "session" && node.is_sidechain) {
          return false;
        }
        // Remove empty non-session containers
        if (node.node_type !== "session" && node.children.length === 0) {
          return false;
        }
        return true;
      });
  }
  return prune(tree);
}

/**
 * Regroup the provider-rooted tree by working directory: one root node per
 * distinct project_path, merged across providers, sessions inside sorted by
 * recency and identified by their provider dot. Sessions without a recorded
 * project path collect under a trailing "no project" group labeled by
 * `noProjectLabel`.
 */
export function groupTreeByDirectory(
  tree: TreeNode[],
  noProjectLabel: string,
): TreeNode[] {
  interface DirBucket {
    path: string;
    label: string;
    sessions: TreeNode[];
  }
  const buckets = new Map<string, DirBucket>();

  function bucketFor(path: string, label: string): DirBucket {
    let bucket = buckets.get(path);
    if (!bucket) {
      bucket = { path, label, sessions: [] };
      buckets.set(path, bucket);
    }
    return bucket;
  }

  function walk(nodes: TreeNode[], projectPath: string, projectLabel: string) {
    for (const node of nodes) {
      if (node.node_type === "session") {
        // Keep the session node intact — subagent children stay nested.
        bucketFor(projectPath, projectLabel).sessions.push(node);
        continue;
      }
      const nextPath =
        node.node_type === "project" ? (node.project_path ?? "") : projectPath;
      const nextLabel =
        node.node_type === "project" ? node.label : projectLabel;
      walk(node.children, nextPath, nextLabel);
    }
  }
  walk(tree, "", noProjectLabel);

  const newestOf = (sessions: TreeNode[]) =>
    sessions.reduce((max, s) => Math.max(max, s.updated_at ?? 0), 0);

  const groups = [...buckets.values()]
    .filter((bucket) => bucket.sessions.length > 0)
    .map((bucket) => {
      const sessions = [...bucket.sessions].sort(
        (a, b) => (b.updated_at ?? 0) - (a.updated_at ?? 0),
      );
      return {
        id: `dir:${bucket.path || "none"}`,
        label: bucket.path ? bucket.label : noProjectLabel,
        node_type: "project" as const,
        children: sessions,
        count: sessions.length,
        provider: null,
        project_path: bucket.path || undefined,
        updated_at: newestOf(sessions),
      };
    });

  // Most recently active directories first; the pathless bucket sinks last.
  groups.sort((a, b) => {
    if (!a.project_path !== !b.project_path) return a.project_path ? -1 : 1;
    return (b.updated_at ?? 0) - (a.updated_at ?? 0);
  });
  return groups;
}

export function buildSessionRef(
  node: TreeNode,
  parentProjectLabel: string,
): SessionRef {
  return {
    id: node.id,
    provider: (node.provider ?? "claude") as Provider,
    title: node.label,
    project_name: parentProjectLabel,
    is_sidechain: node.is_sidechain ?? false,
  };
}
