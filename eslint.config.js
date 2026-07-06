import js from "@eslint/js";
import reactHooks from "eslint-plugin-react-hooks";
import tseslint from "typescript-eslint";

// Formatting is owned by Biome (biome.json); ESLint keeps only typescript-eslint
// correctness rules + react-hooks rules. No formatting rules are enabled here.
//
// MIGRATION (migrate/react): the `ignores` block lists not-yet-ported Solid
// source, excluded until its phase ports it. Shrink as phases land. See
// MIGRATION.md. Kept in sync with tsconfig.json + biome.json exclude lists.
export default tseslint.config(
  {
    ignores: [
      "node_modules/",
      "dist/",
      "src-tauri/",
      ".reference/",
      "**/*.test.ts",
      "**/*.test.tsx",
      "src/components/**",
      "src/App/**",
      "src/index.tsx",
      "src/stores/editorGroups.ts",
      "src/stores/favorites.ts",
      "src/stores/providerSnapshots.ts",
      "src/stores/search.ts",
      "src/stores/selection.ts",
      "src/stores/settings.ts",
      "src/stores/theme.ts",
      "src/stores/updater.ts",
      "src/stores/usageView.ts",
      "src/lib/provider-watch.ts",
      "src/lib/tree-builders.ts",
    ],
  },
  js.configs.recommended,
  ...tseslint.configs.recommended,
  {
    rules: {
      "@typescript-eslint/no-unused-vars": [
        "error",
        {
          argsIgnorePattern: "^_",
          varsIgnorePattern: "^_",
          caughtErrorsIgnorePattern: "^_",
        },
      ],
    },
  },
  {
    files: ["src/**/*.{ts,tsx}"],
    plugins: { "react-hooks": reactHooks },
    rules: {
      "react-hooks/rules-of-hooks": "error",
      "react-hooks/exhaustive-deps": "warn",
    },
  },
);
