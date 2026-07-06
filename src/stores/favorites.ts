import { create } from "zustand";

interface FavoriteState {
  favoriteVersion: number;
}

// Global version counter — incremented whenever any favorite is toggled.
// SessionView watches this to re-check its starred state.
export const useFavoriteStore = create<FavoriteState>(() => ({
  favoriteVersion: 0,
}));

export function bumpFavoriteVersion() {
  useFavoriteStore.setState((state) => ({
    favoriteVersion: state.favoriteVersion + 1,
  }));
}

export function getFavoriteVersion(): number {
  return useFavoriteStore.getState().favoriteVersion;
}

export function useFavoriteVersion(): number {
  return useFavoriteStore((state) => state.favoriteVersion);
}
