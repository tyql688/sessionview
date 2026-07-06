import { useEffect, useState } from "react";
import { useI18n } from "./i18n";
import { getSessionCount } from "./lib/tauri";

// Phase 1 proof-of-life shell: verifies the full stack end to end —
// Vite + @vitejs/plugin-react + React Compiler render, react-i18next `t()`,
// and a real Tauri IPC round-trip. Replaced by the ported App in Phase 9.
export default function App() {
  const { t } = useI18n();
  const [count, setCount] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    getSessionCount()
      .then(setCount)
      .catch((e) => setError(e instanceof Error ? e.message : String(e)));
  }, []);

  return (
    <div style={{ padding: "2rem", fontFamily: "system-ui" }}>
      <h1>CC Session — React migration</h1>
      <p>i18n check: {t("common.loading")}</p>
      {error ? (
        <p style={{ color: "crimson" }}>IPC error: {error}</p>
      ) : (
        <p>Indexed sessions: {count === null ? "…" : count}</p>
      )}
    </div>
  );
}
