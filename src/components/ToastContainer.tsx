import { type Toast, useToastStore } from "@/stores/toast";

function toastIcon(type: Toast["type"]): string {
  switch (type) {
    case "success":
      return "✓";
    case "error":
      return "✕";
    case "info":
      return "ℹ";
  }
}

export function ToastContainer() {
  const toasts = useToastStore((s) => s.toasts);
  return (
    <div className="toast-container">
      {toasts.map((t) => (
        <div key={t.id} className={`toast toast-${t.type}`}>
          <span className="toast-icon">{toastIcon(t.type)}</span>
          <span className="toast-message">{t.message}</span>
        </div>
      ))}
    </div>
  );
}
