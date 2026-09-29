import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { ModelCost } from "@/lib/types";
import { ModelTable } from "./ModelTable";

function setup(overrides: Partial<ModelCost>) {
  const row: ModelCost = {
    model: "example-model",
    turns: 2,
    input_tokens: 100,
    output_tokens: 50,
    cache_tokens: 0,
    cost: 0,
    estimated_turns: 0,
    reported_turns: 0,
    unpriced_turns: 0,
    ...overrides,
  };
  return render(
    <ModelTable
      sortedModels={[row]}
      modelSort={{ col: "cost", asc: false }}
      onSort={() => {}}
      formatModelName={(model) => model}
    />,
  );
}

describe("model cost provenance", () => {
  it("does not call a service-reported zero unpriced", () => {
    const view = setup({ reported_turns: 2 });
    expect(view.getByText("service reported")).toBeInTheDocument();
    expect(view.queryByText("unpriced")).not.toBeInTheDocument();
  });

  it("keeps an explicit free catalog estimate priced", () => {
    const view = setup({ estimated_turns: 2 });
    expect(view.getByText("estimated")).toBeInTheDocument();
    expect(view.queryByText("unpriced")).not.toBeInTheDocument();
  });

  it("shows missing coverage even when the other calls have a positive cost", () => {
    const view = setup({ estimated_turns: 1, unpriced_turns: 1, cost: 0.5 });
    expect(view.getByText("partly unpriced")).toBeInTheDocument();
  });

  it("shows unpriced only when the backend reports missing coverage", () => {
    expect(setup({ unpriced_turns: 2 }).getByText("unpriced")).toBeInTheDocument();
  });
});
