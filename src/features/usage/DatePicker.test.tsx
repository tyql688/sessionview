import { fireEvent, render } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { DatePicker } from "@/features/usage/DatePicker";

describe("DatePicker", () => {
  it("uses the native date input with range constraints", () => {
    const onChange = vi.fn();
    const { getByLabelText } = render(
      <DatePicker
        label="Start date"
        value="2026-07-01"
        min="2000-01-01"
        max="2026-07-20"
        onChange={onChange}
      />,
    );
    const input = getByLabelText<HTMLInputElement>("Start date");

    expect(input).toHaveAttribute("type", "date");
    expect(input).toHaveAttribute("min", "2000-01-01");
    expect(input).toHaveAttribute("max", "2026-07-20");

    fireEvent.change(input, { target: { value: "2026-07-15" } });
    expect(onChange).toHaveBeenCalledWith("2026-07-15");
  });
});
