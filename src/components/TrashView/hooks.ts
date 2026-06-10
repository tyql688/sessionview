import { createMemo, createResource, createSignal, onMount } from "solid-js";
import type { TrashMeta, TreeNode } from "../../lib/types";
import {
  listTrash,
  restoreSession,
  restoreSessionsBatch,
  emptyTrash,
  permanentDeleteTrash,
  permanentDeleteTrashBatch,
} from "../../lib/tauri";
import { collectSessionIds } from "../../lib/tree-utils";
import { buildTrashTree } from "../../lib/tree-builders";
import { useI18n } from "../../i18n/index";
import { toast, toastError } from "../../stores/toast";
import { errorMessage } from "../../lib/errors";

// --- Trash list, tree building, and trash actions ------------------------------

export function createTrashState(onRefreshTree: () => void) {
  const { t } = useI18n();
  const [showEmptyConfirm, setShowEmptyConfirm] = createSignal(false);
  const [showRestoreConfirm, setShowRestoreConfirm] = createSignal(false);
  const [restoreTarget, setRestoreTarget] = createSignal<TreeNode | null>(null);
  const [showDeleteAllConfirm, setShowDeleteAllConfirm] = createSignal(false);
  const [deleteAllTarget, setDeleteAllTarget] = createSignal<TreeNode | null>(
    null,
  );
  const [expandedIds, setExpandedIds] = createSignal<Set<string>>(new Set());

  const [trashItems, { refetch }] = createResource<TrashMeta[]>(() =>
    listTrash(),
  );

  const trashError = createMemo(() =>
    trashItems.error ? errorMessage(trashItems.error) : null,
  );

  onMount(() => refetch());

  const tree = createMemo(() => {
    const items = trashItems() || [];
    const trashTree = buildTrashTree(items, {
      unknown: t("common.unknown"),
      untitled: t("common.untitled"),
    });
    const ids = new Set<string>();
    const collectIds = (nodes: TreeNode[]) => {
      for (const node of nodes) {
        if (node.node_type !== "session") {
          ids.add(node.id);
          collectIds(node.children);
        }
      }
    };
    collectIds(trashTree);
    setExpandedIds(ids);
    return trashTree;
  });

  const itemMap = createMemo(() => {
    const map = new Map<string, TrashMeta>();
    for (const item of trashItems() || []) {
      map.set(item.id, item);
    }
    return map;
  });

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
      refetch();
    } catch (e) {
      toastError(errorMessage(e));
    }
  }

  async function handleEmptyTrash() {
    try {
      await emptyTrash();
      setShowEmptyConfirm(false);
      refetch();
    } catch (e) {
      toastError(errorMessage(e));
      setShowEmptyConfirm(false);
    }
  }

  async function handleRestoreAll(node: TreeNode) {
    const ids = collectSessionIds(node);
    const result = await restoreSessionsBatch(ids);
    await Promise.all([refetch(), onRefreshTree()]);
    if (result.failed > 0)
      toastError(
        `${result.failed}/${result.succeeded + result.failed} ${t("trash.restore")}`,
      );
    else toast(`${result.succeeded} ${t("trash.restoreOk")}`);
  }

  async function handleDeleteAll(node: TreeNode) {
    const ids = collectSessionIds(node);
    await permanentDeleteTrashBatch(ids);
    refetch();
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
