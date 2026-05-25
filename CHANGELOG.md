# Changelog

All notable changes to this project will be documented in this file.

Format based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
versioned with [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.4.3] - 2026-05-25

### Changed

- Kimi provider now reads sessions from `~/.kimi-code/` (kimi-code 0.1.1 CLI replaces legacy kimi-cli); legacy `~/.kimi/` sessions require running `kimi migrate` to remain visible (9ee8aaa)

### Added

- Kimi subagents appear as separate sessions linked to their parent, with titles pulled from the spawning Agent tool's description (9ee8aaa)

### Fixed

- Kimi token usage is now tagged on the assistant text of each turn rather than the trailing tool result, so the model badge and cost render on the right message (9ee8aaa)
- Kimi auto-injected `<system-reminder>` permission banners no longer pollute the transcript or session title (9ee8aaa)

## [0.4.2] - 2026-05-21

### Fixed

- "Refresh usage" no longer wipes token statistics for unchanged sessions (06f4896)

## [0.3.29] - 2026-05-01

### Fixed

- Session token totals were undercounted for large sessions after the 0.3.28 streaming pagination change; totals now read from precomputed DB aggregates in SessionMeta (e449f19)

## [0.3.28] - 2026-05-01

### Fixed

- Windows UI no longer freezes when opening large Claude sessions (b3394f9)

### Changed

- Large sessions now stream the latest 300 messages first and page older entries on scroll, instead of loading the entire transcript up front (b3394f9)
- Persisted-output blocks in tool results resolve lazily on first view rather than during session load (b3394f9)
- Switching tabs during a long session parse cancels the in-flight load (b3394f9)

## [0.3.27] - 2026-05-01

### Fixed

- Kimi Bash tool output now displays stdout correctly instead of being hidden
- Kimi tool call arguments are now fully captured from streamed ToolCallPart events
- Kimi Edit tool diffs now render inline with before/after highlighting

### Added

- Per-message timestamps shown in message bubble footer

### Removed

- Session toolbar copy button due to performance issues

## [0.3.25] - 2026-04-28

### Fixed

- Parse newer Claude and Codex transcript events including agent names, PR links, away summaries, and scheduled task wakeups (3780c43)
- Stabilize in-session search match counter and prevent flicker while typing (3780c43)
- Speed up session refresh with selective per-provider sync and incremental Cursor transcript scanning (3e61e6f)

## [0.3.19] - 2026-04-21

### Fixed

- Claude token usage was severely undercounted when attached to tool_use or thinking-only turns, and synthetic placeholder entries were incorrectly included; totals now match ccusage exactly (2aa0800)

## [0.3.18] - 2026-04-21

### Fixed

- Restore Codex subagent indexing and "Open" button for multi_agents_v2 sessions (a913247)

## [0.3.17] - 2026-04-17

### Fixed

- Close race condition in live session watcher on rapid tab switches (a2dcb64)
- Surface backend command failures via toast instead of silent errors (c6871d2)
- Improve parser error messages with file path and line context (3928674)

### Changed

- Improve stability by removing panic sites and silent fallbacks in Rust backend (e43f6c2, a427860)

## [0.3.16] - 2026-04-16

### Added

- Timeline minimap with click and drag navigation for quick session browsing (096ca6b)

### Fixed

- Cache images during incremental source sync to prevent missing images (0381275)

## [0.3.15] - 2026-04-14

### Added

- Summon-style global search with trigram FTS across dialogue content (1cdbd21)

### Fixed

- Close correctness gaps in global search flow (61cdce0)
- Skip missing OpenCode DB and hide uninstalled providers in usage panel (7ed5d9d)

## [0.3.14] - 2026-04-14

### Fixed

- Pricing lookup now uses deterministic exact-match instead of non-deterministic HashMap iteration, fixing incorrect cost for variant models like gpt-5.4-fast (bd43ef2)

## [0.3.13] - 2026-04-14

### Fixed

- Atomic token stats writes and deterministic pricing lookup for accurate cost tracking (#16)
- Enriched tool metadata for Codex and OpenCode with structured result details (#17)
- Removed silent fallback paths that could mask parser and indexer errors (#17)

## [0.3.12] - 2026-04-14

### Added

- VSCode-style preview tabs: single-click opens italic preview, double-click pins (cc991a9)
- Tab overflow with chevron dropdown for crowded tab bars (cc991a9)

### Fixed

- Subagent Open button not navigating to child session (2f3d2b1)
- SessionView state not resetting on preview tab replacement (1906a92)
- Tab overflow detection not triggering on tab array changes (1906a92)

## [0.3.11] - 2026-04-14

### Added

- VSCode-style horizontal split view with drag & drop tabs, resizable split handles, and "Open to the Side" support
- Keyboard shortcuts for split view: split right, focus between groups
- Keyboard shortcuts overlay now shows split view bindings

### Fixed

- Provider chip counts now reflect the filtered date range instead of total session counts (65c7fca)
- OpenCode usage stats missing due to token usage not being extracted during scan (82c0848)
- Live watch no longer dies when session titles update mid-watch (4064b21)
- Split view resize accuracy: correct flex basis redistribution on group removal (aad4ff2)
- Tab pane height collapse when using display:contents (5919cb7)
- Text selection highlight no longer appears on draggable tabs (f290635)

## [0.3.10] - 2026-04-13

### Added

- Image cache persistence: session images are copied to `app_data_dir/images/` at index time and served as fallback when originals are deleted
- Status bar now shows today's token breakdown (↑input ↓output · cache read · cache write) and cost with color-coded highlights
- Usage panel gains a "Today" time range filter
- FTS5 tokenizer upgraded to `unicode61` with `tokenchars './_-'` for better code/path search
- Batch operation feedback: trash/restore/delete now return per-item success/failure counts
- Markdown export includes token usage summary table
- Global in-memory image cache deduplicates loads across messages

### Changed

- HTML export conditionally bundles KaTeX and Mermaid only when content uses them (~3.1MB saved)
- File watcher uses 500ms debounce window to batch change events and reduce redundant reindexing
- Explorer session lookup uses pre-built Map for O(1) instead of O(n²) tree traversal
- User bubble code blocks now follow light/dark theme correctly (previously always dark)

### Fixed

- KaTeX detection no longer false-positives on single `$` (shell vars, template literals)
- Light-mode copy button hover text is now visible (#1d1d1f instead of white)
- Image cache cleanup runs on all delete paths including empty-trash

## [0.3.9] - 2026-04-11

### Added

- Structured tool displays for Claude and Codex with line-level diffs in the timeline and HTML exports (#16)
- Codex `apply_patch` tool calls now render as diffs alongside Claude edits (#16)

### Fixed

- Hide usage-only assistant placeholders from the rendered timeline and exports (#16)
- Preserve Claude tool names and task status labels in merged tool rows (#16)
- Redact home paths in diff headers without breaking diff rendering (#16)
- Avoid duplicate Claude edit diffs when tool results echo the patch (#16)
- macOS release apps now raise the file descriptor soft limit before starting kqueue watchers, so live follow continues to work with large local session archives launched from Finder (51bcb4a)
- Usage filters now only apply to indexed providers (ae042ba)

## [0.3.8] - 2026-04-11

### Added

- GitHub Copilot provider support for local Copilot session-state transcripts
- Usage dashboard with provider filters, daily token/cost charts, per-model/project/session breakdowns, and previous-period trend summaries

### Changed

- Usage dashboard layout and chart controls were refined for clearer hierarchy and less visual clutter
- macOS builds now disable the app sandbox so local provider directories remain accessible after installation

### Fixed

- Daily usage chart interaction now uses a single hover detail surface and exposes pressed state for the token/cost toggle
- Usage aggregate query types now satisfy strict CI clippy checks


## [0.3.7] - 2026-04-09

### Added

- AST-based markdown rendering for message bubbles with GFM task lists, tables, footnotes, Mermaid, and KaTeX math
- Self-contained HTML export rendering for Mermaid diagrams and math formulas

### Changed

- Message bubble typography and colors realigned with the current in-app and HTML export themes
- Package metadata now uses the current project author and a provider-agnostic description

### Fixed

- Claude Code image markers now support the newer `[Image source: ...]` metadata format
- macOS file-backed providers now use `kqueue` for more reliable live-follow behavior
- HTML export now preserves markdown footnotes
- Codex local image placeholders now resolve correctly, including Windows path variants
- Resume command clipboard copy now works correctly in the context menu (5127bb2)
- Subagent "Open" link now appears in merged tool rows (203c7e4)

## [0.3.6] - 2026-04-04

### Added

- Qwen Code provider — parse `~/.qwen/projects/*/chats/*.jsonl` sessions (#13)
- Community standards: CODE_OF_CONDUCT, CONTRIBUTING, SECURITY, issue/PR templates

### Changed

- Provider brand colors realigned to official palettes: Claude terracotta orange, Kimi blue, Qwen blue-violet (#13)
- Hide subagent "Open" button for providers without separate session files (#13)

## [0.3.5] - 2026-04-03

### Added

- Locate active session button in Explorer header (#12)
- Collapse Explorer sidebar button (#12)
- Drag-to-resize Explorer sidebar (#12)
- Cache read/write token totals in session toolbar (#12)
- Shorten home directory paths in session toolbar (#12)
- Full markdown rendering in HTML export (#12)
- Provider-aware auto-polling for OpenCode and Gemini sessions (#12)
- Trash consistency audit at startup (#12)

### Fixed

- Prevent trashed OpenCode sessions from reappearing after reindex (#12)
- Escape code fence language in HTML export to prevent XSS (#12)
- Log and backup corrupt trash state files instead of silent fallback (#12)
- Fix protective sync for background polling (#12)

## [0.3.3] - 2026-04-03

### Fixed

- Show "up to date" feedback when no update is available (64944ae)
- Fix timer race where error timeout could overwrite subsequent update state (64944ae)
- Show detailed error message on download/install failure (64944ae)

## [0.3.2] - 2026-04-03

### Added

- Auto-check for updates on app startup with status bar badge (c7a72fc)

### Fixed

- Fix session image paths on Windows (8117bed)
- Fix updater signature file not generated in release builds (06bdfe6)
- Fix release workflow build paths and artifact downloads (#11)

## [0.3.1] - 2026-04-03

### Added

- Kimi CLI subagent support with tree navigation and "Open" jump (#10)
- Cursor CLI JSONL transcript parser, replacing old store.db approach (#10)
- Cursor CLI subagent support (#10)
- Trash lifecycle with parent-child session linking and cascading restore (#10)
- Agent "Open" button improvements with agentId matching and regex fallback (#10)

### Fixed

- Preserve Kimi subagent titles after trash by keeping subagents/ directory (#10)
- Fix Kimi tree view order instability (#10)
- Clean up Cursor store.db on permanent delete (#10)
- Strip `[REDACTED]` markers from Cursor assistant text (#10)
- Fix Cursor subagent title collision with full-text matching (#10)
- Restore child sessions reliably across all providers via parent_id (#10)
- Prevent accidental deletion of shared Gemini directories (#10)

## [0.3.0] - 2026-04-02

### Added

- Provider Bridge architecture with per-provider metadata descriptors (#9)
- Per-provider trash strategy replacing centralized if/else dispatch (#9)
- TypeScript provider registry replacing all switch/if-else blocks (#9)
- Claude, Codex, and OpenCode subagent session support (#8, #9)
- Per-message model display showing model name, version, and git branch (#8)
- Session metadata extraction (model, cc_version, git_branch) (#8)
- Parse `<persisted-output>` references in tool results (#8)
- Tab keep-alive preserving scroll position across tab switches (#8)
- Agent jump-to-subagent button in tool calls (#8)
- Orphan subagent management with show/hide toggle (#9)
- Ctrl+Click folder to select all child sessions (#9)
- Recent sessions filtering and metadata display (#9)

### Fixed

- Fix Cursor sessions being permanently deleted instead of trashed (#9)
- Fix Gemini shared log file reviving after trash (#9)
- Sanitize CC-Mirror variant name in resume command to prevent shell injection (#9)
- Wrap OpenCode delete in SQLite transaction (#9)
- Fix subagent tree rendering and deep reveal (#9)
- Fix scroll position restore on tab switch (#8)
- Reclaim disk space properly on clear index (#9)

### Removed

- `providers.ts` switch/if-else dispatch, replaced by provider-registry.ts (#9)

## [0.2.1] - 2026-03-30

### Added

- Windows custom titlebar with minimize/maximize/close buttons (#7)
- Search request versioning to discard stale results (#7)
- Export privacy redaction replacing home paths with `~` (#7)

### Fixed

- Fix CC-Mirror image loading by adding asset scope (#7)
- Fix provider disable filter to support all 7 providers (#7)
- Fix HTML export image detection for markers with trailing text (#7)
- Validate image paths against HOME/tmp allowlist before reading (#7)
- Skip physical deletion of shared SQLite files (Cursor/OpenCode) (#7)
- Use component-aware path validation instead of string prefix matching (#7)

## [0.2.0] - 2026-03-30

### Added

- CC-Mirror provider for multi-variant Claude Code sessions (#5)
- Parser golden tests for Gemini, Cursor CLI, and OpenCode (#6)

### Fixed

- Fix Windows terminal resume for CMD and Windows Terminal (#4)
- Wrap external SQLite connections with `PRAGMA query_only` (#2)
- Prevent full index deletion when provider scan returns 0 sessions (#2)
- Harden terminal command validation (#2)
- Add ErrorBoundary to prevent white screen crashes (#2)

## [0.1.5] - 2026-03-30

### Added

- Linux platform support with terminal auto-detection (#1)
- Linux release builds (.deb and .AppImage) (#1)
- Rust parser golden tests for Claude, Codex, and Kimi (4d97b2e)
- Async heavy commands (reindex, sync, batch delete/export) no longer block main thread (bb34012)
- HTML export with base64 image inlining (94dc93d)

### Fixed

- Fix Kimi incremental sync path detection (1485b2c)
- Fix Windows terminal detection and temp path validation (#1)
- Fix provider constructors panicking when HOME is unavailable (#1)

## [0.1.2] - 2026-03-29

### Added

- Kimi CLI provider with tool calls, thinking blocks, token usage, and images (f8d244f)
- Official brand SVG icons for all providers (3ce4710)
- ESLint + Prettier configuration (1e7d5b4)

### Fixed

- Fix recursive tree search breaking session operations when time grouping is enabled (e2f2a60)
- Fix CSS variable typo `var(--tab-hover)` → `var(--bg-tab-hover)` (e2f2a60)
- Tighten Mermaid security level from "loose" to "strict" (e2f2a60)
- Restrict markdown link schemes to http/https/mailto (e2f2a60)
- Skip destructive sync when scan returns <50% of indexed sessions (e2f2a60)

## [0.1.1] - 2026-03-29

### Added

- Blocked folders panel to exclude folders from session indexing (2247fe1)
- Auto-update support with Tauri updater plugin (2247fe1)

## [0.1.0] - 2026-03-28

### Added

- Multi-provider support: Claude Code, Codex, Gemini CLI, Cursor, OpenCode
- Full-text search across all session content (SQLite FTS5)
- Live session watch with auto-refresh on file changes
- Markdown rendering with syntax highlighting, Mermaid diagrams, and KaTeX math
- Inline image preview with click-to-expand
- Structured tool call display with diff view
- Token usage display (per-message and session totals)
- Collapsible thinking/reasoning blocks
- Export to JSON, Markdown, and HTML
- Session management: rename, trash/restore, favorites, batch operations
- Resume sessions in 7 terminal apps
- Keyboard shortcuts with overlay
- Light / Dark / System theme
- English / Chinese localization
- Window state persistence across restarts
