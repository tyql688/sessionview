# Usage and Cost

## Find the authoritative source

Usage may live on messages, separate events, shutdown summaries, a side database, or mutable metrics files. Determine whether values are per-call, per-turn, cumulative, cache-inclusive, branch-specific, or session-wide. Prefer live per-call rows over a final summary when both describe the same calls; never sum overlapping representations.

Normalize to disjoint components:

- uncached input;
- cache read input;
- cache creation/write input;
- output.

If the provider reports total input including cached tokens, subtract the cache components with saturating and evidence-backed rules. Do not assume every provider uses the same convention.

## Message usage versus `usage_events`

Attach `TokenUsage` to the owning visible assistant/tool message when the source permits an exact association and values fit the message model. Populate `ParsedSession::usage_events` when any of these apply:

- usage exists outside visible messages;
- display follows one branch but abandoned branches consumed real tokens;
- inline subagents require scope-specific attribution;
- timestamps/models are stored separately;
- provider-reported USD cost is available;
- stable hashes are needed for cross-file deduplication.

Once non-empty, `usage_events` are authoritative for totals and indexed stats. Therefore they must cover the intended accounting domain completely; do not mix a partial event list with message-only remainder.

Each usage event needs the real timestamp and model required for UTC 15-minute bucketing. Missing or ambiguous models/timestamps are counted warnings and skipped from stats unless an unambiguous typed session/call-level value exists. Empty model strings may preserve session totals but are skipped by stats; document and test that limitation rather than presenting it as complete cost data.

Use provider-reported `cost_usd` when present. Otherwise let the shared pricing catalog estimate cost; do not bake prices into a provider parser. Preserve stable `usage_hash` only when the provider exposes a real dedup key.

## Branches and subagents

Conversation display and accounting scope can differ. Active-branch UIs may hide abandoned messages while usage still includes calls that ran on those branches. Child calls belong to the executing child, not automatically to the parent. Cumulative streams sharing a message id must keep the authoritative maximum/final total rather than summing every update.

## Required tests

- exact four-component normalization and totals;
- cache-inclusive source converted to disjoint values;
- cumulative updates deduplicated correctly;
- multiple models and model changes;
- missing timestamp/model warning behavior;
- provider cost preferred over local pricing;
- abandoned branch still counted when appropriate;
- root/child usage attribution;
- usage-only assistant call;
- repeated stable hash across sessions;
- timestamp maps to the correct UTC bucket and viewer-timezone queries remain a read concern;
- `LoadedSession` totals equal indexed/session metadata totals for the same accounting domain.
