# CC Session

Desktop app for browsing AI coding sessions. Tauri 2.0 + Solid.js + Rust + SQLite FTS5.

## Commands

```bash
npm run tauri dev             # Dev with hot reload
npm run tauri build           # Production build
cd src-tauri && cargo clippy  # Rust lint
cd src-tauri && cargo test    # Rust tests
npx tsc --noEmit              # TS type check
npm run lint                  # ESLint
npm run format:check          # Prettier check
./scripts/release.sh <version> # Bump, commit, tag, push → triggers CI release
```

## Project Layout

```
src/                       # Solid.js frontend (components, stores, i18n, lib, styles)
src-tauri/src/
  providers/               # claude/, codex/, antigravity/, kimi/, opencode/, cursor/, cc_mirror.rs
  commands/                # sessions.rs, settings.rs, trash.rs, terminal.rs, usage.rs, search.rs, file_access.rs
  services/                # provider_snapshots, session_lifecycle, session_resolution, source_sync, image_cache, terminal (platform launchers), caches
  exporter/                # json.rs, markdown.rs, html.rs, templates.rs
  db/                      # mod.rs, sync.rs, row_mapper.rs, queries.rs + queries/{sessions,tree,favorites,search,usage}.rs
  provider.rs + provider/  # SessionProvider machinery: traits.rs, plan.rs, state.rs, tokens.rs, catalog.rs, trash.rs
  tool_metadata.rs + tool_metadata/  # names.rs, summary.rs, result.rs, build.rs
  indexer.rs  watcher.rs  models.rs  provider_utils.rs  trash_state.rs  pricing.rs
src/components/           # feature dirs (Editor/, SessionView/, Explorer/, UsagePanel/, MessageBubble/, TrashView/, Settings/) + small flat components, icons.tsx
src/stores/               # editorGroups, settings, search, selection, providerSnapshots, updater, favorites, toast, theme, usageView
src/lib/                  # tauri.ts (IPC), tools/, formatters, tree-builders, heatmap, diff, subagent, image-cache, provider-watch
src/styles/               # variables.css (theme tokens) + per-feature files (layout, editor, explorer, session, tools, code, messages, modals, search, trash, settings, feedback, images, utilities, usage); cascade order lives in index.css
```

## Editor UI Architecture

```
App
├── ActivityBar                        # Left icon bar (explorer, favorites, usage, trash, etc.)
├── [Left panel — conditional on activeView()]
│   ├── Explorer                       # Session tree with single-click=preview, double-click=pin
│   ├── FavoritesView / TrashView / BlockedView / UsagePanel / SettingsPanel
├── SearchPanel                        # In titlebar-right, not in left panel
├── EditorGroupsContainer             # Manages split view layout (max 4 groups)
│   ├── SplitHandle                    # Draggable resize between groups
│   └── EditorArea (per group)
│       ├── TabBar                     # Tabs with preview italic, overflow chevron dropdown
│       └── SessionView (per tab)      # Full session viewer (messages, toolbar, search)
└── StatusBar                          # Index count, today's cost, provider info
```

### Editor Groups Store (`src/stores/editorGroups.ts`)

Central store for tabs, split view, and preview mode. All state is immutable (spread updates).

```typescript
interface EditorGroup {
  id: string;
  tabs: SessionRef[];          // open sessions in this group
  activeTabId: string | null;  // currently visible tab
  previewTabId: string | null; // at most one preview (unpinned) tab
  flexBasis: number;           // width percentage for split view
}
```

Key functions:
- `openSession(session)` — pin-open a tab (or focus if already open, auto-pins preview)
- `openPreview(session)` — open as preview tab (italic, replaces previous preview in group)
- `pinTab(sessionId)` — promote preview to permanent
- `splitToRight(sessionId)` — move tab to right group (creates new group if needed)
- `moveTabToGroup(sessionId, targetGroupId, insertIndex?)` — drag-drop between groups
- `createGroupFromDrop(sessionId)` — drop zone creates new rightmost group

### Preview Mode (VSCode-style)

- **Explorer single click** → `openPreview()` — italic tab, replaced by next preview
- **Explorer double click** → `openSession()` — permanent pinned tab
- **Double-click on preview tab** → `pinTab()` — pins it
- **Search/Favorites/Subagent open** → `openSession()` — always pinned
- Each group has at most one `previewTabId`; replacement removes old preview tab from array
- SessionView is wrapped in `<Show when={session().id} keyed>` to force remount on preview replacement (prevents stale local state: filters, search, watch, favorite)

### Tab Overflow

- Hidden-scrollbar scroll container with mouse wheel horizontal scroll
- `ResizeObserver` + `props.tabs` array reference change triggers overflow detection
- `»` chevron button with dropdown listing all tabs (active indicator, italic for preview)

## Event System

Custom DOM events for cross-component communication:

| Event | Dispatcher | Handler | Purpose |
|-------|-----------|---------|---------|
| `open-subagent` | ToolMessage "↗ Open" button | App.handleOpenSubagent | Navigate to child session by agentId/nickname/description |
| `usage-data-changed` | SessionView (on favorite/export) | App (refreshes usage panel) | Sync usage stats |

Tauri backend events:

| Event | Payload | Purpose |
|-------|---------|---------|
| `sessions-changed` | `string[]` (changed paths) | Trigger debounced reindex from file watcher |
| `maintenance-status` | `MaintenanceEvent` | Show indexing/cleanup progress |

### Subagent Open Matching

`handleOpenSubagent` matches child sessions by priority:
1. `agentId` — exact match OR `agent-${agentId}` prefix (Claude files are `agent-{id}.jsonl`)
2. `nickname` — Codex agent nickname from tool output
3. `description` — full description from tool_input JSON (NOT truncated summary)

## Provider Architecture

All providers implement `SessionProvider` trait (`watch_paths` / `scan_all` / `scan_source` / `load_messages` / `deletion_plan` / `restore_action` / `cleanup_on_permanent_delete`).
Metadata via Bridge pattern: `Provider` enum → `ProviderDescriptor` (zero-sized structs).

| Provider    | Path                                   | Format | Watch |
|-------------|----------------------------------------|--------|-------|
| Claude Code | `~/.claude/projects/**/*.jsonl`        | JSONL  | FS    |
| Codex       | `~/.codex/sessions/**/*.jsonl`         | JSONL  | FS    |
| Antigravity | `~/.gemini/antigravity-cli/brain/*/.system_generated/logs/transcript.jsonl` | JSONL | FS |
| Kimi Code   | `~/.kimi-code/sessions/wd_*/<session_dir>/agents/*/wire.jsonl` | JSONL | FS |
| Cursor CLI  | `~/.cursor/projects/<key>/agent-transcripts/<id>/<id>.jsonl` (CLI) + `~/.cursor/acp-sessions/<id>/store.db` (ACP) | JSONL + SQLite | FS |
| OpenCode    | `~/.local/share/opencode/opencode.db`  | SQLite | Poll  |
| CC-Mirror   | `~/.cc-mirror/{variant}/config/projects/**/*.jsonl` | JSONL | FS |

Tool names mapped to canonical set per provider: {Bash, Edit, Read, Write, Glob, Grep, Agent, Plan}.
Resume: Claude `--resume`, Codex `resume`, Antigravity `agy --conversation <id>`, Kimi `--session`, Cursor `agent --resume=<id>`, OpenCode `-s`.

## Testing

- **Rust**: `cd src-tauri && cargo test` — parser golden tests + provider/unit tests + fixture command interface coverage
- **Frontend**: `npm test` (vitest)
- **Manual smoke**: `provider_lifecycle_real_interface.rs` is ignored by default and intended for local/manual real-provider verification

## Key Patterns

- **Message**: `{ role, content, timestamp, tool_name, tool_input, token_usage }` — universal across providers
- **Thinking**: `MessageRole::System` with `[thinking]\n` prefix
- **Images**: `[Image: source: ...]` in content; persistent cache at `app_data_dir/images/{sha256}.ext`; `read_image_base64` reads from cache when original is deleted
- **Tool merge**: `call_id` pairs tool calls with results into single tool message
- **Tool metadata**: Rust `build_tool_metadata` + `enrich_tool_metadata` attaches summary, structured result, status, and result_kind to each tool call
- **Subagents**: `parent_id` links children; "Open" button for providers with separate files (Claude, Codex, Kimi, CC-Mirror, Antigravity). Antigravity links children via UUID scan over parent transcript content (`db/sync.rs::find_uuids`).
- **Provider snapshots**: backend derives provider label/color/order/watch strategy/path info; frontend consumes via `providerSnapshots` store
- **Trash**: `TrashMeta.parent_id` cascades restore/delete; `is_session_dir()` prevents shared dir deletion
- **Immutable state**: All Solid.js store updates use spread (`{ ...prev, field: newValue }`). Never mutate in place.
- **Solid.js reactivity**: Use `Index` (not `For`) for tab panes to preserve component instances across reorders. Use `<Show when={id} keyed>` when component must remount on identity change.

## Pitfalls

- **OpenCode**: Must use `SQLITE_OPEN_READ_WRITE` (not READ_ONLY) for WAL. Uses XDG path, not macOS `~/Library/`.
- **macOS watchers**: File-backed providers use `notify` with `macos_kqueue` for more reliable file-level follow behavior; do not assume `FSEvents`.
- **Codex**: `call_id` pairing, output can be nested JSON.
- **Kimi**: kimi-code 0.1.1+ uses two coexisting wire formats — **migrated** (only `metadata` + `context.append_message` lines, role=user/assistant/tool with `content[]`+`toolCalls[]`, NO per-line `time`) and **native** (`context.append_loop_event` carrying `content.part`/`tool.call`/`tool.result`/`step.*` plus `usage.record`, per-line `time` in epoch ms). Project path comes from `~/.kimi-code/session_index.jsonl` (`sessionId`/`sessionDir` → `workDir`). Subagents are SEPARATE files (`agents/agent-N/wire.jsonl`) linked via `state.json.agents[].parentAgentId`; subagent session id = `<parent-dir>:<agent-name>`. Resume command requires the full prefixed dir name (`session_<uuid>` or `ses_<uuid>`) — bare UUIDs return "Session not found"; resume for subagents falls back to the parent. Image parts use `imageUrl` (camelCase) in native format and `image_url` (snake_case) in migrated.
- **Cursor**: THREE kinds of sessions live under `~/.cursor/`: (1) CLI (`agent` binary), (2) IDE (Composer), and (3) ACP (third-party editors via Agent Client Protocol). CLI + IDE share the `~/.cursor/projects/<key>/agent-transcripts/<id>/<id>.jsonl` layout — we filter with `~/.cursor/chats/<md5>/<id>/store.db` as a whitelist (only IDs with a store.db are CLI; the rest are IDE and dropped). ACP sessions live separately at `~/.cursor/acp-sessions/<id>/{meta.json, store.db}` with **no JSONL on disk** — every chat message is JSON-encoded inside the store.db's content-addressed blobs, reachable by recursively walking the root protobuf blob's `0A 20 <hash>` length-prefixed references. Both CLI and ACP store.db files share the same recovery pipeline for workspace path (`<user_info>` blob), model alias (`meta.lastUsedModel`, default → "Auto"), and inline pasted images (hex-encoded JPEG/PNG in user-blob `content[].image.hex`, dumped to the shared image cache and substituted into `[Image #N]` placeholders). Subagents (`Task`/`Subagent` tool spawns) live at `<sessionId>/subagents/<subagentId>.jsonl` and title themselves from the parent's tool_use `description` matched by `prompt`. `<user_query>` strips, `<image_files>` rewrites to `[Image: source: <path>]`, `<think>…</think>` promotes to `MessageRole::System` with `[thinking]` prefix, `[REDACTED]` placeholders are dropped. Tool arg keys (`path`, `old_str`, `glob_pattern`) canonicalise to `file_path`/`old_string`/`pattern`. ACP messages use slightly different part names than JSONL CLI: `tool-call`/`toolName`/`toolCallId`/`args` (vs `tool_use`/`name`/`id`/`input`), `tool-result` (vs folded into next assistant turn), and `redacted-reasoning` parts that we silently drop. **Token usage is NOT persisted by Cursor** — neither JSONL nor store.db nor any side channel. Usage fields remain 0.
- **CC-Mirror**: Multi-variant under `~/.cc-mirror/`, sanitized variant names.
- **Antigravity**: Steps stream (`USER_INPUT`, `PLANNER_RESPONSE`, tool result). Workspace path comes from `~/.gemini/antigravity-cli/history.jsonl` (`conversationId → workspace`). Subagent linkage isn't in the file — derived from UUID scan during DB upsert, so child sessions inherit `project_path` / `parent_id` only after the parent has been indexed.
- **compact_string**: Rust `compact_string(s, limit)` truncates with `…` suffix. Do NOT use truncated summaries for matching/comparison — always extract full values from source JSON.
- **Session ID vs agentId**: Claude subagent files are `agent-{id}.jsonl`, so session ID = `agent-{id}` but tool result `agentId` = `{id}` (no prefix). Always match both forms.

## Conventions

- Commits: conventional commits (`feat:`, `fix:`, `refactor:`, `chore:`, `test:`, `docs:`). One logical change per commit.
- i18n: all user-facing strings via `t()`. No literal English in JSX.
- Colors: Claude `#d97757`, Codex `#10b981`, Antigravity `#4f46e5`, OpenCode `#06b6d4`, Kimi `#1783ff`, Cursor `#3b82f6`, CC-Mirror `#f472b6`.

## Code Standards

> **Canonical, enforcement-mapped guides:** [`style/ts.md`](style/ts.md) and [`style/rust.md`](style/rust.md).
> Each rule there lists its **enforcing tool** (tsc / biome / eslint / fmt / clippy / review).
> The section below is the always-loaded condensed copy; when they disagree, `style/*.md` wins.
> Local tooling: `biome.json`, `eslint.config.js`, `src-tauri/{rustfmt,clippy}.toml`, `lefthook.yml`
> (pre-commit: biome+eslint on staged files; pre-push: tsc/test/cargo fmt+clippy+test).

Code review (human or agent) **rejects** anything that violates the rules below. They're concrete because most "be reasonable" guidelines are unactionable.

### Rust

- **Format / lint**: `cargo fmt` and `cargo clippy --all-targets` must pass before commit. No exceptions, no `#[allow(...)]` without a one-line comment justifying why.
- **Errors**: propagate with `?`. Wrap with `anyhow::Context` for cross-layer messages; use `thiserror`-derived `ProviderError`-style enums for typed errors crossing module boundaries. Never bubble bare `String` errors.
- **No `unwrap()` / `expect()` outside tests.** In tests they're fine.
- **Logging**: `log::warn!` when skipping a record; `log::error!` for unexpected I/O failures we still recover from; `log::debug!` for parser internals. Never `eprintln!` in production paths — left over from debug sessions, reviewer rejects.
- **Naming**: `snake_case` everywhere; tests named `<unit>_<scenario>_<expected>` (e.g. `parent_backfills_child_when_parser_declares_child_ids`). Modules small and focused; if a file pushes 800 LoC, split it.
- **No helpers used exactly once** — inline them. No premature abstraction across providers; resist common-trait refactors when fewer than 3 providers actually share the shape.
- **Match arms exhaustive**. No `_ => unreachable!()` for `Provider` or other internal enums — adding a variant must force every match to be revisited (compile error is the feature).
- **Test data must be synthetic**. Hardcoded real session UUIDs, usernames, project paths from the developer's machine are forbidden in checked-in tests (use `11111111-1111-4111-a111-111111111111`-style placeholders).

### TypeScript

- **Strict mode** is non-negotiable. No `any`, no `as unknown as T`, no `// @ts-ignore`. If a type is genuinely unknown at a boundary, model it as `unknown` and narrow.
- **No `console.log`** in committed code. Use the toast store for user-visible errors, `console.warn`/`console.error` only at Tauri-IPC boundaries.
- **No empty `catch {}`**. Always log + decide (rethrow, fallback, or surface). `?? defaultValue` is fine for genuine defaults; forbidden when it hides a failed read.
- **Immutability**: all Solid.js store updates use spread (`{ ...prev, field: newValue }`). Never mutate in place. Same for editor groups, settings, provider snapshots.
- **Solid.js reactivity rules**:
  - Use `<Index>` (not `<For>`) for tab/pane collections you want to keep instances stable across reorders.
  - Use `<Show when={x} keyed>` when a component must remount on identity change (e.g. `<Show when={session().id} keyed>` for `SessionView`).
  - Wrap derived values in `createMemo` only when downstream consumers actually run more than once per change; otherwise it's overhead.
  - Read accessors `()` inside JSX/effects, not at component top-level — top-level reads run once and capture a stale value.
- **Props**: explicit `interface Props { … }`; no inline `{ x }: { x: string }` for anything with more than one field.

### Testing

- **Where**: Rust unit tests in `#[cfg(test)] mod tests` at file bottom. Cross-file Rust tests in `src-tauri/tests/<area>.rs`. Frontend tests next to source as `*.test.ts(x)`.
- **Fixtures vs synthetic**: golden fixtures in `src-tauri/tests/fixtures/<provider>/` for parser regression tests. Synthetic in-test JSON for behavioral edge cases. Don't snapshot megabytes of real data.
- **Real-data smoke tests** that depend on `~/.<provider>/` files MUST be `#[ignore]` and assert structural invariants, never hardcoded UUIDs (see `tests/cleanup_stale_links.rs` for the pattern).
- **Add a regression test for every bug fix** — paste the original bad input as a test fixture. PRs without coverage need a written justification in the commit message.
- Run `cargo test`, `cargo clippy --all-targets`, `npx tsc --noEmit`, `npm test`, `npm run lint`, `npm run format:check` before committing. CI gates on all six.

### Security

- **No secrets in code or fixtures**. API keys, auth tokens, real session IDs that tie to a person — all forbidden. Scrub before commit; `git log -p | grep <pattern>` if unsure.
- **No `unsafe`** in Rust without a comment block explaining the invariant being upheld and what would break if violated.
- **Tauri commands are a trust boundary** — validate inputs (`Provider::parse_strict`, `PathBuf` canonicalization for path args). Don't `unwrap()` user-supplied strings.
- **`tauri.conf.json` asset scope** is allowlist-only; adding `$HOME/**` or similar is rejected. New providers add their specific subtree (`$HOME/.<provider>/**`).

### Anti-patterns

The reviewer rejects:

- **Stylistic-only refactors** mixed into a behavioral PR. Rename + behavior change land separately.
- **Heuristic substring scans** when a structured signal exists (see `find_uuids` removal — old code grepped UUIDs from message content; correct fix uses each provider's typed parent/child field).
- **`COALESCE(excluded.x, sessions.x)` for fields the parser is authoritative on** — only valid for genuinely back-filled fields where multiple sync passes converge.
- **Cross-provider parent_id links** — impossible under any real signal. If you see one in the DB, it's stale heuristic damage.
- **Sticky `is_sidechain = 1`** without a path that resets it. Once a session is wrongly flagged, it stays flagged.
- **"Open" / nav buttons without identity** — multi-target subagent UIs must label each button with what it opens (subagent prompt, child title, etc.), not just `#1 / #2 / #3`.
- **Adding a `Provider` variant without updating** `models.rs::Provider`, `provider.rs::PROVIDER_CATALOG` + `provider_entry` match, `tauri.conf.json` scope, `src/lib/types.ts`, `src/styles/variables.css`, and `src/stores/providerSnapshots.ts` fallback — the compile errors will list most but not all.

## Error Handling: No Silent Fallbacks

All code (Rust and TypeScript) must fail explicitly — never silently swallow errors or fall back to defaults that hide problems.

- **No plausible-but-wrong values**: Never substitute a "close enough" value when the correct one is unavailable. A wrong result that looks right is worse than no result. Concrete anti-patterns: using a parent/session-level value where a per-record value is needed (e.g. session timestamp instead of message timestamp); writing `None`/placeholder where a real value should be computed (e.g. `usage_hash: None`); non-deterministic iteration as a lookup fallback (e.g. `HashMap::iter().find_map()`); default values that mask missing data (`?? 0`, `unwrap_or_default()`). If the correct value cannot be obtained, log a warning and skip — do not fabricate a substitute.
- **Rust**: Propagate errors with `?` and context (`anyhow`/`fmt::Error`). Use `log::warn!`/`log::error!` when skipping a record, never silently return `None` or empty. `unwrap()` only in tests.
- **TypeScript**: Catch errors at boundaries (Tauri command calls, event handlers), surface via toast or `console.error`. Never use empty `catch {}` blocks — at minimum log the error. No `?? fallbackValue` that masks broken data.
- **Parsers**: When a JSONL line or JSON field is malformed, log a warning with file path and line context, then skip — do not silently produce partial/empty results.
- **UI**: When a backend call fails, show a toast or error state. Never render stale/empty data as if everything succeeded.
