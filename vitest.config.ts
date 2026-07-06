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
        },
      },
      {
        plugins: [react()],
        test: {
          name: "components",
          environment: "happy-dom",
          include: ["src/**/*.test.tsx"],
          setupFiles: ["./vitest.setup.ts"],
          server: {
            // @lobehub/icons pulls in @lobehub/fluent-emoji, whose ESM entry
            // uses extensionless directory imports that Node's native ESM
            // loader rejects. Inlining routes it through Vite's resolver.
            deps: { inline: [/@lobehub/] },
          },
        },
      },
    ],
  },
});
