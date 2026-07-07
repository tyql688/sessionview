import { useMemo } from "react";
import { useI18n } from "@/i18n/index";
import type { ModelCost } from "@/lib/types";
import type { UsageSortState } from "@/lib/usage";
import { fmtCost, fmtTokens, sortIcon } from "@/features/usage/formatters";

export interface ModelTableProps {
  sortedModels: ModelCost[];
  modelSort: UsageSortState;
  onSort: (col: string) => void;
  formatModelName: (model: string) => string;
}

export function ModelTable(props: ModelTableProps) {
  const { t } = useI18n();
  const icon = (col: string) => sortIcon(props.modelSort, col);
  const totals = useMemo(
    () =>
      props.sortedModels.reduce(
        (acc, r) => {
          acc.count += 1;
          acc.turns += r.turns;
          acc.input += r.input_tokens;
          acc.output += r.output_tokens;
          acc.cache += r.cache_tokens;
          acc.cost += r.cost;
          return acc;
        },
        { count: 0, turns: 0, input: 0, output: 0, cache: 0, cost: 0 },
      ),
    [props.sortedModels],
  );

  return (
    <section className="usage-card usage-table-card">
      <div className="usage-section-header">
        <div>
          <div className="usage-section-title">{t("usage.costByModel")}</div>
          <div className="usage-section-subtitle">{t("usage.estCost")}</div>
        </div>
      </div>
      <div className="usage-table-wrap">
        <table className="usage-table">
          <thead>
            <tr>
              <th>{t("usage.model")}</th>
              <th className="r" onClick={() => props.onSort("turns")}>
                {t("usage.turns")}
                <span className="usage-sort-icon">{icon("turns")}</span>
              </th>
              <th className="r" onClick={() => props.onSort("input_tokens")}>
                {t("usage.input")}
                <span className="usage-sort-icon">{icon("input_tokens")}</span>
              </th>
              <th className="r" onClick={() => props.onSort("output_tokens")}>
                {t("usage.output")}
                <span className="usage-sort-icon">{icon("output_tokens")}</span>
              </th>
              <th className="r" onClick={() => props.onSort("cache_tokens")}>
                {t("usage.cache")}
                <span className="usage-sort-icon">{icon("cache_tokens")}</span>
              </th>
              <th className="r" onClick={() => props.onSort("cost")}>
                {t("usage.cost")}
                <span className="usage-sort-icon">{icon("cost")}</span>
              </th>
            </tr>
          </thead>
          <tbody>
            {props.sortedModels.map((row) => (
              <tr key={row.model}>
                <td>
                  <div className="usage-model-cell">
                    <span className="usage-model-tag">{props.formatModelName(row.model)}</span>
                    {row.cost === 0 && row.input_tokens + row.output_tokens + row.cache_tokens > 0 && (
                      <span className="usage-price-badge">{t("usage.unpriced")}</span>
                    )}
                  </div>
                </td>
                <td className="r">{row.turns.toLocaleString()}</td>
                <td className="r">{fmtTokens(row.input_tokens)}</td>
                <td className="r">{fmtTokens(row.output_tokens)}</td>
                <td className="r">{fmtTokens(row.cache_tokens)}</td>
                <td className="r usage-cost-val">{fmtCost(row.cost)}</td>
              </tr>
            ))}
          </tbody>
          {totals.count > 0 && (
            <tfoot>
              <tr className="usage-total-row">
                <td>
                  {t("usage.total")} ({totals.count})
                </td>
                <td className="r">{totals.turns.toLocaleString()}</td>
                <td className="r">{fmtTokens(totals.input)}</td>
                <td className="r">{fmtTokens(totals.output)}</td>
                <td className="r">{fmtTokens(totals.cache)}</td>
                <td className="r usage-cost-val">{fmtCost(totals.cost)}</td>
              </tr>
            </tfoot>
          )}
        </table>
      </div>
    </section>
  );
}
