import { lazy, Suspense, useState, useMemo } from "react";
import { Button } from "@/components/ui/button";
import type { Message, Provider } from "@/lib/types";
import { ProviderIcon, UserIcon } from "@/components/icons";
import { useI18n } from "@/i18n/index";
import { parseTimestamp } from "@/lib/formatters";
import {
  extractImages,
  sanitizeMessageForClipboard,
} from "@/lib/message-content";
import {
  ImagePreview,
  isLocalPath,
  LocalImage,
  RemoteImage,
} from "@/features/session/MessageBubble/ImagePreview";
import { ThinkingBlock } from "@/features/session/MessageBubble/ThinkingBlock";
import {
  CopyMessageButton,
  TokenUsageDisplay,
} from "@/features/session/MessageBubble/TokenUsage";
import { ToolMessage } from "@/features/session/MessageBubble/ToolMessage";

// The markdown engine (streamdown + shiki/katex/mermaid plugins) is by far
// the heaviest frontend dependency — load it on demand so the app shell and
// explorer render without it. Rendering per bubble is eager: the virtualizer
// only mounts the rows near the viewport, so each mount parses exactly one
// message. The fallback shows the raw text during the one-time chunk load.
const Markdown = lazy(() =>
  import("@/features/session/timeline/Markdown").then((module) => ({
    default: module.Markdown,
  })),
);

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
  const [expanded, setExpanded] = useState(false);
  const match = props.content.match(/^\[(\w+)\]\s*(.*)/s);
  const config = match ? SYSTEM_SUBTYPE_CONFIG[match[1]] : undefined;
  if (!match || !config) {
    return <div className="msg-system">{props.content}</div>;
  }

  const detail = match[2].trim();
  const collapsible = detail.includes("\n");
  const summary = collapsible ? (detail.split("\n", 1)[0] ?? "") : detail;

  if (!collapsible) {
    return (
      <div className={`msg-system msg-system-tag ${config.cls}`}>
        <span className="sys-icon">{config.icon}</span>
        <span className="sys-label">{t(config.labelKey)}</span>
        <span className="sys-detail">{detail}</span>
      </div>
    );
  }

  // Long payloads (hook output, compaction summaries) collapse to one quiet
  // row; the full text renders only on demand.
  return (
    <div className={`msg-system-block ${config.cls}`}>
      <Button
        variant="ghost"
        type="button"
        className="msg-system msg-system-tag msg-system-toggle h-auto justify-start active:translate-y-0"
        aria-expanded={expanded}
        onClick={() => setExpanded((v) => !v)}
      >
        <span className="sys-icon">{config.icon}</span>
        <span className="sys-label">{t(config.labelKey)}</span>
        <span className="sys-detail">{expanded ? "" : summary}</span>
        <span
          className={`sys-chevron${expanded ? " sys-chevron-open" : ""}`}
          aria-hidden="true"
        >
          {"\u203A"}
        </span>
      </Button>
      {expanded && <pre className="msg-system-body">{detail}</pre>}
    </div>
  );
}

export function MessageBubble(props: {
  message: Message;
  provider?: Provider;
  parentSessionId?: string;
}) {
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
  // The backend embeds images as `[Image: source: …]` text placeholders that
  // the markdown renderer doesn't understand — strip them out and render the
  // image strip separately below the prose.
  const { markdown: displayMarkdown, images } = useMemo(
    () => extractImages(displayContent),
    [displayContent],
  );
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
                  <ProviderIcon
                    provider={props.provider ?? "claude"}
                    size={20}
                  />
                )}
              </div>
              <div
                className={`msg-bubble msg-bubble-${props.message.role}${isCommandMessage() ? " msg-bubble-command" : ""}`}
              >
                <Suspense
                  fallback={
                    <div className="whitespace-pre-wrap">{displayMarkdown}</div>
                  }
                >
                  <Markdown text={displayMarkdown} />
                </Suspense>
                {images.length > 0 && (
                  <div className="msg-image-strip">
                    {images.map((image, i) =>
                      image.source === null ? null : isLocalPath(
                          image.source,
                        ) ? (
                        <LocalImage
                          key={i}
                          path={image.source}
                          onPreview={(src, source) =>
                            setPreviewImage({ src, source })
                          }
                        />
                      ) : (
                        <RemoteImage
                          key={i}
                          src={image.source}
                          onPreview={(src, source) =>
                            setPreviewImage({ src, source })
                          }
                        />
                      ),
                    )}
                  </div>
                )}
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
