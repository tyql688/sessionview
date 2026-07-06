import { useI18n } from "@/i18n/index";
import type { ModelCost } from "@/lib/types";
import { fmtCost, fmtTokens } from "@/features/usage/formatters";

export interface TopModelsProps {
  topModels: ModelCost[];
  maxTopModelCost: number;
  formatModelName: (model: string) => string;
}

export function TopModels(props: TopModelsProps) {
  const { t } = useI18n();

  return (
    <section className="usage-card usage-spotlight-card">
      <div className="usage-section-header">
        <div>
          <div className="usage-section-title">{t("usage.topModels")}</div>
          <div className="usage-section-subtitle">{t("usage.costByModel")}</div>
        </div>
      </div>

      {props.topModels.length > 0 ? (
        <div className="usage-spotlight-list">
          {props.topModels.map((row) => (
            <div key={row.model} className="usage-spotlight-item">
              <div className="usage-spotlight-meta">
                <span className="usage-model-tag">
                  {props.formatModelName(row.model)}
                </span>
                <span className="usage-spotlight-tokens">
                  {fmtTokens(
                    row.input_tokens + row.output_tokens + row.cache_tokens,
                  )}
                </span>
              </div>
              <div className="usage-spotlight-bar">
                <div
                  className="usage-spotlight-bar-fill"
                  style={{
                    width: `${Math.max(
                      8,
                      props.maxTopModelCost > 0
                        ? (row.cost / props.maxTopModelCost) * 100
                        : 0,
                    )}%`,
                  }}
                />
              </div>
              <div className="usage-spotlight-cost">{fmtCost(row.cost)}</div>
            </div>
          ))}
        </div>
      ) : (
        <div className="usage-empty-inline">{t("usage.noData")}</div>
      )}
    </section>
  );
}
