# Changelog

## [0.6.0] - Unreleased

0.6.0 is the React release of CC Session. The frontend has been rebuilt around
React 19, zustand, react-i18next, Base UI primitives, and React Compiler, with a
stronger session viewer, cleaner navigation, and a more reliable indexing path.

### Added

- React 19 app shell with Activity Bar, split editor groups, preview tabs,
  pinned tabs, tab overflow, and VS Code-style session opening behavior.
- Focus mode for session reading, with user and assistant messages emphasized by
  default.
- Virtualized session timeline for very large conversations, including smoother
  far jumps, stable row measurements, and minimap navigation that survives huge
  sessions.
- Calendar date picker and accessible controls for usage heatmaps and date-range
  filtering.
- Shared UI primitives for buttons, dialogs, menus, selects, toggles, tooltips,
  and toasts.
- File-link actions that reveal referenced files in the platform file manager.

### Changed

- Migrated frontend state to zustand stores with reactive hooks for components
  and imperative getters/actions for non-React callers.
- Migrated localization to react-i18next and kept all user-facing strings behind
  i18n keys.
- Reworked the session viewer layout, message bubbles, markdown rendering,
  collapsible system notices, tool cards, diff presentation, and toolbar
  controls for the React UI.
- Replaced the old live watch path with explicit sync/indexing flows that avoid
  unnecessary background churn.
- Derived provider display metadata from a single backend snapshot source so
  labels, colors, ordering, and path information stay consistent.
- Updated frontend documentation and style guidance for React, Biome, ESLint,
  and React Compiler.

### Fixed

- Session loading now orders concurrent requests by client sequence and retries
  canceled current opens, preventing stale loads from winning after rapid tab
  switches.
- Source sync only deletes sessions when their source file is verifiably gone,
  avoiding data loss from empty or partial provider scans.
- Usage refresh no longer clears global stats or rewrites FTS rows for unchanged
  content, making refreshes faster and less disruptive.
- SQLite now uses a busy timeout and keeps pricing writes off the async runtime,
  reducing lock contention during indexing and refresh work.
- Windows path normalization and provider path checks handle verbatim paths more
  reliably.
