# TypeScript & React Style Guide

The canonical coding standard for the `src/` frontend (React + TypeScript).
`AGENTS.md` links here instead of duplicating these rules — this file is the
single source of truth.

Every rule lists its **enforcing tool** so you know whether a violation fails the
build automatically or is caught only in review:

| Tag | Meaning |
|-----|---------|
| `tsc` | TypeScript compiler (`npm run typecheck`) |
| `biome` | Biome formatter/linter/import sorting (`npm run lint` / `npm run format`) |
| `eslint` | ESLint, trimmed to typescript-eslint + react-hooks rules Biome cannot replicate (`npm run lint`) |
| `knip` | Release-time dead-code, unused-export, and dependency audit (`npm run knip`) |
| `review` | No automated check; enforced by human/agent review |

Run `npm run check` before committing larger frontend changes. The lefthook
pre-commit hook formats staged frontend files with Biome and runs ESLint on
staged TS/TSX files; the pre-push hook runs `npm run check` and `npm test`.
Run `npm run knip` before a release and after broad frontend refactors; it is
intentionally not a pre-push hook because it is noisier than type/lint/test
feedback during normal feature work. Knip errors block release. Type-only unused
exports are warnings so shared model files can be cleaned opportunistically
without blocking unrelated release work.

---

## 1. Type safety (non-negotiable)

- **Strict mode is on and stays on.** `tsconfig.json` has `strict: true`. — `tsc`
- **No `any`.** Model genuinely-unknown boundary data as `unknown` and narrow it. — `review` (biome `noExplicitAny` advisory)
- **No `as unknown as T`, no `@ts-ignore`, no `@ts-expect-error`** to silence the compiler. If a type is wrong, fix the type. — `tsc` / `review`
- **Boundary data is `unknown`.** `tool_metadata.structured`, `JSON.parse` output, and `CustomEvent.detail` are modeled as `unknown` and narrowed with type guards before use. — `review`

```ts
// ✅ narrow at the boundary
const detail: unknown = event.detail;
if (isOpenSubagentDetail(detail)) handleOpen(detail);

// ❌ never
const detail = event.detail as unknown as OpenSubagentDetail;
```

## 2. Error handling — no silent fallbacks

- **No empty `catch {}`.** At minimum log; then rethrow, fall back deliberately, or surface via the toast store. — `review`
- **No `?? fallback` that masks a failed read.** `?? 0` / `?? []` are fine for a genuine empty state, forbidden when they hide a broken load. Distinguish *loading* from *empty*. — `review`
- **No `console.log` in committed code.** Use the toast store for user-visible errors; `console.warn` / `console.error` only at the Tauri-IPC boundary. — `review` (biome `noConsole` allows only warn/error)
- **Surface backend failures.** When a Tauri command fails, show a toast or error state — never render stale/empty data as success. — `review`

## 3. Immutability

- **All store updates use spread copies.** `editorGroups`, `settings`, `providerSnapshots`, `search` — never mutate in place. — `review`
- Return the *previous reference* when an update is a no-op (see `syncAllTabTitles`) to avoid spurious reactivity. — `review`

```ts
// ✅
setGroups((prev) => prev.map((g) => (g.id === id ? { ...g, activeTabId } : g)));
// ❌
group.activeTabId = id;
```

## 4. React reactivity

- **Hooks stay at the top level.** `useState` / `useEffect` / `useMemo` / store hooks are never called in loops, conditions, or callbacks. — `eslint` / `review`
- **Use stable keys for collections.** `.map()` uses item IDs, not array indexes, for tab/pane/session collections that should survive reorders. — `review`
- **Use `key={id}` for intentional remounts** when component-local state must reset on identity change, such as `SessionView` on session replacement. — `review`
- **Use `useMemo` only when it has a real job:** expensive computation or identity used by a downstream dependency. React Compiler handles routine memoization. — `review`
- **Store reads are split by context.** Components read via reactive `useX()` hooks; event handlers/effects use imperative getters/actions when they need current state outside render. — `review`

## 5. Components & structure

- **Explicit `interface Props { … }`** for any component with more than one prop. No inline `{ x }: { x: string }`. — `review`
- **Many small files over few large ones.** Target 200–400 LOC, 800 hard max. Extract hooks and sub-components when a file mixes orchestration with rendering. — `review`
- **Organize by feature/domain**, not by type. — `review`
- **No single-use helpers** — inline them. — `review`

## 6. i18n

- **All user-facing strings go through `t()`.** No literal English in JSX. — `review`
- **`en.json` and `zh.json` keys stay in parity.** Every leaf key must be referenced by at least one `t()` call (guarded by `i18n.test.ts`). — `vitest`

## 7. Formatting & Biome linting

- 2-space indent, 120-column width, double quotes, semicolons, trailing commas, LF line endings. — `biome`
- Never hand-format; run `npm run format`. The pre-commit hook formats staged
  files automatically. Use `npm run format:check` when you only want to verify
  formatting. — `biome`
- Biome runs the `preset: "recommended"` linter alongside ESLint. A few
  recommended rules are **intentionally disabled** in `biome.json` (documented
  here because Biome's config cannot hold inline comments):
  - **`a11y` group** (`useButtonType`, `noSvgWithoutTitle`, `noStaticElementInteractions`,
    `useKeyWithClickEvents`) — full WCAG linting is out of scope for this icon-heavy
    desktop app; revisit as a dedicated initiative.
  - **`style/noNonNullAssertion`** — non-null assertions are a deliberate, widespread
    choice; type safety is enforced via `tsc` strict + review.
  - **`style/noDescendingSpecificity`** — reordering the hand-tuned cascade in the
    existing stylesheets risks visual regressions.
  - **`suspicious/noAssignInExpressions`** — the `while ((m = re.exec(s)) !== null)`
    regex-iteration idiom is correct and clearer than the alternatives.

---

### Quick checklist before commit

- [ ] `npm run check` clean
- [ ] No `any` / `as unknown as` / `@ts-ignore` / `console.log` / empty `catch`
- [ ] Stores updated immutably (spread)
- [ ] Reactivity: hooks at top level, stable keys, deliberate remount keys
- [ ] User-facing strings via `t()`, both locales in parity
- [ ] New behavior has a `*.test.ts(x)` next to the source

### Release checklist

- [ ] `npm run knip` has no errors; type-only warnings are either cleaned up or
      accepted as advisory
