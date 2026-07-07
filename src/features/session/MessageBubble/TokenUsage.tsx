import { Check, Copy } from "lucide-react";
import { useState } from "react";
import { Button } from "@/components/ui/button";
import { useI18n } from "@/i18n/index";
import type { TokenUsage } from "@/lib/types";
import { toastError } from "@/stores/toast";

/** How long a copy button shows its "copied ✓" state before resetting. */
export const COPY_FEEDBACK_MS = 1500;

export function CopyMessageButton(props: { content: string; copyText?: string }) {
  const { t } = useI18n();
  const [copied, setCopied] = useState(false);

  async function handleCopy() {
    try {
      await navigator.clipboard.writeText(props.copyText ?? props.content);
      setCopied(true);
      setTimeout(() => setCopied(false), COPY_FEEDBACK_MS);
    } catch (error) {
      console.error("Failed to copy message:", error);
      toastError(t("toast.copyFailed"));
    }
  }

  return (
    // msg-copy-btn stays as the hover-reveal hook (messages.css shows it on
    // bubble hover); the visual base comes from the shared Button.
    <Button
      variant="ghost"
      size="icon-xs"
      className="msg-copy-btn"
      onClick={handleCopy}
      title={t("common.copyMessage")}
      aria-label={t("common.copyMessage")}
    >
      {copied ? (
        <Check className="size-3 text-success" aria-hidden="true" />
      ) : (
        <Copy className="size-3" aria-hidden="true" />
      )}
    </Button>
  );
}

export function TokenUsageDisplay(props: { usage: TokenUsage }) {
  const { t } = useI18n();
  const fmt = (n: number) => n.toLocaleString();
  const cached = props.usage.cache_read_input_tokens;
  const created = props.usage.cache_creation_input_tokens;
  return (
    <div className="msg-token-usage">
      <span title={t("common.inputTokens")}>↑{fmt(props.usage.input_tokens)}</span>
      <span className="msg-token-sep">·</span>
      <span title={t("common.outputTokens")}>↓{fmt(props.usage.output_tokens)}</span>
      {cached > 0 && (
        <>
          <span className="msg-token-sep">·</span>
          <span className="msg-token-cached" title={t("common.cacheReadTokens")}>
            {t("common.cacheRead")} {fmt(cached)}
          </span>
        </>
      )}
      {created > 0 && (
        <>
          <span className="msg-token-sep">·</span>
          <span className="msg-token-cache-write" title={t("common.cacheWriteTokens")}>
            {t("common.cacheWrite")} {fmt(created)}
          </span>
        </>
      )}
    </div>
  );
}
