import { createSignal, createMemo, Show, createUniqueId } from "solid-js";
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

function SystemMessage(props: { content: string }) {
  const { t } = useI18n();
  const match = props.content.match(/^\[(\w+)\]\s*(.*)/s);
  if (match) {
    const config = SYSTEM_SUBTYPE_CONFIG[match[1]];
    if (config) {
      return (
        <div class={`msg-system msg-system-tag ${config.cls}`}>
          <span class="sys-icon">{config.icon}</span>
          <span class="sys-label">{t(config.labelKey)}</span>
          <span class="sys-detail">{match[2]}</span>
        </div>
      );
    }
  }
  return <div class="msg-system">{props.content}</div>;
}

export function MessageBubble(props: {
  message: Message;
  provider?: Provider;
  highlightTerm?: string;
}) {
  const footnotePrefix = createUniqueId();
  const [previewImage, setPreviewImage] = createSignal<{
    src: string;
    source?: string;
  } | null>(null);
  const copyText = createMemo(() =>
    sanitizeMessageForClipboard(props.message.content),
  );
  // Split the markdown parse (expensive, content-only) from the render
  // (highlight-dependent). Keying the parse on content alone means committing a
  // new in-session Cmd+F query re-renders highlights without re-parsing the AST
  // of every visible bubble — the prior jank source.
  const parsedMarkdown = createMemo(() =>
    parseMarkdownDocument(props.message.content),
  );
  const markdownContent = createMemo(() =>
    renderParsedMarkdown(parsedMarkdown(), {
      footnotePrefix,
      highlightTerm: props.highlightTerm,
      onPreview: (src, source) => setPreviewImage({ src, source }),
    }),
  );
  const msgTs = createMemo(() => {
    const ts = props.message.timestamp;
    if (!ts) return null;
    const ms = parseTimestamp(ts);
    if (!ms) return null;
    const d = new Date(ms);
    return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
  });

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

  if (isEmpty() || isSystemContent()) return null;

  return (
    <>
      <Show
        when={props.message.role !== "tool"}
        fallback={
          <ToolMessage message={props.message} provider={props.provider} />
        }
      >
        <Show
          when={props.message.role !== "system"}
          fallback={
            props.message.content.startsWith("[thinking]\n") ? (
              <ThinkingBlock
                content={props.message.content.slice("[thinking]\n".length)}
              />
            ) : (
              <SystemMessage content={props.message.content} />
            )
          }
        >
          <div class={`msg-row msg-row-${props.message.role}`}>
            <div
              class={`msg-avatar msg-avatar-${props.message.role}${props.message.role === "assistant" ? ` ${props.provider ?? "claude"}` : ""}`}
            >
              <Show
                when={props.message.role === "user"}
                fallback={
                  <ProviderIcon provider={props.provider ?? "claude"} />
                }
              >
                <UserIcon />
              </Show>
            </div>
            <div class={`msg-bubble msg-bubble-${props.message.role}`}>
              {markdownContent()}
              <CopyMessageButton
                content={props.message.content}
                copyText={copyText()}
              />
              <Show when={msgTs()}>
                <div class="msg-bubble-footer">
                  <span class="msg-bubble-ts">{msgTs()}</span>
                </div>
              </Show>
            </div>
          </div>
          <Show
            when={
              props.message.role === "assistant" &&
              (props.message.token_usage || props.message.model)
            }
          >
            <div class="msg-token-row">
              <Show when={props.message.model}>
                <span class="msg-model-label">{props.message.model}</span>
              </Show>
              <Show when={props.message.token_usage}>
                <TokenUsageDisplay usage={props.message.token_usage!} />
              </Show>
            </div>
          </Show>
        </Show>
      </Show>
      <Show when={previewImage()}>
        <ImagePreview
          src={previewImage()!.src}
          source={previewImage()!.source}
          onClose={() => setPreviewImage(null)}
        />
      </Show>
    </>
  );
}
