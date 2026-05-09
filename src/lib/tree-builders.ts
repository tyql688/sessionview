import type { SessionMeta, TrashMeta, TreeNode, Provider } from "./types";
import {
  getProviderLabel,
  getProviderSortOrder,
} from "../stores/providerSnapshots";

const KNOWN_PROVIDER_KEYS = new Set<string>([
  "claude",
  "codex",
  "gemini",
  "opencode",
  "kimi",
  "cc-mirror",
  "qwen",
]);

function parseProviderKey(provider: string): Provider | null {
  return KNOWN_PROVIDER_KEYS.has(provider) ? (provider as Provider) : null;
}

function projectFromTrashPath(item: TrashMeta, unknownLabel: string): string {
  const provider = item.provider || "claude";
  const path = item.original_path.replaceAll("\\", "/");
  const segments = path.split("/").filter(Boolean);
  if (segments.length === 0) {
    return unknownLabel;
  }

  const projectsIndex = segments.lastIndexOf("projects");
  if (projectsIndex >= 0 && projectsIndex + 1 < segments.length) {
    return segments[projectsIndex + 1] || unknownLabel;
  }

  switch (provider) {
    case "claude":
    case "cc-mirror":
      return segments.at(-2) || unknownLabel;
    case "codex":
    case "gemini":
    case "kimi":
    case "opencode":
    case "qwen":
    default:
      return unknownLabel;
  }
}

type ProviderGroup<T> = {
  provider: Provider;
  label: string;
  projectMap: Map<string, T>;
};

function providerGroupKey(provider: Provider, variantName?: string): string {
  if (provider !== "cc-mirror") {
    return provider;
  }
  return variantName ? `cc-mirror:${variantName}` : "cc-mirror";
}

function sortProviderGroups<T>(
  entries: [string, ProviderGroup<T>][],
): [string, ProviderGroup<T>][] {
  return entries.sort(([, left], [, right]) => {
    const orderDiff =
      getProviderSortOrder(left.provider) -
      getProviderSortOrder(right.provider);
    if (orderDiff !== 0) {
      return orderDiff;
    }
    return left.label.localeCompare(right.label);
  });
}

export function buildFavoritesTree(
  sessions: SessionMeta[],
  noProjectLabel: string,
): TreeNode[] {
  const providerMap = new Map<
    string,
    ProviderGroup<{ label: string; sessions: SessionMeta[] }>
  >();

  for (const session of sessions) {
    const provider = session.provider || "claude";
    const key = providerGroupKey(provider, session.variant_name);
    const projectKey = session.project_path || "__no_project__";
    const projectLabel = session.project_name || noProjectLabel;

    if (!providerMap.has(key)) {
      providerMap.set(key, {
        provider,
        label: getProviderLabel(provider, session.variant_name),
        projectMap: new Map(),
      });
    }

    const projectMap = providerMap.get(key)!.projectMap;
    if (!projectMap.has(projectKey)) {
      projectMap.set(projectKey, { label: projectLabel, sessions: [] });
    }
    projectMap.get(projectKey)!.sessions.push(session);
  }

  const tree: TreeNode[] = [];
  for (const [providerKey, group] of sortProviderGroups([
    ...providerMap.entries(),
  ])) {
    const projectNodes: TreeNode[] = [];
    for (const [projectKey, projectGroup] of group.projectMap) {
      const sessionNodes: TreeNode[] = projectGroup.sessions.map((session) => ({
        id: session.id,
        label: session.title,
        node_type: "session" as const,
        children: [],
        count: 0,
        provider: session.provider as Provider,
      }));
      projectNodes.push({
        id: `fav-${providerKey}-${projectKey}`,
        label: projectGroup.label,
        node_type: "project" as const,
        children: sessionNodes,
        count: sessionNodes.length,
        provider: null,
        project_path: projectKey === "__no_project__" ? undefined : projectKey,
      });
    }
    tree.push({
      id: `fav-${providerKey}`,
      label: group.label,
      node_type: "provider" as const,
      children: projectNodes,
      count: projectNodes.reduce((sum, node) => sum + node.count, 0),
      provider: group.provider,
    });
  }

  return tree;
}

export function buildTrashTree(
  items: TrashMeta[],
  labels: { unknown: string; untitled: string },
): TreeNode[] {
  const providerMap = new Map<string, ProviderGroup<TrashMeta[]>>();

  for (const item of items) {
    const provider = parseProviderKey(item.provider || "claude");
    if (!provider) {
      console.warn(
        `skipping trash entry ${item.id} with unsupported provider ${item.provider}`,
      );
      continue;
    }
    const key = providerGroupKey(provider, item.variant_name);
    const project =
      item.project_name?.trim() || projectFromTrashPath(item, labels.unknown);

    if (!providerMap.has(key)) {
      providerMap.set(key, {
        provider,
        label: getProviderLabel(provider, item.variant_name),
        projectMap: new Map(),
      });
    }

    const projectMap = providerMap.get(key)!.projectMap;
    if (!projectMap.has(project)) {
      projectMap.set(project, []);
    }
    projectMap.get(project)!.push(item);
  }

  const tree: TreeNode[] = [];
  for (const [providerKey, group] of sortProviderGroups([
    ...providerMap.entries(),
  ])) {
    const projectNodes: TreeNode[] = [];
    for (const [project, sessions] of group.projectMap) {
      const sessionNodes: TreeNode[] = sessions.map((item) => ({
        id: item.id,
        label: item.title || labels.untitled,
        node_type: "session" as const,
        children: [],
        count: 0,
        provider: group.provider,
      }));
      projectNodes.push({
        id: `trash-${providerKey}-${project}`,
        label: project,
        node_type: "project" as const,
        children: sessionNodes,
        count: sessionNodes.length,
        provider: null,
      });
    }
    tree.push({
      id: `trash-${providerKey}`,
      label: group.label,
      node_type: "provider" as const,
      children: projectNodes,
      count: projectNodes.reduce((sum, node) => sum + node.count, 0),
      provider: group.provider,
    });
  }

  return tree;
}
