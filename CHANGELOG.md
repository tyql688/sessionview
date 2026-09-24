# Changelog

## [0.8.3] - Unreleased

### Changed

- In-session search counts every match in the session's user and assistant messages without loading the whole session into the timeline. Stepping through matches loads the window around each one and centers the match itself, also inside long messages. Queries typed while the session's text is still loading share one request.
- Global search covers the same user and assistant messages as in-session search; tool output and thinking are not indexed. OpenCode sessions index all of their text parts.
- The index database stores each session's search text zstd-compressed, which shrinks the file, and compacts itself when rewritten sessions have grown it. The first launch converts the existing database in place and re-indexes every session once. Earlier SessionView versions, including an older `npx sessionview` still in the npm cache, cannot use the converted database: indexing and search fail with `unknown function: session_content_text()`. To go back to an earlier version, quit SessionView and delete `sessions.db`, `sessions.db-wal`, and `sessions.db-shm` from `~/.sessionview` (or from the directory passed to `--data-dir`). The earlier version rebuilds its index on the next launch; favorites and renamed titles are stored in that database and are lost.

### Fixed

- A large session is parsed once while it opens: the background parse, the minimap outline, message windows, and in-session search share a single parse, and a parse that finishes is kept even when the request that started it was canceled. On a 1.8 GB Codex session, opening it and searching during the load peaks at 1.7 GB of memory instead of 3.3 GB, and typing a query during the load at 1.4 GB instead of 5.8 GB.
- Codex `WebSearch` completion records are parsed as web searches with their query, actions, and results.
- Pi sessions parse current records: system-prompt snapshots, usage entries such as cache warming (counted toward usage), and context edits are recognized, and an entry that fails to parse keeps the rest of its branch linked. Compaction and branch-summary usage is attributed to the model in effect at that point in the tree; usage without an attributable model is skipped with a parse warning. Extension JSONL files without a session header are skipped, and a session file copied into several project directories is indexed once, from its most recently modified copy.
- The timeline stays still when rows below the viewport resize or rows change height during a rubber-band bounce at either edge, and expanding a thinking block keeps its header in place.
- Replies that quote `<system-reminder>` in their text are shown; a message is hidden as injected content only when a system reminder opens it.
- Terminal tool output that starts with a brace is shown as-is without console warnings.

## [0.8.2] - 2026-09-08

### Fixed

- Codex histories retained after a revert now resolve the referenced physical rollout file while preserving the logical session identity, byte and ordinal boundaries, and duplicate protection. Existing Codex indexes refresh automatically to recover previously unreadable history.
- Codex image-generation completion events retain their prompt and saved image even when the log has no preceding call record. Repeated events and completed-item mirrors merge into one tool entry. Completed `clock.sleep` records retain their duration; malformed or unknown records still raise parse warnings.
- Image caching skips directories, including placeholder paths such as `...` that Windows can resolve to the current directory, preventing failed image-copy warnings.

### Changed

- Provider icons use official static LobeHub SVGs, removing unrelated UI and emoji dependencies, React 19 peer conflicts, and deprecated packages. Dependency patches address known npm audit findings, and the reviewed Lefthook install script is explicitly allowed for newer npm versions.
- SVG transforms are limited to SVG imports, and production React Compiler transforms skip unused source maps. The plugin timing advisory is disabled for these intentional transforms; runtime warning reporting remains enabled.

## [0.8.1] - 2026-09-07

### Fixed

- Current Kimi usage records are paired per model step without double-counting their fallback records; interruptions, retries, and profile metadata are retained. DSH agent presets and Grok hook/plugin bookkeeping are recognized, and session toolbars show the provider's agent or profile when available.
- Pi messages containing unpaired UTF-16 surrogate escapes retain their readable content with replacement characters instead of dropping the entire record; other malformed JSON still raises a parse warning.
- Refreshing model prices now recalculates historical usage before reporting success, with per-provider pricing revisions so unchanged files and interrupted refreshes are retried safely. Catalog writes are atomic and existing statistics survive refresh failures.
- Pi client-calculated zero costs no longer override available model rates. Cost coverage distinguishes estimates, service-reported amounts (including zero), and fully or partially unpriced usage; explicit free model rates are retained, and cached model aliases resolve deterministically without borrowing prices from a different model version or paid tier.
- Codex desktop rollouts now retain nested command executions, file changes, MCP and dynamic tools, image previews/generation, web searches, agent activity, reasoning, and goal updates from current structured records. Code-mode `exec` is displayed separately from shell commands; mirrored assistant messages and tool outputs are deduplicated by their recorded ids, and command/MCP failures keep their error status.
- Codex usage emitted after `task_started` but before `turn_context` is attributed to the new turn and explicitly applied model settings. Existing Codex indexes refresh once when the parser changes, so previously unchanged logs gain the corrected transcript, search text, tool counts, and usage without clearing favorites or existing data first.
- Codex repeated cumulative token snapshots no longer inflate usage when re-emitted at a later timestamp. New response records, cache components, equal-sized requests with advancing totals, and usage after compaction remain counted; existing Codex statistics are rebuilt automatically.
- Fresh Codex subagents retain their first model response's usage. Parent replay is skipped only when an explicit fork or inherited session metadata identifies it, rather than assuming every spawned agent starts with replayed usage.
- Codex paginated history now resolves `history_base` using its thread identity, byte boundary, and ordinal boundary, retaining the referenced prefix before reading the continuation. Physical segments no longer overwrite each other's session and token statistics during full and incremental scans; missing or ambiguous history fails explicitly, and loading or renaming a continued session uses its logical thread identity.

## [0.8.0] - 2026-09-02

### Added

- Command Code sessions: the provider reads the append-only v3 transcript tree under `~/.commandcode/projects/<project>/<session-id>.jsonl` together with its mutable `.meta.json` sidecar. The timeline follows the active last-leaf `parentId` chain after rewind/branch operations, while usage and provider-reported USD cost include every assistant call that actually ran across all branches. Text, thinking, images, tool calls/results, compaction summaries, visible mod messages, model changes, renamed sessions, and git-branch metadata are preserved. Typed `agent` calls become limited inline children (`<session>:<tool-call-id>`) that can be opened from the parent, including foreground results and background `agent_output` waits. Command Code does not persist the child agent's internal trace, model, or disjoint usage, so SessionView leaves those unknown fields empty instead of inventing them. Fork/clone lineage is not misclassified as a subagent, malformed records surface through the parse-warning badge, and sessions resume through `commandcode --session <id>`.

- GitHub Copilot CLI sessions: the provider reads `$COPILOT_HOME/session-state/<uuid>/events.jsonl` (`~/.copilot` by default), mutable `workspace.yaml`, and `session-store.db` plus its WAL as one freshness graph. User turns use the wire's `content`, never the system-context-wrapped `transformedContent`; assistant reasoning stays out of the transcript; `tool.execution_start` / `tool.execution_complete` pair into one Tool message by `toolCallId`. Image attachments resolve only from complete typed `session.binary_asset` records with an explicit MIME type; malformed or unresolved attachments retain useful sibling text, render an honest attachment marker, and raise a parse warning. `task` subagents run inline in the parent log; their events are routed by `parentToolCallId` into child sessions (`<session>:<toolCallId>`, titled by `agentDisplayName`, agent type as `variant_name`), and the parent's Agent tool message links to them so "Open subagent" works. Usage prefers `session-store.db`'s per-call `assistant_usage_events` (timestamped, per model, subagent calls attributed to their child) and falls back to `session.shutdown.modelMetrics`; unknown child scopes warn and are skipped instead of being charged to the root. Both sources are cache-inclusive and normalised to disjoint input / cache-read / cache-write. Under auto mode the model is refined from `assistant.message.model` (the selection itself is the literal `auto`). Resumes through `copilot --resume <id>`.

- MiniMax Code (mcode) sessions: the provider reads the SQLite index, non-empty WAL, every session `manifest.json`, and referenced `messages.jsonl` under `~/.minimax/v2/` (or `$MINIMAX_DATA_DIR/v2`, with `$MAVIS_DATA_DIR` as the legacy fallback) as one incremental freshness graph. User prompts use the wire's `canonicalTextRange` so the injected `<system-reminder>` block is not treated as the user turn. Assistant thinking, prose, and tool calls stay in wire order; results merge by `toolCallId`, while orphan results remain standalone Tool messages with structured status instead of becoming system prose. Images require both an explicit MIME type and payload; malformed blocks raise parse warnings without dropping valid sibling text. Unknown visible content is counted, known siblings remain visible, and unknown tool-result payloads are preserved as raw output. Per-turn `usage` blobs become authoritative `UsageEvent` rows; missing model/timestamp fields warn while totals remain preserved. Resumes through `mcode --session <id>`. The provider filters by `runtime = 'pi-agent'`, hides `origin = 'root-repair'` scaffolding, and uses `parent_session_id` as the typed subagent signal. A `task` tool result carries `details.sub_session_id` (also in `<task_result session_id>`); that becomes the child's Agent `agentId` so "Open subagent" works. `agent_name` (mavis / explore / worker / verifier) is stored as `variant_name`. Model prefers `extra_data_json.effectiveModel` and falls back to the first assistant turn's `message.model`.

### Fixed

- OpenCode: after the v2 `workspace_domain` migration the `workspace` table no longer has a `branch` column, so every scan failed with `no such column: w.branch` and the provider was skipped. The branch lookup now gates on the column, and the session simply has no branch when it is absent.

## [0.7.8] - 2026-08-17

### Fixed

- DSH subagent sessions now take their display title from the parent-chosen
  delegation label (`subagent/descriptor`), instead of the five-word fallback
  title every delegated sibling shared.
- Forked/resumed DSH sessions are no longer misclassified as sidechains:
  `parentSession` alone is seed lineage; only `origin: "subagent"` marks a
  delegated child.
- An interrupted DSH step keeps its streamed token usage: the step's usage
  chunk is buffered and folded into the stats at flush, matching the
  assembled-message path.

### Changed

- DSH subagent traffic renders as tagged collapsible system rows (subagent
  report / subagent finished) instead of raw "Background subagent <uuid> …"
  boilerplate walls.
- The collapsed thinking preview (all providers) prefers a bold `**Title**`
  lead and hard-caps an untitled first line at 80 characters.

## [0.7.7] - 2026-08-17

### Added

- DSH (DeepSeek Harness) sessions: the provider reads the zstd-compressed
  JSONL event logs under `$DSH_HOME/sessions` (default `~/.dsh/sessions`),
  surfaces the ordered conversation (user prompts, assistant text and
  reasoning, tool calls with merged results), links subagent sessions to
  their parents via the session header, and resumes through
  `dsh --profile tui --resume <id>`.
- DSH compaction is honored: `surfaceOp: replace` checkpoints splice the
  condensed summary into the transcript in place of the shadowed history,
  while shadowed token usage stays in the stats (matching how DSH's own
  usage collector folds the log) and the search index keeps the old text.
- DSH robustness: interrupted streams are reconstructed from their chunk
  rows, a torn tail is ignored the same way DSH's own scanner does, and
  retry/compaction/subagent-descriptor rows are recognized log-only events.

## [0.7.6] - 2026-07-24

### Added

- Grok Build sessions surface the newer on-disk data: backend web/X
  searches render as tool calls (web search lists its sources; X search
  shows an honest empty result since Grok never persists X hits), image
  generation results preview inline, plan / goal / recap updates appear
  as timeline notes, forked subagent sessions link to their parent, and
  sessions pick up their git branch and active agent name.
- Session cost prefers the provider's own reported USD (Grok, Pi) over
  the models.dev estimate, Grok reasoning tokens count toward output
  totals, and served model ids like `grok-4.5-build` resolve pricing
  through their base model without rewriting what's stored.

### Fixed

- Grok tool results carry real error status from the update stream, and
  successful edits render as diffs even when the transcript only stores
  a bare success string.

## [0.7.5] - 2026-07-22

### Added

- The headless web UI now works on phones. Narrow viewports get a
  single-pane layout: a centered bottom navigation bar, full-width session
  list and reader, stacked settings sections, and an activity calendar that
  keeps readable cells behind a horizontal scroller opened on the newest
  weeks. Touch devices get long-press context menus on tabs and the session
  tree, always-visible tab close buttons, and split view stays desktop-only.
- The compact session view trims its chrome: the meta strip collapses to one
  swipeable line and role-filter chips scroll instead of wrapping.

### Fixed

- Claude Code compaction summaries render as the collapsed "context
  compacted" row instead of a plain user bubble, and teammate messages from
  other Claude sessions surface as agent-mail system rows with the
  model-facing boilerplate stripped.

## [0.7.4] - 2026-07-21

### Added

- Tool results follow one presentation contract across every provider:
  readable output is the default, Bash gets the terminal view, successful
  edits render as diffs, and unknown wire shapes are preserved verbatim in a
  marked raw view instead of being dropped or guessed at.
- Structured result media (connector screenshots, MCP images) is extracted by
  the backend and rendered inline in tool output.
- Parsers surface what they skip: unknown record types now count toward the
  session's parse-warning badge, Kimi runtime context (steering input, task
  notifications, skills, compaction summaries) renders as system messages,
  Claude model-fallback events are shown, and OpenCode file attachments stay
  visible.
- Codex agent-team sessions recover tool calls that only exist as lifecycle
  events (connector MCP calls, desktop patch applies) and re-attribute
  subagent token usage to the file's actual model.

### Fixed

- Expanding a tool, terminal, or system block keeps the clicked header in
  place — details open downward instead of pushing the header up.
- Replayed fork bursts in Codex rollouts no longer double-count the parent
  session's token usage, and usage survives files whose fork markers never
  fire.
- The image coordinate-scale note Claude Code injects after downsizing a
  screenshot no longer renders as a user message.
- Grok raw-result verdicts survive status-only tool call updates.

### Changed

- `[turn_duration]` renders as a hairline divider and `[away_summary]`
  collapses to a flat label that expands on demand.
- Time separators disappear in focus mode, and separator runs left by hidden
  roles collapse to a single marker.

## [0.7.3] - 2026-07-20

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
