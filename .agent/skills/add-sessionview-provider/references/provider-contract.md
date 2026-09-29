# Provider Contract

Read the live types before writing provider code:

- `src-tauri/src/provider/traits.rs`: `ProviderDescriptor` and `SessionProvider`.
- `src-tauri/src/provider/state.rs`: `SourceState`, `ScanOutcome`, `ParsedSession`, and `LoadedSession`.
- `src-tauri/src/provider/tokens.rs`: authoritative usage events and token-stat bucketing.
- `src-tauri/src/models.rs`: `Provider`, `SessionMeta`, `Message`, roles, usage, and tool metadata.
- `src-tauri/src/provider/catalog.rs`: provider identity, runtime construction, labels, ordering, colors, and resume security metadata.

Do not copy one provider wholesale. Select references by behavior:

| Source behavior | Useful references |
| --- | --- |
| Append-only JSONL and structured content blocks | `claude`, `codex`, `commandcode` |
| Active-leaf conversation tree or rewinds | `commandcode`, `pi` |
| Inline subagents and binary assets | `copilot` |
| Separate child directories, sidecars, and generated assets | `grok`, `claude` |
| Multiple native/legacy layouts | `kimi`, `cursor` |
| SQLite plus WAL freshness | `opencode` |
| Compressed transcripts | `dsh` |
| Manifest/index plus message files | `mcode` |

## Identity and metadata

Resolve each `SessionMeta` field from an authoritative source:

- `id`: use the provider's stable session id. If child sessions need synthetic ids, define a collision-free, deterministic composition and make `load_messages` understand it.
- `provider`: the new exhaustive `Provider` variant.
- `title`: explicit current title first, then a provider-defined semantic fallback such as the first visible user turn. A cleared title must not resurrect stale sidecar text.
- `project_path` and `project_name`: use structured cwd/workspace metadata. Do not decode folder slugs unless the encoding is documented or verified.
- `created_at` and `updated_at`: typed timestamps only. Reject a file or skip a damaged entry rather than fabricate epoch zero or filesystem time as conversation time unless that fallback is the provider's documented contract.
- `model`, version, and git branch: preserve explicit values and absence. Model changes must follow the same branch/scope as the message or usage they describe.
- parent/child fields: follow [Subagents](subagents.md); fork, clone, branch, and resume lineage are not automatically subagents.
- token totals: derive from the same normalized source used by indexed stats.

`content_text` must contain exactly the normalized searchable content for the session. Include visible dialogue, reasoning when SessionView surfaces it, and useful tool content; do not index hidden system dumps, abandoned branches, or private provider scaffolding that is deliberately omitted from display.

## Parser shape

Document the wire layout and non-obvious decisions as module doc-comments next to the parser. Separate concerns when the source is substantial:

- wire types and schema versions;
- file/database discovery;
- ordering, branching, and scope routing;
- message/tool/image conversion;
- usage normalization;
- synthetic regression tests.

Use this repository layout as the default for a substantial provider, omitting only files whose concern genuinely does not exist:

```text
src-tauri/src/providers/<provider-key>/
├── mod.rs                 # descriptor, discovery, scan/load contract, provider-level tests
├── types.rs               # deserialized wire and sidecar types
├── parser.rs              # orchestration, identity, ordering, metadata, usage ownership
└── parser/
    ├── messages.rs        # visible messages, tools, reasoning, images/assets
    ├── subagents.rs       # typed parent/child routing and inline-child construction
    └── tests.rs           # synthetic parser regressions

src-tauri/tests/fixtures/<provider-key>/
└── ...                    # synthetic cross-file fixtures only when integration tests need them
```

A small provider may keep `types`, message conversion, and tests beside `mod.rs` or `parser.rs`; do not create empty layers. Conversely, split a growing parser by semantic ownership before it exceeds the Rust style limit. Provider registration and frontend files remain in the cross-layer paths listed in [Cross-layer checklist](cross-layer-checklist.md), not inside this folder.

Keep Rust files under the repository's line limit. Preserve unknown structured tool results as raw output, but do not dump unknown hidden/system records into the visible transcript merely to avoid data loss.

Directory structure is part of completion, not optional cleanup. Move wire types, message/tool/image mapping, subagent routing, and synthetic tests into the semantic files above before any file crosses the enforced line limit; do not hide an oversized provider behind lint allowances.

## Scan and load agreement

`scan_all` and `scan_incremental` return indexed `ParsedSession`s. `load_messages(session_id, source_path)` must reconstruct the exact requested session, validate that the source still represents that id, and return its parse warning count and authoritative totals. Test mismatched ids, missing sources, child ids, and stale source paths.

A single source file may yield root plus child sessions. In that case, parsing, source liveness, warning ownership, and child-specific loading must be deliberate rather than relying on the default one-file/one-session assumption.

## Failure semantics

- File-wide unreadable/unsupported data: warn with the source and skip or return a provider error.
- Per-record damage: increment `parse_warning_count`, log file plus record/line context, and keep remaining valid content browseable.
- Unknown schema version: reject or explicitly enter a tested compatibility path. Do not optimistically parse a newer version as the current one.
- Missing structural values: warn and skip the affected entity. Do not insert plausible placeholders into parent links, timestamps, models, paths, or usage.
- Unknown visible content: preserve known siblings, increment `parse_warning_count`, and either preserve raw tool-result output or skip the unknown block deliberately. A silent `#[serde(other)]` branch is incomplete coverage.
