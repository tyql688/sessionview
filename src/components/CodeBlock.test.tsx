import { afterEach, describe, it, expect, vi } from "vitest";
import { render, waitFor } from "@testing-library/react";
import hljs from "highlight.js/lib/core";
import { CodeBlock } from "./CodeBlock";

describe("CodeBlock", () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("renders the provided code text", () => {
    const { container } = render(
      <CodeBlock code="const answer = 42;" language="typescript" />,
    );
    const code = container.querySelector("code");
    expect(code).not.toBeNull();
    expect(code?.textContent).toBe("const answer = 42;");
  });

  it("shows the language label when a language is given", () => {
    const { getByText } = render(
      <CodeBlock code="print('hi')" language="python" />,
    );
    expect(getByText("python")).toHaveClass("code-block-lang");
  });

  it("omits the language label when no language is given", () => {
    const { container } = render(<CodeBlock code="plain text" />);
    expect(container.querySelector(".code-block-lang")).toBeNull();
  });

  it("renders a copy button", () => {
    const { container } = render(<CodeBlock code="x = 1" language="python" />);
    expect(container.querySelector(".code-block-copy")).not.toBeNull();
  });

  it("reuses cached syntax highlighting for identical code and language", async () => {
    const highlightSpy = vi.spyOn(hljs, "highlight");
    const code = `const cacheProbe${Date.now()} = 42;`;

    const first = render(<CodeBlock code={code} language="typescript" />);
    await waitFor(() => expect(highlightSpy).toHaveBeenCalledTimes(1));
    first.unmount();

    const second = render(<CodeBlock code={code} language="typescript" />);
    await waitFor(() =>
      expect(second.container.querySelector("code")?.textContent).toBe(code),
    );

    expect(highlightSpy).toHaveBeenCalledTimes(1);
  });
});
