import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

export default defineConfig({
  plugins: [
    react({
      // React Compiler: automatic memoization. Removes the manual memo/useMemo
      // tax that is React's weak axis for high-frequency updates (session viewer).
      babel: {
        plugins: [["babel-plugin-react-compiler", { target: "19" }]],
      },
    }),
  ],
  server: {
    port: 1420,
    strictPort: true,
  },
  build: {
    target: "esnext",
    chunkSizeWarningLimit: 1000,
  },
  clearScreen: false,
});
