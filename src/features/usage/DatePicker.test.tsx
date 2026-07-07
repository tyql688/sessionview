import { fireEvent, render } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { DatePicker } from "@/features/usage/DatePicker";

describe("DatePicker", () => {
  it("renders a custom popover calendar instead of a native date input", async () => {
    const onChange = vi.fn();
    const { container, findByText, getByRole } = render(
      <DatePicker
        label="Start date"
        value="2026-07-01"
        min="2000-01-01"
        max="2026-07-20"
        onChange={onChange}
      />,
    );

    expect(container.querySelector('input[type="date"]')).toBeNull();

    fireEvent.click(getByRole("button", { name: "Start date" }));
    expect(await findByText("Jul 2026")).toBeInTheDocument();

    expect(getByRole("button", { name: "21" })).toBeDisabled();
    fireEvent.click(getByRole("button", { name: "15" }));

    expect(onChange).toHaveBeenCalledWith("2026-07-15");
  });
});
