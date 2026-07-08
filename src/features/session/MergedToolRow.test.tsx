import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import type { Message } from "@/lib/types";
import { MergedToolRow } from "@/features/session/MergedToolRow";

function toolMessage(name: string, category: string): Message {
  return {
    role: "tool",
    content: "",
    timestamp: null,
    tool_name: name,
    tool_input: null,
    token_usage: null,
    tool_metadata: {
      raw_name: name,
      canonical_name: name,
      display_name: name,
      category,
    },
  };
}

describe("MergedToolRow", () => {
  it("groups repeated tools without emoji text", () => {
    const messages = Array.from({ length: 5 }, () => toolMessage("Bash", "shell"));
    const { container } = render(<MergedToolRow tools={messages.map((message) => message.tool_name!)} messages={messages} />);

    expect(container.textContent).toContain("Bash");
    expect(container.textContent).toContain("x5");
    expect(container.textContent).not.toContain("💻");
  });

  it("keeps mixed tool categories visible in the collapsed summary", () => {
    const messages = [
      toolMessage("Bash", "shell"),
      toolMessage("Read", "file"),
      toolMessage("Read", "file"),
      toolMessage("Grep", "search"),
    ];
    const { container } = render(<MergedToolRow tools={messages.map((message) => message.tool_name!)} messages={messages} />);

    expect(container.textContent).toContain("Bash");
    expect(container.textContent).toContain("Read");
    expect(container.textContent).toContain("Grep");
    expect(container.textContent).toContain("x2");
    expect(container.textContent).not.toContain("📄");
    expect(container.textContent).not.toContain("🔎");
  });
});
