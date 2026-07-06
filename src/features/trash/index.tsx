import type { TreeNode } from "@/lib/types";
import { ProviderDot } from "@/components/icons";
import { useI18n } from "@/i18n/index";
import { formatAbsoluteTime } from "@/lib/formatters";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { useTrashState } from "@/features/trash/hooks";

export function TrashView(props: { onRefreshTree: () => void }) {
  const { t } = useI18n();
  const {
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
  } = useTrashState(props.onRefreshTree);

  function TrashTreeNode(nodeProps: { node: TreeNode; depth: number }) {
    const { node, depth } = nodeProps;
    const isLeaf = node.node_type === "session";
    const isGroup = node.node_type === "project";
    const expanded = expandedIds.has(node.id);
    const trashItem = itemMap.get(node.id);

    return (
      <div>
        <div
          className={`trash-tree-node${isLeaf ? " trash-tree-leaf" : " trash-tree-group"}`}
          style={{ paddingLeft: `${depth * 16 + 12}px` }}
          onClick={() => !isLeaf && toggleExpanded(node.id)}
        >
          {!isLeaf && (
            <svg
              width="14"
              height="14"
              fill="none"
              stroke="currentColor"
              strokeWidth="1.5"
              viewBox="0 0 24 24"
              className={`chevron${expanded ? " expanded" : ""}`}
            >
              <polyline points="9 18 15 12 9 6" />
            </svg>
          )}
          {isLeaf && <span className="trash-tree-spacer" />}

          {node.node_type === "provider" && node.provider && (
            <ProviderDot provider={node.provider} />
          )}
          {isGroup && (
            <span className="trash-tree-icon">
              <svg
                width="14"
                height="14"
                fill="none"
                stroke="currentColor"
                strokeWidth="1.5"
                viewBox="0 0 24 24"
              >
                <path d="M22 19a2 2 0 01-2 2H4a2 2 0 01-2-2V5a2 2 0 012-2h5l2 3h9a2 2 0 012 2z" />
              </svg>
            </span>
          )}
          {isLeaf && (
            <span className="trash-tree-icon trash-tree-icon-session">
              <svg
                width="14"
                height="14"
                fill="none"
                stroke="currentColor"
                strokeWidth="1.5"
                viewBox="0 0 24 24"
              >
                <path d="M21 15a2 2 0 01-2 2H7l-4 4V5a2 2 0 012-2h14a2 2 0 012 2z" />
              </svg>
            </span>
          )}

          <span
            className={`trash-tree-label${node.node_type === "provider" ? " bold" : ""}`}
            title={isLeaf ? node.label : undefined}
          >
            {isLeaf
              ? node.label.length > 50
                ? `${node.label.slice(0, 47)}...`
                : node.label
              : node.label}
          </span>

          {!isLeaf && node.count > 0 && (
            <>
              <span className="tree-node-count">{node.count}</span>
              <div className="trash-tree-actions">
                <button
                  className="trash-action-btn trash-action-btn-restore"
                  onClick={(e) => {
                    e.stopPropagation();
                    setRestoreTarget(node);
                    setShowRestoreConfirm(true);
                  }}
                  title={t("trash.restore")}
                >
                  <svg
                    width="12"
                    height="12"
                    fill="none"
                    stroke="currentColor"
                    strokeWidth="2"
                    viewBox="0 0 24 24"
                  >
                    <polyline points="1 4 1 10 7 10" />
                    <path d="M3.51 15a9 9 0 1 0 2.13-9.36L1 10" />
                  </svg>
                </button>
                <button
                  className="trash-action-btn trash-action-btn-danger"
                  onClick={(e) => {
                    e.stopPropagation();
                    setDeleteAllTarget(node);
                    setShowDeleteAllConfirm(true);
                  }}
                  title={t("trash.permanentDelete")}
                >
                  <svg
                    width="12"
                    height="12"
                    fill="none"
                    stroke="currentColor"
                    strokeWidth="2"
                    viewBox="0 0 24 24"
                  >
                    <line x1="18" y1="6" x2="6" y2="18" />
                    <line x1="6" y1="6" x2="18" y2="18" />
                  </svg>
                </button>
              </div>
            </>
          )}

          {isLeaf && trashItem && (
            <>
              <span className="trash-tree-date">
                {formatAbsoluteTime(trashItem.trashed_at)}
              </span>
              <div className="trash-tree-actions">
                <button
                  className="trash-action-btn trash-action-btn-restore"
                  onClick={(e) => {
                    e.stopPropagation();
                    handleRestore(node.id);
                  }}
                  title={t("trash.restore")}
                >
                  <svg
                    width="12"
                    height="12"
                    fill="none"
                    stroke="currentColor"
                    strokeWidth="2"
                    viewBox="0 0 24 24"
                  >
                    <polyline points="1 4 1 10 7 10" />
                    <path d="M3.51 15a9 9 0 1 0 2.13-9.36L1 10" />
                  </svg>
                </button>
                <button
                  className="trash-action-btn trash-action-btn-danger"
                  onClick={(e) => {
                    e.stopPropagation();
                    handlePermanentDelete(node.id);
                  }}
                  title={t("trash.permanentDelete")}
                >
                  <svg
                    width="12"
                    height="12"
                    fill="none"
                    stroke="currentColor"
                    strokeWidth="2"
                    viewBox="0 0 24 24"
                  >
                    <line x1="18" y1="6" x2="6" y2="18" />
                    <line x1="6" y1="6" x2="18" y2="18" />
                  </svg>
                </button>
              </div>
            </>
          )}
        </div>

        {expanded &&
          !isLeaf &&
          node.children.map((child) => (
            <TrashTreeNode key={child.id} node={child} depth={depth + 1} />
          ))}
      </div>
    );
  }

  return (
    <div className="trash-view">
      <div className="trash-header">
        <span className="trash-title">
          {t("trash.title")}
          {trashItems && trashItems.length > 0 && (
            <span className="trash-count"> ({trashItems.length})</span>
          )}
        </span>
        {trashItems && trashItems.length > 0 && (
          <button
            className="trash-empty-btn"
            onClick={() => setShowEmptyConfirm(true)}
          >
            {t("trash.emptyTrash")}
          </button>
        )}
      </div>

      <div className="trash-list">
        {!trashLoading &&
          !trashError &&
          trashItems &&
          trashItems.length === 0 && (
            <div className="trash-empty-state">
              <svg
                className="icon-faded"
                width="32"
                height="32"
                fill="none"
                stroke="currentColor"
                strokeWidth="1"
                viewBox="0 0 24 24"
              >
                <polyline points="3 6 5 6 21 6" />
                <path d="M19 6v14a2 2 0 01-2 2H7a2 2 0 01-2-2V6m3 0V4a2 2 0 012-2h4a2 2 0 012 2v2" />
              </svg>
              <span>{t("trash.empty")}</span>
            </div>
          )}

        {trashLoading && (
          <div className="trash-empty-state">
            <div className="spinner spinner-sm" />
          </div>
        )}

        {!trashLoading && trashError && (
          <div className="empty-state">
            <p className="empty-state-text">{t("error.title")}</p>
            <p className="empty-state-hint">{trashError}</p>
          </div>
        )}

        {tree.map((node) => (
          <TrashTreeNode key={node.id} node={node} depth={0} />
        ))}
      </div>

      <ConfirmDialog
        open={showEmptyConfirm}
        title={t("trash.emptyTrash")}
        message={t("trash.emptyTrashConfirm")}
        confirmLabel={t("trash.emptyTrash")}
        onConfirm={handleEmptyTrash}
        onCancel={() => setShowEmptyConfirm(false)}
        danger={true}
      />

      <ConfirmDialog
        open={showDeleteAllConfirm}
        title={t("trash.permanentDelete")}
        message={t("trash.deleteAllConfirm")}
        confirmLabel={t("trash.permanentDelete")}
        onConfirm={async () => {
          const node = deleteAllTarget;
          setShowDeleteAllConfirm(false);
          setDeleteAllTarget(null);
          if (node) await handleDeleteAll(node);
        }}
        onCancel={() => {
          setShowDeleteAllConfirm(false);
          setDeleteAllTarget(null);
        }}
        danger={true}
      />

      <ConfirmDialog
        open={showRestoreConfirm}
        title={t("trash.restore")}
        message={t("trash.restoreAllConfirm")}
        confirmLabel={t("trash.restore")}
        onConfirm={async () => {
          const node = restoreTarget;
          setShowRestoreConfirm(false);
          setRestoreTarget(null);
          if (node) await handleRestoreAll(node);
        }}
        onCancel={() => {
          setShowRestoreConfirm(false);
          setRestoreTarget(null);
        }}
      />
    </div>
  );
}
