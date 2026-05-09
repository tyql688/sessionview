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
) {
  window.dispatchEvent(
    new CustomEvent("open-subagent", {
      detail: { description, nickname, agentId },
    }),
  );
}

/** Providers where subagents are stored as separate session files (can be opened). */
const SUBAGENT_FILE_PROVIDERS = new Set([
  "claude",
  "codex",
  "kimi",
  "cc-mirror",
  "gemini",
]);

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
  return <DiffRows lines={buildToolLineDiff(props.oldText, props.newText)} />;
}

export function ToolMessage(props: { message: Message; provider?: string }) {
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
  /** Parsed tool_input JSON, memoized so each Agent-related extractor reuses
   *  the same JSON.parse call. Logs at most once per message on parse failure. */
  const toolInputObj = createMemo<Record<string, unknown> | undefined>(() => {
    if (!hasInput()) return undefined;
    try {
      const parsed = JSON.parse(props.message.tool_input!);
      return typeof parsed === "object" && parsed !== null ? parsed : undefined;
    } catch (error) {
      console.warn("Failed to parse tool_input JSON:", error);
      return undefined;
    }
  });
  const toolOutputObj = createMemo<Record<string, unknown> | undefined>(() => {
    if (!hasOutput()) return undefined;
    try {
      const parsed = JSON.parse(props.message.content);
      return typeof parsed === "object" && parsed !== null ? parsed : undefined;
    } catch {
      return undefined;
    }
  });
  /** Extract nickname from Agent tool output (Codex: {"nickname":"Faraday"}) */
  const agentNickname = createMemo(() => {
    if (name() !== "Agent") return undefined;
    const obj = toolOutputObj();
    return typeof obj?.nickname === "string" ? obj.nickname : undefined;
  });
  /** Full description from Agent tool input (not truncated, for subagent matching).
   *  Codex spawn_agent carries the task text in `message`, not `description`/`prompt`. */
  const agentDescription = createMemo(() => {
    if (name() !== "Agent") return undefined;
    const obj = toolInputObj();
    if (!obj) return undefined;
    const candidate = obj.description ?? obj.prompt ?? obj.message;
    return typeof candidate === "string" ? candidate : undefined;
  });
  /** Extract agent_id from Agent tool output/structured/input.
   *  Priority:
   *    1. Kimi output format: "agent_id: xxx"
   *    2. Structured metadata agentId (set by successful spawn_agent)
   *    3. Tool input target / agent_id (codex wait_agent / send_input / close_agent) */
  const agentId = createMemo(() => {
    if (name() !== "Agent") return undefined;
    if (hasOutput()) {
      const m = props.message.content.match(/^agent_id:\s*(\S+)/m);
      if (m) return m[1];
    }
    const structured = props.message.tool_metadata?.structured;
    if (
      structured &&
      typeof structured === "object" &&
      !Array.isArray(structured) &&
      "agentId" in structured
    ) {
      return String(structured.agentId);
    }
    const obj = toolInputObj();
    if (obj) {
      const single = obj.target ?? obj.agent_id ?? obj.agentId;
      if (typeof single === "string") return single;
      const targets = obj.targets;
      if (
        Array.isArray(targets) &&
        targets.length === 1 &&
        typeof targets[0] === "string"
      ) {
        return targets[0];
      }
    }
    return undefined;
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
            (agentNickname() || agentId()) &&
            SUBAGENT_FILE_PROVIDERS.has(props.provider ?? "")
          }
        >
          <button
            class="msg-tool-subagent-link"
            onClick={(e) => {
              e.stopPropagation();
              openSubagent(
                agentDescription() ?? summary(),
                agentNickname(),
                agentId(),
              );
            }}
            title="Open subagent session"
          >
            ↗ Open
          </button>
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
        <Show when={hasOutput() && !suppressRawOutput()}>
          <div class="msg-tool-output">
            <For each={parseContent(resolvedContent())}>
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
