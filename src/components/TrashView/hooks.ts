import { useCallback, useEffect, useMemo, useState } from "react";
import type { Dispatch, SetStateAction } from "react";
import type { TrashMeta, TreeNode } from "@/lib/types";
import {
  listTrash,
  restoreSession,
  restoreSessionsBatch,
  emptyTrash,
  permanentDeleteTrash,
  permanentDeleteTrashBatch,
} from "@/lib/tauri";
import { collectSessionIds } from "@/lib/tree-utils";
import { buildTrashTree } from "@/lib/tree-builders";
import { useI18n } from "@/i18n/index";
import { toast, toastError } from "@/stores/toast";
import { errorMessage } from "@/lib/errors";

// --- Trash list, tree building, and trash actions ------------------------------

export interface UseTrashStateResult {
  trashItems: TrashMeta[] | undefined;
  trashLoading: boolean;
  trashError: string | null;
  tree: TreeNode[];
  itemMap: Map<string, TrashMeta>;
  expandedIds: Set<string>;
  toggleExpanded: (nodeId: string) => void;
  showEmptyConfirm: boolean;
  setShowEmptyConfirm: Dispatch<SetStateAction<boolean>>;
  showRestoreConfirm: boolean;
  setShowRestoreConfirm: Dispatch<SetStateAction<boolean>>;
  restoreTarget: TreeNode | null;
  setRestoreTarget: Dispatch<SetStateAction<TreeNode | null>>;
  showDeleteAllConfirm: boolean;
  setShowDeleteAllConfirm: Dispatch<SetStateAction<boolean>>;
  deleteAllTarget: TreeNode | null;
  setDeleteAllTarget: Dispatch<SetStateAction<TreeNode | null>>;
  handleRestore: (id: string) => Promise<void>;
  handlePermanentDelete: (id: string) => Promise<void>;
  handleEmptyTrash: () => Promise<void>;
  handleRestoreAll: (node: TreeNode) => Promise<void>;
  handleDeleteAll: (node: TreeNode) => Promise<void>;
}

// Now a React hook: call it at the top level of a component.
export function useTrashState(onRefreshTree: () => void): UseTrashStateResult {
  const { t } = useI18n();
  const [showEmptyConfirm, setShowEmptyConfirm] = useState(false);
  const [showRestoreConfirm, setShowRestoreConfirm] = useState(false);
  const [restoreTarget, setRestoreTarget] = useState<TreeNode | null>(null);
  const [showDeleteAllConfirm, setShowDeleteAllConfirm] = useState(false);
  const [deleteAllTarget, setDeleteAllTarget] = useState<TreeNode | null>(null);
  const [expandedIds, setExpandedIds] = useState<Set<string>>(new Set());

  const [trashItems, setTrashItems] = useState<TrashMeta[] | undefined>(
    undefined,
  );
  const [trashLoading, setTrashLoading] = useState(true);
  const [resourceError, setResourceError] = useState<unknown>(null);

  const refetch = useCallback(async () => {
    setTrashLoading(true);
    try {
      const data = await listTrash();
      setTrashItems(data);
      setResourceError(null);
    } catch (e) {
      setResourceError(e);
    } finally {
      setTrashLoading(false);
    }
  }, []);

  const trashError = useMemo(
    () => (resourceError ? errorMessage(resourceError) : null),
    [resourceError],
  );

  useEffect(() => {
    void refetch();
  }, [refetch]);

  const unknownLabel = t("common.unknown");
  const untitledLabel = t("common.untitled");

  const tree = useMemo(
    () =>
      buildTrashTree(trashItems ?? [], {
        unknown: unknownLabel,
        untitled: untitledLabel,
      }),
    [trashItems, unknownLabel, untitledLabel],
  );

  // Auto-expand every non-session node whenever the tree is rebuilt (mirrors
  // the side effect the Solid `tree` memo performed inline on each recompute).
  useEffect(() => {
    const ids = new Set<string>();
    const collectIds = (nodes: TreeNode[]) => {
      for (const node of nodes) {
        if (node.node_type !== "session") {
          ids.add(node.id);
          collectIds(node.children);
        }
      }
    };
    collectIds(tree);
    setExpandedIds(ids);
  }, [tree]);

  const itemMap = useMemo(() => {
    const map = new Map<string, TrashMeta>();
    for (const item of trashItems ?? []) {
      map.set(item.id, item);
    }
    return map;
  }, [trashItems]);

  async function handleRestore(id: string) {
    try {
      await restoreSession(id);
      await Promise.all([refetch(), onRefreshTree()]);
      toast(t("trash.restoreOk"));
    } catch (e) {
      await refetch();
      toastError(`${t("trash.restore")}: ${errorMessage(e)}`);
    }
  }

  async function handlePermanentDelete(id: string) {
    try {
      await permanentDeleteTrash(id);
      void refetch();
    } catch (e) {
      toastError(errorMessage(e));
    }
  }

  async function handleEmptyTrash() {
    try {
      await emptyTrash();
      setShowEmptyConfirm(false);
      void refetch();
    } catch (e) {
      toastError(errorMessage(e));
      setShowEmptyConfirm(false);
    }
  }

  async function handleRestoreAll(node: TreeNode) {
    const ids = collectSessionIds(node);
    try {
      const result = await restoreSessionsBatch(ids);
      await Promise.all([refetch(), onRefreshTree()]);
      if (result.failed > 0)
        toastError(
          `${result.failed}/${result.succeeded + result.failed} ${t("trash.restore")}`,
        );
      else toast(`${result.succeeded} ${t("trash.restoreOk")}`);
    } catch (e) {
      toastError(`${t("trash.restore")}: ${errorMessage(e)}`);
      // The batch may have partially applied before failing — resync the list.
      void refetch();
    }
  }

  async function handleDeleteAll(node: TreeNode) {
    const ids = collectSessionIds(node);
    try {
      await permanentDeleteTrashBatch(ids);
    } catch (e) {
      toastError(errorMessage(e));
    } finally {
      // Deletes may have partially applied before a failure — resync either way.
      void refetch();
    }
  }

  function toggleExpanded(nodeId: string) {
    setExpandedIds((prev) => {
      const next = new Set(prev);
      if (next.has(nodeId)) next.delete(nodeId);
      else next.add(nodeId);
      return next;
    });
  }

  return {
    trashItems,
    trashLoading,
    trashError,
    tree,
    itemMap,
    expandedIds,
    toggleExpanded,
    showEmptyConfirm,
    setShowEmptyConfirm,
    showRestoreConfirm,
    setShowRestoreConfirm,
    restoreTarget,
    setRestoreTarget,
    showDeleteAllConfirm,
    setShowDeleteAllConfirm,
    deleteAllTarget,
    setDeleteAllTarget,
    handleRestore,
    handlePermanentDelete,
    handleEmptyTrash,
    handleRestoreAll,
    handleDeleteAll,
  };
}
