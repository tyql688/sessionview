import type React from "react";
import { Button } from "@/components/ui/button";
import { formatTreeTime } from "@/lib/formatters";
import type { TreeNode } from "@/lib/types";
import { useI18n } from "@/i18n/index";
import { isSelected, toggleSelected } from "@/features/explorer/selection";
import { getProviderColor } from "@/stores/providerSnapshots";
import { ProviderDot } from "@/components/icons";

export { collectSessionNodes } from "@/lib/tree-utils";

function ChevronIcon(props: { expanded: boolean }) {
  return (
    <svg
      width="16"
      height="16"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.5"
      viewBox="0 0 24 24"
      className={`chevron${props.expanded ? " expanded" : ""}`}
    >
      <polyline points="9 18 15 12 9 6" />
    </svg>
  );
}

function FolderIcon() {
  return (
    <svg
      width="16"
      height="16"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.5"
      viewBox="0 0 24 24"
    >
      <path d="M22 19a2 2 0 01-2 2H4a2 2 0 01-2-2V5a2 2 0 012-2h5l2 3h9a2 2 0 012 2z" />
    </svg>
  );
}

function ChatIcon() {
  return (
    <svg
      width="16"
      height="16"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.5"
      viewBox="0 0 24 24"
    >
      <path d="M21 15a2 2 0 01-2 2H7l-4 4V5a2 2 0 012-2h14a2 2 0 012 2z" />
    </svg>
  );
}

function ClockIcon() {
  return (
    <svg
      width="16"
      height="16"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.5"
      viewBox="0 0 24 24"
    >
      <circle cx="12" cy="12" r="10" />
      <polyline points="12 6 12 12 16 14" />
    </svg>
  );
}

function formatSessionLabel(raw: string, fallback = "Untitled"): string {
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

  // No JS truncation: .tree-node-label ellipsizes via CSS, which adapts to
  // the actual panel width instead of a hardcoded character count.
  return label || fallback;
}

/** Distinct providers among a directory group's sessions, in child order. */
function directoryProviders(
  node: TreeNode,
): NonNullable<TreeNode["provider"]>[] {
  const seen = new Set<NonNullable<TreeNode["provider"]>>();
  for (const child of node.children) {
    if (child.node_type === "session" && child.provider) {
      seen.add(child.provider);
    }
  }
  return [...seen];
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
    e: React.MouseEvent,
    node: TreeNode,
    parentProjectLabel: string,
  ) => void;
  onNodeContextMenu: (e: React.MouseEvent, node: TreeNode) => void;
  onSessionClick: (
    e: React.MouseEvent,
    node: TreeNode,
    parentProjectLabel: string,
  ) => void;
  onSessionDblClick?: (
    e: React.MouseEvent,
    node: TreeNode,
    parentProjectLabel: string,
  ) => void;
  /** Directory grouping merges providers, so each session row identifies its
   * provider with a colored dot instead of the generic chat icon. */
  sessionProviderDot?: boolean;
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

  const handleClick = (e: React.MouseEvent) => {
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

  const handleDblClick = (e: React.MouseEvent) => {
    if (isSession() && props.onSessionDblClick) {
      e.preventDefault();
      props.onSessionDblClick(e, props.node, props.parentProjectLabel ?? "");
    }
  };

  const handleContextMenu = (e: React.MouseEvent) => {
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
    <div className="tree-node-wrapper">
      <Button
        variant="ghost"
        className={`tree-node justify-start rounded-none active:translate-y-0 tree-node-${props.node.node_type}${isSession() && props.activeSessionId === props.node.id ? " active" : ""}${nodeSelected() ? " selected" : ""}`}
        style={{ paddingLeft: `${props.depth * 16 + 8}px` }}
        onClick={handleClick}
        onDoubleClick={handleDblClick}
        onContextMenu={handleContextMenu}
        data-session-id={isSession() ? props.node.id : undefined}
      >
        {!isLeaf() && !isSubagentParent() && (
          <ChevronIcon expanded={expanded()} />
        )}
        {(isLeaf() || isSubagentParent()) && (
          <span className="tree-node-icon-spacer" />
        )}

        {props.node.node_type === "provider" && props.node.provider && (
          <ProviderDot provider={props.node.provider} />
        )}
        {props.node.node_type === "project" &&
          props.node.project_path &&
          !isOrphanFolder() && (
            <span className="tree-node-icon">
              <FolderIcon />
            </span>
          )}
        {props.sessionProviderDot &&
          props.node.node_type === "project" &&
          directoryProviders(props.node).length > 0 && (
            <span className="tree-provider-cluster">
              {directoryProviders(props.node).map((provider) => (
                <i
                  key={provider}
                  className="tree-provider-cluster-dot"
                  style={{ background: getProviderColor(provider) }}
                />
              ))}
            </span>
          )}
        {isOrphanFolder() && (
          <span className="tree-node-icon tree-node-icon-orphan-folder">⤷</span>
        )}
        {props.node.node_type === "project" && !props.node.project_path && (
          <span className="tree-node-icon tree-node-icon-time">
            <ClockIcon />
          </span>
        )}
        {props.node.node_type === "session" &&
          isSession() &&
          props.node.is_sidechain &&
          !isSubagentParent() && (
            <span className="tree-node-icon tree-node-icon-orphan">⤷</span>
          )}
        {props.node.node_type === "session" &&
          !(props.node.is_sidechain && !isSubagentParent()) &&
          (props.sessionProviderDot && props.node.provider ? (
            <ProviderDot provider={props.node.provider} />
          ) : (
            <span className="tree-node-icon">
              <ChatIcon />
            </span>
          ))}

        <span
          className={`tree-node-label${props.node.node_type === "provider" ? " bold" : ""}`}
          title={
            props.node.node_type === "session" ? props.node.label : undefined
          }
        >
          {props.node.node_type === "session"
            ? formatSessionLabel(props.node.label, t("common.untitled"))
            : displayLabel()}
        </span>

        {props.node.is_sidechain && (
          <span
            className="tree-node-sidechain"
            title={t("common.subagentSession")}
          >
            ⤷
          </span>
        )}
        {isSession() && props.node.updated_at !== undefined && (
          <span className="tree-node-time">
            {formatTreeTime(props.node.updated_at)}
          </span>
        )}
        {props.node.count > 0 && !isLeaf() && (
          <span className="tree-node-count">{props.node.count}</span>
        )}
      </Button>

      {/* Subagent children always visible under parent session */}
      {isSubagentParent() &&
        props.node.children.map((child) => (
          <TreeNodeComponent
            key={child.id}
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
            sessionProviderDot={props.sessionProviderDot}
          />
        ))}
      {/* Provider/project children use expand/collapse */}
      {expanded() &&
        !isLeaf() &&
        !isSubagentParent() &&
        props.node.children.map((child) => (
          <TreeNodeComponent
            key={child.id}
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
            sessionProviderDot={props.sessionProviderDot}
          />
        ))}
    </div>
  );
}
