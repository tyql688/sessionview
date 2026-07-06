import { create } from "zustand";

interface SelectionState {
  selectedIds: Set<string>;
}

export const useSelectionStore = create<SelectionState>(() => ({
  selectedIds: new Set<string>(),
}));

export function toggleSelected(id: string) {
  useSelectionStore.setState((state) => {
    const next = new Set(state.selectedIds);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    return { selectedIds: next };
  });
}

export function clearSelection() {
  useSelectionStore.setState({ selectedIds: new Set<string>() });
}

// Imperative reads for event handlers (non-reactive).
export function isSelected(id: string): boolean {
  return useSelectionStore.getState().selectedIds.has(id);
}

export function selectionCount(): number {
  return useSelectionStore.getState().selectedIds.size;
}

// Reactive hooks for components.
export function useSelectedIds(): Set<string> {
  return useSelectionStore((state) => state.selectedIds);
}

export function useIsSelected(id: string): boolean {
  return useSelectionStore((state) => state.selectedIds.has(id));
}

export function useSelectionCount(): number {
  return useSelectionStore((state) => state.selectedIds.size);
}
