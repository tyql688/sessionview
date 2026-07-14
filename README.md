<p align="center">
  <a href="README.md">English</a> | <a href="README.zh-CN.md">中文</a>
</p>

<p align="center">
  <img src="assets/logo-text.svg" alt="SessionView" width="240">
</p>

<p align="center">
  <b>A local desktop workspace for reading, searching, analyzing, and resuming AI coding sessions.</b>
</p>

<p align="center">
  <a href="https://github.com/tyql688/sessionview/releases/latest"><img alt="Latest Release" src="https://img.shields.io/github/v/release/tyql688/sessionview?style=flat-square&color=blue"></a>
  <a href="https://github.com/tyql688/sessionview/actions/workflows/ci.yml"><img alt="CI" src="https://img.shields.io/github/actions/workflow/status/tyql688/sessionview/ci.yml?branch=master&style=flat-square"></a>
  <img alt="Platform" src="https://img.shields.io/badge/platform-macOS%20%7C%20Windows%20%7C%20Linux-lightgrey?style=flat-square">
  <a href="LICENSE"><img alt="License" src="https://img.shields.io/github/license/tyql688/sessionview?style=flat-square"></a>
</p>

<p align="center">
  <a href="assets/show.png"><img src="assets/show.png" alt="SessionView — session browser" width="860"></a>
</p>

---

## Why SessionView?

AI coding tools leave valuable context on your machine: decisions, patches, tool calls, costs, images, and unfinished threads. The problem is that every tool stores that history differently.

**SessionView turns those local histories into one fast desktop workspace.** Browse every supported tool in one explorer, read full sessions with rich rendering, search across your archive, inspect usage, export clean records, and resume work in the terminal when you need to continue.

Your data stays local. SessionView builds a local index for search and analytics; it does not upload your conversations.

## What you can do

- **Browse one workspace across tools**: Claude Code, Codex CLI, Antigravity, Kimi Code, Cursor CLI, OpenCode, CC-Mirror, Pi, and Grok Build.
- **Read sessions like documents**: Markdown, code blocks, Mermaid, KaTeX, inline images, reasoning blocks, and structured tool-call output.
- **Search without digging through folders**: global full-text search plus in-session find.
- **Understand the work**: token timelines, tool-call mix, context/cache pressure, cost trends, and model breakdowns.
- **Resume fast**: reopen a session in the matching terminal agent when the source tool supports it.
- **Keep history tidy**: rename, favorite, export, trash/restore, hide noisy folders, and manage batches.
- **Stay keyboard-friendly**: navigate tabs, panes, search, and common actions without leaving the keyboard.

## Session Analytics

Inspect a single session from the side panel: token timeline, tool-call mix, cache/context pressure, and quick workflow signals without leaving the conversation.

<p align="center">
  <a href="assets/session-analytics.png"><img src="assets/session-analytics.png" alt="SessionView session analytics side panel" width="860"></a>
</p>

## Usage Analytics

Track daily spend, model-level token totals, cache reads/writes, and provider trends from one dashboard.

<p align="center">
  <a href="assets/usage.png"><img src="assets/usage.png" alt="SessionView usage analytics" width="860"></a>
</p>

## Supported Tools

SessionView currently reads local history from Claude Code, Codex CLI, Antigravity, Kimi Code, Cursor CLI, OpenCode, CC-Mirror, Pi, and Grok Build.

When a tool exposes enough information, SessionView can also resume the selected session in the matching terminal agent. CC-Mirror follows its configured variant. Parsing depth depends on what each tool records locally, but SessionView normalizes messages, tool calls, reasoning/thinking blocks, token usage, images, and child sessions wherever the source data exposes them.

## Install

Grab the latest build from [**Releases**](https://github.com/tyql688/sessionview/releases):

| Platform | File |
|----------|------|
| macOS | `.dmg` |
| Windows | `.exe` (NSIS installer) |
| Linux | `.deb` / `.AppImage` |

> **macOS Gatekeeper:** depending on the release certificate available for a build, macOS may block the first launch. If that happens, clear the quarantine flag:
>
> ```bash
> xattr -cr "/Applications/SessionView.app"
> ```

## Quick Start

1. Install and open SessionView
2. Let it index your local tool histories
3. Browse a session, search across your history, or resume right where you left off

## Build From Source

Requires [Rust](https://rustup.rs/) and [Node.js](https://nodejs.org/) 22.12+.

```bash
git clone https://github.com/tyql688/sessionview.git
cd sessionview
npm install
npm run tauri build              # Production build
npx tauri build --bundles dmg    # DMG only
```

## Development

```bash
npm run tauri dev                # Dev with hot reload
npm run check                    # Type-check + Biome + ESLint (frontend)
npm test                         # Frontend tests (Vitest)
cd src-tauri && cargo fmt --check
cd src-tauri && cargo test       # Rust tests
cd src-tauri && cargo clippy --all-targets --all-features -- -D warnings
npm run knip                     # Release dead-code/dependency audit
```

Code style is documented in [`style/ts.md`](style/ts.md) and [`style/rust.md`](style/rust.md), enforced by Biome, ESLint, Clippy, Knip, and lefthook. Knip runs as a release gate rather than a per-push hook.

## Built With

- [Tauri 2](https://v2.tauri.app/) — desktop shell and native integrations
- [React 19](https://react.dev/) — frontend UI with React Compiler
- [Rust](https://www.rust-lang.org/) — provider parsing, indexing, export, and session lifecycle
- [SQLite](https://www.sqlite.org/) + FTS5 — local storage and full-text search
- [Vitest](https://vitest.dev/), [Biome](https://biomejs.dev/), [ESLint](https://eslint.org/), and [Clippy](https://doc.rust-lang.org/clippy/) — testing and code quality

## License

[MIT](LICENSE) © tyql688
