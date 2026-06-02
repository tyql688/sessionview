import { For, Show, createSignal } from "solid-js";
import type { Accessor } from "solid-js";
import { useI18n } from "../../i18n/index";
import type { ProjectCost } from "../../lib/types";
import { ROW_LIMIT_OPTIONS, type UsageSortState } from "../../lib/usage";
import { fmtCost, fmtTokens, sortIcon } from "./formatters";
import type { ProviderChipInfo } from "./Toolbar";

export type LimitOption = 10 | 25 | 50 | 100;

export interface ProjectTableProps {
  visibleProjects: Accessor<ProjectCost[]>;
  totalProjectCount: Accessor<number>;
  projectLimit: Accessor<LimitOption>;
  onLimitChange: (limit: LimitOption) => void;
  projectSort: Accessor<UsageSortState>;
  onSort: (col: string) => void;
  providerInfo: (key: string) => ProviderChipInfo;
  formatProjectName: (project: string, projectPath: string) => string;
  formatProjectPath: (projectPath: string) => string;
}

export function ProjectTable(props: ProjectTableProps) {
  const { t } = useI18n();
  const icon = (col: string) => sortIcon(props.projectSort(), col);
  const [expanded, setExpanded] = createSignal<Set<string>>(new Set());
  const toggleRow = (path: string) =>
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });

  return (
    <section class="usage-card usage-table-card">
      <div class="usage-section-header">
        <div>
          <div class="usage-section-title">{t("usage.costByProject")}</div>
          <div class="usage-section-subtitle">
            {Math.min(props.projectLimit(), props.totalProjectCount())}/
            {props.totalProjectCount()}
          </div>
        </div>
        <div class="usage-section-actions">
          <For each={ROW_LIMIT_OPTIONS}>
            {(limit) => (
              <button
                class={`usage-limit-btn${props.projectLimit() === limit ? " active" : ""}`}
                onClick={() => props.onLimitChange(limit)}
                type="button"
              >
                {limit}
              </button>
            )}
          </For>
        </div>
      </div>
      <div class="usage-table-wrap">
        <table class="usage-table">
          <thead>
            <tr>
              <th>{t("usage.project")}</th>
              <th>{t("usage.provider")}</th>
              <th class="r" onClick={() => props.onSort("sessions")}>
                {t("usage.sessions")}
                <span class="usage-sort-icon">{icon("sessions")}</span>
              </th>
              <th class="r" onClick={() => props.onSort("turns")}>
                {t("usage.turns")}
                <span class="usage-sort-icon">{icon("turns")}</span>
              </th>
              <th class="r" onClick={() => props.onSort("tokens")}>
                {t("usage.tokens")}
                <span class="usage-sort-icon">{icon("tokens")}</span>
              </th>
              <th class="r" onClick={() => props.onSort("cost")}>
                {t("usage.cost")}
                <span class="usage-sort-icon">{icon("cost")}</span>
              </th>
            </tr>
          </thead>
          <tbody>
            <For each={props.visibleProjects()}>
              {(row) => {
                const expandable = row.by_provider.length > 1;
                const isOpen = () => expanded().has(row.project_path);
                return (
                  <>
                    <tr
                      classList={{ "usage-row-expandable": expandable }}
                      onClick={() => expandable && toggleRow(row.project_path)}
                    >
                      <td>
                        <div class="usage-entity-cell">
                          <div class="usage-entity-title">
                            <Show when={expandable}>
                              <span class="usage-expand-icon">
                                {isOpen() ? "▾" : "▸"}
                              </span>
                            </Show>
                            {props.formatProjectName(
                              row.project,
                              row.project_path,
                            )}
                          </div>
                          <div
                            class="usage-entity-subtitle"
                            title={props.formatProjectPath(row.project_path)}
                          >
                            {props.formatProjectPath(row.project_path)}
                          </div>
                        </div>
                      </td>
                      <td class="usage-provider-cell">
                        <For each={row.providers}>
                          {(prov) => {
                            const info = props.providerInfo(prov);
                            return (
                              <span
                                class="usage-provider-chip"
                                title={info.label}
                              >
                                <span
                                  class="usage-provider-dot"
                                  style={{ background: info.color }}
                                />
                                {info.label}
                              </span>
                            );
                          }}
                        </For>
                      </td>
                      <td class="r">{row.sessions}</td>
                      <td class="r">{row.turns.toLocaleString()}</td>
                      <td class="r">{fmtTokens(row.tokens)}</td>
                      <td class="r usage-cost-val">{fmtCost(row.cost)}</td>
                    </tr>
                    <Show when={expandable && isOpen()}>
                      <For each={row.by_provider}>
                        {(pp) => {
                          const info = props.providerInfo(pp.provider);
                          return (
                            <tr class="usage-subrow">
                              <td>
                                <span class="usage-subrow-label">
                                  <span
                                    class="usage-provider-dot"
                                    style={{ background: info.color }}
                                  />
                                  {info.label}
                                </span>
                              </td>
                              <td />
                              <td class="r">{pp.sessions}</td>
                              <td class="r">{pp.turns.toLocaleString()}</td>
                              <td class="r">{fmtTokens(pp.tokens)}</td>
                              <td class="r usage-cost-val">
                                {fmtCost(pp.cost)}
                              </td>
                            </tr>
                          );
                        }}
                      </For>
                    </Show>
                  </>
                );
              }}
            </For>
          </tbody>
        </table>
      </div>
    </section>
  );
}
