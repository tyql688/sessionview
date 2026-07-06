import { create } from "zustand";

export type ToastType = "success" | "error" | "info";

export interface Toast {
  id: number;
  type: ToastType;
  message: string;
}

interface ToastState {
  toasts: Toast[];
  add: (type: ToastType, message: string, duration?: number) => void;
  remove: (id: number) => void;
}

let nextId = 0;
const toastTimers = new Map<number, ReturnType<typeof setTimeout>>();

export const useToastStore = create<ToastState>((set, get) => ({
  toasts: [],
  remove: (id) => {
    set((state) => ({ toasts: state.toasts.filter((t) => t.id !== id) }));
    const timer = toastTimers.get(id);
    if (timer) {
      clearTimeout(timer);
      toastTimers.delete(id);
    }
  },
  add: (type, message, duration = 3000) => {
    const id = ++nextId;
    set((state) => ({ toasts: [...state.toasts, { id, type, message }] }));
    const timer = setTimeout(() => get().remove(id), duration);
    toastTimers.set(id, timer);
  },
}));

// Imperative helpers so non-React callers (e.g. lib/tauri.ts) keep the same
// import surface as the old Solid module — named exports unchanged.
export function toast(message: string): void {
  useToastStore.getState().add("success", message);
}

export function toastInfo(message: string): void {
  useToastStore.getState().add("info", message);
}

export function toastError(message: string): void {
  useToastStore.getState().add("error", message, 5000);
}
