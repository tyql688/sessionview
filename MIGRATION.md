# Solid → React migration (`migrate/react`)

Big-bang framework migration on a dedicated branch. `master` (Solid) stays the
shippable release **and the behavioral oracle** — to check original behavior,
read `git show master:src/<path>`.

Not-yet-ported Solid source stays in-tree but is **excluded from tsc / eslint /
biome** (the exclude lists in `tsconfig.json`, `eslint.config.js`, `biome.json`,
kept in sync) so every checkpoint stays green. Each phase deletes the Solid file
it supersedes and replaces it with the React port; the exclude lists shrink to
empty by Phase 10. Superseded entry/shell (`src/index.tsx`, `src/App/`) are
already removed in favor of `src/main.tsx` + `src/App.tsx`.

## Why

Solid's ecosystem gaps that drove this: (1) thin off-the-shelf component/widget
ecosystem, (2) small community — maintenance/hiring risk, (3) weak LLM/AI-coding
support. All three point at **React**, not Vue (Vue would be a cheaper port but
loses on exactly the three axes we're optimizing for).

## Target stack

| Concern | Choice | Notes |
|---------|--------|-------|
| Framework | **React 19** | stable `use`, Actions, ref-as-prop |
| Compiler | **React Compiler** (babel plugin) | auto-memoization; removes the manual `memo`/`useMemo` tax that is React's weak axis |
| Build | **Vite 6 + @vitejs/plugin-react** | Babel path required by React Compiler |
| State | **zustand** | near 1:1 with the current hand-rolled spread-immutable stores; `getState()` for non-React callers |
| i18n | **react-i18next** | `en.json`/`zh.json` transfer unchanged |
| Virtualization | **@tanstack/react-virtual** | message list; also fixes the Solid `<For>` full-row-recreation cost |
| UI primitives | **shadcn/ui + Radix** (Phase 8, deferred) | copy-in, desktop-density friendly |
| Test | **@testing-library/react** + vitest | vitest + happy-dom stay |

## What survives untouched

- **`src-tauri/` (Rust backend + Tauri IPC): 100%.**
- **~36% of frontend TS** — the framework-agnostic layer: `types.ts`, most of
  `lib/*` (formatters, tree-builders, diff, heatmap, backend-events, tools/*,
  the remark/unified markdown parser core), the module-level parse LRU cache, the
  highlight.js LRU cache. These are plain functions; they move as-is.
- **`src/styles/*` CSS** — carries over directly.
- **`i18n/en.json`, `i18n/zh.json`** — data only.

## What gets rewritten

70 Solid-coupled files: `src/stores/*` (→ zustand), `src/i18n/index.ts`
(→ react-i18next), the 52 `.tsx` components, and the Solid hooks. Three `lib`
files reach into stores and need small edits (`tauri.ts`→toast,
`provider-watch.ts`/`tree-builders.ts`→providerSnapshots).

## Reactivity mapping (Solid → React)

| Solid | React |
|-------|-------|
| component body runs once | body re-runs every render — **the main mental-model flip** |
| `createSignal` | `useState` |
| `createMemo` | `useMemo` (mostly unnecessary under React Compiler) |
| `createEffect` | `useEffect` (mind deps + double-run in StrictMode) |
| `createStore` (nested reactive) | zustand store |
| `<For each>` | `.map()` with stable `key` |
| `<Index>` | `.map()` with index-stable `key` |
| `<Show when>` | `&&` / ternary |
| props **not** destructured | destructure freely |
| `props.x` accessor in JSX | plain `x` |

## Staging (tsc stays green at each checkpoint)

`tsconfig.json` `exclude` lists the not-yet-ported dirs; each phase removes its
dir from the exclude list once green. Phases: see the task list (Phase 1–10).
Order is bottom-up: toolchain → asset layer → stores → i18n → leaf render
components → SessionView → editor groups → panels → App shell → tests/perf.

## Ground rules

- One phase per set of commits; conventional-commit messages; app must type-check
  at each checkpoint (excluded dirs aside).
- Behavior parity is verified against `master` as the oracle, not from memory.
- No Solid remnant left by Phase 10 (`grep -r solid-js src` must be empty).
