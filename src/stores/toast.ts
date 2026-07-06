import { toast as sonnerToast } from "sonner";

// Imperative helpers so non-React callers (e.g. lib/tauri.ts) keep the same
// import surface — rendering is delegated to sonner's Toaster (mounted in
// the app shell).
export function toast(message: string): void {
  sonnerToast.success(message);
}

export function toastInfo(message: string): void {
  sonnerToast.info(message);
}

export function toastError(message: string): void {
  sonnerToast.error(message, { duration: 5000 });
}
