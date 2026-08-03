import { useEffect, useMemo, useRef, useState } from "react";
import { Star } from "lucide-react";
import { Button } from "@/components/ui/button";
import { listFavorites } from "@/lib/tauri";
import type { SessionMeta, SessionRef, TreeNode } from "@/lib/types";
import { useI18n } from "@/i18n/index";
import { buildFavoritesTree } from "@/lib/tree-builders";
import { toastError } from "@/stores/toast";
import { errorMessage } from "@/lib/errors";
import { useFavoriteVersion } from "@/features/favorites/favorites";
import { TreeNodeComponent } from "@/features/explorer/TreeNode";

interface FavoritesViewProps {
  onOpenSession: (session: SessionRef) => void;
  onOpenExplorer: () => void;
}

export function FavoritesView(props: FavoritesViewProps) {
  const { t } = useI18n();
  const [favorites, setFavorites] = useState<SessionMeta[]>([]);
  const [loading, setLoading] = useState(true);
  const [expandedIds, setExpandedIds] = useState<Set<string>>(new Set());
  const [focusedNodeId, setFocusedNodeId] = useState<string | null>(null);
  const initializedRef = useRef(false);
  const favoriteVersion = useFavoriteVersion();

  const noProjectLabel = t("explorer.noProject");

  const tree = useMemo(() => buildFavoritesTree(favorites, noProjectLabel), [favorites, noProjectLabel]);
  const visibleTreeIds = useMemo(() => {
    const ids = new Set<string>();
    const pending = [...tree];
    while (pending.length > 0) {
      const node = pending.pop();
      if (!node) continue;
      ids.add(node.id);
      if (expandedIds.has(node.id)) pending.push(...node.children);
    }
    return ids;
  }, [expandedIds, tree]);

  useEffect(() => {
    setFocusedNodeId((current) => (current && visibleTreeIds.has(current) ? current : (tree[0]?.id ?? null)));
  }, [tree, visibleTreeIds]);

  function autoExpand(nodes: TreeNode[]) {
    const ids = new Set<string>();
    for (const n of nodes) {
      ids.add(n.id);
      for (const c of n.children) {
        ids.add(c.id);
      }
    }
    return ids;
  }

  async function refresh() {
    try {
      const data = await listFavorites();
      setFavorites(data);
      if (!initializedRef.current) {
        setExpandedIds(autoExpand(buildFavoritesTree(data, noProjectLabel)));
        initializedRef.current = true;
      }
    } catch (e) {
      toastError(errorMessage(e));
    } finally {
      setLoading(false);
    }
  }

  // Initial load.
  useEffect(() => {
    void refresh();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Re-fetch when favorite version changes (e.g. toggled from Explorer or
  // SessionView), and only after the initial load has completed.
  useEffect(() => {
    if (initializedRef.current) void refresh();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [favoriteVersion]);

  function toggleExpanded(nodeId: string) {
    setExpandedIds((prev) => {
      const next = new Set(prev);
      if (next.has(nodeId)) next.delete(nodeId);
      else next.add(nodeId);
      return next;
    });
  }

  function isNodeExpanded(nodeId: string): boolean {
    return expandedIds.has(nodeId);
  }

  function findSession(id: string): SessionMeta | undefined {
    return favorites.find((s) => s.id === id);
  }

  function handleSessionClick(_e: React.MouseEvent, node: TreeNode) {
    const session = findSession(node.id);
    if (session) {
      props.onOpenSession(session);
    }
  }

  // no-op for context menus — no special behavior
  function handleContextMenu(_e: { clientX: number; clientY: number }, _node: TreeNode) {}

  return (
    <div className="favorites-view">
      <div className="explorer-header">
        <span>{t("favorites.title")}</span>
        {favorites.length > 0 && <span className="count-badge">{favorites.length}</span>}
      </div>
      {loading && (
        <div className="loading-center">
          <div className="spinner spinner-sm" />
        </div>
      )}
      {!loading && favorites.length === 0 && (
        <div className="empty-state">
          <Star size={32} strokeWidth={1.5} color="var(--text-tertiary)" aria-hidden="true" />
          <p className="empty-state-text">{t("favorites.empty")}</p>
          <p className="empty-state-hint">{t("favorites.emptyHint")}</p>
          <Button variant="outline" size="sm" onClick={props.onOpenExplorer}>
            {t("favorites.browseSessions")}
          </Button>
        </div>
      )}
      {!loading && favorites.length > 0 && (
        <div className="explorer-tree" role="tree" aria-label={t("favorites.sessionsTree")}>
          {tree.map((node) => (
            <TreeNodeComponent
              key={node.id}
              node={node}
              depth={0}
              activeSessionId={null}
              focusedNodeId={focusedNodeId}
              onNodeFocus={setFocusedNodeId}
              isNodeExpanded={isNodeExpanded}
              toggleExpanded={toggleExpanded}
              onSessionContextMenu={(e, n, _p) => handleContextMenu(e, n)}
              onNodeContextMenu={handleContextMenu}
              onSessionClick={(e, n, _p) => handleSessionClick(e, n)}
            />
          ))}
        </div>
      )}
    </div>
  );
}
