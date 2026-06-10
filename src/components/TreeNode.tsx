import { For, Show } from "solid-js";
import type { TreeNode } from "../lib/types";
import { useI18n } from "../i18n/index";
import { isSelected, toggleSelected } from "../stores/selection";
import { ProviderDot } from "./icons";

// Re-exports for backward compatibility
export { ProviderDot } from "./icons";
export { collectSessionIds, collectSessionNodes } from "../lib/tree-utils";

export function ChevronIcon(props: { expanded: boolean }) {
  return (
    <svg
      width="16"
      height="16"
      fill="none"
      stroke="currentColor"
      stroke-width="1.5"
      viewBox="0 0 24 24"
      class={`chevron${props.expanded ? " expanded" : ""}`}
    >
      <polyline points="9 18 15 12 9 6" />
    </svg>
  );
}

export function FolderIcon() {
  return (
    <svg
      width="16"
      height="16"
      fill="none"
      stroke="currentColor"
      stroke-width="1.5"
      viewBox="0 0 24 24"
    >
      <path d="M22 19a2 2 0 01-2 2H4a2 2 0 01-2-2V5a2 2 0 012-2h5l2 3h9a2 2 0 012 2z" />
    </svg>
  );
}

export function ChatIcon() {
  return (
    <svg
      width="16"
      height="16"
      fill="none"
      stroke="currentColor"
      stroke-width="1.5"
      viewBox="0 0 24 24"
    >
      <path d="M21 15a2 2 0 01-2 2H7l-4 4V5a2 2 0 012-2h14a2 2 0 012 2z" />
    </svg>
  );
}

export function ClockIcon() {
  return (
    <svg
      width="16"
      height="16"
      fill="none"
      stroke="currentColor"
      stroke-width="1.5"
      viewBox="0 0 24 24"
    >
      <circle cx="12" cy="12" r="10" />
      <polyline points="12 6 12 12 16 14" />
    </svg>
  );
}

export function formatSessionLabel(raw: string, fallback = "Untitled"): string {
  let label = raw;
  label = label.replace(/^##\s*TASK:\s*/i, "");
  label = label.replace(/^\d+\.\s*TASK:\s*/i, "");
  label = label.replace(/^\[search-mode\]\s*/i, "");
  label = label.replace(/^CONTEXT:\s*/i, "");
  label = label.replace(/^TASK:\s*/i, "");
  label = label.trim();

  if (/^[/~.]/.test(label) && label.includes("/")) {
    const segments = label.split("/").filter(Boolean);
    if (segments.length > 0) {
      label = segments[segments.length - 1];
    }
  }

  if (label.length > 40) {
    label = `${label.slice(0, 37)}...`;
  }

  return label || fallback;
}

/** Recursively collect all session node IDs under a tree node. */
function collectAllSessions(nodes: TreeNode[]): TreeNode[] {
  const result: TreeNode[] = [];
  for (const n of nodes) {
    if (n.node_type === "session") result.push(n);
    if (n.children.length > 0) result.push(...collectAllSessions(n.children));
  }
  return result;
}

export function TreeNodeComponent(props: {
  node: TreeNode;
  depth: number;
  activeSessionId: string | null;
  parentProjectLabel?: string;
  isNodeExpanded: (nodeId: string) => boolean;
  toggleExpanded: (nodeId: string) => void;
  onSessionContextMenu: (
    e: MouseEvent,
    node: TreeNode,
    parentProjectLabel: string,
  ) => void;
  onNodeContextMenu: (e: MouseEvent, node: TreeNode) => void;
  onSessionClick: (
    e: MouseEvent,
    node: TreeNode,
    parentProjectLabel: string,
  ) => void;
  onSessionDblClick?: (
    e: MouseEvent,
    node: TreeNode,
    parentProjectLabel: string,
  ) => void;
}) {
  const { t } = useI18n();
  const hasChildren = () => props.node.children.length > 0;
  const isSession = () => props.node.node_type === "session";
  const isSubagentParent = () => isSession() && hasChildren();
  // Project folder where ALL session descendants are sidechain (orphans)
  const isOrphanFolder = () => {
    if (props.node.node_type !== "project" || !props.node.project_path)
      return false;
    function collectSessions(nodes: TreeNode[]): TreeNode[] {
      const result: TreeNode[] = [];
      for (const n of nodes) {
        if (n.node_type === "session") result.push(n);
        else result.push(...collectSessions(n.children));
      }
      return result;
    }
    const sessions = collectSessions(props.node.children);
    return sessions.length > 0 && sessions.every((s) => s.is_sidechain);
  };
  const isLeaf = () => props.node.node_type === "session" && !hasChildren();
  const expanded = () => props.isNodeExpanded(props.node.id);

  const handleClick = (e: MouseEvent) => {
    if (isSession()) {
      props.onSessionClick(e, props.node, props.parentProjectLabel ?? "");
      // Auto-expand parent sessions that have subagents
      if (isSubagentParent() && !expanded()) {
        props.toggleExpanded(props.node.id);
      }
    } else if (e.metaKey || e.ctrlKey) {
      // Ctrl+Click on folder: select all sessions under it
      const sessions = collectAllSessions(props.node.children);
      for (const s of sessions) toggleSelected(s.id);
    } else {
      props.toggleExpanded(props.node.id);
    }
  };

  const handleDblClick = (e: MouseEvent) => {
    if (isSession() && props.onSessionDblClick) {
      e.preventDefault();
      props.onSessionDblClick(e, props.node, props.parentProjectLabel ?? "");
    }
  };

  const handleContextMenu = (e: MouseEvent) => {
    e.preventDefault();
    e.stopPropagation();
    if (isSession()) {
      props.onSessionContextMenu(e, props.node, props.parentProjectLabel ?? "");
    } else {
      props.onNodeContextMenu(e, props.node);
    }
  };

  const projectLabel = () =>
    props.node.node_type === "project"
      ? props.node.label === "(No Project)"
        ? t("explorer.noProject")
        : props.node.label
      : props.parentProjectLabel;

  const displayLabel = () => {
    if (
      props.node.node_type === "project" &&
      props.node.label === "(No Project)"
    ) {
      return t("explorer.noProject");
    }
    return props.node.label;
  };

  const nodeSelected = () => isSession() && isSelected(props.node.id);

  return (
    <div class="tree-node-wrapper">
      <button
        class={`tree-node tree-node-${props.node.node_type}${isSession() && props.activeSessionId === props.node.id ? " active" : ""}${nodeSelected() ? " selected" : ""}`}
        style={{ "padding-left": `${props.depth * 16 + 8}px` }}
        onClick={handleClick}
        onDblClick={handleDblClick}
        onContextMenu={handleContextMenu}
        data-session-id={isSession() ? props.node.id : undefined}
      >
        <Show when={!isLeaf() && !isSubagentParent()}>
          <ChevronIcon expanded={expanded()} />
        </Show>
        <Show when={isLeaf() || isSubagentParent()}>
          <span class="tree-node-icon-spacer" />
        </Show>

        <Show when={props.node.node_type === "provider" && props.node.provider}>
          <ProviderDot provider={props.node.provider!} />
        </Show>
        <Show
          when={
            props.node.node_type === "project" &&
            props.node.project_path &&
            !isOrphanFolder()
          }
        >
          <span class="tree-node-icon">
            <FolderIcon />
          </span>
        </Show>
        <Show when={isOrphanFolder()}>
          <span class="tree-node-icon tree-node-icon-orphan-folder">⤷</span>
        </Show>
        <Show
          when={props.node.node_type === "project" && !props.node.project_path}
        >
          <span class="tree-node-icon tree-node-icon-time">
            <ClockIcon />
          </span>
        </Show>
        <Show
          when={
            props.node.node_type === "session" &&
            isSession() &&
            props.node.is_sidechain &&
            !isSubagentParent()
          }
        >
          <span class="tree-node-icon tree-node-icon-orphan">⤷</span>
        </Show>
        <Show
          when={
            props.node.node_type === "session" &&
            !(props.node.is_sidechain && !isSubagentParent())
          }
        >
          <span class="tree-node-icon">
            <ChatIcon />
          </span>
        </Show>

        <span
          class={`tree-node-label${props.node.node_type === "provider" ? " bold" : ""}`}
          title={
            props.node.node_type === "session" ? props.node.label : undefined
          }
        >
          {props.node.node_type === "session"
            ? formatSessionLabel(props.node.label, t("common.untitled"))
            : displayLabel()}
        </span>

        <Show when={props.node.is_sidechain}>
          <span class="tree-node-sidechain" title={t("common.subagentSession")}>
            ⤷
          </span>
        </Show>
        <Show when={props.node.count > 0 && !isLeaf()}>
          <span class="tree-node-count">{props.node.count}</span>
        </Show>
      </button>

      {/* Subagent children always visible under parent session */}
      <Show when={isSubagentParent()}>
        <For each={props.node.children}>
          {(child) => (
            <TreeNodeComponent
              node={child}
              depth={props.depth + 1}
              activeSessionId={props.activeSessionId}
              parentProjectLabel={projectLabel()}
              isNodeExpanded={props.isNodeExpanded}
              toggleExpanded={props.toggleExpanded}
              onSessionContextMenu={props.onSessionContextMenu}
              onNodeContextMenu={props.onNodeContextMenu}
              onSessionClick={props.onSessionClick}
              onSessionDblClick={props.onSessionDblClick}
            />
          )}
        </For>
      </Show>
      {/* Provider/project children use expand/collapse */}
      <Show when={expanded() && !isLeaf() && !isSubagentParent()}>
        <For each={props.node.children}>
          {(child) => (
            <TreeNodeComponent
              node={child}
              depth={props.depth + 1}
              activeSessionId={props.activeSessionId}
              parentProjectLabel={projectLabel()}
              isNodeExpanded={props.isNodeExpanded}
              toggleExpanded={props.toggleExpanded}
              onSessionContextMenu={props.onSessionContextMenu}
              onNodeContextMenu={props.onNodeContextMenu}
              onSessionClick={props.onSessionClick}
              onSessionDblClick={props.onSessionDblClick}
            />
          )}
        </For>
      </Show>
    </div>
  );
}
