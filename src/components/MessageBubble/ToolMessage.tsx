import { useState, useMemo, useEffect } from "react";
import type { Message } from "@/lib/types";
import { useI18n } from "@/i18n/index";
import { readToolResultText } from "@/lib/tauri";
import { buildToolLineDiff, type ToolDiffLine } from "@/lib/diff";
import {
  formatToolInput,
  formatToolResultMetadata,
  parseMcpToolName,
  toolDisplayName,
  toolIcon,
  toolSummary,
} from "@/lib/tools";
import {
  extractPersistedOutputPaths,
  loadPersistedOutput,
  substitutePersistedOutputs,
} from "@/lib/persisted-output";
import {
  SUBAGENT_FILE_PROVIDERS,
  extractAgentChildIds,
  extractAgentChildPrompts,
  extractAgentDescription,
  extractAgentId,
  extractAgentNickname,
  isAgentToolMessage,
  parseToolJsonObject,
} from "@/lib/subagent";
import { parseContent } from "@/lib/message-content";
import { SubagentInline } from "@/components/MessageBubble/SubagentInline";
import {
  ImagePreview,
  LocalImage,
  RemoteImage,
  isLocalPath,
} from "@/components/MessageBubble/ImagePreview";

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

const SUBAGENT_LABEL_LIMIT = 48;

function compactSubagentLabel(value: string): string {
  const singleLine = value.replace(/\s+/g, " ").trim();
  if (singleLine.length <= SUBAGENT_LABEL_LIMIT) return singleLine;
  return `${singleLine.slice(0, SUBAGENT_LABEL_LIMIT - 3)}...`;
}

function subagentButtonLabel(
  t: (key: string, options?: Record<string, unknown>) => string,
  prompt: string | undefined,
  agentId: string | undefined,
  index: number,
  total: number,
): string {
  if (total <= 1) return t("tool.openSubagent");
  const identity =
    compactSubagentLabel(prompt ?? "") ||
    compactSubagentLabel(agentId ?? "") ||
    `#${index + 1}`;
  return t("tool.openSubagentNamed", { name: identity });
}

function DiffRows(props: { lines: ToolDiffLine[] }) {
  return (
    <div className="msg-tool-line-diff">
      {props.lines.map((line, i) => (
        <div className={`msg-tool-diff-line ${line.type}`} key={i}>
          <span className="msg-tool-diff-gutter msg-tool-diff-gutter-old">
            {line.oldLine ?? ""}
          </span>
          <span className="msg-tool-diff-gutter msg-tool-diff-gutter-new">
            {line.newLine ?? ""}
          </span>
          <span className="msg-tool-diff-marker">
            {line.type === "add"
              ? "+"
              : line.type === "remove"
                ? "-"
                : line.type === "skip"
                  ? "⋯"
                  : " "}
          </span>
          <span className="msg-tool-diff-code">{line.text || " "}</span>
        </div>
      ))}
    </div>
  );
}

function LineDiff(props: { oldText: string; newText: string }) {
  const lines = useMemo(
    () => buildToolLineDiff(props.oldText, props.newText),
    [props.oldText, props.newText],
  );
  return <DiffRows lines={lines} />;
}

export function ToolMessage(props: {
  message: Message;
  provider?: string;
  parentSessionId?: string;
}) {
  const { t } = useI18n();
  // Const copy so truthiness narrowing survives into nested JSX callbacks.
  const parentSessionId = props.parentSessionId;
  const [expanded, setExpanded] = useState(false);
  const [previewImage, setPreviewImage] = useState<{
    src: string;
    source?: string;
  } | null>(null);
  const [fullResult, setFullResult] = useState<string | null>(null);
  const [fullResultError, setFullResultError] = useState<string | null>(null);
  const [loadingFullResult, setLoadingFullResult] = useState(false);

  const hasInput = () =>
    !!props.message.tool_input && props.message.tool_input.trim().length > 0;
  const hasOutput = () =>
    !!props.message.content && props.message.content.trim().length > 0;
  const hasName = () =>
    !!props.message.tool_name && props.message.tool_name.trim().length > 0;

  // <persisted-output> tag blocks are no longer resolved at parse time
  // (see src-tauri/src/providers/claude/mod.rs comment) so we resolve
  // them here on first render. Cache hits are synchronous; first-time
  // reads briefly show the raw tag block, then swap in the file
  // content once `loadPersistedOutput` completes.
  const [resolvedReplacements, setResolvedReplacements] = useState<
    Map<string, string>
  >(new Map());
  useEffect(() => {
    const content = props.message.content || "";
    const paths = extractPersistedOutputPaths(content);
    if (paths.length === 0) return;
    let cancelled = false;
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
    return () => {
      cancelled = true;
    };
  }, [props.message.content]);
  const resolvedContent = useMemo(() => {
    const raw = props.message.content || "";
    const replacements = resolvedReplacements;
    return replacements.size === 0
      ? raw
      : substitutePersistedOutputs(raw, replacements);
  }, [props.message.content, resolvedReplacements]);

  const name = () => props.message.tool_name || "";
  const metadata = () => props.message.tool_metadata;
  const mcp = () => metadata()?.mcp ?? parseMcpToolName(name());
  const icon = () => toolIcon(name(), metadata());
  const displayName = () => toolDisplayName(name(), metadata());
  const summary = useMemo(() => toolSummary(props.message), [props.message]);
  const formatted = useMemo(
    () => formatToolInput(props.message),
    [props.message],
  );
  const resultMetadata = useMemo(
    () => formatToolResultMetadata(props.message.tool_metadata),
    [props.message.tool_metadata],
  );
  const persistedOutputPath = () => resultMetadata?.persistedOutputPath;
  const resultHasDiff = () =>
    !!resultMetadata?.diff || !!resultMetadata?.patchDiff;
  const showInputDetail = () => !!formatted && !resultHasDiff();
  const isAgent = () => isAgentToolMessage(props.message);
  /** Parsed tool_input/tool_output JSON, memoized so each downstream
   *  extractor reuses the same JSON.parse call. Most tool outputs are
   *  plain text (Bash stdout, file contents, …), so we pre-screen the
   *  shape before calling JSON.parse — otherwise every non-JSON output
   *  spams `SyntaxError: JSON Parse error` into the console. Only a
   *  malformed JSON-looking payload is worth a warn. */
  const toolInputObj = useMemo<Record<string, unknown> | undefined>(
    () => parseToolJsonObject(props.message.tool_input, "tool_input"),
    [props.message.tool_input],
  );
  const toolOutputObj = useMemo<Record<string, unknown> | undefined>(
    () => parseToolJsonObject(props.message.content, "tool output"),
    [props.message.content],
  );
  // Subagent extraction lives in lib/subagent.ts (pure, provider-specific);
  // these memos are thin wrappers gated on the Agent tool name.
  const agentNickname = useMemo(
    () => (isAgent() ? extractAgentNickname(toolOutputObj) : undefined),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [props.message, toolOutputObj],
  );
  const agentDescription = useMemo(
    () => (isAgent() ? extractAgentDescription(toolInputObj) : undefined),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [props.message, toolInputObj],
  );
  const agentId = useMemo(
    () =>
      isAgent()
        ? extractAgentId(
            props.message.content,
            props.message.tool_metadata,
            toolInputObj,
          )
        : undefined,
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [props.message, toolInputObj],
  );
  const agentChildIds = useMemo<string[] | undefined>(
    () =>
      isAgent() ? extractAgentChildIds(props.message.tool_metadata) : undefined,
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [props.message],
  );
  const agentChildPrompts = useMemo<string[]>(
    () =>
      isAgent() ? extractAgentChildPrompts(props.message.tool_metadata) : [],
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [props.message],
  );
  const agentPromptTargets = useMemo<string[]>(() => {
    if (
      !isAgent() ||
      props.provider !== "antigravity" ||
      metadata()?.raw_name !== "invoke_subagent" ||
      agentChildIds
    ) {
      return [];
    }
    return agentChildPrompts.filter((prompt) => prompt.trim().length > 0);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [props.message, props.provider, agentChildIds, agentChildPrompts]);
  const canOpenSingleAgent = useMemo(() => {
    if (!isAgent()) return false;
    if (props.provider === "antigravity") {
      return metadata()?.raw_name === "invoke_subagent";
    }
    return true;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [props.message, props.provider]);

  async function loadFullResult() {
    const path = persistedOutputPath();
    if (!path || loadingFullResult) return;

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
    const policy = props.message.tool_metadata?.presentation?.rawOutputPolicy;
    if (policy === "suppress_terminal") return !!resultMetadata;
    if (policy === "suppress_patch_when_diff_present") {
      return !!resultMetadata && resultHasDiff();
    }
    if (policy === "keep") return false;

    const kind = props.message.tool_metadata?.result_kind;
    return (
      !!resultMetadata &&
      (kind === "terminal_output" || (kind === "file_patch" && resultHasDiff()))
    );
  };
  const showRawOutput = () => expanded && hasOutput() && !suppressRawOutput();
  const outputSegments = useMemo(
    () => (showRawOutput() ? parseContent(resolvedContent) : []),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [
      expanded,
      props.message.content,
      props.message.tool_metadata,
      resultMetadata,
      resolvedContent,
    ],
  );

  if (!hasName()) return null;

  return (
    <div className={`msg-tool${expanded ? " expanded" : ""}`}>
      <div className="msg-tool-header" onClick={() => setExpanded(!expanded)}>
        <span className="msg-tool-icon">{icon()}</span>
        <span className="msg-tool-name">{displayName()}</span>
        {mcp() && <span className="msg-tool-server">{mcp()!.server}</span>}
        {summary && <span className="msg-tool-summary">{summary}</span>}
        {isAgent() &&
          SUBAGENT_FILE_PROVIDERS.has(props.provider ?? "") &&
          (
            agentChildIds ??
            (agentPromptTargets.length > 0 ? agentPromptTargets : undefined) ??
            (canOpenSingleAgent &&
            (agentNickname || agentId || agentDescription)
              ? [null]
              : [])
          ).length > 0 &&
          (agentChildIds ? (
            agentChildIds.map((childId, i) => {
              const prompt = agentChildPrompts[i] ?? "";
              const label = subagentButtonLabel(
                t,
                prompt,
                childId,
                i,
                (agentChildIds ?? []).length,
              );
              return (
                <button
                  className="msg-tool-subagent-link"
                  key={childId}
                  onClick={(e) => {
                    e.stopPropagation();
                    openSubagent(
                      prompt || agentDescription || summary,
                      undefined,
                      childId,
                      props.parentSessionId,
                    );
                  }}
                  title={
                    prompt
                      ? prompt
                      : t("tool.openSubagentTitleId", { id: childId })
                  }
                >
                  {label}
                </button>
              );
            })
          ) : agentPromptTargets.length > 0 ? (
            agentPromptTargets.map((prompt, i) => {
              const label = subagentButtonLabel(
                t,
                prompt,
                undefined,
                i,
                agentPromptTargets.length,
              );
              return (
                <button
                  className="msg-tool-subagent-link"
                  key={i}
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
                  {label}
                </button>
              );
            })
          ) : (
            <button
              className="msg-tool-subagent-link"
              onClick={(e) => {
                e.stopPropagation();
                openSubagent(
                  agentDescription ?? summary,
                  agentNickname,
                  agentId,
                  props.parentSessionId,
                );
              }}
              title={t("tool.openSubagentTitle")}
            >
              {t("tool.openSubagent")}
            </button>
          ))}
        {(hasInput() || hasOutput() || resultMetadata) && (
          <span className="tool-expand-indicator">{expanded ? "▾" : "▸"}</span>
        )}
      </div>
      {expanded && (
        <>
          {showInputDetail() && (
            <div className="msg-tool-detail">
              {formatted!.lines.map((line, i) => (
                <div className="msg-tool-field" key={i}>
                  <span className="msg-tool-field-label">{line.label}</span>
                  <pre className="msg-tool-field-value">{line.value}</pre>
                </div>
              ))}
              {formatted!.diff && (
                <LineDiff
                  oldText={formatted!.diff!.old}
                  newText={formatted!.diff!.new}
                />
              )}
              {formatted!.patchDiff && (
                <DiffRows lines={formatted!.patchDiff!} />
              )}
            </div>
          )}
          {resultMetadata && (
            <div className="msg-tool-detail msg-tool-result-detail">
              {resultMetadata.lines.map((line, i) => (
                <div className="msg-tool-field" key={i}>
                  <span className="msg-tool-field-label">{line.label}</span>
                  <pre className="msg-tool-field-value">{line.value}</pre>
                </div>
              ))}
              {resultMetadata.diff && (
                <LineDiff
                  oldText={resultMetadata.diff.old}
                  newText={resultMetadata.diff.new}
                />
              )}
              {resultMetadata.patchDiff && (
                <DiffRows lines={resultMetadata.patchDiff} />
              )}
              {persistedOutputPath() && (
                <button
                  className="msg-tool-subagent-link"
                  disabled={loadingFullResult}
                  onClick={(event) => {
                    event.stopPropagation();
                    void loadFullResult();
                  }}
                  type="button"
                >
                  {loadingFullResult
                    ? t("common.loading")
                    : t("tool.loadFullResult")}
                </button>
              )}
              {fullResultError && (
                <pre className="msg-tool-input">{fullResultError}</pre>
              )}
              {fullResult && <pre className="msg-tool-input">{fullResult}</pre>}
            </div>
          )}
          {!showInputDetail() && !resultHasDiff() && hasInput() && (
            <pre className="msg-tool-input">{props.message.tool_input!}</pre>
          )}
          {showRawOutput() && (
            <div className="msg-tool-output">
              {outputSegments.map((seg, i) => {
                if (seg.type === "image") {
                  if (isLocalPath(seg.content)) {
                    return (
                      <LocalImage
                        key={i}
                        path={seg.content}
                        onPreview={(src, source) =>
                          setPreviewImage({ src, source })
                        }
                      />
                    );
                  }
                  return (
                    <RemoteImage
                      key={i}
                      src={seg.content}
                      onPreview={(src, source) =>
                        setPreviewImage({ src, source })
                      }
                    />
                  );
                }
                return <pre key={i}>{seg.content}</pre>;
              })}
            </div>
          )}
          {parentSessionId &&
            isAgent() &&
            SUBAGENT_FILE_PROVIDERS.has(props.provider ?? "") &&
            (agentChildIds ? (
              agentChildIds.map((childId, i) => (
                <SubagentInline
                  key={childId}
                  parentSessionId={parentSessionId}
                  request={{
                    agentId: childId,
                    description: agentChildPrompts[i] || agentDescription,
                  }}
                  label={
                    agentChildPrompts[i]
                      ? compactSubagentLabel(agentChildPrompts[i])
                      : childId
                  }
                />
              ))
            ) : canOpenSingleAgent &&
              (agentNickname || agentId || agentDescription) ? (
              <SubagentInline
                parentSessionId={parentSessionId}
                request={{
                  agentId,
                  nickname: agentNickname,
                  description: agentDescription ?? summary,
                }}
                label={null}
              />
            ) : null)}
        </>
      )}
      {previewImage && (
        <ImagePreview
          src={previewImage.src}
          source={previewImage.source}
          onClose={() => setPreviewImage(null)}
        />
      )}
    </div>
  );
}
