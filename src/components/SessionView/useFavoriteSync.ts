import { useEffect, useState } from "react";
import {
  invokeWithFallback,
  isFavorite,
  toggleFavorite as invokeToggleFavorite,
} from "../../lib/tauri";
import { useI18n } from "../../i18n/index";
import { toast, toastError } from "../../stores/toast";
import {
  bumpFavoriteVersion,
  useFavoriteVersion,
} from "../../stores/favorites";

export interface UseFavoriteSyncResult {
  starred: boolean | null;
  toggleFavorite: () => Promise<void>;
}

/**
 * Keeps a session's favorite state in sync with the backend, re-checking
 * whenever `favoriteVersion` bumps (e.g. after another tab toggles the same
 * session). Returns a `toggleFavorite` handler that flips state, bumps the
 * version, and surfaces a toast.
 */
export function useFavoriteSync(sessionId: string): UseFavoriteSyncResult {
  const { t } = useI18n();
  const [starred, setStarred] = useState<boolean | null>(null);
  const favoriteVersion = useFavoriteVersion();

  useEffect(() => {
    void (async () => {
      const id = sessionId;
      const fav = await invokeWithFallback(
        isFavorite(id),
        starred,
        `refresh favorite state for session ${id}`,
      );
      setStarred(fav);
    })();
    // Mirrors Solid `on(() => favoriteVersion(), ...)`: re-check only when the
    // version bumps. `sessionId`/`starred` are read but intentionally not deps
    // (SessionView remounts on session id change).
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [favoriteVersion]);

  const toggleFavorite = async () => {
    try {
      const newState = await invokeToggleFavorite(sessionId);
      setStarred(newState);
      bumpFavoriteVersion();
      toast(t(newState ? "toast.favoriteAdded" : "toast.favoriteRemoved"));
    } catch (_e) {
      toastError(t("toast.favoriteFailed"));
    }
  };

  return { starred, toggleFavorite };
}
