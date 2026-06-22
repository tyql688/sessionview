import { Show } from "solid-js";
import type { Accessor } from "solid-js";
import type { SessionMeta, Message } from "../../lib/types";
import { useI18n } from "../../i18n/index";
import { getProviderLabel } from "../../stores/providerSnapshots";
import {
  formatTimestamp,
  formatDuration,
  fmtTokens,
  formatFileSize,
  shortenHomePath,
} from "../../lib/formatters";
import type { ProcessedEntry } from "./hooks";

export function SessionToolbar(props: {
  meta: Accessor<SessionMeta>;
  messages: Accessor<Message[]>;
  processedEntries: Accessor<ProcessedEntry[]>;
  watching: Accessor<boolean>;
  starred: Accessor<boolean | null>;
  parseWarningCount: Accessor<number>;
  onToggleWatch: () => void;
  onToggleFavorite: () => void;
  onResume: () => void;
  onExport: () => void;
  onDelete: () => void;
}) {
  const { t, locale } = useI18n();

  const providerLabel = () => {
    const meta = props.meta();
    return getProviderLabel(meta.provider, meta.variant_name);
  };

  // Total token usage from session meta (aggregated in DB, unaffected by paging)
  const totalTokens = () => {
    const meta = props.meta();
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
      <div class="session-header">
        <div class="session-breadcrumb">
          <div class="breadcrumb-nav">
            <span
              class="breadcrumb-provider"
              style={{ color: `var(--${props.meta().provider})` }}
            >
              {providerLabel()}
            </span>
            <span class="breadcrumb-sep">&rsaquo;</span>
            <span class="breadcrumb-project">
              {props.meta().project_name || t("explorer.noProject")}
            </span>
          </div>
          <div class="breadcrumb-title">{props.meta().title}</div>
        </div>
        <div class="session-actions">
          <button
            class={`session-action-btn session-action-btn-icon${props.watching() ? " watching" : ""}`}
            onClick={props.onToggleWatch}
            title={
              props.watching()
                ? t("session.watchStop")
                : t("session.watchStart")
            }
          >
            {props.watching() ? "\u25C9" : "\u25CE"}
          </button>
          <button
            class={`session-action-btn session-action-btn-icon${props.starred() === true ? " starred" : ""}`}
            onClick={props.onToggleFavorite}
            title={
              props.starred() === null
                ? t("common.loading")
                : props.starred()
                  ? t("session.favoriteRemove")
                  : t("session.favoriteAdd")
            }
          >
            {props.starred() === null
              ? "..."
              : props.starred()
                ? "\u2605"
                : "\u2606"}
          </button>
          <Show
            when={
              !(props.meta().is_sidechain && props.meta().provider === "kimi")
            }
          >
            <button
              class="session-action-btn primary"
              onClick={props.onResume}
              title={t("session.resume")}
            >
              {t("session.resume")}
            </button>
          </Show>
          <button
            class="session-action-btn"
            onClick={props.onExport}
            title={t("session.export")}
          >
            {t("session.export")}
          </button>
          <button
            class="session-action-btn session-action-btn-danger"
            onClick={props.onDelete}
            title={t("session.delete")}
          >
            {t("session.delete")}
          </button>
        </div>
      </div>

      {/* Info bar */}
      <div class="session-info">
        <span>
          {t("session.created")}:{" "}
          {formatTimestamp(props.meta().created_at, locale())}
        </span>
        <Show when={props.meta().updated_at > props.meta().created_at}>
          <span class="info-sep">&middot;</span>
          <span title={t("session.duration")}>
            {"\u23F1"}{" "}
            {formatDuration(
              (props.meta().updated_at - props.meta().created_at) * 1000,
            )}
          </span>
        </Show>
        <span class="info-sep">&middot;</span>
        <span>
          {props.meta().message_count || props.messages().length}{" "}
          {t("session.messages")}
        </span>
        <span class="info-sep">&middot;</span>
        {/* OpenCode reuses file_size_bytes to carry the whole opencode.db size
            for incremental-poll freshness, not a per-session size. Surfacing it
            would show the same DB size on every session, so render it as unknown. */}
        <span>
          {formatFileSize(
            props.meta().provider === "opencode"
              ? 0
              : props.meta().file_size_bytes,
          )}
        </span>
        <Show when={totalTokens()}>
          <span class="info-sep">&middot;</span>
          <span
            class="session-info-tokens"
            title={`${t("common.inputTokens")}: ${totalTokens()!.input.toLocaleString()}, ${t("common.outputTokens")}: ${totalTokens()!.output.toLocaleString()}${totalTokens()!.cacheWrite > 0 ? `, ${t("common.cacheWriteTokens")}: ${totalTokens()!.cacheWrite.toLocaleString()}` : ""}${totalTokens()!.cacheRead > 0 ? `, ${t("common.cacheReadTokens")}: ${totalTokens()!.cacheRead.toLocaleString()}` : ""}`}
          >
            {"\u2191"}
            {fmtTokens(totalTokens()!.input)} {"\u2193"}
            {fmtTokens(totalTokens()!.output)} {t("common.tokens")}
            <Show
              when={totalTokens()!.cacheWrite + totalTokens()!.cacheRead > 0}
            >
              {" · "}
              <span class="cache-read-label">
                {t("common.cacheRead")} {fmtTokens(totalTokens()!.cacheRead)}
              </span>
              {" · "}
              {t("common.cacheWrite")} {fmtTokens(totalTokens()!.cacheWrite)}
            </Show>
          </span>
        </Show>
        <Show when={props.meta().is_sidechain}>
          <span class="info-sep">&middot;</span>
          <span class="session-info-sidechain">
            {"\u2937"} {t("session.subagent")}
          </span>
        </Show>
        <Show when={props.meta().model}>
          <span class="info-sep">&middot;</span>
          <span class="session-info-model" title={props.meta().model}>
            {props.meta().model}
          </span>
        </Show>
        <Show when={props.meta().cc_version}>
          <span class="info-sep">&middot;</span>
          <span class="session-info-version">v{props.meta().cc_version}</span>
        </Show>
        <Show when={props.meta().git_branch}>
          <span class="info-sep">&middot;</span>
          <span class="session-info-branch" title={props.meta().git_branch}>
            {"\u2387"} {props.meta().git_branch}
          </span>
        </Show>
        <Show when={props.meta().project_path}>
          <span class="info-sep">&middot;</span>
          <span
            class="session-info-path"
            title={shortenHomePath(props.meta().project_path)}
          >
            {shortenHomePath(props.meta().project_path)}
          </span>
        </Show>
        <Show when={props.parseWarningCount() > 0}>
          <span class="info-sep">&middot;</span>
          <span
            class="session-info-parse-warn"
            title={t("session.parseWarningTooltip").replace(
              "{count}",
              String(props.parseWarningCount()),
            )}
          >
            {"\u26A0"}{" "}
            {t("session.parseWarningBadge").replace(
              "{count}",
              String(props.parseWarningCount()),
            )}
          </span>
        </Show>
      </div>
    </>
  );
}
