import { ChevronDown, ExternalLink } from "lucide-react";
import { useState } from "react";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { useI18n } from "@/i18n/index";
import { fmtTokens } from "@/lib/formatters";
import { matchesSubagentSession, type SubagentMatchRequest } from "@/lib/subagent";
import { getChildSessions, getSessionOpenWindow } from "@/lib/tauri";
import type { Message, SessionMeta } from "@/lib/types";
import { errorMessage } from "@/lib/errors";
import { openSession } from "@/features/editor/editorGroups";

const TAIL_MESSAGES = 4;

interface LoadedPreview {
  meta: SessionMeta;
  tail: Message[];
}

/**
 * Collapsible inline peek into a subagent session, rendered inside the parent
 * Task tool card: resolves the child on first expand, then shows its stats
 * and the last few dialogue messages without leaving the parent session.
 * The external-link button opens the full session in a tab.
 */
export function SubagentInline(props: {
  parentSessionId: string;
  request: SubagentMatchRequest;
  /** Short identity shown in the header (child prompt or agent id). */
  label: string | null;
}) {
  const { t } = useI18n();
  const [expanded, setExpanded] = useState(false);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [preview, setPreview] = useState<LoadedPreview | null>(null);

  async function loadPreview() {
    setLoading(true);
    setError(null);
    try {
      const children = await getChildSessions(props.parentSessionId);
      const match = children.find((candidate) =>
        matchesSubagentSession(candidate, props.parentSessionId, props.request),
      );
      if (!match) {
        setError(t("toast.subagentNotFound"));
        return;
      }
      const open = await getSessionOpenWindow(match.id, -TAIL_MESSAGES * 3, TAIL_MESSAGES * 3);
      const tail = open.window.messages
        .filter((msg) => (msg.role === "user" || msg.role === "assistant") && msg.content.trim().length > 0)
        .slice(-TAIL_MESSAGES);
      setPreview({ meta: open.meta, tail });
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setLoading(false);
    }
  }

  function toggle() {
    const next = !expanded;
    setExpanded(next);
    if (next && !preview && !loading) void loadPreview();
  }

  const meta = preview?.meta;
  const totalTokens = meta
    ? meta.input_tokens + meta.output_tokens + meta.cache_read_tokens + meta.cache_write_tokens
    : 0;

  return (
    <div className="mt-1.5 rounded-lg border border-border-subtle bg-surface-code/50">
      <div className="flex items-center gap-1.5 px-2 py-1">
        <Button
          variant="ghost"
          type="button"
          onClick={toggle}
          className="flex h-auto min-w-0 flex-1 justify-start gap-1.5 rounded-none px-0 py-0 text-xs text-text-secondary transition-colors hover:bg-transparent hover:text-text-primary active:translate-y-0"
        >
          <ChevronDown
            className={cn("size-3 shrink-0 transition-transform", !expanded && "-rotate-90")}
            aria-hidden="true"
          />
          <span className="shrink-0">{t("tool.subagentPreview")}</span>
          {props.label && (
            <span className="min-w-0 whitespace-pre-wrap text-text-tertiary [overflow-wrap:anywhere]">
              {props.label}
            </span>
          )}
        </Button>
        {meta && (
          <Button
            variant="ghost"
            size="icon-xs"
            type="button"
            title={t("tool.openInTab")}
            onClick={() => openSession(meta)}
            className="size-5 shrink-0 rounded p-0.5 text-text-tertiary transition-colors hover:text-text-primary active:translate-y-0"
          >
            <ExternalLink className="size-3" aria-hidden="true" />
          </Button>
        )}
      </div>
      {expanded && (
        <div className="border-t border-border-subtle px-2 py-1.5">
          {loading && <div className="text-xs text-text-tertiary">{t("common.loading")}</div>}
          {error && <div className="text-xs text-danger">{error}</div>}
          {meta && (
            <>
              <div className="flex flex-wrap items-center gap-x-2 text-2xs text-text-tertiary">
                <span
                  className="min-w-0 whitespace-pre-wrap font-medium text-text-secondary [overflow-wrap:anywhere]"
                  title={meta.title}
                >
                  {meta.title}
                </span>
                <span>
                  {meta.message_count} {t("session.messages")}
                </span>
                {totalTokens > 0 && (
                  <span>
                    {fmtTokens(totalTokens)} {t("common.tokens")}
                  </span>
                )}
                {meta.model && <span>{meta.model}</span>}
              </div>
              <div className="mt-1 flex flex-col gap-1">
                {preview?.tail.map((msg, i) => (
                  <div key={i} className="flex gap-1.5 text-xs">
                    <span className={cn("shrink-0 font-medium", msg.role === "user" ? "text-brand" : "text-success")}>
                      {msg.role === "user" ? "›" : "‹"}
                    </span>
                    <span className="min-w-0 whitespace-pre-wrap text-text-secondary [overflow-wrap:anywhere]">
                      {msg.content}
                    </span>
                  </div>
                ))}
              </div>
            </>
          )}
        </div>
      )}
    </div>
  );
}
