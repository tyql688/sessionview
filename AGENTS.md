# SessionView

Guidance for Claude Code (claude.ai/code) and other agents working in this
repository. `CLAUDE.md` includes this file via `@AGENTS.md`.

SessionView is a desktop app that brings local AI coding sessions from many
tools — Claude Code, Codex, Antigravity, Kimi Code, Cursor, OpenCode, CC-Mirror,
Pi, and Grok Build — into one place to read, search, analyze usage, export, and
resume.
Stack: Tauri 2 + React 19 (with React Compiler) + Rust + SQLite (FTS5).
Enforcement-mapped coding standards live in `style/ts.md` and `style/rust.md`;
when this file and those disagree, those win.

## Commands

```bash
# App / frontend
npm run tauri dev        # run the app with hot reload
npm run tauri build      # production bundle

# Headless server (browser shell; build dist first so it can be embedded)
npm run build && cd src-tauri && \
  cargo build --release --no-default-features --features headless
# gates must pass under BOTH feature sets: default (gui) and
# --no-default-features --features headless
npm run check            # typecheck + Biome + ESLint — the frontend gate
npm test                 # frontend tests (Vitest, run mode)
npm run knip             # dead-code / dependency audit — a release gate

# Rust backend (run from src-tauri/)
cargo test               # backend tests
cargo fmt --check        # format check
cargo clippy --all-targets --all-features -- -D warnings   # lint, CI-strict

# Release
./scripts/release.sh <version>   # bump, commit, tag, push -> CI release
```

Run a single test:

```bash
npm test -- src/lib/foo.test.ts               # one frontend file
npx vitest run -t "resolves subagent"         # frontend, by test name
cd src-tauri && cargo test parent_backfills   # one Rust test by name substring
cd src-tauri && cargo test --test parser_tests# one Rust integration-test file
```

Real-data smoke tests (`src-tauri/tests/*_real_*.rs`) are `#[ignore]` by default;
run them explicitly with `cargo test -- --ignored`. Git hooks (lefthook)
pre-commit format staged frontend files and run ESLint; pre-push runs
`npm run check` + `npm test` and the Rust fmt/clippy/test gate. CI and
`scripts/release.sh` are the source of truth for exact commands.

## Architecture

The core job: normalize many tools' on-disk session logs into one queryable
model, then render, search, and analyze them. Understanding it means reading
across `src-tauri/src/{providers,provider,indexer.rs,db,models.rs}` and
`src/{features,stores}`.

### Backend pipeline (`src-tauri/src/`)

- **Providers** (`providers/`) each implement the `SessionProvider` trait
  (`provider/traits.rs`) and parse one tool's logs into a normalized
  `ParsedSession` / `Message` model (`models.rs`). A provider is authoritative
  for its own fields. Provider identity and metadata are bridged through a
  `Provider` enum + descriptor (`provider/`), so **adding a provider is a
  cross-layer change**, not just a parser: enum + catalog + Tauri asset-scope
  allowlist + frontend provider type + theme/snapshot fallback + resume
  behavior + tests. Exhaustive `match` on `Provider` makes the compiler surface
  most of these.
- **Indexing** (`indexer.rs`) is incremental: providers short-circuit unchanged
  files by `(size, mtime)` via `scan_incremental`, and a `maintenance_running`
  guard (`commands/sessions.rs`) serializes passes so a scan never races the
  app. Parsed sessions upsert into **SQLite** (`db/`) with FTS5 backing
  full-text search; progress streams to the UI as `maintenance-status` events.
- **Commands** (`commands/`) are the backend surface and the trust boundary —
  validate provider strings and path inputs here, and keep the asset scope
  allowlist-based. Command bodies are transport-agnostic core functions
  (`fn(state: AppState, …)`); two thin shells wrap them: `commands/gui.rs`
  (`#[tauri::command]`, feature `gui`, default) and `server/dispatch.rs`
  (HTTP invoke, feature `headless`). Events go through the `EventBus` trait
  (`services/events.rs`) — Tauri emit in the GUI, SSE broadcast headless.
  **Adding a command — or changing its signature — means updating all four:
  the core, the gui wrapper + `generate_handler!` list, the dispatch match
  arm, and the `BackendCommandMap` entry in `src/lib/tauri.ts`.** Every usage
  query also takes an optional IANA `timezone`; the frontend sends the
  viewer's zone, so a remote headless client gets its own day boundaries.
- **Headless shell** (`server/`, feature `headless`): axum server (default
  port 9921) serving the embedded `dist/` plus `POST /api/invoke/{command}`,
  `GET /api/events` (SSE), and export-download endpoints. It shares the GUI's
  data dir and SQLite index (WAL + busy_timeout make the two processes safe
  to run concurrently), so it never re-indexes what the GUI already did — a
  schema change is the exception: the first process on the new schema drops
  the derived stats and resets `source_mtime`, forcing one full re-index, and
  running mixed binary versions against one data dir is not supported. The
  frontend picks its transport at runtime via `src/lib/runtime.ts`
  (`__TAURI_INTERNALS__` detection); `npm/` holds the `npx
  sessionview-headless` launcher and platform-package generator.
- **Parent/child trees.** Subagents and sidechains are child sessions, linked to
  their parent by *typed* provider signals — never by scanning message text.
  Some providers store children in separate files; the "Open subagent" UI
  navigates to them.
- **Usage & cost** (`models.rs`, `pricing.rs`, `provider/tokens.rs`) aggregate
  token usage against a pricing table. **`ParsedSession::usage_events` is
  authoritative when non-empty** — both the default `compute_token_stats` and
  `LoadedSession::from_parsed` prefer it over `messages[].token_usage` (Codex,
  Grok, Kimi and Pi populate it; the rest attach usage to messages). Every
  provider normalizes to **disjoint** input / cache-read counts, so summing the
  four components never double-counts. Rows land in `session_token_stats` keyed
  by a **UTC 15-minute bucket** (`provider::STATS_BUCKET_SECONDS`), never a
  pre-baked local date; read paths fold buckets into civil days for the
  caller's timezone (`services/timeday.rs`), so one index serves every zone.
  Claude Code streams *cumulative* usage across several JSONL lines that share
  one message id — aggregation keeps the max-total entry per id, not the first.

### Frontend (`src/`)

- UI state lives in **zustand** stores: components read via reactive `useX()`
  hooks, while non-React code uses imperative getters/actions. Feature state
  lives under `src/features/*`; only truly global slices sit in `src/stores/`.
- The editor is **VSCode-style**: tab groups with split view, plus preview
  (single-click, italic, replaceable) vs pinned (double-click) tabs, owned by
  the editor-groups store.
- The **session timeline is a `column-reverse` + `content-visibility` scroller,
  not a JS virtualizer.** Real rows stay in normal flow, newest-first in the
  DOM, so the scroll coordinate system anchors to the newest message: loading
  older history lands outside that coordinate space and can never move the
  viewport (WKWebView has no native scroll anchoring, and its rubber-band
  animation overrides programmatic scrollTop writes — the compensation designs
  this replaced, including react-virtuoso/virtua, are all broken there).
  Coordinate math lives in `session/timelineGeometry.ts`. Don't swap the model
  out without revalidating against WKWebView's elastic scrolling.
- User-facing strings go through i18n (`react-i18next`), English and Chinese in
  parity.

### Boundary discipline (the invariant that spans both sides)

No silent fallbacks: when a correct value can't be obtained, log a warning and
skip — never fabricate a plausible-but-wrong substitute, and never render
stale/empty data as if a request succeeded. Provider-specific quirks (wire-format
variants, blob decoding, subagent file layouts) are documented as module
doc-comments and regression tests next to each provider; read those before
changing a parser rather than rediscovering them by trial.

## Conventions

- Standards in `style/ts.md` / `style/rust.md` name their enforcing tool per rule
  (tsc / biome / eslint / knip / fmt / clippy / review).
- Conventional commits (`feat:`, `fix:`, `refactor:`, `docs:`, `test:`,
  `chore:`); one logical change per commit.
- Rust unit tests go in `#[cfg(test)]` at file bottom, cross-file tests in
  `src-tauri/tests/`; frontend tests are `*.test.ts(x)` next to their source.
  Use golden fixtures for parser regressions and synthetic placeholder data —
  never real session ids, usernames, or paths.
- `tauri.conf.json` sets `dangerousDisableAssetCspModification: ["style-src"]`
  because the app injects `<style>` elements at runtime (search highlighting,
  mermaid theming, shiki/katex); Tauri's nonce rewriting would make browsers
  ignore the `'unsafe-inline'` that those need. `script-src` keeps Tauri's
  nonce hardening — don't add it to that list.
