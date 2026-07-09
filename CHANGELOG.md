# Changelog

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
