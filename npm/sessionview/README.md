# sessionview

Headless server for [SessionView](https://github.com/tyql688/sessionview) —
browse your local AI coding sessions (Claude Code, Codex, Antigravity, Kimi
Code, Cursor, OpenCode, CC-Mirror, Pi, Grok Build) from any browser.

```bash
npx sessionview            # http://127.0.0.1:9921
npx sessionview --open     # …and open the browser
```

The server shares its index (SQLite in `~/.sessionview`) with the SessionView
desktop app, so sessions indexed by either are instantly visible in both. The
launcher checks for newer releases on startup and downloads them automatically
(fail-soft: offline it runs the installed version).

## Options

| Flag | Default | Description |
| --- | --- | --- |
| `--port <port>` | `9921` | Port to listen on |
| `--host <addr>` | `127.0.0.1` | Bind address; non-loopback requires `--token` |
| `--token <secret>` | — | Require this token on every API request (`SESSIONVIEW_TOKEN` env also works) |
| `--data-dir <dir>` | `~/.sessionview` | Override the data directory |
| `--open` | — | Open the browser after startup |

## How the binary is resolved

1. Newest released version (npm registry check; skipped when offline).
2. Platform package installed as an optional dependency (`sessionview-<os>-<arch>`).
3. Per-user cache (`~/.cache/sessionview/bin/<version>`).
4. Downloaded from the matching GitHub release, sha256-verified.
