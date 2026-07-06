import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

// Two test projects: logic/store/parser tests (`*.test.ts`, plain node env) and
// React component render tests (`*.test.tsx`, DOM + JSX via happy-dom).
export default defineConfig({
  test: {
    projects: [
      {
        test: {
          name: "unit",
          environment: "node",
          include: ["src/**/*.test.ts"],
          // MIGRATION: un-ported logic tests that transitively import Solid
          // rendering code. Removed as each phase ports the module. See
          // MIGRATION.md.
          exclude: ["src/components/MessageBubble/MarkdownRenderer.test.ts"],
        },
      },
      {
        plugins: [react()],
        test: {
          name: "components",
          environment: "happy-dom",
          include: ["src/**/*.test.tsx"],
          setupFiles: ["./vitest.setup.ts"],
        },
      },
    ],
  },
});
