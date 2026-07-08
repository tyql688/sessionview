import { ChevronDown, ChevronRight } from "lucide-react";
import { useState, useMemo, useEffect } from "react";
import { Button } from "@/components/ui/button";
import type { Message } from "@/lib/types";
import { useI18n } from "@/i18n/index";
import { readToolResultText } from "@/lib/tauri";
import { formatToolInput, formatToolResultMetadata, parseMcpToolName, toolDisplayName, toolSummary } from "@/lib/tools";
import { extractPersistedOutputPaths, loadPersistedOutput, substitutePersistedOutputs } from "@/lib/persisted-output";
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
import { SubagentInline } from "@/features/session/MessageBubble/SubagentInline";
import { ImagePreview, LocalImage, RemoteImage, isLocalPath } from "@/features/session/MessageBubble/ImagePreview";
import { SessionDiffView, SessionLineDiffView } from "@/features/session/MessageBubble/SessionDiffView";
import { TerminalToolMessage } from "@/features/session/MessageBubble/TerminalToolMessage";
import { ToolGlyph } from "@/features/session/ToolGlyph";

/** Dispatch a custom event to open a subagent session by description, nickname, or agent ID. */
function openSubagent(description: string, nickname?: string, agentId?: string, parentSessionId?: string) {
  window.dispatchEvent(
    new CustomEvent("open-subagent", {
      detail: { description, nickname, agentId, parentSessionId },
    }),
  );
}

function subagentButtonLabel(
  t: (key: string, options?: Record<string, unknown>) => string,
  prompt: string | undefined,
  agentId: string | undefined,
  index: number,
  total: number,
): string {
  if (total <= 1) return t("tool.openSubagent");
  const identity = prompt?.replace(/\s+/g, " ").trim() || agentId?.replace(/\s+/g, " ").trim() || `#${index + 1}`;
  return t("tool.openSubagentNamed", { name: identity });
}

function toolStatusKind(status: string): "success" | "error" | "neutral" {
  const normalized = status.toLowerCase();
  if (["success", "completed", "ok", "done"].includes(normalized)) return "success";
  if (["failed", "failure", "error", "cancelled", "canceled", "timeout"].includes(normalized)) return "error";
  return "neutral";
}

interface ToolMessageProps {
  message: Message;
  provider?: string;
  parentSessionId?: string;
}

export function ToolMessage(props: ToolMessageProps) {
  if ((props.message.tool_metadata?.canonical_name ?? props.message.tool_name) === "Bash") {
    return <TerminalToolMessage message={props.message} />;
  }

  return <GenericToolMessage {...props} />;
}

function GenericToolMessage(props: ToolMessageProps) {
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

  const hasInput = () => !!props.message.tool_input && props.message.tool_input.trim().length > 0;
  const hasOutput = () => !!props.message.content && props.message.content.trim().length > 0;
  const hasName = () => !!props.message.tool_name && props.message.tool_name.trim().length > 0;

  // <persisted-output> tag blocks are no longer resolved at parse time
  // (see src-tauri/src/providers/claude/mod.rs comment) so we resolve
  // them here on first render. Cache hits are synchronous; first-time
  // reads briefly show the raw tag block, then swap in the file
  // content once `loadPersistedOutput` completes.
  const [resolvedReplacements, setResolvedReplacements] = useState<Map<string, string>>(new Map());
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
    return replacements.size === 0 ? raw : substitutePersistedOutputs(raw, replacements);
  }, [props.message.content, resolvedReplacements]);

  const name = () => props.message.tool_name || "";
  const metadata = () => props.message.tool_metadata;
  const mcp = () => metadata()?.mcp ?? parseMcpToolName(name());
  const displayName = () => toolDisplayName(name(), metadata());
  const summary = useMemo(() => toolSummary(props.message), [props.message]);
  const formatted = useMemo(() => formatToolInput(props.message), [props.message]);
  const resultMetadata = useMemo(
    () => formatToolResultMetadata(props.message.tool_metadata),
    [props.message.tool_metadata],
  );
  const persistedOutputPath = () => resultMetadata?.persistedOutputPath;
  const resultHasDiff = () => !!resultMetadata?.diff || !!resultMetadata?.patchDiff;
  const showInputDetail = () => !!formatted && !resultHasDiff();
  const isAgent = () => isAgentToolMessage(props.message);
  /** Parsed tool_input/tool_output JSON, memoized so downstream extractors
   *  reuse the same JSON.parse call. Many real tool payloads are plain text
   *  or partial streamed values, so parse misses simply mean "no metadata". */
  const toolInputObj = useMemo<Record<string, unknown> | undefined>(
    () => parseToolJsonObject(props.message.tool_input),
    [props.message.tool_input],
  );
  const toolOutputObj = useMemo<Record<string, unknown> | undefined>(
    () => parseToolJsonObject(props.message.content),
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
    () => (isAgent() ? extractAgentId(props.message.content, props.message.tool_metadata, toolInputObj) : undefined),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [props.message, toolInputObj],
  );
  const agentChildIds = useMemo<string[] | undefined>(
    () => (isAgent() ? extractAgentChildIds(props.message.tool_metadata) : undefined),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [props.message],
  );
  const agentChildPrompts = useMemo<string[]>(
    () => (isAgent() ? extractAgentChildPrompts(props.message.tool_metadata) : []),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [props.message],
  );
  const agentPromptTargets = useMemo<string[]>(() => {
    if (!isAgent() || props.provider !== "antigravity" || metadata()?.raw_name !== "invoke_subagent" || agentChildIds) {
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
    return !!resultMetadata && (kind === "terminal_output" || (kind === "file_patch" && resultHasDiff()));
  };
  const showRawOutput = () => expanded && hasOutput() && !suppressRawOutput();
  const outputSegments = useMemo(
    () => (showRawOutput() ? parseContent(resolvedContent) : []),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [expanded, props.message.content, props.message.tool_metadata, resultMetadata, resolvedContent],
  );
  const canExpand = hasInput() || hasOutput() || !!resultMetadata;
  const status = metadata()?.status?.trim() ?? "";
  const statusKind = toolStatusKind(status);

  if (!hasName()) return null;

  return (
    <div className={`msg-tool msg-tool-${statusKind}${expanded ? " expanded" : ""}`}>
      <div className="msg-tool-header">
        {canExpand ? (
          <button
            type="button"
            className="msg-tool-toggle-row"
            onClick={() => setExpanded((value) => !value)}
            aria-expanded={expanded}
          >
            <ToolGlyph name={name()} metadata={metadata()} className="msg-tool-icon" />
            <span className="msg-tool-name">{displayName()}</span>
            {mcp() && <span className="msg-tool-server">{mcp()!.server}</span>}
            {status && <span className="msg-tool-status-dot" title={status} aria-hidden="true" />}
            {summary && <span className="msg-tool-summary">{summary}</span>}
            {expanded ? (
              <ChevronDown className="msg-tool-chevron" aria-hidden="true" />
            ) : (
              <ChevronRight className="msg-tool-chevron" aria-hidden="true" />
            )}
          </button>
        ) : (
          <div className="msg-tool-toggle-row msg-tool-toggle-row-static">
            <ToolGlyph name={name()} metadata={metadata()} className="msg-tool-icon" />
            <span className="msg-tool-name">{displayName()}</span>
            {mcp() && <span className="msg-tool-server">{mcp()!.server}</span>}
            {status && <span className="msg-tool-status-dot" title={status} aria-hidden="true" />}
            {summary && <span className="msg-tool-summary">{summary}</span>}
          </div>
        )}
        {isAgent() &&
          SUBAGENT_FILE_PROVIDERS.has(props.provider ?? "") &&
          (
            agentChildIds ??
            (agentPromptTargets.length > 0 ? agentPromptTargets : undefined) ??
            (canOpenSingleAgent && (agentNickname || agentId || agentDescription) ? [null] : [])
          ).length > 0 &&
          (agentChildIds ? (
            agentChildIds.map((childId, i) => {
              const prompt = agentChildPrompts[i] ?? "";
              const label = subagentButtonLabel(t, prompt, childId, i, (agentChildIds ?? []).length);
              return (
                <Button
                  variant="ghost"
                  size="xs"
                  className="msg-tool-subagent-link h-auto min-w-0 active:translate-y-0"
                  key={childId}
                  onClick={(e) => {
                    e.stopPropagation();
                    openSubagent(prompt || agentDescription || summary, undefined, childId, props.parentSessionId);
                  }}
                  title={prompt ? prompt : t("tool.openSubagentTitleId", { id: childId })}
                >
                  {label}
                </Button>
              );
            })
          ) : agentPromptTargets.length > 0 ? (
            agentPromptTargets.map((prompt, i) => {
              const label = subagentButtonLabel(t, prompt, undefined, i, agentPromptTargets.length);
              return (
                <Button
                  variant="ghost"
                  size="xs"
                  className="msg-tool-subagent-link h-auto min-w-0 active:translate-y-0"
                  key={i}
                  onClick={(e) => {
                    e.stopPropagation();
                    openSubagent(prompt, undefined, undefined, props.parentSessionId);
                  }}
                  title={prompt}
                >
                  {label}
                </Button>
              );
            })
          ) : (
            <Button
              variant="ghost"
              size="xs"
              className="msg-tool-subagent-link h-auto min-w-0 active:translate-y-0"
              onClick={(e) => {
                e.stopPropagation();
                openSubagent(agentDescription ?? summary, agentNickname, agentId, props.parentSessionId);
              }}
              title={t("tool.openSubagentTitle")}
            >
              {t("tool.openSubagent")}
            </Button>
          ))}
      </div>
      {expanded && (
        <div className="msg-tool-body">
          {showInputDetail() && (
            <div className="msg-tool-detail">
              {formatted!.lines.map((line, i) => (
                <div className="msg-tool-field" key={i}>
                  <span className="msg-tool-field-label">{line.label}</span>
                  <pre className="msg-tool-field-value">{line.value}</pre>
                </div>
              ))}
              {formatted!.diff && <SessionLineDiffView oldText={formatted!.diff!.old} newText={formatted!.diff!.new} />}
              {formatted!.patchDiff && <SessionDiffView lines={formatted!.patchDiff!} />}
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
                <SessionLineDiffView oldText={resultMetadata.diff.old} newText={resultMetadata.diff.new} />
              )}
              {resultMetadata.patchDiff && <SessionDiffView lines={resultMetadata.patchDiff} />}
              {persistedOutputPath() && (
                <Button
                  variant="ghost"
                  size="xs"
                  className="msg-tool-subagent-link h-auto min-w-0 active:translate-y-0"
                  disabled={loadingFullResult}
                  onClick={(event) => {
                    event.stopPropagation();
                    void loadFullResult();
                  }}
                  type="button"
                >
                  {loadingFullResult ? t("common.loading") : t("tool.loadFullResult")}
                </Button>
              )}
              {fullResultError && <pre className="msg-tool-input">{fullResultError}</pre>}
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
                        onPreview={(src, source) => setPreviewImage({ src, source })}
                      />
                    );
                  }
                  return (
                    <RemoteImage
                      key={i}
                      src={seg.content}
                      onPreview={(src, source) => setPreviewImage({ src, source })}
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
                  label={agentChildPrompts[i]?.replace(/\s+/g, " ").trim() || childId}
                />
              ))
            ) : canOpenSingleAgent && (agentNickname || agentId || agentDescription) ? (
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
        </div>
      )}
      {previewImage && (
        <ImagePreview src={previewImage.src} source={previewImage.source} onClose={() => setPreviewImage(null)} />
      )}
    </div>
  );
}
