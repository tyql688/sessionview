import { Download, Radio, Star, Trash2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import type { SessionMeta, Message } from "@/lib/types";
import { useI18n } from "@/i18n/index";
import {
  getProviderLabel,
  useProviderSnapshotVersion,
} from "@/stores/providerSnapshots";
import {
  formatTimestamp,
  formatDuration,
  fmtTokens,
  formatFileSize,
  shortenHomePath,
} from "@/lib/formatters";

export function SessionToolbar(props: {
  meta: SessionMeta;
  messages: Message[];
  watching: boolean;
  starred: boolean | null;
  parseWarningCount: number;
  onToggleWatch: () => void;
  onToggleFavorite: () => void;
  onResume: () => void;
  onExport: () => void;
  onDelete: () => void;
}) {
  const { t, locale } = useI18n();
  // Re-render when provider snapshots finish loading so the label reflects the
  // resolved provider name (mirrors the Solid reactive snapshot read).
  useProviderSnapshotVersion();

  const providerLabel = () => {
    const meta = props.meta;
    return getProviderLabel(meta.provider, meta.variant_name);
  };

  // Total token usage from session meta (aggregated in DB, unaffected by paging)
  const totalTokens = () => {
    const meta = props.meta;
    const input = meta.input_tokens ?? 0;
    const output = meta.output_tokens ?? 0;
    const cacheRead = meta.cache_read_tokens ?? 0;
    const cacheWrite = meta.cache_write_tokens ?? 0;
    return input + output + cacheRead + cacheWrite > 0
      ? { input, output, cacheRead, cacheWrite }
      : null;
  };

  return (
    <>
      {/* Header */}
      <div className="session-header">
        <div className="session-breadcrumb">
          <div className="breadcrumb-nav">
            <span
              className="breadcrumb-provider"
              style={{ color: `var(--${props.meta.provider})` }}
            >
              {providerLabel()}
            </span>
            <span className="breadcrumb-sep">&rsaquo;</span>
            <span className="breadcrumb-project">
              {props.meta.project_name || t("explorer.noProject")}
            </span>
          </div>
          <div className="breadcrumb-title">{props.meta.title}</div>
        </div>
        <div className="session-actions flex items-center gap-1.5">
          <Button
            variant="ghost"
            size="icon-sm"
            onClick={props.onToggleWatch}
            title={
              props.watching ? t("session.watchStop") : t("session.watchStart")
            }
          >
            <Radio
              className={cn(
                "size-4",
                props.watching && "animate-pulse text-success",
              )}
              aria-hidden="true"
            />
          </Button>
          <Button
            variant="ghost"
            size="icon-sm"
            disabled={props.starred === null}
            onClick={props.onToggleFavorite}
            title={
              props.starred === null
                ? t("common.loading")
                : props.starred
                  ? t("session.favoriteRemove")
                  : t("session.favoriteAdd")
            }
          >
            <Star
              className={cn(
                "size-4",
                props.starred && "fill-warning text-warning",
              )}
              aria-hidden="true"
            />
          </Button>
          {!(props.meta.is_sidechain && props.meta.provider === "kimi") && (
            <Button size="sm" onClick={props.onResume}>
              {t("session.resume")}
            </Button>
          )}
          <Button variant="outline" size="sm" onClick={props.onExport}>
            <Download className="size-3.5" aria-hidden="true" />
            {t("session.export")}
          </Button>
          <Button variant="destructive" size="sm" onClick={props.onDelete}>
            <Trash2 className="size-3.5" aria-hidden="true" />
            {t("session.delete")}
          </Button>
        </div>
      </div>

      {/* Info bar */}
      <div className="session-info">
        <span>
          {t("session.created")}:{" "}
          {formatTimestamp(props.meta.created_at, locale)}
        </span>
        {props.meta.updated_at > props.meta.created_at && (
          <>
            <span className="info-sep">&middot;</span>
            <span title={t("session.duration")}>
              {"\u23F1"}{" "}
              {formatDuration(
                (props.meta.updated_at - props.meta.created_at) * 1000,
              )}
            </span>
          </>
        )}
        <span className="info-sep">&middot;</span>
        <span>
          {props.meta.message_count || props.messages.length}{" "}
          {t("session.messages")}
        </span>
        <span className="info-sep">&middot;</span>
        {/* OpenCode reuses file_size_bytes to carry the whole opencode.db size
            for incremental-poll freshness, not a per-session size. Surfacing it
            would show the same DB size on every session, so render it as unknown. */}
        <span>
          {formatFileSize(
            props.meta.provider === "opencode" ? 0 : props.meta.file_size_bytes,
          )}
        </span>
        {totalTokens() && (
          <>
            <span className="info-sep">&middot;</span>
            <span
              className="session-info-tokens"
              title={`${t("common.inputTokens")}: ${totalTokens()!.input.toLocaleString()}, ${t("common.outputTokens")}: ${totalTokens()!.output.toLocaleString()}${totalTokens()!.cacheWrite > 0 ? `, ${t("common.cacheWriteTokens")}: ${totalTokens()!.cacheWrite.toLocaleString()}` : ""}${totalTokens()!.cacheRead > 0 ? `, ${t("common.cacheReadTokens")}: ${totalTokens()!.cacheRead.toLocaleString()}` : ""}`}
            >
              {"\u2191"}
              {fmtTokens(totalTokens()!.input)} {"\u2193"}
              {fmtTokens(totalTokens()!.output)} {t("common.tokens")}
              {totalTokens()!.cacheWrite + totalTokens()!.cacheRead > 0 && (
                <>
                  {" · "}
                  <span className="cache-read-label">
                    {t("common.cacheRead")}{" "}
                    {fmtTokens(totalTokens()!.cacheRead)}
                  </span>
                  {" · "}
                  {t("common.cacheWrite")}{" "}
                  {fmtTokens(totalTokens()!.cacheWrite)}
                </>
              )}
            </span>
          </>
        )}
        {props.meta.is_sidechain && (
          <>
            <span className="info-sep">&middot;</span>
            <span className="session-info-sidechain">
              {"\u2937"} {t("session.subagent")}
            </span>
          </>
        )}
        {props.meta.model && (
          <>
            <span className="info-sep">&middot;</span>
            <span className="session-info-model" title={props.meta.model}>
              {props.meta.model}
            </span>
          </>
        )}
        {props.meta.cc_version && (
          <>
            <span className="info-sep">&middot;</span>
            <span className="session-info-version">
              v{props.meta.cc_version}
            </span>
          </>
        )}
        {props.meta.git_branch && (
          <>
            <span className="info-sep">&middot;</span>
            <span className="session-info-branch" title={props.meta.git_branch}>
              {"\u2387"} {props.meta.git_branch}
            </span>
          </>
        )}
        {props.meta.project_path && (
          <>
            <span className="info-sep">&middot;</span>
            <span
              className="session-info-path"
              title={shortenHomePath(props.meta.project_path)}
            >
              {shortenHomePath(props.meta.project_path)}
            </span>
          </>
        )}
        {props.parseWarningCount > 0 && (
          <>
            <span className="info-sep">&middot;</span>
            <span
              className="session-info-parse-warn"
              title={t("session.parseWarningTooltip").replace(
                "{count}",
                String(props.parseWarningCount),
              )}
            >
              {"\u26A0"}{" "}
              {t("session.parseWarningBadge").replace(
                "{count}",
                String(props.parseWarningCount),
              )}
            </span>
          </>
        )}
      </div>
    </>
  );
}
