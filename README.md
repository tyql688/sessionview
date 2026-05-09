<p align="center">
  <a href="README.md">English</a> | <a href="README.zh-CN.md">中文</a>
</p>

<p align="center">
  <img src="assets/logo-text.svg" alt="CC Session" width="240">
</p>

<p align="center">
  Browse, search, resume and manage your AI coding sessions in one desktop app.
</p>

<p align="center">
  <a href="https://github.com/tyql688/cc-session/releases/latest"><img alt="Latest Release" src="https://img.shields.io/github/v/release/tyql688/cc-session?style=flat-square&color=blue"></a>
  <a href="https://github.com/tyql688/cc-session/actions/workflows/ci.yml"><img alt="CI" src="https://img.shields.io/github/actions/workflow/status/tyql688/cc-session/ci.yml?branch=master&style=flat-square"></a>
  <img alt="Platform" src="https://img.shields.io/badge/platform-macOS%20%7C%20Windows%20%7C%20Linux-lightgrey?style=flat-square">
  <a href="LICENSE"><img alt="License" src="https://img.shields.io/github/license/tyql688/cc-session?style=flat-square"></a>
</p>

---

## Why CC Session?

AI coding tools like Claude Code, Codex, Gemini CLI, and Qwen Code store session data locally, but there's no easy way to browse, search, or revisit past conversations. CC Session brings all your sessions together in one unified interface — view full conversation histories, search across all providers with full-text search, export records, and resume any session directly in your terminal.

> **One app for all your local coding sessions**
>
> Browse history, search across providers, restore deleted sessions, export clean archives, and jump back into a session with one click.

## Features

- **Unified view** — All your AI coding sessions from multiple providers in one place
- **Full-text search** — Search across all session content with SQLite FTS5
- **Resume sessions** — Jump back into any session in your terminal
- **Live watch** — File-based providers auto-refresh via OS watchers; Gemini and OpenCode use provider-aware polling
- **Rich rendering** — Markdown, syntax highlighting, Mermaid diagrams, KaTeX math, inline images, structured tool call diffs
- **Token usage** — Per-message and session-level token counts with cache hit/write breakdown
- **Export** — JSON, Markdown, or self-contained HTML (dark mode, collapsible tools & thinking blocks)
- **Session management** — Rename, trash/restore, favorites, batch operations
- **Auto-update** — Built-in updater checks for new releases automatically
- **Keyboard-friendly** — Fast navigation and actions without leaving the keyboard
- **i18n** — English / Chinese
- **Blocked folders** — Hide sessions from specific project directories

## Supported Tools

CC Session currently supports:

- Claude Code
- Codex CLI
- Gemini CLI
- Kimi CLI
- OpenCode
- Qwen Code
- CC-Mirror

Across providers, CC Session parses messages, tool calls, thinking/reasoning blocks, token usage, inline images, Markdown, Mermaid diagrams, and KaTeX math where the source format supports them.

## Install

Download the latest release from [Releases](https://github.com/tyql688/cc-session/releases):

- **macOS** — `.dmg`
- **Windows** — `.exe` (NSIS installer)
- **Linux** — `.deb` / `.AppImage`

> **macOS Gatekeeper:** The app is not code-signed. On first launch, macOS may block it. Fix with:
>
> ```bash
> xattr -cr "/Applications/CC Session.app"
> ```

## Quick Start

1. Install and open CC Session
2. Let it index supported local provider data
3. Open a session, search across history, or resume where you left off

## Build from Source

Requires [Rust](https://rustup.rs/) and [Node.js](https://nodejs.org/) 18+.

```bash
git clone https://github.com/tyql688/cc-session.git
cd cc-session
npm install
npm run tauri build              # Production build
npx tauri build --bundles dmg    # DMG only
```

## Development

```bash
npm run tauri dev                # Dev with hot reload
npm test                         # Frontend tests
npx tsc --noEmit                 # Type-check frontend
cd src-tauri && cargo test       # Rust tests
cd src-tauri && cargo clippy     # Lint Rust
```

On macOS, file-based live watch uses the `notify` crate's `kqueue` backend for more reliable file-level updates.

## Built With

- [Tauri 2](https://v2.tauri.app/) for the desktop shell and native integrations
- [SolidJS](https://www.solidjs.com/) for the frontend UI
- [Rust](https://www.rust-lang.org/) for provider parsing, indexing, export, and session lifecycle management
- [SQLite](https://www.sqlite.org/) + FTS5 for local storage and full-text search
- [Vitest](https://vitest.dev/), [ESLint](https://eslint.org/), [Prettier](https://prettier.io/), and [Clippy](https://doc.rust-lang.org/clippy/) for testing and code quality

## License

[MIT](LICENSE)
