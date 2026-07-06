import { describe, it, expect } from "vitest";
import { render } from "@testing-library/react";
import type { TokenUsage } from "@/lib/types";
import { TokenUsageDisplay, CopyMessageButton } from "@/components/MessageBubble/TokenUsage";

function makeUsage(overrides: Partial<TokenUsage> = {}): TokenUsage {
  return {
    input_tokens: 0,
    output_tokens: 0,
    cache_creation_input_tokens: 0,
    cache_read_input_tokens: 0,
    ...overrides,
  };
}

describe("TokenUsageDisplay", () => {
  it("renders formatted input and output token counts", () => {
    const { getByText } = render(
      <TokenUsageDisplay
        usage={makeUsage({ input_tokens: 1234, output_tokens: 56 })}
      />,
    );
    expect(getByText("↑1,234")).toBeInTheDocument();
    expect(getByText("↓56")).toBeInTheDocument();
  });

  it("hides cache rows when cache token counts are zero", () => {
    const { container } = render(
      <TokenUsageDisplay
        usage={makeUsage({ input_tokens: 10, output_tokens: 5 })}
      />,
    );
    expect(container.querySelector(".msg-token-cached")).toBeNull();
    expect(container.querySelector(".msg-token-cache-write")).toBeNull();
  });

  it("shows cache read and write rows when those counts are positive", () => {
    const { container } = render(
      <TokenUsageDisplay
        usage={makeUsage({
          input_tokens: 10,
          output_tokens: 5,
          cache_read_input_tokens: 2048,
          cache_creation_input_tokens: 512,
        })}
      />,
    );
    expect(container.querySelector(".msg-token-cached")).not.toBeNull();
    expect(container.querySelector(".msg-token-cache-write")).not.toBeNull();
  });
});

describe("CopyMessageButton", () => {
  it("renders a labelled copy button", () => {
    const { getByRole } = render(<CopyMessageButton content="hello world" />);
    const button = getByRole("button");
    expect(button).toHaveClass("msg-copy-btn");
    expect(button).toHaveAttribute("aria-label");
  });
});
