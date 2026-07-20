# Changelog

## [0.7.3] - Unreleased

### Changed

- Restyled the main layout as floating chrome cards: the explorer, editor
  area, settings, usage view and status bar now render as rounded cards with
  a shared gap over the window background.
- The title bar only renders in the desktop (Tauri) runtime, so the headless
  browser shell gets a clean edge-to-edge layout.

## [0.7.2] - 2026-07-20

### Fixed

- `npx sessionview` platform binaries now publish under the
  `@echo0321/sessionview-<platform>` scope. The registry's spam filter had
  rejected the unscoped `sessionview-win32-x64` name since 0.7.0, which also
  blocked the 0.7.1 launcher package from publishing at all.

## [0.7.1] - 2026-07-20

### Added

- Usage statistics follow the viewer's timezone: totals, daily charts, the
  activity calendar, and today's cost fold the shared index into civil days
  for each client's IANA zone, so a remote headless viewer gets its own day
  boundaries.
- The headless invoke API rejects unknown argument keys instead of silently
  ignoring them, so a typo like `range_days` fails loudly rather than running
  an unfiltered query.

### Changed

- Rust 2024 edition with a pinned stable toolchain, plus dependency updates
  (rusqlite 0.40, zip 8, infer 0.22, sha2 0.11, KaTeX 0.18).

## [0.7.0] - 2026-07-18

### Added

- Headless mode: `npx sessionview-headless` serves the full SessionView UI in
  a browser on port 9921. Same Rust core, same frontend, and the same SQLite
  index/data dir as the desktop app — nothing is re-indexed or duplicated.
  Backend commands travel over `POST /api/invoke/{command}`, backend events
  over SSE; exports become browser downloads; localhost-only by default with
  optional `--token` auth for remote access.

## [0.6.3] - 2026-07-15

### Fixed

- Improved Mermaid diagrams with theme-aware rendering and reliable source
  copying.
- Removed SessionView's legacy Windows app-data directory when users select
  "Delete application data" during uninstall, without deleting it during app
  updates.

## [0.6.2] - 2026-07-14

### Removed

- Session deletion and trash/restore functionality across the frontend, IPC
  boundary, provider runtimes, and backend lifecycle services.

## [0.6.1] - 2026-07-11

### Added

- Codex 0.144 multi-agent sessions: nested subagents keep their hierarchy in
  the tree at any depth, spawn/send tool rows link straight to the child
  session, reasoning renders as collapsible thinking blocks, and inter-agent
  mail shows its readable routing header.
- Session-wide role counts in the filter toolbar — fixed numbers for the whole
  session instead of counts that grew while scrolling.
- Common keyboard shortcuts: Cmd+B toggles the sidebar, Cmd+D toggles
  favorite, Cmd+Shift+T reopens closed tabs, Cmd+G / Cmd+Shift+G step through
  search matches, Cmd+P opens global search.

### Changed

- Subagents now collapse under their parent session by default; the chevron
  expands them level by level.
- Keyboard shortcut hints render with platform-correct modifier order from a
  single manifest shared by the overlay and Settings (#20).

### Fixed

- Rebuilt the session timeline on a bottom-anchored (column-reverse) scroller:
  fast scrolling no longer blanks or teleports at the top edge, bubbles no
  longer reflow after opening, and loading history lands without freezing the
  frame. Scroll anchoring is now handled explicitly for WKWebView.
- Cmd+Backspace typed inside a text field no longer opens the session-delete
  confirm, and single-letter shortcuts work with CapsLock on.

## [0.6.0] - 2026-07-09

0.6.0 is a major refresh of SessionView: a new React-based desktop UI, stronger
session reading tools, and a more reliable indexing/sync pipeline.

### Highlights

- Rebuilt the app with React 19, zustand, react-i18next, Base UI primitives, and React Compiler.
- Added VS Code-style navigation with Activity Bar, split editor groups, preview/pinned tabs, and tab overflow.
- Reworked session reading with focus mode, improved message/tool/diff rendering, minimap navigation, and smoother large-session performance.
- Improved usage and search workflows with accessible date-range controls, heatmaps, and consistent provider metadata.
- Made indexing and source sync safer: fewer stale loads, verified deletes only, faster unchanged refreshes, and lower SQLite lock contention.
- Added file reveal actions and refreshed frontend documentation/style guidance.
