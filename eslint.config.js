import js from "@eslint/js";
import reactHooks from "eslint-plugin-react-hooks";
import tseslint from "typescript-eslint";

// Formatting is owned by Biome (biome.json); ESLint keeps only typescript-eslint
// correctness rules + react-hooks rules. No formatting rules are enabled here.
export default tseslint.config(
  {
    ignores: [
      "node_modules/",
      "dist/",
      "src-tauri/",
      ".reference/",
      "**/*.test.ts",
      "**/*.test.tsx",
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
