# Source Research

Research is part of implementation, not a preliminary optional step. Do not design a provider from marketing documentation or one transcript sample.

## Resolve the exact product and version

Use the installed executable, package metadata, symlink target, version output, CLI help, and default data directory to establish the canonical product name and installed version. Resolve ambiguous spelling from evidence. Record environment overrides and platform-specific roots.

Determine whether the target implementation is actually open source. Check CLI/package metadata, licenses, published package contents, and official product links rather than trusting an unrelated repository with a similar name. Classify an official repository that contains only issues, documentation, release wrappers, install scripts, or binary assets as distribution/support evidence—not implementation source—and follow the closed-source fallback for persistence semantics.

## Open-source targets

Locate the official repository and read current primary source. If it is already installed or checked out locally, inspect that code first; otherwise browse or fetch the official repository as permitted. Compare the installed version with its matching tag/commit and with the current default branch when they differ.

Search and read the code that defines or performs:

- session/message/content wire types and schema version constants;
- session creation, append/update, branch/rewind, compaction, fork/clone, and deletion;
- file/database paths, environment overrides, migrations, WAL/sidecars, and asset storage;
- tool call/result ids and canonical tool names;
- subagent spawn, event ownership, parent/child metadata, nested/background execution, and resume;
- image/attachment serialization, binary asset ids, local cache paths, URLs, and MIME handling;
- usage counters, cache semantics, cumulative versus per-call values, cost, models, and timestamps;
- CLI session selection/resume arguments and accepted id/path forms.

Read implementations, types, tests, and migrations—not only READMEs. Search from writer code to reader code so field meaning is proven in both directions. Treat source comments as claims to verify against code and tests.

When the installed version and repository main differ, support only formats backed by evidence. Add compatibility branches for observed versions with fixtures; do not invent forward compatibility. State which versions were inspected.

## Real local sessions

Inspect the actual source root directly and select representative artifacts rather than only the newest or smallest file:

- basic dialogue;
- multiple tools and interleaved results;
- image or attachment messages;
- subagent parent and child, including nested/background runs when present;
- branch/rewind/compaction/fork/resume;
- model change and usage-bearing calls;
- live/incomplete and malformed sessions;
- older and current schema versions;
- sidecars, indexes, databases/WAL, manifests, and asset directories.

Begin with directory layout, file signatures, JSON keys/types, SQLite schema, and discriminator counts. Then read the minimum real records and values needed to follow typed links and ordering. It is acceptable—and often necessary—to read real prompts or results locally to distinguish visible user content from injected context, but never reproduce them outside the inspection.

Use ordinary read-only tools appropriate to the format (`rg`, `jq`, `sqlite3` schema/select queries, `file`, archive/decompression readers, or the target's installed decoder). Avoid running commands that migrate, compact, resume, or rewrite the user's real history.

If the installed corpus does not exercise a required capability, absence is inconclusive. When live generation is authorized, follow [Controlled live-session generation](live-session-generation.md): use the target CLI to create a minimal persistent session containing synthetic content, then inspect the new artifact structurally. Prefer a bounded non-interactive/print mode when it writes the normal session format; use tmux only for behavior that genuinely requires a TTY, such as interactive branch, rewind, or slash-command flows.

Do not claim support for a feature merely because its key exists. Trace at least one real complete relationship: call → result, parent → child, asset id → image, branch leaf → ancestors, or usage event → model/timestamp/message.

## Evidence map

Before coding, maintain a working evidence map with these columns:

| Concern | Real artifact evidence | Target source/type evidence | SessionView normalization |
| --- | --- | --- | --- |
| identity and source root | observed header/path | path/session constructor | `SessionMeta.id`, `source_path` |
| ordering/branching | linked real entries | append/branch reader | displayed message order |
| subagents | real typed parent/child ids | spawn/router types | child sessions and `agentId` |
| images | real asset/source link | serializer/cache code | canonical image marker |
| usage | real counters and scope | accounting code | disjoint `UsageEvent`s |

The map can stay in working notes; do not add real values to the repository. Every non-trivial parser rule should be traceable to target source, a real artifact, or both.

Label each capability as confirmed persisted, writer-supported but unobserved, confirmed absent, remote-only, or unknown using [Evidence and work modes](evidence-and-modes.md). Absence from the selected corpus is never enough to mark a capability unsupported.

## Closed-source fallback

For a closed-source tool—or a product whose public repository is not its implementation—inspect installed bundles, source maps, type declarations, migrations, CLI help, and multiple real artifacts. If field semantics remain ambiguous, warn and skip them. Clearly report that source-level verification was unavailable; do not convert guesses into compatibility code.
