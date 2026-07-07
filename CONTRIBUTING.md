# Contributing to CC Session

Thanks for your interest in contributing! This guide will help you get started.

## Development Setup

### Prerequisites

- [Node.js](https://nodejs.org/) (v22.12+)
- [Rust](https://rustup.rs/) (stable)
- [Tauri CLI](https://v2.tauri.app/start/prerequisites/)

### Getting Started

```bash
git clone https://github.com/tyql688/cc-session.git
cd cc-session
npm install
npm run tauri dev
```

### Useful Commands

```bash
npm run tauri dev             # Dev with hot reload
npm run tauri build           # Production build
cd src-tauri && cargo test    # Rust tests
npm test                      # Frontend tests
cd src-tauri && cargo clippy  # Rust lint
npm run typecheck             # TypeScript type check
npm run lint                  # Biome formatter/linter/import-sort check + ESLint
npm run format                # Biome format write
npm run format:check          # Biome format-only check
npm run knip                  # Release dead-code/dependency audit
```

## How to Contribute

### Reporting Bugs

1. Search [existing issues](https://github.com/tyql688/cc-session/issues) first
2. Open a new issue using the **Bug Report** template
3. Include: OS, app version, steps to reproduce, expected vs actual behavior

### Suggesting Features

1. Open an issue using the **Feature Request** template
2. Describe the use case and why it would be useful

### Submitting Code

1. Fork the repository
2. Create a feature branch (`git checkout -b feat/my-feature`)
3. Make your changes
4. Ensure all checks pass:
   ```bash
   cd src-tauri && cargo fmt --check && cargo clippy && cargo test
   npm run check && npm test
   ```
   Run `npm run knip` before releases or broad frontend refactors.
5. Commit using [conventional commits](https://www.conventionalcommits.org/) (`feat:`, `fix:`, `refactor:`, etc.)
6. Open a pull request against `master`

## Code Style

- **Rust**: `cargo fmt` + `cargo clippy` — no warnings
- **TypeScript**: strict mode, no `any`, formatted with Biome
- **Commits**: conventional commits format
- **i18n**: all user-facing strings via `t()`, never hardcoded

## Project Structure

- `src/` — React frontend
- `src-tauri/src/` — Rust backend (providers, commands, database, exporter)
- See [CLAUDE.md](CLAUDE.md) for detailed layout
