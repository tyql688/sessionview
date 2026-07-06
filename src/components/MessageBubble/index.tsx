import { useState, useMemo, useId } from "react";
import type { Message, Provider } from "../../lib/types";
import { ProviderIcon, UserIcon } from "../icons";
import { useI18n } from "../../i18n/index";
import { parseTimestamp } from "../../lib/formatters";
import {
  parseMarkdownDocument,
  renderParsedMarkdown,
  sanitizeMessageForClipboard,
} from "./MarkdownRenderer";
import { ImagePreview } from "./ImagePreview";
import { ThinkingBlock } from "./ThinkingBlock";
import { CopyMessageButton, TokenUsageDisplay } from "./TokenUsage";
import { ToolMessage } from "./ToolMessage";

// Re-export for backward compatibility
export { ProviderIcon } from "../icons";
export { formatMcpLabel } from "./ToolMessage";

const SYSTEM_SUBTYPE_CONFIG: Record<
  string,
  { icon: string; labelKey: string; cls: string }
> = {
  turn_duration: {
    icon: "\u23F1",
    labelKey: "system.turnDuration",
    cls: "sys-duration",
  },
  compact_boundary: {
    icon: "\u2702",
    labelKey: "system.compact",
    cls: "sys-compact",
  },
  microcompact_boundary: {
    icon: "\u2702",
    labelKey: "system.microcompact",
    cls: "sys-compact",
  },
  stop_hook_summary: {
    icon: "\u2699",
    labelKey: "system.hooks",
    cls: "sys-hook",
  },
  api_error: { icon: "\u26A0", labelKey: "system.apiError", cls: "sys-error" },
  away_summary: {
    icon: "\u23F8",
    labelKey: "system.awaySummary",
    cls: "sys-info",
  },
  scheduled_task_fire: {
    icon: "\u23F0",
    labelKey: "system.scheduledTask",
    cls: "sys-info",
  },
  pr_link: { icon: "\uD83D\uDD17", labelKey: "system.prLink", cls: "sys-info" },
  error: { icon: "\u26A0", labelKey: "system.error", cls: "sys-error" },
  turn_aborted: {
    icon: "\u23F9",
    labelKey: "system.turnAborted",
    cls: "sys-error",
  },
  context_compacted: {
    icon: "\u2702",
    labelKey: "system.contextCompacted",
    cls: "sys-compact",
  },
};

const LEGACY_LOCAL_COMMAND_PREFIX = "[local_command]";

function SystemMessage(props: { content: string }) {
  const { t } = useI18n();
  const match = props.content.match(/^\[(\w+)\]\s*(.*)/s);
  if (match) {
    const config = SYSTEM_SUBTYPE_CONFIG[match[1]];
    if (config) {
      return (
        <div className={`msg-system msg-system-tag ${config.cls}`}>
          <span className="sys-icon">{config.icon}</span>
          <span className="sys-label">{t(config.labelKey)}</span>
          <span className="sys-detail">{match[2]}</span>
        </div>
      );
    }
  }
  return <div className="msg-system">{props.content}</div>;
}

export function MessageBubble(props: {
  message: Message;
  provider?: Provider;
  parentSessionId?: string;
  highlightTerm?: string;
}) {
  const footnotePrefix = useId();
  const [previewImage, setPreviewImage] = useState<{
    src: string;
    source?: string;
  } | null>(null);

  const isEmpty = (): boolean => {
    const msg = props.message;
    if (msg.role === "tool") {
      // Hide tool_result entries (toulu_ IDs from Anthropic API)
      if (msg.tool_name?.startsWith("toolu_") && !msg.tool_metadata) {
        return true;
      }
      return !msg.content && !msg.tool_input && !msg.tool_name;
    }
    return !msg.content || msg.content.trim().length === 0;
  };

  const isSystemContent = (): boolean => {
    const msg = props.message;
    if (msg.role === "tool") return false;
    if (!msg.content || msg.content.trim().length === 0) return false;
    const c = msg.content.trimStart();
    // Skip known system/template content markers
    const systemMarkers = [
      "</observation>",
      "</command-message>",
      "<INSTRUCTIONS>",
      "<environment_context>",
      "<permissions instructions>",
      "</facts>",
      "</narrative>",
      "</concepts>",
      "<system-reminder>",
    ];
    return systemMarkers.some((marker) => c.includes(marker));
  };

  const hasLegacyLocalCommandPrefix = () =>
    props.message.content.trimStart().startsWith(LEGACY_LOCAL_COMMAND_PREFIX);

  const isCommandMessage = () =>
    props.message.message_kind === "command_input" ||
    props.message.message_kind === "command_output" ||
    ((props.message.role === "user" || props.message.role === "assistant") &&
      hasLegacyLocalCommandPrefix());

  const displayContent = useMemo(() => {
    if (!hasLegacyLocalCommandPrefix()) return props.message.content;
    return props.message.content
      .trimStart()
      .slice(LEGACY_LOCAL_COMMAND_PREFIX.length)
      .trimStart();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [props.message.content]);

  const rendersMarkdown = () =>
    props.message.role !== "tool" &&
    props.message.role !== "system" &&
    !isEmpty() &&
    !isSystemContent();

  const copyText = useMemo(
    () =>
      rendersMarkdown() ? sanitizeMessageForClipboard(displayContent) : "",
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [props.message, displayContent],
  );
  // Split the markdown parse (expensive, content-only) from the render
  // (highlight-dependent). Keying the parse on content alone means committing a
  // new in-session Cmd+F query re-renders highlights without re-parsing the AST
  // of every visible bubble — the prior jank source.
  const parsedMarkdown = useMemo(() => {
    if (!rendersMarkdown()) return null;
    return parseMarkdownDocument(displayContent);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [props.message, displayContent]);
  const markdownContent = useMemo(() => {
    const parsed = parsedMarkdown;
    if (!parsed) return null;
    return renderParsedMarkdown(parsed, {
      footnotePrefix,
      highlightTerm: props.highlightTerm,
      onPreview: (src, source) => setPreviewImage({ src, source }),
    });
  }, [parsedMarkdown, props.highlightTerm, footnotePrefix]);
  const msgTs = useMemo(() => {
    const ts = props.message.timestamp;
    if (!ts) return null;
    const ms = parseTimestamp(ts);
    if (!ms) return null;
    const d = new Date(ms);
    return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
  }, [props.message.timestamp]);

  if (isEmpty() || isSystemContent()) return null;

  return (
    <>
      {props.message.role !== "tool" ? (
        props.message.role !== "system" ? (
          <>
            <div className={`msg-row msg-row-${props.message.role}`}>
              <div
                className={`msg-avatar msg-avatar-${props.message.role}${props.message.role === "assistant" ? ` ${props.provider ?? "claude"}` : ""}`}
              >
                {props.message.role === "user" ? (
                  <UserIcon />
                ) : (
                  <ProviderIcon provider={props.provider ?? "claude"} />
                )}
              </div>
              <div
                className={`msg-bubble msg-bubble-${props.message.role}${isCommandMessage() ? " msg-bubble-command" : ""}`}
              >
                {markdownContent}
                <CopyMessageButton
                  content={displayContent}
                  copyText={copyText}
                />
                {msgTs && (
                  <div className="msg-bubble-footer">
                    <span className="msg-bubble-ts">{msgTs}</span>
                  </div>
                )}
              </div>
            </div>
            {props.message.role === "assistant" &&
              (props.message.token_usage || props.message.model) && (
                <div className="msg-token-row">
                  {props.message.model && (
                    <span className="msg-model-label">
                      {props.message.model}
                    </span>
                  )}
                  {props.message.token_usage && (
                    <TokenUsageDisplay usage={props.message.token_usage!} />
                  )}
                </div>
              )}
          </>
        ) : props.message.content.startsWith("[thinking]\n") ? (
          <ThinkingBlock
            content={props.message.content.slice("[thinking]\n".length)}
          />
        ) : (
          <SystemMessage content={props.message.content} />
        )
      ) : (
        <ToolMessage
          message={props.message}
          provider={props.provider}
          parentSessionId={props.parentSessionId}
        />
      )}
      {previewImage && (
        <ImagePreview
          src={previewImage.src}
          source={previewImage.source}
          onClose={() => setPreviewImage(null)}
        />
      )}
    </>
  );
}
