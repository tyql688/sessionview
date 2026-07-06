import { useI18n } from "@/i18n/index";
import type { SessionCostRow } from "@/lib/types";
import { ROW_LIMIT_OPTIONS, type UsageSortState } from "@/lib/usage";
import {
  fmtActive,
  fmtCost,
  fmtTokens,
  sortIcon,
} from "@/features/usage/formatters";
import type { LimitOption } from "@/features/usage/usageView";
import type { ProviderChipInfo } from "@/features/usage/Toolbar";

export interface SessionTableProps {
  visibleSessions: SessionCostRow[];
  totalSessionCount: number;
  sessionLimit: LimitOption;
  onLimitChange: (limit: LimitOption) => void;
  sessionSort: UsageSortState;
  onSort: (col: string) => void;
  providerInfo: (key: string) => ProviderChipInfo;
  formatProjectName: (project: string, projectPath: string) => string;
  formatProjectPath: (projectPath: string) => string;
  formatModelName: (model: string) => string;
}

export function SessionTable(props: SessionTableProps) {
  const { t } = useI18n();
  const icon = (col: string) => sortIcon(props.sessionSort, col);

  return (
    <section className="usage-card usage-table-card">
      <div className="usage-section-header">
        <div>
          <div className="usage-section-title">{t("usage.recentSessions")}</div>
          <div className="usage-section-subtitle">
            {Math.min(props.sessionLimit, props.totalSessionCount)}/
            {props.totalSessionCount}
          </div>
        </div>
        <div className="usage-section-actions">
          {ROW_LIMIT_OPTIONS.map((limit) => (
            <button
              key={limit}
              className={`usage-limit-btn${props.sessionLimit === limit ? " active" : ""}`}
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
              <th>{t("usage.model")}</th>
              <th className="r" onClick={() => props.onSort("updated_at")}>
                {t("usage.active")}
                <span className="usage-sort-icon">{icon("updated_at")}</span>
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
            {props.visibleSessions.map((row) => {
              const info = props.providerInfo(row.provider);
              return (
                <tr key={row.id}>
                  <td>
                    <div className="usage-entity-cell">
                      <div className="usage-entity-title">
                        {props.formatProjectName(row.project, row.project_path)}
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
                    <span
                      className="usage-provider-dot"
                      style={{ background: info.color }}
                    />
                    {info.label}
                  </td>
                  <td>
                    <span className="usage-model-tag">
                      {props.formatModelName(row.model)}
                    </span>
                  </td>
                  <td className="r usage-dim">{fmtActive(row.updated_at)}</td>
                  <td className="r">{row.turns.toLocaleString()}</td>
                  <td className="r">{fmtTokens(row.tokens)}</td>
                  <td className="r usage-cost-val">{fmtCost(row.cost)}</td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
    </section>
  );
}
