# Messages, Tools, and Images

## Message ordering and visibility

Preserve the provider's semantic order, including interleaved text, reasoning, tool calls, results, and images. For a tree-shaped transcript, display only the active typed parent chain; account for executed abandoned branches separately when usage requires it.

Map visible roles deliberately:

- user and assistant dialogue stay in their roles;
- surfaced reasoning/thinking uses the repository's established System-message convention;
- internal system prompts, synthetic context injection, hidden metadata, and provider bookkeeping stay out of the transcript;
- compaction/branch summaries are included only when they are user-meaningful and tagged consistently;
- an assistant turn with usage but no visible content may need a placeholder for display accounting, following the closest provider's established behavior.

Keep timestamps and models on the specific message or tool call they belong to. Do not smear the last model across unrelated branches or child scopes.

## Tool calls and results

Pair calls/results by the provider's structured call id using shared helpers such as `ToolCallPairer`. Build metadata through `build_tool_metadata` and enrich results through `enrich_tool_metadata`; do not create provider-specific canonical-name registries.

Preserve:

- raw tool name and canonical name;
- structured input without lossy field guessing;
- call id and any typed agent/session id;
- result status/error state;
- complete structured result where useful;
- unknown result shapes as raw output.

An orphan result should become a deterministic standalone Tool message or a counted warning according to the source semantics. Never attach it to the nearest tool call by position when a call id is available. Test repeated names, interleaving, missing results, results-before-calls, MCP names, agent calls, file patches, shell aliases, and oversized/persisted outputs as applicable.

## Canonical image contract

Every provider emits images in visible content as:

```text
[Image: source: <local-path-or-url-or-data-uri>]
```

Use `src-tauri/src/services/image_markers.rs` and existing provider helpers rather than inventing another marker. Preserve document order and support only source forms actually present in the wire:

- local absolute path;
- remote URL;
- `data:<mime>;base64,<payload>`;
- structured binary asset resolved by a typed asset id;
- provider placeholder merged with a separately recorded source.

Do not expose raw base64 twice in tool output and a marker. Do not fabricate a path for an unresolved asset. Warn/count malformed structured blocks when the rest of the message remains useful; preserve an unknown tool-result payload as raw instead of interpreting arbitrary JSON as an image.

Local sources must fall inside the narrow Tauri asset allowlist added for the provider. Do not widen scope to an entire home directory merely because one image path was observed. Remote URLs remain data, not authorization to fetch them during parsing.

## Image tests

- base64 with exact MIME and payload;
- local path on POSIX and Windows when the provider is cross-platform;
- URL image;
- asset-id lookup and missing asset;
- mixed text/image/tool blocks preserve order;
- placeholder/source merge without duplication;
- images in tool results retain structured tool metadata;
- malformed or unsupported image blocks increment warnings without dropping valid sibling text;
- frontend rendering/cache extraction recognizes the resulting marker.
