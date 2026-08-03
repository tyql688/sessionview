import type { MenuItemDef } from "@/components/ContextMenu";
import type { Provider, TreeNode } from "@/lib/types";
import { openInFolder } from "@/lib/tauri";
import { errorMessage } from "@/lib/errors";
import { toast, toastError } from "@/stores/toast";
import { selectionCount } from "@/features/explorer/selection";
import { bumpFavoriteVersion } from "@/features/favorites/favorites";

export interface SessionMenuContext {
  node: TreeNode;
  sessionProjectPath: string;
  resumeCommand: string | null;
  favorite: boolean | null;
  t: (key: string) => string;
  terminalApp: string;
  resumeSession: (id: string, terminal: string) => Promise<void>;
  toggleFavorite: (id: string) => Promise<boolean>;
  setRenameTarget: (target: { id: string; label: string }) => void;
}

export function buildSessionMenuItems(ctx: SessionMenuContext): MenuItemDef[] {
  const { node, sessionProjectPath, t } = ctx;
  return [
    {
      label: t("contextMenu.copySessionId"),
      onClick: () => {
        void navigator.clipboard.writeText(node.id).then(() => toast(t("toast.idCopied")));
      },
    },
    {
      label: t("contextMenu.copyResumeCommand"),
      onClick: async () => {
        try {
          if (!ctx.resumeCommand) throw new Error("resume command unavailable");
          await navigator.clipboard.writeText(ctx.resumeCommand);
          toast(t("toast.cmdCopied"));
        } catch (_e) {
          toastError(t("toast.copyFailed"));
        }
      },
    },
    ...(sessionProjectPath
      ? [
          {
            label: t("contextMenu.openInFinder"),
            onClick: async () => {
              try {
                await openInFolder(sessionProjectPath);
              } catch (error) {
                toastError(errorMessage(error));
              }
            },
          },
          {
            label: t("contextMenu.copyPath"),
            onClick: () => {
              void navigator.clipboard.writeText(sessionProjectPath).then(() => toast(t("toast.copied")));
            },
          },
        ]
      : []),
    { label: "", separator: true, onClick: () => {} },
    {
      label: t("contextMenu.resumeSession"),
      onClick: async () => {
        await ctx.resumeSession(node.id, ctx.terminalApp);
      },
    },
    { label: "", separator: true, onClick: () => {} },
    {
      label:
        ctx.favorite === null
          ? t("contextMenu.toggleFavorite")
          : t(ctx.favorite ? "session.favoriteRemove" : "session.favoriteAdd"),
      onClick: async () => {
        try {
          const newState = await ctx.toggleFavorite(node.id);
          bumpFavoriteVersion();
          toast(t(newState ? "toast.favoriteAdded" : "toast.favoriteRemoved"));
        } catch (_e) {
          toastError(t("toast.favoriteFailed"));
        }
      },
    },
    {
      label: t("contextMenu.rename"),
      onClick: () => {
        ctx.setRenameTarget({ id: node.id, label: node.label });
      },
    },
  ];
}

export interface SelectionMenuContext {
  t: (key: string) => string;
  exportSelectedBatch: () => void;
}

export function buildSelectionMenuItems(ctx: SelectionMenuContext): MenuItemDef[] {
  return [
    {
      label: () => `${ctx.t("contextMenu.exportSelected")} (${selectionCount()})`,
      onClick: ctx.exportSelectedBatch,
    },
  ];
}

export interface NodeMenuContext {
  node: TreeNode;
  t: (key: string) => string;
  collapseAllChildren: (node: TreeNode) => void;
  expandAllChildren: (node: TreeNode) => void;
  collapseNode: (nodeId: string) => void;
  onRefreshTree?: () => void;
  onRefreshProvider?: (provider: Provider) => void;
  addBlockedFolder: (path: string) => void;
}

export function buildNodeMenuItems(ctx: NodeMenuContext): MenuItemDef[] {
  const { node, t } = ctx;
  if (node.node_type === "provider") {
    return [
      {
        label: t("contextMenu.collapseAll"),
        onClick: () => ctx.collapseAllChildren(node),
      },
      {
        label: t("contextMenu.refresh"),
        onClick: () => {
          if (node.provider) ctx.onRefreshProvider?.(node.provider);
          else ctx.onRefreshTree?.();
        },
      },
    ];
  }
  const projectPath = node.project_path ?? "";
  const hasPath = projectPath.length > 0;
  return [
    ...(hasPath
      ? [
          {
            label: t("contextMenu.openInFinder"),
            onClick: async () => {
              try {
                await openInFolder(projectPath);
              } catch (error) {
                toastError(errorMessage(error));
              }
            },
          },
          {
            label: t("contextMenu.copyPath"),
            onClick: () => {
              void navigator.clipboard.writeText(projectPath).then(() => toast(t("toast.copied")));
            },
          },
          { label: "", separator: true, onClick: () => {} },
        ]
      : []),
    {
      label: t("contextMenu.expandAll"),
      onClick: () => ctx.expandAllChildren(node),
    },
    {
      label: t("contextMenu.collapseAll"),
      onClick: () => ctx.collapseNode(node.id),
    },
    ...(hasPath
      ? [
          {
            label: t("contextMenu.blockFolder"),
            onClick: () => {
              ctx.addBlockedFolder(projectPath);
              ctx.onRefreshTree?.();
            },
          },
        ]
      : []),
  ];
}
