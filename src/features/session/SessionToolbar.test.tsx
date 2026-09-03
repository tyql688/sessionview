import { render, screen } from "@testing-library/react";
import { beforeAll, describe, expect, it } from "vitest";

import { i18next } from "@/i18n/index";
import type { Provider, SessionMeta } from "@/lib/types";

import { SessionToolbar } from "./SessionToolbar";

function meta(provider: Provider, variantName: string): SessionMeta {
  return {
    id: "11111111-1111-4111-a111-111111111111",
    provider,
    title: "Synthetic session",
    project_path: "/tmp/sessionview",
    project_name: "sessionview",
    created_at: 0,
    updated_at: 0,
    message_count: 0,
    file_size_bytes: 0,
    source_path: "/tmp/sessionview/session.jsonl",
    is_sidechain: false,
    variant_name: variantName,
    input_tokens: 0,
    output_tokens: 0,
    cache_read_tokens: 0,
    cache_write_tokens: 0,
  };
}

function renderToolbar(sessionMeta: SessionMeta) {
  render(
    <SessionToolbar
      meta={sessionMeta}
      messages={[]}
      starred={false}
      parseWarningCount={0}
      onToggleFavorite={() => {}}
      onAnalyze={() => {}}
      onResume={() => {}}
      onExport={() => {}}
    />,
  );
}

beforeAll(async () => {
  await i18next.changeLanguage("en");
});

describe("SessionToolbar provider metadata", () => {
  it.each([
    ["dsh", "reviewer"],
    ["kimi", "kimi-for-coding"],
  ] as const)("renders the %s agent variant", (provider, variantName) => {
    renderToolbar(meta(provider, variantName));

    expect(screen.getByText(`Agent: ${variantName}`)).toBeVisible();
  });

  it("does not repeat the CC-Mirror variant already used as its provider label", () => {
    renderToolbar(meta("cc-mirror", "cczai"));

    expect(screen.getAllByText("cczai")).toHaveLength(1);
    expect(screen.queryByText("Agent: cczai")).not.toBeInTheDocument();
  });
});
