# Subagents

Subagent support is a typed graph problem. Never infer it by searching prompts, assistant prose, tool output, or directory names that merely look agent-related unless the provider documents that directory structure as the typed signal.

A writer may establish the child with a typed call/start id but omit that id from one persisted child block, such as the delegated opening prompt. After the child already exists from typed evidence, a writer-verified unique correlation may attach that otherwise unscoped block; it must not create the child, and zero or multiple matches must stay unassigned/root with a counted warning. Document this evidence-limited exception and test ambiguity explicitly.

## Establish the source semantics

Determine which of these layouts the provider uses:

- separate child transcript with parent metadata in its header or path;
- parent transcript listing stable child ids;
- inline child events scoped by an agent/tool-call id;
- child directory plus parent-side link metadata;
- task/agent tool call whose result exposes a runtime id that must map back to a stable call id.

Distinguish delegated execution from session fork, clone, resume, alternate branch, compaction, and ordinary tool calls. If the source exposes lineage but not delegated ownership, leave SessionView's subagent fields empty.

A semantic subagent does not require a physically separate child transcript. A typed agent/task invocation plus its typed completion result is authoritative evidence of delegated execution even when the producer persists only those two blocks in the parent log. In that case, first prove what is and is not stored, then represent only the recoverable child boundary. Never turn “no child file” into “no subagent.”

## Normalized graph contract

For a real child session:

- create an independently loadable `ParsedSession`;
- assign a stable, collision-free id; for inline children, compose it from the root/parent id and typed child call id, including every nesting level;
- set child `parent_id`, `is_sidechain = true`, and an explicit `variant_name`/agent type when available;
- inherit project metadata only when the provider's execution model makes that correct;
- add the child's id to the direct parent's `child_session_ids` when the parent emits the typed link;
- enrich the parent's Agent tool message with the same stable child identifier as structured `agentId`, so the frontend's “Open subagent” action resolves the child;
- attribute messages, timestamps, model, tools, warnings, and usage to the owning scope.

When persistence is partial, a limited normalized child may contain only the typed delegated prompt and typed final result. Derive its stable id from the parent/root id plus the provider's call id, document that the internal trace is unavailable, and leave unknown model or disjoint usage empty. Do not copy the parent's aggregate usage into every child, parse runtime ids from prose unless the provider defines that result envelope, or fabricate intermediate child messages.

For inline logs, keep maps for typed child id → accumulator, tool call → owning session, and runtime alias → stable child id when needed. Opening/closing brackets alone are often insufficient: synchronous completion may be interleaved, and background children may run while new parent messages arrive.

`load_messages` must select the requested root or child from the shared source. Do not return the root just because the child uses the same `source_path`.

## Usage and lifecycle

Route every model call to the session that executed it. Parent totals must not silently absorb child usage unless the provider reports only an inseparable aggregate and the limitation is documented. Nested children attach to the child that spawned them, not automatically to the root.

Do not require a completed event to index a live child if typed start/message data already makes it valid. Torn or reordered events should produce counted warnings and deterministic ownership; an unknown child scope must not be silently guessed.

## Required tests

- parent plus one child, including database backfill/linking;
- parent Agent tool message opens the same child id;
- separate-file or inline `load_messages` round trip;
- nested child ownership and ids;
- parallel calls emitted in one parent turn keep distinct ids and results;
- synchronous and background launch/result-fetch flows when supported;
- orphan/unknown child id behavior and warning count;
- incomplete live child;
- child title/type/model precedence;
- usage attributed to root versus each child;
- fork/clone/resume lineage remains non-subagent;
- aggressive and incremental scans keep child rows alive.

Use `copilot` as the main inline-routing example, `grok` for parent/child sidecars and generated assets, `claude` for path-owned child transcripts, and `kimi` for native subagent state.
