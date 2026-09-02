# Real-Data Validation

Keep real-data work read-only. Never add real session content, ids, usernames, paths, tokens, credentials, or images to source control or normal test output.

## Evidence pass

Collect and reconcile:

1. official current storage/session and CLI documentation;
2. installed CLI version and `--help`/resume syntax;
3. for open-source targets, the official repository's matching version plus current relevant source, tests, types, and migrations;
4. installed source, bundles, type definitions, migrations, or schema declarations;
5. direct inspection of multiple representative local sessions and their sidecars/assets;
6. comparison across old, new, branched, tool-heavy, image-bearing, and child-agent sessions when available.
7. controlled CLI-generated sessions for required capabilities absent from the existing corpus, when authorized; prove the behavior from the newly written source artifacts rather than terminal prose.

Read [Source research](source-research.md) and perform the inspection directly. Start with structure, then follow enough real values to prove typed relationships and visibility semantics. For compressed, protobuf, encrypted, or proprietary binary sources, read the installed decoder/types and use an appropriate read-only decoder; do not run the target tool to rewrite real history.

Read [Controlled live-session generation](live-session-generation.md) before invoking the target agent. Live generation can consume credits, contact remote services, execute tools, and mutate local history; existing read-only inspection does not imply authorization for those effects.

## Synthetic regression layer

Create minimal synthetic fixtures or temp trees that pin every observed semantic variant:

- discovery and exclusions;
- metadata precedence and clears;
- ordering/branching/compaction;
- malformed line and unknown schema behavior;
- message, reasoning, tools, images, and raw results;
- subagent graph and nested/background routing;
- usage/cost normalization;
- incremental transcript and sidecar changes;
- `load_messages` identity and resume command.

Use golden files when wire shape is large, but keep values unmistakably synthetic.

## Ignored real-data smoke

Add a read-only `#[ignore]` test that scans the actual configured root and asserts structural invariants across representative sessions: non-empty ids/source paths, expected provider, loadable sessions, message counts, parent consistency, images/tools when present, and totals consistency. It must not print message content or require real data during normal `cargo test`.

The ignored smoke must not print real ids, paths, titles, parent links, prompts, message previews, image payloads, or failure objects that embed those values, even with output capture disabled. Assertions should report invariant names only. The cross-provider coverage audit may print provider keys, aggregate counts, and static logger targets/reason codes; it must not print raw warning text because warnings commonly contain paths, ids, or record values.

Run the provider smoke explicitly, then audit parser coverage:

```bash
cd src-tauri
cargo test <provider_real_smoke_name> -- --ignored
cargo test --test parse_coverage_real_audit -- --ignored --nocapture
```

Run the provider smoke without `--nocapture` unless its output has been proven aggregate-only; the smoke should normally emit nothing. Use `--nocapture` for the sanitized coverage audit so aggregate counts and static targets are visible.

Treat warnings as evidence. Classify distinct unknown record/content types and add support or a deliberate documented exclusion. A smoke test that merely finds one file does not prove message, image, child, or usage coverage.

Do not store a raw audit transcript in the repository. If a local diagnostic temporarily needs private warning text, inspect it directly on the machine, keep it out of model commentary/final output, and remove the exact temporary artifact after deriving sanitized reason categories.

## Index and command round trip

Use a temporary SessionView data directory so runtime verification does not mutate the user's primary index:

1. build `dist/` and the headless feature;
2. start the headless server on a free loopback port with `--data-dir <temporary-dir>`;
3. call `get_provider_snapshots` and confirm the provider exists/path status;
4. call `reindex_providers` for only the new provider;
5. confirm session count, list a session without printing private fields, load detail, inspect structural counts, usage presence, child links, and parse warnings;
6. call `get_resume_command` and validate only its shape/prefix;
7. stop the server and move the temporary directory to trash or otherwise remove it safely.

For a provider with child sessions or images, open representative root/child/image-bearing sessions in the running UI. Verify parent navigation, “Open subagent”, image display, tool details, active branch, settings filter, analytics color/label, and light/dark themes. A production build or API response is not visual validation.

## Final gates

Run the current commands from `AGENTS.md`; at minimum:

```bash
npm run check
npm test
npm run knip
npm run build

cd src-tauri
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
cargo test --no-default-features --features headless
```

Finish with `git diff --check`, a scoped status/diff review, and explicit separation of uncommitted changes, local commits, pushed refs, built artifacts, real-data smoke, runtime API validation, and visible UI validation.
