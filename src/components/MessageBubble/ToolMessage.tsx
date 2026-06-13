import {
  createSignal,
  createMemo,
  createEffect,
  onCleanup,
  Show,
  For,
} from "solid-js";
import type { Message } from "../../lib/types";
import { readToolResultText } from "../../lib/tauri";
import { buildToolLineDiff, type ToolDiffLine } from "../../lib/diff";
import {
  formatToolInput,
  formatToolResultMetadata,
  parseMcpToolName,
  toolDisplayName,
  toolIcon,
  toolSummary,
} from "../../lib/tools";
import {
  extractPersistedOutputPaths,
  loadPersistedOutput,
  substitutePersistedOutputs,
} from "../../lib/persisted-output";
import {
  SUBAGENT_FILE_PROVIDERS,
  extractAgentChildIds,
  extractAgentChildPrompts,
  extractAgentDescription,
  extractAgentId,
  extractAgentNickname,
  parseToolJsonObject,
} from "../../lib/subagent";
import { parseContent } from "./MarkdownRenderer";
import {
  ImagePreview,
  LocalImage,
  RemoteImage,
  isLocalPath,
} from "./ImagePreview";

export { formatMcpLabel } from "../../lib/tools";

/** Dispatch a custom event to open a subagent session by description, nickname, or agent ID. */
function openSubagent(
  description: string,
  nickname?: string,
  agentId?: string,
  parentSessionId?: string,
) {
  window.dispatchEvent(
    new CustomEvent("open-subagent", {
      detail: { description, nickname, agentId, parentSessionId },
    }),
  );
}

function DiffRows(props: { lines: ToolDiffLine[] }) {
  return (
    <div class="msg-tool-line-diff">
      <For each={props.lines}>
        {(line) => (
          <div class={`msg-tool-diff-line ${line.type}`}>
            <span class="msg-tool-diff-gutter msg-tool-diff-gutter-old">
              {line.oldLine ?? ""}
            </span>
            <span class="msg-tool-diff-gutter msg-tool-diff-gutter-new">
              {line.newLine ?? ""}
            </span>
            <span class="msg-tool-diff-marker">
              {line.type === "add"
                ? "+"
                : line.type === "remove"
                  ? "-"
                  : line.type === "skip"
                    ? "⋯"
                    : " "}
            </span>
            <span class="msg-tool-diff-code">{line.text || " "}</span>
          </div>
        )}
      </For>
    </div>
  );
}

function LineDiff(props: { oldText: string; newText: string }) {
  const lines = createMemo(() =>
    buildToolLineDiff(props.oldText, props.newText),
  );
  return <DiffRows lines={lines()} />;
}

export function ToolMessage(props: {
  message: Message;
  provider?: string;
  parentSessionId?: string;
}) {
  const [expanded, setExpanded] = createSignal(false);
  const [previewImage, setPreviewImage] = createSignal<{
    src: string;
    source?: string;
  } | null>(null);
  const [fullResult, setFullResult] = createSignal<string | null>(null);
  const [fullResultError, setFullResultError] = createSignal<string | null>(
    null,
  );
  const [loadingFullResult, setLoadingFullResult] = createSignal(false);

  const hasInput = () =>
    !!props.message.tool_input && props.message.tool_input.trim().length > 0;
  const hasOutput = () =>
    !!props.message.content && props.message.content.trim().length > 0;
  const hasName = () =>
    !!props.message.tool_name && props.message.tool_name.trim().length > 0;

  if (!hasName()) return null;

  // <persisted-output> tag blocks are no longer resolved at parse time
  // (see src-tauri/src/providers/claude/mod.rs comment) so we resolve
  // them here on first render. Cache hits are synchronous; first-time
  // reads briefly show the raw tag block, then swap in the file
  // content once `loadPersistedOutput` completes.
  const [resolvedReplacements, setResolvedReplacements] = createSignal<
    Map<string, string>
  >(new Map());
  createEffect(() => {
    const content = props.message.content || "";
    const paths = extractPersistedOutputPaths(content);
    if (paths.length === 0) return;
    let cancelled = false;
    // Solid does not treat the return value of `createEffect` as a
    // cleanup; we must register one via `onCleanup` so that re-runs
    // (e.g., props.message.content change) and unmount drop the
    // pending setSignal call.
    onCleanup(() => {
      cancelled = true;
    });
    void Promise.all(
      paths.map((path) =>
        loadPersistedOutput(path)
          .then((value) => ({ path, value }))
          .catch((error) => {
            console.warn(`failed to resolve persisted output ${path}:`, error);
            return null;
          }),
      ),
    ).then((results) => {
      if (cancelled) return;
      setResolvedReplacements((prev) => {
        const next = new Map(prev);
        for (const r of results) {
          if (r) next.set(r.path, r.value);
        }
        return next;
      });
    });
  });
  const resolvedContent = createMemo(() => {
    const raw = props.message.content || "";
    const replacements = resolvedReplacements();
    return replacements.size === 0
      ? raw
      : substitutePersistedOutputs(raw, replacements);
  });

  const name = () => props.message.tool_name || "";
  const metadata = () => props.message.tool_metadata;
  const mcp = () => metadata()?.mcp ?? parseMcpToolName(name());
  const icon = () => toolIcon(name(), metadata());
  const displayName = () => toolDisplayName(name(), metadata());
  const summary = createMemo(() => toolSummary(props.message));
  const formatted = createMemo(() => formatToolInput(props.message));
  const resultMetadata = createMemo(() =>
    formatToolResultMetadata(props.message.tool_metadata),
  );
  const persistedOutputPath = () => resultMetadata()?.persistedOutputPath;
  const resultHasDiff = () =>
    !!resultMetadata()?.diff || !!resultMetadata()?.patchDiff;
  const showInputDetail = () => !!formatted() && !resultHasDiff();
  const isAgent = () => name() === "Agent";
  /** Parsed tool_input/tool_output JSON, memoized so each downstream
   *  extractor reuses the same JSON.parse call. Most tool outputs are
   *  plain text (Bash stdout, file contents, …), so we pre-screen the
   *  shape before calling JSON.parse — otherwise every non-JSON output
   *  spams `SyntaxError: JSON Parse error` into the console. Only a
   *  malformed JSON-looking payload is worth a warn. */
  const toolInputObj = createMemo<Record<string, unknown> | undefined>(() =>
    parseToolJsonObject(props.message.tool_input, "tool_input"),
  );
  const toolOutputObj = createMemo<Record<string, unknown> | undefined>(() =>
    parseToolJsonObject(props.message.content, "tool output"),
  );
  // Subagent extraction lives in lib/subagent.ts (pure, provider-specific);
  // these memos are thin wrappers gated on the Agent tool name.
  const agentNickname = createMemo(() =>
    isAgent() ? extractAgentNickname(toolOutputObj()) : undefined,
  );
  const agentDescription = createMemo(() =>
    isAgent() ? extractAgentDescription(toolInputObj()) : undefined,
  );
  const agentId = createMemo(() =>
    isAgent()
      ? extractAgentId(
          props.message.content,
          props.message.tool_metadata,
          toolInputObj(),
        )
      : undefined,
  );
  const agentChildIds = createMemo<string[] | undefined>(() =>
    isAgent() ? extractAgentChildIds(props.message.tool_metadata) : undefined,
  );
  const agentChildPrompts = createMemo<string[]>(() =>
    isAgent() ? extractAgentChildPrompts(props.message.tool_metadata) : [],
  );
  const agentPromptTargets = createMemo<string[]>(() => {
    if (
      !isAgent() ||
      props.provider !== "antigravity" ||
      metadata()?.raw_name !== "invoke_subagent" ||
      agentChildIds()
    ) {
      return [];
    }
    return agentChildPrompts().filter((prompt) => prompt.trim().length > 0);
  });
  const canOpenSingleAgent = createMemo(() => {
    if (!isAgent()) return false;
    if (props.provider === "antigravity") {
      return metadata()?.raw_name === "invoke_subagent";
    }
    return true;
  });

  async function loadFullResult() {
    const path = persistedOutputPath();
    if (!path || loadingFullResult()) return;

    setLoadingFullResult(true);
    setFullResultError(null);
    try {
      setFullResult(await readToolResultText(path));
    } catch (error) {
      setFullResultError(String(error));
    } finally {
      setLoadingFullResult(false);
    }
  }

  const suppressRawOutput = () => {
    const kind = props.message.tool_metadata?.result_kind;
    return (
      !!resultMetadata() &&
      (kind === "terminal_output" || (kind === "file_patch" && resultHasDiff()))
    );
  };
  const showRawOutput = () => expanded() && hasOutput() && !suppressRawOutput();
  const outputSegments = createMemo(() =>
    showRawOutput() ? parseContent(resolvedContent()) : [],
  );

  return (
    <div class={`msg-tool${expanded() ? " expanded" : ""}`}>
      <div class="msg-tool-header" onClick={() => setExpanded(!expanded())}>
        <span class="msg-tool-icon">{icon()}</span>
        <span class="msg-tool-name">{displayName()}</span>
        <Show when={mcp()}>
          <span class="msg-tool-server">{mcp()!.server}</span>
        </Show>
        <Show when={summary()}>
          <span class="msg-tool-summary">{summary()}</span>
        </Show>
        <Show
          when={
            name() === "Agent" &&
            SUBAGENT_FILE_PROVIDERS.has(props.provider ?? "") &&
            (
              agentChildIds() ??
              (agentPromptTargets().length > 0
                ? agentPromptTargets()
                : undefined) ??
              (canOpenSingleAgent() &&
              (agentNickname() || agentId() || agentDescription())
                ? [null]
                : [])
            ).length > 0
          }
        >
          <Show
            when={agentChildIds()}
            fallback={
              <Show
                when={agentPromptTargets().length > 0}
                fallback={
                  <button
                    class="msg-tool-subagent-link"
                    onClick={(e) => {
                      e.stopPropagation();
                      openSubagent(
                        agentDescription() ?? summary(),
                        agentNickname(),
                        agentId(),
                        props.parentSessionId,
                      );
                    }}
                    title="Open subagent session"
                  >
                    ↗ Open
                  </button>
                }
              >
                <For each={agentPromptTargets()}>
                  {(prompt, i) => {
                    const label = () =>
                      agentPromptTargets().length > 1
                        ? `↗ Open #${i() + 1}`
                        : "↗ Open";
                    return (
                      <button
                        class="msg-tool-subagent-link"
                        onClick={(e) => {
                          e.stopPropagation();
                          openSubagent(
                            prompt,
                            undefined,
                            undefined,
                            props.parentSessionId,
                          );
                        }}
                        title={prompt}
                      >
                        {label()}
                      </button>
                    );
                  }}
                </For>
              </Show>
            }
          >
            <For each={agentChildIds()!}>
              {(childId, i) => {
                const prompt = () => agentChildPrompts()[i()] ?? "";
                const label = () => {
                  const ids = agentChildIds() ?? [];
                  return ids.length > 1 ? `↗ Open #${i() + 1}` : "↗ Open";
                };
                return (
                  <button
                    class="msg-tool-subagent-link"
                    onClick={(e) => {
                      e.stopPropagation();
                      openSubagent(
                        prompt() || agentDescription() || summary(),
                        undefined,
                        childId,
                        props.parentSessionId,
                      );
                    }}
                    title={prompt() ? prompt() : `Open subagent ${childId}`}
                  >
                    {label()}
                  </button>
                );
              }}
            </For>
          </Show>
        </Show>
        <Show when={hasInput() || hasOutput() || resultMetadata()}>
          <span class="tool-expand-indicator">{expanded() ? "▾" : "▸"}</span>
        </Show>
      </div>
      <Show when={expanded()}>
        <Show when={showInputDetail()}>
          <div class="msg-tool-detail">
            <For each={formatted()!.lines}>
              {(line) => (
                <div class="msg-tool-field">
                  <span class="msg-tool-field-label">{line.label}</span>
                  <pre class="msg-tool-field-value">{line.value}</pre>
                </div>
              )}
            </For>
            <Show when={formatted()!.diff}>
              <LineDiff
                oldText={formatted()!.diff!.old}
                newText={formatted()!.diff!.new}
              />
            </Show>
            <Show when={formatted()!.patchDiff}>
              <DiffRows lines={formatted()!.patchDiff!} />
            </Show>
          </div>
        </Show>
        <Show when={resultMetadata()}>
          <div class="msg-tool-detail msg-tool-result-detail">
            <For each={resultMetadata()!.lines}>
              {(line) => (
                <div class="msg-tool-field">
                  <span class="msg-tool-field-label">{line.label}</span>
                  <pre class="msg-tool-field-value">{line.value}</pre>
                </div>
              )}
            </For>
            <Show when={resultMetadata()!.diff}>
              <LineDiff
                oldText={resultMetadata()!.diff!.old}
                newText={resultMetadata()!.diff!.new}
              />
            </Show>
            <Show when={resultMetadata()!.patchDiff}>
              <DiffRows lines={resultMetadata()!.patchDiff!} />
            </Show>
            <Show when={persistedOutputPath()}>
              <button
                class="msg-tool-subagent-link"
                disabled={loadingFullResult()}
                onClick={(event) => {
                  event.stopPropagation();
                  void loadFullResult();
                }}
                type="button"
              >
                {loadingFullResult() ? "Loading..." : "Load full result"}
              </button>
            </Show>
            <Show when={fullResultError()}>
              <pre class="msg-tool-input">{fullResultError()}</pre>
            </Show>
            <Show when={fullResult()}>
              <pre class="msg-tool-input">{fullResult()}</pre>
            </Show>
          </div>
        </Show>
        <Show when={!showInputDetail() && !resultHasDiff() && hasInput()}>
          <pre class="msg-tool-input">{props.message.tool_input!}</pre>
        </Show>
        <Show when={showRawOutput()}>
          <div class="msg-tool-output">
            <For each={outputSegments()}>
              {(seg) => {
                if (seg.type === "image") {
                  if (isLocalPath(seg.content)) {
                    return (
                      <LocalImage
                        path={seg.content}
                        onPreview={(src, source) =>
                          setPreviewImage({ src, source })
                        }
                      />
                    );
                  }
                  return (
                    <RemoteImage
                      src={seg.content}
                      onPreview={(src, source) =>
                        setPreviewImage({ src, source })
                      }
                    />
                  );
                }
                return <pre>{seg.content}</pre>;
              }}
            </For>
          </div>
        </Show>
      </Show>
      <Show when={previewImage()}>
        <ImagePreview
          src={previewImage()!.src}
          source={previewImage()!.source}
          onClose={() => setPreviewImage(null)}
        />
      </Show>
    </div>
  );
}
