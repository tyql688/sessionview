import type React from "react";
import { ChevronRight, Clock3, CornerDownRight, Folder, MessageSquare } from "lucide-react";
import { Button } from "@/components/ui/button";
import { formatTreeTime } from "@/lib/formatters";
import type { TreeNode } from "@/lib/types";
import { useI18n } from "@/i18n/index";
import { isSelected, toggleSelected } from "@/features/explorer/selection";
import { getProviderColor } from "@/stores/providerSnapshots";
import { ProviderDot } from "@/components/icons";
import { useLongPress } from "@/lib/useLongPress";
import { collectSessionNodes } from "@/lib/tree-utils";

/** The coordinates a context menu anchors to — satisfied by a real mouse
 * event or by a synthesized long-press position. */
export interface MenuAnchorEvent {
  clientX: number;
  clientY: number;
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
    if (segments.length > 0) label = segments[segments.length - 1];
  }

  return label || fallback;
}

/** Distinct providers among a directory group's sessions, in child order. */
function directoryProviders(node: TreeNode): NonNullable<TreeNode["provider"]>[] {
  const seen = new Set<NonNullable<TreeNode["provider"]>>();
  for (const child of node.children) {
    if (child.node_type === "session" && child.provider) seen.add(child.provider);
  }
  return [...seen];
}

interface TreeNodeComponentProps {
  node: TreeNode;
  depth: number;
  activeSessionId: string | null;
  focusedNodeId: string | null;
  onNodeFocus: (nodeId: string) => void;
  parentNodeId?: string;
  parentProjectLabel?: string;
  isNodeExpanded: (nodeId: string) => boolean;
  toggleExpanded: (nodeId: string) => void;
  onSessionContextMenu: (event: MenuAnchorEvent, node: TreeNode, parentProjectLabel: string) => void;
  onNodeContextMenu: (event: MenuAnchorEvent, node: TreeNode) => void;
  onSessionClick: (event: React.MouseEvent, node: TreeNode, parentProjectLabel: string) => void;
  onSessionDblClick?: (event: React.MouseEvent, node: TreeNode, parentProjectLabel: string) => void;
  /** Directory grouping merges providers, so each session row identifies its
   * provider with a colored dot instead of the generic chat icon. */
  sessionProviderDot?: boolean;
  /** Directory grouping is visually denser because the root already carries
   * the working directory identity. */
  directoryGrouping?: boolean;
}

export function TreeNodeComponent(props: TreeNodeComponentProps) {
  const { t } = useI18n();
  const hasChildren = () => props.node.children.length > 0;
  const isSession = () => props.node.node_type === "session";
  const isSubagentParent = () => isSession() && hasChildren();
  const isOrphanFolder = () => {
    if (props.node.node_type !== "project" || !props.node.project_path) return false;
    const sessions = collectSessionNodes(props.node);
    return sessions.length > 0 && sessions.every((session) => session.is_sidechain);
  };
  const isLeaf = () => props.node.node_type === "session" && !hasChildren();
  const expanded = () => props.isNodeExpanded(props.node.id);

  const handleClick = (event: React.MouseEvent) => {
    if (isSubagentParent() && (event.target as Element).closest("[data-subagent-chevron]")) {
      props.toggleExpanded(props.node.id);
      return;
    }
    if (isSession()) {
      props.onSessionClick(event, props.node, props.parentProjectLabel ?? "");
    } else if (event.metaKey || event.ctrlKey) {
      for (const session of collectSessionNodes(props.node)) toggleSelected(session.id);
    } else {
      props.toggleExpanded(props.node.id);
    }
  };

  const handleDblClick = (event: React.MouseEvent) => {
    if (isSession() && props.onSessionDblClick) {
      event.preventDefault();
      props.onSessionDblClick(event, props.node, props.parentProjectLabel ?? "");
    }
  };

  const openMenuAt = (anchor: MenuAnchorEvent) => {
    if (isSession()) {
      props.onSessionContextMenu(anchor, props.node, props.parentProjectLabel ?? "");
    } else {
      props.onNodeContextMenu(anchor, props.node);
    }
  };

  const handleContextMenu = (event: React.MouseEvent) => {
    event.preventDefault();
    event.stopPropagation();
    openMenuAt(event);
  };

  const handleKeyDown = (event: React.KeyboardEvent<HTMLButtonElement>) => {
    if (event.key === "ContextMenu" || (event.shiftKey && event.key === "F10")) {
      event.preventDefault();
      const rect = event.currentTarget.getBoundingClientRect();
      openMenuAt({ clientX: rect.left + 16, clientY: rect.top + rect.height });
      return;
    }

    const tree = event.currentTarget.closest<HTMLElement>('[role="tree"]');
    if (!tree) return;
    const items = [...tree.querySelectorAll<HTMLButtonElement>('[role="treeitem"]')];
    const index = items.indexOf(event.currentTarget);
    let target: HTMLButtonElement | null = null;

    switch (event.key) {
      case "ArrowDown":
        target = items[Math.min(index + 1, items.length - 1)] ?? null;
        break;
      case "ArrowUp":
        target = items[Math.max(index - 1, 0)] ?? null;
        break;
      case "Home":
        target = items[0] ?? null;
        break;
      case "End":
        target = items.at(-1) ?? null;
        break;
      case "ArrowRight":
        if (!isLeaf() && !expanded()) {
          props.toggleExpanded(props.node.id);
        } else if (!isLeaf()) {
          target = items[index + 1] ?? null;
        }
        break;
      case "ArrowLeft":
        if (!isLeaf() && expanded()) {
          props.toggleExpanded(props.node.id);
        } else if (props.parentNodeId) {
          target = tree.querySelector<HTMLButtonElement>(`[data-tree-node-id="${CSS.escape(props.parentNodeId)}"]`);
        }
        break;
      default:
        return;
    }

    event.preventDefault();
    target?.focus();
  };

  const longPress = useLongPress((pos) => openMenuAt({ clientX: pos.x, clientY: pos.y }));
  const projectLabel = () =>
    props.node.node_type === "project"
      ? props.node.label === "(No Project)"
        ? t("explorer.noProject")
        : props.node.label
      : props.parentProjectLabel;
  const displayLabel = () =>
    props.node.node_type === "project" && props.node.label === "(No Project)"
      ? t("explorer.noProject")
      : props.node.label;
  const nodeSelected = () => isSession() && isSelected(props.node.id);
  const indentLeft = () => props.depth * (props.directoryGrouping ? 14 : 16) + (props.directoryGrouping ? 4 : 8);
  const providers = props.sessionProviderDot ? directoryProviders(props.node) : [];

  return (
    <div className="tree-node-wrapper">
      <Button
        variant="ghost"
        role="treeitem"
        tabIndex={props.focusedNodeId === props.node.id ? 0 : -1}
        aria-level={props.depth + 1}
        aria-expanded={isLeaf() ? undefined : expanded()}
        aria-selected={isSession() ? nodeSelected() || props.activeSessionId === props.node.id : undefined}
        className={`tree-node justify-start rounded-none active:translate-y-0 tree-node-${props.node.node_type}${props.directoryGrouping ? " tree-node-directory" : ""}${isSession() && props.activeSessionId === props.node.id ? " active" : ""}${nodeSelected() ? " selected" : ""}`}
        style={{ paddingLeft: `${indentLeft()}px` }}
        onFocus={() => props.onNodeFocus(props.node.id)}
        onKeyDown={handleKeyDown}
        onClick={handleClick}
        onDoubleClick={handleDblClick}
        onContextMenu={handleContextMenu}
        onPointerDown={longPress.onPointerDown}
        onPointerMove={longPress.onPointerMove}
        onPointerUp={longPress.onPointerUp}
        onPointerCancel={longPress.onPointerCancel}
        onClickCapture={longPress.onClickCapture}
        data-tree-node-id={props.node.id}
        data-session-id={isSession() ? props.node.id : undefined}
      >
        {!isLeaf() && isSubagentParent() ? (
          <span data-subagent-chevron>
            <ChevronRight className={`chevron${expanded() ? " expanded" : ""}`} size={16} aria-hidden="true" />
          </span>
        ) : !isLeaf() ? (
          <ChevronRight className={`chevron${expanded() ? " expanded" : ""}`} size={16} aria-hidden="true" />
        ) : (
          <span className="tree-node-icon-spacer" />
        )}

        {props.node.node_type === "provider" && props.node.provider && <ProviderDot provider={props.node.provider} />}
        {props.node.node_type === "project" && props.node.project_path && !isOrphanFolder() && (
          <span className="tree-node-icon">
            <Folder size={16} strokeWidth={1.5} aria-hidden="true" />
          </span>
        )}
        {props.sessionProviderDot && props.node.node_type === "project" && providers.length > 0 && (
          <span className="tree-provider-cluster" aria-hidden="true">
            {providers.map((provider) => (
              <i
                key={provider}
                className="tree-provider-cluster-dot"
                style={{ background: getProviderColor(provider) }}
              />
            ))}
          </span>
        )}
        {isOrphanFolder() && (
          <span className="tree-node-icon tree-node-icon-orphan-folder">
            <CornerDownRight size={16} strokeWidth={1.7} aria-hidden="true" />
          </span>
        )}
        {props.node.node_type === "project" && !props.node.project_path && (
          <span className="tree-node-icon tree-node-icon-time">
            <Clock3 size={16} strokeWidth={1.5} aria-hidden="true" />
          </span>
        )}
        {props.node.node_type === "session" && props.node.is_sidechain && !isSubagentParent() && (
          <span className="tree-node-icon tree-node-icon-orphan">
            <CornerDownRight size={16} strokeWidth={1.7} aria-hidden="true" />
          </span>
        )}
        {props.node.node_type === "session" &&
          !(props.node.is_sidechain && !isSubagentParent()) &&
          (props.sessionProviderDot && props.node.provider ? (
            <ProviderDot provider={props.node.provider} />
          ) : (
            <span className="tree-node-icon">
              <MessageSquare size={16} strokeWidth={1.5} aria-hidden="true" />
            </span>
          ))}

        <span
          className={`tree-node-label${props.node.node_type === "provider" ? " bold" : ""}`}
          title={props.node.node_type === "session" ? props.node.label : undefined}
        >
          {props.node.node_type === "session"
            ? formatSessionLabel(props.node.label, t("common.untitled"))
            : displayLabel()}
        </span>

        {props.node.is_sidechain && (
          <span className="tree-node-sidechain" title={t("common.subagentSession")}>
            <CornerDownRight size={16} strokeWidth={1.7} aria-hidden="true" />
          </span>
        )}
        {isSession() && props.node.updated_at !== undefined && (
          <span className="tree-node-time">{formatTreeTime(props.node.updated_at)}</span>
        )}
        {props.node.count > 0 && !isLeaf() && <span className="tree-node-count">{props.node.count}</span>}
      </Button>

      {expanded() && !isLeaf() && (
        <fieldset className="tree-node-group">
          {props.node.children.map((child) => (
            <TreeNodeComponent
              key={child.id}
              node={child}
              depth={props.depth + 1}
              activeSessionId={props.activeSessionId}
              focusedNodeId={props.focusedNodeId}
              onNodeFocus={props.onNodeFocus}
              parentNodeId={props.node.id}
              parentProjectLabel={projectLabel()}
              isNodeExpanded={props.isNodeExpanded}
              toggleExpanded={props.toggleExpanded}
              onSessionContextMenu={props.onSessionContextMenu}
              onNodeContextMenu={props.onNodeContextMenu}
              onSessionClick={props.onSessionClick}
              onSessionDblClick={props.onSessionDblClick}
              sessionProviderDot={props.sessionProviderDot}
              directoryGrouping={props.directoryGrouping}
            />
          ))}
        </fieldset>
      )}
    </div>
  );
}
