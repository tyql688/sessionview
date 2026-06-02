import { For, Show, createMemo } from "solid-js";
import type { Accessor } from "solid-js";
import { useI18n } from "../../i18n/index";
import type { ModelCost } from "../../lib/types";
import type { UsageSortState } from "../../lib/usage";
import { fmtCost, fmtTokens, sortIcon } from "./formatters";

export interface ModelTableProps {
  sortedModels: Accessor<ModelCost[]>;
  modelSort: Accessor<UsageSortState>;
  onSort: (col: string) => void;
  formatModelName: (model: string) => string;
}

export function ModelTable(props: ModelTableProps) {
  const { t } = useI18n();
  const icon = (col: string) => sortIcon(props.modelSort(), col);
  const totals = createMemo(() =>
    props.sortedModels().reduce(
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
  );

  return (
    <section class="usage-card usage-table-card">
      <div class="usage-section-header">
        <div>
          <div class="usage-section-title">{t("usage.costByModel")}</div>
          <div class="usage-section-subtitle">{t("usage.estCost")}</div>
        </div>
      </div>
      <div class="usage-table-wrap">
        <table class="usage-table">
          <thead>
            <tr>
              <th>{t("usage.model")}</th>
              <th class="r" onClick={() => props.onSort("turns")}>
                {t("usage.turns")}
                <span class="usage-sort-icon">{icon("turns")}</span>
              </th>
              <th class="r" onClick={() => props.onSort("input_tokens")}>
                {t("usage.input")}
                <span class="usage-sort-icon">{icon("input_tokens")}</span>
              </th>
              <th class="r" onClick={() => props.onSort("output_tokens")}>
                {t("usage.output")}
                <span class="usage-sort-icon">{icon("output_tokens")}</span>
              </th>
              <th class="r" onClick={() => props.onSort("cache_tokens")}>
                {t("usage.cache")}
                <span class="usage-sort-icon">{icon("cache_tokens")}</span>
              </th>
              <th class="r" onClick={() => props.onSort("cost")}>
                {t("usage.cost")}
                <span class="usage-sort-icon">{icon("cost")}</span>
              </th>
            </tr>
          </thead>
          <tbody>
            <For each={props.sortedModels()}>
              {(row) => (
                <tr>
                  <td>
                    <div class="usage-model-cell">
                      <span class="usage-model-tag">
                        {props.formatModelName(row.model)}
                      </span>
                      <Show
                        when={
                          row.cost === 0 &&
                          row.input_tokens +
                            row.output_tokens +
                            row.cache_tokens >
                            0
                        }
                      >
                        <span class="usage-price-badge">
                          {t("usage.unpriced")}
                        </span>
                      </Show>
                    </div>
                  </td>
                  <td class="r">{row.turns.toLocaleString()}</td>
                  <td class="r">{fmtTokens(row.input_tokens)}</td>
                  <td class="r">{fmtTokens(row.output_tokens)}</td>
                  <td class="r">{fmtTokens(row.cache_tokens)}</td>
                  <td class="r usage-cost-val">{fmtCost(row.cost)}</td>
                </tr>
              )}
            </For>
          </tbody>
          <Show when={totals().count > 0}>
            <tfoot>
              <tr class="usage-total-row">
                <td>
                  {t("usage.total")} ({totals().count})
                </td>
                <td class="r">{totals().turns.toLocaleString()}</td>
                <td class="r">{fmtTokens(totals().input)}</td>
                <td class="r">{fmtTokens(totals().output)}</td>
                <td class="r">{fmtTokens(totals().cache)}</td>
                <td class="r usage-cost-val">{fmtCost(totals().cost)}</td>
              </tr>
            </tfoot>
          </Show>
        </table>
      </div>
    </section>
  );
}
