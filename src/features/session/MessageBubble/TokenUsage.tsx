import { useState } from "react";
import type { TokenUsage } from "@/lib/types";
import { useI18n } from "@/i18n/index";
import { toastError } from "@/stores/toast";

export function CopyMessageButton(props: {
  content: string;
  copyText?: string;
}) {
  const { t } = useI18n();
  const [copied, setCopied] = useState(false);

  async function handleCopy() {
    try {
      await navigator.clipboard.writeText(props.copyText ?? props.content);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch (error) {
      console.error("Failed to copy message:", error);
      toastError(t("toast.copyFailed"));
    }
  }

  return (
    <button
      className="msg-copy-btn"
      onClick={handleCopy}
      title={t("common.copyMessage")}
      aria-label={t("common.copyMessage")}
    >
      {copied ? (
        <svg
          width="12"
          height="12"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth="2"
          strokeLinecap="round"
        >
          <polyline points="20 6 9 17 4 12" />
        </svg>
      ) : (
        <svg
          width="12"
          height="12"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth="2"
          strokeLinecap="round"
        >
          <rect x="9" y="9" width="13" height="13" rx="2" ry="2" />
          <path d="M5 15H4a2 2 0 01-2-2V4a2 2 0 012-2h9a2 2 0 012 2v1" />
        </svg>
      )}
    </button>
  );
}

export function TokenUsageDisplay(props: { usage: TokenUsage }) {
  const { t } = useI18n();
  const fmt = (n: number) => n.toLocaleString();
  const cached = props.usage.cache_read_input_tokens;
  const created = props.usage.cache_creation_input_tokens;
  return (
    <div className="msg-token-usage">
      <span title={t("common.inputTokens")}>
        ↑{fmt(props.usage.input_tokens)}
      </span>
      <span className="msg-token-sep">·</span>
      <span title={t("common.outputTokens")}>
        ↓{fmt(props.usage.output_tokens)}
      </span>
      {cached > 0 && (
        <>
          <span className="msg-token-sep">·</span>
          <span
            className="msg-token-cached"
            title={t("common.cacheReadTokens")}
          >
            {t("common.cacheRead")} {fmt(cached)}
          </span>
        </>
      )}
      {created > 0 && (
        <>
          <span className="msg-token-sep">·</span>
          <span
            className="msg-token-cache-write"
            title={t("common.cacheWriteTokens")}
          >
            {t("common.cacheWrite")} {fmt(created)}
          </span>
        </>
      )}
    </div>
  );
}
