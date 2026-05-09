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
  providers/               # claude/, codex/, gemini/, kimi/, opencode/, qwen/, cc_mirror.rs
  commands/                # sessions.rs, settings.rs, trash.rs, terminal.rs
  services/                # provider_snapshots.rs, session_lifecycle.rs, session_resolution.rs, source_sync.rs, image_cache.rs
  exporter/                # json.rs, markdown.rs, html.rs, templates.rs
  db/                      # mod.rs, queries.rs, sync.rs, row_mapper.rs
  indexer.rs  watcher.rs  models.rs  provider.rs  provider_utils.rs  trash_state.rs
src/stores/               # editorGroups, settings, search, selection, providerSnapshots, updater, favorites, toast, theme
src/lib/                  # tauri.ts, provider-watch.ts, formatters, tree-builders, icons, image-cache.ts
src/styles/               # variables.css (theme tokens), layout.css, components.css, messages.css, usage.css
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
| Gemini      | `~/.gemini/tmp/*/chats/*.json`         | JSON   | Poll  |
| Kimi CLI    | `~/.kimi/sessions/**/wire.jsonl`       | JSONL  | FS    |
| OpenCode    | `~/.local/share/opencode/opencode.db`  | SQLite | Poll  |
| Qwen Code   | `~/.qwen/projects/*/chats/*.jsonl`     | JSONL  | FS    |
| CC-Mirror   | `~/.cc-mirror/{variant}/config/projects/**/*.jsonl` | JSONL | FS |

Tool names mapped to canonical set per provider: {Bash, Edit, Read, Write, Glob, Grep, Agent, Plan}.
Resume: Claude `--resume`, Codex `resume`, Gemini `--resume`, Kimi `--session`, OpenCode `-s`, Qwen `--resume`.

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
- **Subagents**: `parent_id` links children; "Open" button for providers with separate files (Claude, Codex, Kimi, CC-Mirror)
- **Provider snapshots**: backend derives provider label/color/order/watch strategy/path info; frontend consumes via `providerSnapshots` store
- **Trash**: `TrashMeta.parent_id` cascades restore/delete; `is_session_dir()` prevents shared dir deletion
- **Immutable state**: All Solid.js store updates use spread (`{ ...prev, field: newValue }`). Never mutate in place.
- **Solid.js reactivity**: Use `Index` (not `For`) for tab panes to preserve component instances across reorders. Use `<Show when={id} keyed>` when component must remount on identity change.

## Pitfalls

- **OpenCode**: Must use `SQLITE_OPEN_READ_WRITE` (not READ_ONLY) for WAL. Uses XDG path, not macOS `~/Library/`.
- **macOS watchers**: File-backed providers use `notify` with `macos_kqueue` for more reliable file-level follow behavior; do not assume `FSEvents`.
- **Codex**: `call_id` pairing, output can be nested JSON.
- **Kimi**: MD5 project path, event stream format, float-second timestamps, truncated parallel agent args.
- **CC-Mirror**: Multi-variant under `~/.cc-mirror/`, sanitized variant names.
- **Qwen**: `sanitizeCwd()` path (hyphens, not SHA256). `thought: true` boolean + `text` field. Subagents embedded in parent (no separate files). Skip `ui_telemetry`/`slash_command`/`at_command`/`chat_compression`.
- **compact_string**: Rust `compact_string(s, limit)` truncates with `…` suffix. Do NOT use truncated summaries for matching/comparison — always extract full values from source JSON.
- **Session ID vs agentId**: Claude subagent files are `agent-{id}.jsonl`, so session ID = `agent-{id}` but tool result `agentId` = `{id}` (no prefix). Always match both forms.

## Conventions

- Rust: `cargo fmt` + `cargo clippy` before commit
- TypeScript: strict mode, no `any`, ESLint + Prettier
- Commits: conventional commits (`feat:`, `fix:`, `refactor:`)
- i18n: all user-facing strings via `t()`
- Colors: Claude `#d97757`, Codex `#10b981`, Gemini `#f59e0b`, OpenCode `#06b6d4`, Kimi `#1783ff`, CC-Mirror `#f472b6`, Qwen `#6c3cf5`

## Error Handling: No Silent Fallbacks

All code (Rust and TypeScript) must fail explicitly — never silently swallow errors or fall back to defaults that hide problems.

- **No plausible-but-wrong values**: Never substitute a "close enough" value when the correct one is unavailable. A wrong result that looks right is worse than no result. Concrete anti-patterns: using a parent/session-level value where a per-record value is needed (e.g. session timestamp instead of message timestamp); writing `None`/placeholder where a real value should be computed (e.g. `usage_hash: None`); non-deterministic iteration as a lookup fallback (e.g. `HashMap::iter().find_map()`); default values that mask missing data (`?? 0`, `unwrap_or_default()`). If the correct value cannot be obtained, log a warning and skip — do not fabricate a substitute.
- **Rust**: Propagate errors with `?` and context (`anyhow`/`fmt::Error`). Use `log::warn!`/`log::error!` when skipping a record, never silently return `None` or empty. `unwrap()` only in tests.
- **TypeScript**: Catch errors at boundaries (Tauri command calls, event handlers), surface via toast or `console.error`. Never use empty `catch {}` blocks — at minimum log the error. No `?? fallbackValue` that masks broken data.
- **Parsers**: When a JSONL line or JSON field is malformed, log a warning with file path and line context, then skip — do not silently produce partial/empty results.
- **UI**: When a backend call fails, show a toast or error state. Never render stale/empty data as if everything succeeded.
