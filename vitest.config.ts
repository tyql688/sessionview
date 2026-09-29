import path from "node:path";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";
import svgr from "vite-plugin-svgr";

const alias = {
  "@": path.resolve(import.meta.dirname, "./src"),
};

// Two test projects: logic/store/parser tests (`*.test.ts`, plain node env) and
// React component render tests (`*.test.tsx`, DOM + JSX via happy-dom).
export default defineConfig({
  test: {
    projects: [
      {
        resolve: { alias },
        test: {
          name: "unit",
          environment: "node",
          include: ["src/**/*.test.ts"],
        },
      },
      {
        resolve: { alias },
        plugins: [svgr(), react()],
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
