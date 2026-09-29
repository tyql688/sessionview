# Cross-Layer Checklist

Review every applicable item against the current tree. Search exhaustively for existing provider keys, `Provider` matches, typed frontend records, theme tokens, and user-facing provider lists rather than relying on a remembered file list.

## Backend identity and runtime

- `src-tauri/src/models.rs`: exhaustive `Provider` variant and exact serialized key.
- `src-tauri/src/providers/mod.rs`: module exported.
- `src-tauri/src/provider/catalog.rs`: builder, catalog entry, key, display label, descriptor, sort order, and array/match parity.
- provider descriptor: resume command, display key, color, CLI executable whitelist value, variant parsing when dynamic.
- provider runtime: source roots, `scan_all`, incremental scan, exact `load_messages`, and test-only root constructor.
- `src-tauri/src/commands/session_tail.rs`: explicit tail support or explicit full-load choice.
- `src-tauri/src/services/provider_snapshots.rs`: exhaustive order test.
- `src-tauri/tauri.conf.json`: narrow source/image asset scope.

If the provider needs a new backend command or changes a command signature, update all four command surfaces: transport-agnostic core, GUI wrapper plus handler registration, headless dispatch allowlist/match, and `BackendCommandMap` in `src/lib/tauri.ts`.

## Frontend

- `src/lib/types.ts`: exact wire key in the provider tuple.
- `src/stores/settings.ts`: persisted-provider validation.
- `src/stores/providerSnapshots.ts`: complete fallback snapshot.
- `src/stores/providerSnapshots.test.ts`: expected order/key.
- `src/components/icons.tsx`: provider icon with suitable light/dark visibility.
- `src/styles/variables.css`: light and dark provider tokens.
- `src/styles/theme.css`: Tailwind/theme bridge.
- i18n English/Chinese parity for any new user-facing strings not supplied by provider snapshots.
- search, usage filters, analytics legends, export metadata, session toolbar, and resume UI work without provider-specific fallbacks.

## Documentation and security

- `README.md`, `README.zh-CN.md`, `AGENTS.md`, and the active `CHANGELOG.md` list the provider when release-visible.
- Touched Markdown keeps each prose paragraph and changelog bullet on one physical line; lists, tables, code fences, and headings retain their natural structure.
- Synthetic fixtures contain no real ids, usernames, paths, prompts, image payloads, or proprietary session content.
- Resume arguments cannot escape the command boundary. Validate provider ids/variants and use the descriptor's CLI command allowlist.
- Asset scope exposes only documented provider roots needed for rendered local files.

Finish with compiler-enforced exhaustive matches, frontend type checking, focused searches for the new key/variant, and a manual diff review. Static presence still does not validate storage discovery, parser correctness, subagent graph semantics, image contents, usage accounting, runtime indexing, or UI appearance.

Also review the provider directory against [Provider contract](provider-contract.md): no oversized parser/module, tests separated from orchestration for substantial providers, and no empty abstraction layers.
