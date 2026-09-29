---
name: add-sessionview-provider
description: Add, extend, audit, or repair a local coding-session provider in SessionView by proactively reading real local sessions and the actual current implementation when source is available, then integrating discovery, Rust, React, indexing, subagents, tools, images, usage, resume behavior, and isolated runtime validation end to end.
---

# Add a SessionView Provider

Treat a provider as a normalization and indexing boundary, not just a parser. A complete integration discovers current source artifacts, maps them into SessionView's model without invented values, connects every backend/frontend boundary, and proves the result against synthetic and real sessions.

The AI performs the investigation directly with ordinary read-only tools. Do not create persistent discovery, parsing, or validation scripts as a substitute for reading actual sessions and source. Add a helper script only when the user explicitly asks for one or a repeated deterministic transformation genuinely requires it; scripts are never the default provider workflow.

Read `AGENTS.md`, `style/rust.md`, and `style/ts.md` first. Then read:

- [Source research](references/source-research.md) before designing or changing a parser.
- [Evidence and work modes](references/evidence-and-modes.md) to establish authority, output, and proof status.
- [Provider contract](references/provider-contract.md) for every provider.
- [Discovery and freshness](references/discovery-and-freshness.md) when locating files, databases, sidecars, assets, or incremental state.
- [Subagents](references/subagents.md) when the tool delegates work, stores child logs, or exposes agent/task calls.
- [Messages, tools, and images](references/messages-tools-images.md) when mapping content blocks, tool calls/results, reasoning, attachments, or generated media.
- [Usage and cost](references/usage-and-cost.md) whenever the source records tokens, cache traffic, model calls, or provider cost.
- [Controlled live-session generation](references/live-session-generation.md) when the existing local corpus does not exercise a source capability that must be verified.
- [Cross-layer checklist](references/cross-layer-checklist.md) before declaring implementation complete.
- [Real-data validation](references/real-data-validation.md) before final handoff.

## Workflow

1. Select add, extend, audit, or repair mode and write the evidence/acceptance matrix from [Evidence and work modes](references/evidence-and-modes.md). Audit is read-only unless the user also requests fixes.
2. Establish the current product identity, CLI executable, installed version, source roots, resume syntax, wire/schema version, and mutable sidecars. Read representative real local sessions directly. Do not infer a product from a misspelling when local evidence can resolve it.
3. Classify the official repository correctly. An issue tracker, binary distribution repository, documentation repository, or package shell is not an open-source implementation. When implementation source exists, read the writer and reader code, types, migrations, subagents, tools, images/assets, usage/cost, branching/compaction, and CLI resume, reconciling installed and repository versions. Otherwise use the closed-source fallback and inspect installed bundles/types/schema; documentation alone is insufficient.
4. Choose the closest existing SessionView providers by storage and behavior; complex integrations usually need more than one reference provider. Record which target-source fields authoritatively supply identity, ordering, title, project, model, parent/child links, tools, images, and usage.
5. Build read-only discovery and parsing around provider-specific typed signals. Preserve valid partial sessions, count malformed or unknown visible records, and log exact context. Never scan message text to invent structural relationships.
6. Normalize into `ParsedSession` and `Message`, then implement `load_messages` against the same identity rules. Add incremental freshness only after the complete source graph is mapped and every mutable input has its own mutation regression.
7. Connect all backend, frontend, security, documentation, and resume surfaces. Use exhaustive Rust matches and `Record<Provider, ...>` maps to expose missing work; do not weaken them with wildcard/default branches. Follow the substantial-provider directory layout and file-size limit in [Provider contract](references/provider-contract.md).
8. Verify synthetic edge cases, multiple representative real sessions, sanitized parse coverage, isolated index/command round trips, GUI/headless gates, and the running UI where visible behavior changed. When the existing real corpus lacks a required capability, report it as unobserved or generate the smallest controlled persistent session when authorized; never treat absence as proof of unsupported behavior.

## Real evidence and fixtures

The AI must inspect the actual configured source tree and open representative sessions itself. Start with structure, but read the minimum real values needed to prove relationships such as parent/child ids, tool call/result pairing, asset references, branch ancestry, model changes, and cumulative usage. Do not stop at documentation or filenames.

Existing history is the first evidence source, not always the last. If it does not contain a required behavior and running the target CLI is authorized and proportionate, read [Controlled live-session generation](references/live-session-generation.md), then create a synthetic-content real session with the target itself. Prefer a bounded persistent print/headless mode; use an isolated tmux TTY only for interactive-only behavior. Verify the resulting on-disk records directly. A prompt that merely asks the agent to use a feature is not evidence unless the typed artifacts prove it happened.

If the project is open source, source research is mandatory even when real sessions appear self-explanatory. The code that persists sessions is authoritative for optional fields, version transitions, delayed sidecars, and meanings that cannot be inferred safely from samples.

Real content stays local and must not be copied into fixtures, patches, commentary, or final answers. Fixtures must use synthetic ids, users, paths, prompts, models, and image payloads. Do not copy a real transcript and merely replace its obvious title.

## Completion contract

Do not call the provider complete until it can discover, index, list, load, search, account for, export, and resume representative real structurally valid sessions to the extent the source supports those capabilities. Report each capability using the evidence states in [Evidence and work modes](references/evidence-and-modes.md), and distinguish source evidence, synthetic fixtures, static gates, a real read-only smoke, an isolated running index round trip, and visible UI validation. Completion also requires every provider file to stay within the repository limit, touched Markdown prose (including changelog entries) to remain one physical line per paragraph, and the final scoped diff to contain no real ids, paths, prompts, payloads, or other private artifacts.

When materially changing this skill, forward-test it against at least two existing providers with different storage graphs—for example one JSONL plus sidecars/assets provider and one database plus WAL/manifest provider. Record which checklist gaps the exercise found; a validator proving only frontmatter and links is not semantic validation.
