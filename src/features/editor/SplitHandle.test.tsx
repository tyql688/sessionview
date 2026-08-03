import { fireEvent, render } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { SplitHandle } from "@/features/editor/SplitHandle";

describe("SplitHandle", () => {
  it("resizes with arrow keys and resets with Enter", () => {
    const onResize = vi.fn();
    const onDoubleClick = vi.fn();
    const { getByRole } = render(
      <SplitHandle label="Resize panels" valueNow={50} onResize={onResize} onDoubleClick={onDoubleClick} />,
    );
    const separator = getByRole("separator", { name: "Resize panels" });

    fireEvent.keyDown(separator, { key: "ArrowRight" });
    fireEvent.keyDown(separator, { key: "ArrowLeft", shiftKey: true });
    fireEvent.keyDown(separator, { key: "Enter" });

    expect(onResize).toHaveBeenNthCalledWith(1, 8);
    expect(onResize).toHaveBeenNthCalledWith(2, -32);
    expect(onDoubleClick).toHaveBeenCalledTimes(1);
    expect(separator).toHaveAttribute("aria-valuenow", "50");
  });
});
