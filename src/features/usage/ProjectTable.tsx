import { Fragment, useState } from "react";
import { useI18n } from "@/i18n/index";
import type { ProjectCost } from "@/lib/types";
import { ROW_LIMIT_OPTIONS, type UsageSortState } from "@/lib/usage";
import { fmtCost, fmtTokens, sortIcon } from "@/features/usage/formatters";
import type { LimitOption } from "@/features/usage/usageView";
import type { ProviderChipInfo } from "@/features/usage/Toolbar";

export interface ProjectTableProps {
  visibleProjects: ProjectCost[];
  totalProjectCount: number;
  projectLimit: LimitOption;
  onLimitChange: (limit: LimitOption) => void;
  projectSort: UsageSortState;
  onSort: (col: string) => void;
  providerInfo: (key: string) => ProviderChipInfo;
  formatProjectName: (project: string, projectPath: string) => string;
  formatProjectPath: (projectPath: string) => string;
}

export function ProjectTable(props: ProjectTableProps) {
  const { t } = useI18n();
  const icon = (col: string) => sortIcon(props.projectSort, col);
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const toggleRow = (path: string) =>
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });

  return (
    <section className="usage-card usage-table-card">
      <div className="usage-section-header">
        <div>
          <div className="usage-section-title">{t("usage.costByProject")}</div>
          <div className="usage-section-subtitle">
            {Math.min(props.projectLimit, props.totalProjectCount)}/
            {props.totalProjectCount}
          </div>
        </div>
        <div className="usage-section-actions">
          {ROW_LIMIT_OPTIONS.map((limit) => (
            <button
              key={limit}
              className={`usage-limit-btn${props.projectLimit === limit ? " active" : ""}`}
              onClick={() => props.onLimitChange(limit)}
              type="button"
            >
              {limit}
            </button>
          ))}
        </div>
      </div>
      <div className="usage-table-wrap">
        <table className="usage-table">
          <thead>
            <tr>
              <th>{t("usage.project")}</th>
              <th>{t("usage.provider")}</th>
              <th className="r" onClick={() => props.onSort("sessions")}>
                {t("usage.sessions")}
                <span className="usage-sort-icon">{icon("sessions")}</span>
              </th>
              <th className="r" onClick={() => props.onSort("turns")}>
                {t("usage.turns")}
                <span className="usage-sort-icon">{icon("turns")}</span>
              </th>
              <th className="r" onClick={() => props.onSort("tokens")}>
                {t("usage.tokens")}
                <span className="usage-sort-icon">{icon("tokens")}</span>
              </th>
              <th className="r" onClick={() => props.onSort("cost")}>
                {t("usage.cost")}
                <span className="usage-sort-icon">{icon("cost")}</span>
              </th>
            </tr>
          </thead>
          <tbody>
            {props.visibleProjects.map((row) => {
              const expandable = row.by_provider.length > 1;
              const isOpen = expanded.has(row.project_path);
              return (
                <Fragment key={row.project_path}>
                  <tr
                    className={expandable ? "usage-row-expandable" : undefined}
                    onClick={() => expandable && toggleRow(row.project_path)}
                  >
                    <td>
                      <div className="usage-entity-cell">
                        <div className="usage-entity-title">
                          {expandable && (
                            <span className="usage-expand-icon">
                              {isOpen ? "▾" : "▸"}
                            </span>
                          )}
                          {props.formatProjectName(
                            row.project,
                            row.project_path,
                          )}
                        </div>
                        <div
                          className="usage-entity-subtitle"
                          title={props.formatProjectPath(row.project_path)}
                        >
                          {props.formatProjectPath(row.project_path)}
                        </div>
                      </div>
                    </td>
                    <td className="usage-provider-cell">
                      {row.providers.map((prov) => {
                        const info = props.providerInfo(prov);
                        return (
                          <span
                            key={prov}
                            className="usage-provider-chip"
                            title={info.label}
                          >
                            <span
                              className="usage-provider-dot"
                              style={{ background: info.color }}
                            />
                            {info.label}
                          </span>
                        );
                      })}
                    </td>
                    <td className="r">{row.sessions}</td>
                    <td className="r">{row.turns.toLocaleString()}</td>
                    <td className="r">{fmtTokens(row.tokens)}</td>
                    <td className="r usage-cost-val">{fmtCost(row.cost)}</td>
                  </tr>
                  {expandable &&
                    isOpen &&
                    row.by_provider.map((pp) => {
                      const info = props.providerInfo(pp.provider);
                      return (
                        <tr key={pp.provider} className="usage-subrow">
                          <td>
                            <span className="usage-subrow-label">
                              <span
                                className="usage-provider-dot"
                                style={{ background: info.color }}
                              />
                              {info.label}
                            </span>
                          </td>
                          <td />
                          <td className="r">{pp.sessions}</td>
                          <td className="r">{pp.turns.toLocaleString()}</td>
                          <td className="r">{fmtTokens(pp.tokens)}</td>
                          <td className="r usage-cost-val">
                            {fmtCost(pp.cost)}
                          </td>
                        </tr>
                      );
                    })}
                </Fragment>
              );
            })}
          </tbody>
        </table>
      </div>
    </section>
  );
}
