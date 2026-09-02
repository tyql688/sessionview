# Discovery and Freshness

## Map the complete source graph

Before implementing discovery, inventory:

- default roots on each supported OS and any documented environment override;
- one-file-per-session transcripts, shared databases, indexes, manifests, WAL files, compressed logs, and nested child-session directories;
- mutable title/model/workspace sidecars;
- asset stores referenced by messages;
- checkpoint, prompt-cache, telemetry, or summary files that must not be indexed as sessions.

Confirm layouts from current installed source/types and real schema-only inspection. Documentation may lag installed builds. Keep test constructors such as `with_root` so synthetic trees do not depend on the developer's home directory.

Before coding, write a freshness matrix with one row for every mutable input: canonical source path, contribution to `ParsedSession`, change signal, combined-fingerprint rule, and a synthetic mutation test. Include shared databases, non-empty WALs, manifests, referenced JSONL, mutable titles/workspaces/models, child-link metadata, and any asset metadata that changes indexed or displayed output. If an input is intentionally excluded, prove that changing it cannot alter the normalized session.

## Discovery rules

- Return only canonical session sources. Exclude sidecars even when they share `.jsonl` or another transcript-looking extension.
- Use stable provider ids rather than filenames when both exist.
- Sort discovered paths or outputs where deterministic order improves tests.
- Treat permission errors and damaged directory entries as warnings; do not make one unreadable child hide every valid session.
- Validate provider strings and paths at the command/security boundary. Asset scope must remain allowlist-based.

## Incremental freshness

Every mutable input that changes a `ParsedSession` must participate in freshness. Examples include transcript, title sidecar, workspace metadata, model index, manifest, SQLite WAL, child-link metadata, and generated summary files.

Use the shared `(size, mtime)` helper only for a true single-file source. For multi-file sources, compute and compare a provider-specific combined state with sufficient timestamp resolution. A naive sum-of-sizes plus maximum-mtime can miss a same-length rewrite when another component already has a newer timestamp; fold each component's identity, presence, size, and high-resolution mtime into a deterministic fingerprint or use an equivalently complete scheme. The exact values written into every `ParsedSession.meta.file_size_bytes` and `source_mtime` must be the values `scan_incremental` compares. Use `SourceState.title` only when an external title index can change without a reliable content fingerprint and the current database title is safe to compare.

`scan_incremental` must return matching sources in `unchanged_source_paths`; otherwise aggressive synchronization may delete valid rows that were merely skipped. Test all of these transitions:

1. first scan parses the source;
2. unchanged scan short-circuits;
3. transcript append reparses;
4. sidecar-only rename/model/project change reparses;
5. WAL-only or manifest-only change reparses when applicable;
6. removed source is pruned only under the intended aggressive scan;
7. a transient empty/non-aggressive scan does not erase the provider snapshot.

Add one explicit regression per applicable matrix row rather than one broad test that happens to touch several files. For shared-source providers, also prove that a changed shared database or WAL reparses every dependent row and that unchanged accounting preserves all root and child sessions.

For one source that yields multiple sessions, verify that unchanged-source and deletion accounting keeps both root and children alive.

## Schema evolution

Record the supported schema/wire versions. Add an explicit compatibility branch only when real evidence and fixtures demonstrate both formats. For a newer unknown version, log the observed version and skip instead of silently applying an older parser. A database provider should inspect table/column presence rather than assume migrations ran uniformly on every machine.

Unknown visible record/content variants must increment parse coverage or be handled by an exhaustive, documented exclusion. Catch-all deserialization that silently drops a new visible block is not forward compatibility. Preserve unknown tool-result payloads as raw output when safe; do not dump unknown hidden/system records into the transcript.
