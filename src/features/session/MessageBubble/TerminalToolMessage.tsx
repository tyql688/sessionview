import { Check, ChevronDown, ChevronRight, Copy, Terminal, WrapText } from "lucide-react";
import { useMemo, useState } from "react";
import { Button } from "@/components/ui/button";
import { useI18n } from "@/i18n/index";
import type { Message } from "@/lib/types";
import { readToolResultText } from "@/lib/tauri";
import { formatToolInput, formatToolResultMetadata, toolSummary } from "@/lib/tools";
import { parseContent } from "@/lib/message-content";
import { toastError } from "@/stores/toast";
import { COPY_FEEDBACK_MS } from "@/features/session/MessageBubble/TokenUsage";
import { ImagePreview, LocalImage, RemoteImage, isLocalPath } from "@/features/session/MessageBubble/ImagePreview";

type ToolDetail = NonNullable<ReturnType<typeof formatToolResultMetadata>>;

type CopyTarget = "command" | "output";

interface TerminalData {
  command: string;
  cwd: string;
  source: string;
  exitCode: string;
  duration: string;
  status: string;
  stdout: string;
  stderr: string;
  persistedOutputPath: string;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function valueToText(value: unknown): string {
  if (typeof value === "string") return value;
  if (typeof value === "number" || typeof value === "boolean") return String(value);
  if (Array.isArray(value)) {
    const values = value.map(valueToText).filter((part) => part.length > 0);
    return values.join(" ");
  }
  return "";
}

function parseJsonRecord(raw: string | null, context: string): Record<string, unknown> | null {
  if (!raw) return null;
  const trimmed = raw.trim();
  if (!trimmed.startsWith("{")) return null;
  try {
    const parsed: unknown = JSON.parse(trimmed);
    return isRecord(parsed) ? parsed : null;
  } catch (error) {
    console.warn(`failed to parse terminal tool ${context} JSON:`, error);
    return null;
  }
}

function fieldValue(record: Record<string, unknown> | null | undefined, keys: string[]): string {
  if (!record) return "";
  for (const key of keys) {
    const text = valueToText(record[key]);
    if (text.length > 0) return text;
  }
  return "";
}

function detailLineValue(detail: ToolDetail | null | undefined, labels: string[]): string {
  if (!detail) return "";
  const wanted = new Set(labels.map((label) => label.toLowerCase()));
  const line = detail.lines.find((entry) => wanted.has(entry.label.toLowerCase()));
  return line?.value ?? "";
}

function firstText(...values: string[]): string {
  return values.find((value) => value.trim().length > 0) ?? "";
}

function normalizeDuration(duration: string): string {
  const trimmed = duration.trim();
  if (trimmed.length === 0) return "";
  if (/[a-z]/iu.test(trimmed)) return trimmed;
  return `${trimmed}s`;
}

function statusKind(status: string, exitCode: string): "success" | "error" | "neutral" {
  const normalized = status.toLowerCase();
  if (exitCode.trim().length > 0) {
    return exitCode.trim() === "0" ? "success" : "error";
  }
  if (["success", "completed", "ok", "done"].includes(normalized)) return "success";
  if (["failed", "failure", "error", "cancelled", "canceled", "timeout"].includes(normalized)) return "error";
  return "neutral";
}

function displayStatus(
  status: string,
  exitCode: string,
  t: (key: string, options?: Record<string, unknown>) => string,
): string {
  const kind = statusKind(status, exitCode);
  if (kind === "success") return t("tool.statusSuccess");
  if (kind === "error") return t("tool.statusFailed");
  return status;
}

function collapsedSummary(message: Message, t: (key: string, options?: Record<string, unknown>) => string): string {
  const raw = toolSummary(message).trim();
  if (raw.length === 0) return "";
  const command = fieldValue(parseJsonRecord(message.tool_input, "input"), ["command", "cmd", "CommandLine"]);
  return raw === command ? "" : raw || t("tool.terminal");
}

function buildTerminalData(message: Message): TerminalData {
  const inputRecord = parseJsonRecord(message.tool_input, "input");
  const contentRecord = parseJsonRecord(message.content, "content");
  const structured = isRecord(message.tool_metadata?.structured) ? message.tool_metadata.structured : null;
  const inputDetail = formatToolInput(message);
  const resultDetail = formatToolResultMetadata(message.tool_metadata);
  const providerOutput = message.content.trim().length > 0 ? message.content : "";

  const command = firstText(
    detailLineValue(inputDetail, ["command", "raw"]),
    fieldValue(inputRecord, ["command", "cmd", "CommandLine"]),
    fieldValue(structured, ["command", "cmd", "CommandLine"]),
    message.tool_input ?? "",
  );
  const stdout = providerOutput
    ? providerOutput
    : firstText(
        fieldValue(structured, ["stdout", "output", "aggregated_output", "formatted_output"]),
        fieldValue(contentRecord, ["stdout", "output", "aggregated_output", "formatted_output"]),
        detailLineValue(resultDetail, ["stdout", "output"]),
      );

  return {
    command,
    cwd: firstText(fieldValue(structured, ["cwd"]), detailLineValue(resultDetail, ["cwd"])),
    source: firstText(fieldValue(structured, ["source"]), detailLineValue(resultDetail, ["source"])),
    exitCode: firstText(fieldValue(structured, ["exitCode", "exit_code"]), detailLineValue(resultDetail, ["exit"])),
    duration: normalizeDuration(
      firstText(
        fieldValue(structured, ["durationSeconds", "duration_seconds"]),
        detailLineValue(resultDetail, ["duration"]),
      ),
    ),
    status: firstText(message.tool_metadata?.status ?? "", detailLineValue(resultDetail, ["status"])),
    stdout,
    stderr: providerOutput
      ? ""
      : firstText(
          fieldValue(structured, ["stderr"]),
          fieldValue(contentRecord, ["stderr"]),
          detailLineValue(resultDetail, ["stderr"]),
        ),
    persistedOutputPath: resultDetail?.persistedOutputPath ?? "",
  };
}

export function TerminalToolMessage(props: { message: Message }) {
  const { t } = useI18n();
  const [expanded, setExpanded] = useState(false);
  const [wrapOutput, setWrapOutput] = useState(false);
  const [copiedTarget, setCopiedTarget] = useState<CopyTarget | null>(null);
  const [fullResult, setFullResult] = useState<string | null>(null);
  const [fullResultError, setFullResultError] = useState<string | null>(null);
  const [loadingFullResult, setLoadingFullResult] = useState(false);
  const [previewImage, setPreviewImage] = useState<{ src: string; source?: string } | null>(null);
  const data = useMemo(() => buildTerminalData(props.message), [props.message]);
  const summary = useMemo(() => collapsedSummary(props.message, t), [props.message, t]);
  const stdout = fullResult ?? data.stdout;
  const structuredImageSources = useMemo(
    () => formatToolResultMetadata(props.message.tool_metadata)?.media ?? [],
    [props.message.tool_metadata],
  );
  const outputSegments = useMemo(
    () => parseContent(stdout, structuredImageSources, { parseCodeFences: false }),
    [stdout, structuredImageSources],
  );
  const outputLabel = props.message.content.trim().length > 0 ? t("tool.output") : t("tool.stdout");
  const kind = statusKind(data.status, data.exitCode);
  const status = displayStatus(data.status, data.exitCode, t);
  const toolLabel = props.message.tool_metadata?.display_name || props.message.tool_name || t("tool.terminal");
  const hasPrimaryOutput = stdout.trim().length > 0 || structuredImageSources.length > 0;
  const hasOutput = hasPrimaryOutput || data.stderr.trim().length > 0;
  const canExpand =
    data.command.length > 0 || hasOutput || data.cwd.length > 0 || data.source.length > 0 || !!data.persistedOutputPath;
  const meta = [
    data.exitCode && data.exitCode !== "0" ? t("tool.exitCode", { code: data.exitCode }) : "",
    data.duration,
  ].filter((value) => value.length > 0);

  async function copyText(text: string, target: CopyTarget) {
    try {
      await navigator.clipboard.writeText(text);
      setCopiedTarget(target);
      window.setTimeout(() => setCopiedTarget(null), COPY_FEEDBACK_MS);
    } catch (error) {
      console.error("Failed to copy terminal tool text:", error);
      toastError(t("toast.copyFailed"));
    }
  }

  async function loadFullResult() {
    if (!data.persistedOutputPath || loadingFullResult) return;

    setLoadingFullResult(true);
    setFullResultError(null);
    try {
      setFullResult(await readToolResultText(data.persistedOutputPath));
    } catch (error) {
      console.error("Failed to load full terminal tool result:", error);
      setFullResultError(String(error));
    } finally {
      setLoadingFullResult(false);
    }
  }

  return (
    <div className={`terminal-tool terminal-tool-${kind}${expanded ? " expanded" : ""}`}>
      <div className="terminal-tool-header">
        {canExpand ? (
          <button
            type="button"
            className="terminal-tool-toggle"
            onClick={() => setExpanded((value) => !value)}
            aria-expanded={expanded}
          >
            <Terminal className="terminal-tool-icon" aria-hidden="true" />
            <span className="terminal-tool-name">{toolLabel}</span>
            <span className="terminal-tool-status-dot" title={status} aria-hidden="true" />
            {summary ? <span className="terminal-tool-summary">{summary}</span> : null}
            {meta.length > 0 && <span className="terminal-tool-meta">{meta.join(" · ")}</span>}
            {expanded ? (
              <ChevronDown className="terminal-tool-chevron" aria-hidden="true" />
            ) : (
              <ChevronRight className="terminal-tool-chevron" aria-hidden="true" />
            )}
          </button>
        ) : (
          <div className="terminal-tool-toggle terminal-tool-toggle-static">
            <Terminal className="terminal-tool-icon" aria-hidden="true" />
            <span className="terminal-tool-name">{toolLabel}</span>
            <span className="terminal-tool-status-dot" title={status} aria-hidden="true" />
            {summary ? <span className="terminal-tool-summary">{summary}</span> : null}
            {meta.length > 0 && <span className="terminal-tool-meta">{meta.join(" · ")}</span>}
          </div>
        )}
        {expanded && data.command && (
          <div className="terminal-tool-actions">
            <Button
              type="button"
              variant="ghost"
              size="icon-xs"
              className="terminal-tool-action"
              onClick={() => void copyText(data.command, "command")}
              title={t("tool.copyCommand")}
              aria-label={t("tool.copyCommand")}
            >
              {copiedTarget === "command" ? <Check className="size-3" /> : <Copy className="size-3" />}
            </Button>
          </div>
        )}
      </div>

      {expanded && (
        <div className="terminal-tool-body">
          {data.command && (
            <div className="terminal-tool-command">
              <span className="terminal-tool-prompt" aria-hidden="true">
                $
              </span>
              <pre className="terminal-tool-command-text">{data.command}</pre>
            </div>
          )}

          {hasOutput ? (
            <>
              {hasPrimaryOutput && (
                <section className="terminal-tool-stream">
                  <div className="terminal-tool-stream-header">
                    <span>{outputLabel}</span>
                    {stdout.trim().length > 0 && (
                      <div className="terminal-tool-stream-actions">
                        <Button
                          type="button"
                          variant="ghost"
                          size="icon-xs"
                          className="terminal-tool-action"
                          onClick={() => setWrapOutput((value) => !value)}
                          title={t("tool.wrapOutput")}
                          aria-label={t("tool.wrapOutput")}
                        >
                          <WrapText className="size-3" />
                        </Button>
                        <Button
                          type="button"
                          variant="ghost"
                          size="icon-xs"
                          className="terminal-tool-action"
                          onClick={() => void copyText(stdout, "output")}
                          title={t("tool.copyOutput")}
                          aria-label={t("tool.copyOutput")}
                        >
                          {copiedTarget === "output" ? <Check className="size-3" /> : <Copy className="size-3" />}
                        </Button>
                      </div>
                    )}
                  </div>
                  <div className="terminal-tool-output msg-tool-output">
                    {outputSegments.map((segment, index) => {
                      if (segment.type === "image") {
                        if (isLocalPath(segment.content)) {
                          return (
                            <LocalImage
                              key={index}
                              path={segment.content}
                              onPreview={(src, source) => setPreviewImage({ src, source })}
                            />
                          );
                        }
                        return (
                          <RemoteImage
                            key={index}
                            src={segment.content}
                            onPreview={(src, source) => setPreviewImage({ src, source })}
                          />
                        );
                      }
                      if (!segment.content) return null;
                      return (
                        <pre key={index} className={`terminal-tool-code${wrapOutput ? " wrap" : ""}`}>
                          {segment.content}
                        </pre>
                      );
                    })}
                  </div>
                </section>
              )}
              {data.stderr.trim().length > 0 && (
                <section className="terminal-tool-stream terminal-tool-stream-error">
                  <div className="terminal-tool-stream-header">
                    <span>{t("tool.stderr")}</span>
                  </div>
                  <div className="terminal-tool-output terminal-tool-error-output">
                    <pre className={`terminal-tool-code${wrapOutput ? " wrap" : ""}`}>{data.stderr}</pre>
                  </div>
                </section>
              )}
            </>
          ) : (
            <div className="terminal-tool-empty">{t("tool.noOutput")}</div>
          )}

          {(data.cwd || data.source || data.persistedOutputPath) && (
            <div className="terminal-tool-footer">
              {data.cwd && (
                <span>
                  {t("tool.cwd")} <code>{data.cwd}</code>
                </span>
              )}
              {data.source && (
                <span>
                  {t("tool.source")} <code>{data.source}</code>
                </span>
              )}
              {data.persistedOutputPath && (
                <Button
                  type="button"
                  variant="ghost"
                  size="xs"
                  className="terminal-tool-load-full active:translate-y-0"
                  disabled={loadingFullResult}
                  onClick={() => void loadFullResult()}
                >
                  {loadingFullResult ? t("common.loading") : t("tool.loadFullResult")}
                </Button>
              )}
            </div>
          )}
          {fullResultError && <pre className="terminal-tool-load-error">{fullResultError}</pre>}
        </div>
      )}
      {previewImage && (
        <ImagePreview src={previewImage.src} source={previewImage.source} onClose={() => setPreviewImage(null)} />
      )}
    </div>
  );
}
